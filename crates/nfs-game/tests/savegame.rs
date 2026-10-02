//! The original save game against the shipped tables (`RE/notes/savegame.md`).
use nfs_assets::GamePaths;
use nfs_formats::sav::{SaveFile, FE_CAR_IMAGE};
use nfs_game::career::{Career, CareerTables, EventResult, OwnedCar, TournamentState};
use nfs_game::save::{car, career_from_sav, career_to_sav, career_to_sav_with, Profile};

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
    let mut owned = OwnedCar::construct(&t, car_id, 1, 30_000);
    assert!(!owned.stock.is_empty(), "the constructor fits the record's stock parts (0x4EE9A0)");
    owned.mileage = 9000;
    owned.wins = 2;
    owned.asking_price = Some(41_000);
    owned.offer = Some(38_500);
    // one stock part carries damage, so both lists' `+0x164` must survive
    owned.part_state[0].1 = 12.5;
    c.garage.push(owned.clone());
    let tour = t.tournaments[0].id;
    c.tournaments.insert(
        tour,
        TournamentState {
            entered: true,
            completed: true,
            completions: 2,
            events: vec![EventResult { position: 1, prize: 5000, bonus: 2500, points: [8, 7, 6, 5, 4, 3, 2, 1], earned: [7500, 3000, 2000, 1500, 1000, 800, 600, 400], times: [10_000, 10_500, 11_000, 0, 0, 0, 0, 0], event: Some(1) }],
        },
    );
    c.missions = t.missions.iter().enumerate().map(|(i, _)| if i == 3 { 1 } else { 0 }).collect();
    // record 3's promotion taken: `fac+0xC = −1` (0x51AF80)
    c.prize_taken = t.missions.iter().enumerate().map(|(i, _)| i == 3).collect();
    // record 3's last attempt: time, average damage, smackables (0x51AF2B…0x51AF3E)
    c.set_mission_stats(&t, 3, Career::mission_stats_of(70_000, &[12.9, 7.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 30.0], 5));

    let p = Profile::new("Hans", 3);
    let sav = career_to_sav(&c, &t, &p);
    let bytes = sav.write();
    // The container the original writes: no header, module names NUL-terminated, in its own order.
    assert!(bytes.starts_with(b"Missions\0"));
    let parsed = SaveFile::parse(&bytes).expect("the writer produces a file the reader accepts");
    // `PartsMarket` is the player's parts inventory (§4.2), written even when it is empty
    // and `UsedCarMarket` (§4.4) is always registered, so always written, empty market or not
    assert_eq!(parsed.order, ["Missions", "PartsMarket", "Tournament", "PlayerInfo", "TrackRecords", "UsedCarMarket", "Personalities", "Knockout", "CDPlayer", "accessor"]);
    // the knockout module is the raw 0x84-byte state block, switched off when no series is running
    let k = nfs_formats::sav::KnockoutState::parse(parsed.knockout.as_ref().unwrap());
    assert!(!k.active && k.opponents == 0);
    // the accessor block carries the values the port has, in the original's map order
    let acc = parsed.accessors.as_ref().expect("accessor block");
    assert!(acc.entries.iter().any(|(n, v)| n == "Chronicle.Year" && *v == nfs_formats::sav::AccessorValue::Dword(1969)));
    assert!(acc.entries.iter().any(|(n, _)| n == "OwnedCars.Car"));
    assert!(!acc.entries.iter().any(|(n, _)| n == "Game.GameType"), "the mode is not the career's to write");
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
    // `car+0x75C`, the stock list, is written first, then `car+0x758` (0x4EE8D0)
    assert_eq!(saved.removed.iter().map(|p| p.id as i16).collect::<Vec<_>>(), owned.stock.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    assert_eq!(saved.installed.iter().map(|p| p.id as i16).collect::<Vec<_>>(), owned.parts);
    assert_eq!(saved.installed[0].state, 12.5);

    let back = career_from_sav(&parsed, &t);
    assert_eq!(back.money, c.money);
    assert_eq!(back.year, 1969);
    assert_eq!(back.era, nfs_formats::career::Era::Golden);
    assert_eq!(back.garage.len(), 1);
    assert_eq!(back.garage[0].car_id, owned.car_id);
    assert_eq!(back.garage[0].paint_set, owned.paint_set, "the paint set is recovered from the stored colours");
    assert_eq!(back.garage[0].trim, owned.trim, "a bought car's rows are all 0 and come back so");
    assert_eq!(back.garage[0].parts, owned.parts);
    assert_eq!(back.garage[0].stock, owned.stock);
    assert_eq!(back.garage[0].part_state, owned.part_state);
    assert_eq!(back.garage[0].mileage, owned.mileage);
    assert_eq!(back.garage[0].wins, owned.wins);
    assert_eq!(back.garage[0].asking_price, owned.asking_price);
    assert_eq!(back.garage[0].offer, owned.offer);
    assert_eq!(back.missions, c.missions);
    assert_eq!(back.prize_taken, c.prize_taken, "the taken promotion rides in the Missions image");
    // 70 000 & 0xFFFF; 49 / 10 (partial sums 12, 19, 49) with each partial sum truncated (0x4EE137…0x4EE14C); 5
    assert_eq!(c.mission_stats[3], [4464, 4, 5]);
    assert_eq!(back.mission_stats[3], c.mission_stats[3], "+0xEA / +0xEC / +0xEE ride in the Missions image");
    assert_eq!(back.mission_stats[0], [0; 3]);
    let fac = parsed.missions.as_ref().unwrap();
    let rec = nfs_formats::career::MISSION_RECORD_SIZE;
    assert_eq!(&fac[3 * rec + 0xC..3 * rec + 0x10], &[0xFF; 4]);
    assert_eq!(&fac[2 * rec + 0xC..2 * rec + 0x10], &t.raw.fac[2 * rec + 0xC..2 * rec + 0x10], "the others keep their text id");
    assert!(!back.seen_evolution_intro);
    let mut seen = parsed.clone();
    seen.player.as_mut().unwrap().seen_evolution_intro = 1;
    assert!(career_from_sav(&seen, &t).seen_evolution_intro, "[0x691DD5] comes back (0x501859)");
    // a `.sav` the port wrote before the stock list: no `+0x75C` parts and only what was bought
    // installed — the stock list comes back from the record and fills the empty slots
    let mut old = parsed.clone();
    let oc = &mut old.player.as_mut().unwrap().cars[0];
    oc.removed.clear();
    oc.installed.truncate(1);
    let back_old = career_from_sav(&old, &t);
    assert_eq!(back_old.garage[0].stock.iter().map(|(id, _)| *id).collect::<Vec<_>>(), owned.stock.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    let mut ids = back_old.garage[0].parts.clone();
    ids.sort_unstable();
    let mut want = owned.parts.clone();
    want.sort_unstable();
    assert_eq!(ids, want);
    let st = back.tournaments.get(&tour).expect("the tournament state survives in the nfs5.trn image");
    assert!(st.completed);
    assert_eq!(st.events.len(), 1);
    assert_eq!(st.events[0].points, c.tournaments[&tour].events[0].points);
    assert_eq!(st.events[0].times, c.tournaments[&tour].events[0].times);
    assert_eq!(st.events[0].earned, c.tournaments[&tour].events[0].earned);
    assert_eq!(st.events[0].prize, 5000);
    assert_eq!(st.events[0].bonus, 2500);
    // the second event record, raced first (0x504F00): it comes back in its own slot
    assert_eq!(st.events[0].event, Some(1));
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
    // …and no shipped record reads as a taken promotion (every `+0xC` is a text id)
    assert!(career_from_sav(&sav, &t).prize_taken.iter().all(|x| !x));
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

    let sav = nfs_game::save::career_to_sav_with(&career, &tables, &profile, &nfs_game::save::SaveSide { settings, ..Default::default() });
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
    // `[0x6115F8]`, the shipped `packages.ini`'s `[header] count`
    assert_eq!(t.package_count, 8);
    // the original's 16-bit LCG stands in for `0x538040`; any uniform source exercises the rules
    let mut seed = 12345u32;
    let mut rand = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) & 0x7FFF
    };
    c.refresh_market(&t, &mut rand, &mut || 1);
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
        // the appearance 0x4F82C0 draws: one paint set for the body and both stripes, an interior
        // of its own, a package below `packages.ini`'s count and a race number −1…98
        assert!((car.paint_set as usize) < rec.paint_sets.len());
        assert_eq!((car.trim.stripe1, car.trim.stripe2), (car.paint_set, car.paint_set));
        assert!((car.trim.interior as usize) < rec.interior_colours.len());
        assert!((car.trim.package as u32) < t.package_count);
        assert!((-1..=98).contains(&car.trim.decal));
        assert_eq!(car.asking_price, Some(c.value_of(&t, car).unwrap().max(0) as u32));
        // a market car is mechanically fresh — the generator never damages it
        assert_eq!(car.damage_pct, 0.0);
        assert!(car.part_state.iter().all(|(_, s)| *s == 0.0));
        // and it never picks two engines (0x4F877E's four compares) — the back-fill
        // (0x4F89D7…) may still put a stock `"Engine (Standard)"` beside a picked `"Engine"`
        let engines = car
            .parts
            .iter()
            .filter(|id| !car.stock.iter().any(|(s, _)| s == *id))
            .filter_map(|id| t.parts.iter().find(|p| p.id == *id))
            .filter(|p| p.kind_name == "Engine" || p.kind_name == "Engine (Standard)")
            .count();
        assert!(engines <= 1, "{} has {engines} engines", rec.display_name);
    }
    // refreshing again keeps the cap
    for _ in 0..4 {
        c.refresh_market(&t, &mut rand, &mut || 1);
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

/// `car+0x00` (`FECar::init` 0x4EEBA0's instance loop, the `car` module doc in `save.rs`): every
/// touch of the registry `[0x69188C]` was traced (`career.md` §10.9 "The instance dword, settled
/// as far as it goes") and no reader of the field turned up anywhere but the constructor's own
/// loop — Save 0x4EE8D0 and Load 0x4EE6A0 both move it only as part of the flat `rep movsd`. The
/// port used to write the car's index here (garage first, then the market): an invented value,
/// since nothing ever reads it back. It now leaves the field at the fresh buffer's own zero.
#[test]
fn the_saved_cars_instance_dword_is_zero() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let car_id = t.catalog.iter().find(|c| !c.paint_sets.is_empty()).unwrap().id;
    let mut c = Career::default();
    c.year = 1975;
    c.garage.push(OwnedCar::construct(&t, car_id, 0, 1000));
    c.garage.push(OwnedCar::construct(&t, car_id, 0, 1000));
    let mut seed = 1u32;
    let mut rand = || {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) & 0x7FFF
    };
    c.refresh_market(&t, &mut rand, &mut || 1);
    assert!(!c.market.is_empty(), "a 1975 market should hold something");
    let profile = Profile { name: "X".into(), picture: 0 };
    let sav = career_to_sav(&c, &t, &profile);
    for car in &sav.player.expect("player module").cars {
        assert_eq!(car.field_u32(0), 0, "car+0x00 has no reader; the port must not invent a per-car index there");
    }
    for car in &sav.used_cars.expect("used-car module").cars {
        assert_eq!(car.field_u32(0), 0, "car+0x00 has no reader; the port must not invent a per-car index there");
    }
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
    owned.trim = nfs_game::career::Trim { interior: 1, stripe1: 1, stripe2: 2, package: 3, decal: 42 };
    c.garage.push(owned.clone());
    let parsed = SaveFile::parse(&career_to_sav(&c, &t, &Profile::new("Hans", 0)).write()).unwrap();
    let img = &parsed.player.as_ref().unwrap().cars[0];
    let f = |o: usize| u32::from_le_bytes([img.image[o], img.image[o + 1], img.image[o + 2], img.image[o + 3]]);
    assert_eq!(f(car::S1_COLOUR), e.paint_sets[1].packed[1]);
    assert_eq!(f(car::S2_COLOUR), e.paint_sets[2].packed[2]);
    assert_eq!(f(car::PACKAGE_NUM), 3);
    assert_eq!(f(car::DECAL_NUM), 42, "the race number, FECar+0x688");
    assert_eq!(career_from_sav(&parsed, &t).garage[0].trim, owned.trim);

    // a saved custom colour on the exterior and stripe 2 (the paint shop's rows past the custom
    // row, 0x4B92ED) is the dword itself in `FECar+0x674` / `+0x680`, and comes back as that
    owned.custom = [Some(0xFF3F_BFBF), None, None, Some(0xFF12_3456)];
    c.garage[0] = owned.clone();
    let parsed = SaveFile::parse(&career_to_sav(&c, &t, &Profile::new("Hans", 0)).write()).unwrap();
    let img = &parsed.player.as_ref().unwrap().cars[0];
    let f = |o: usize| u32::from_le_bytes([img.image[o], img.image[o + 1], img.image[o + 2], img.image[o + 3]]);
    assert_eq!((f(car::EXT_COLOUR), f(car::S2_COLOUR)), (0xFF3F_BFBF, 0xFF12_3456));
    assert_eq!(f(car::S1_COLOUR), e.paint_sets[1].packed[1]);
    let back = &career_from_sav(&parsed, &t).garage[0];
    assert_eq!((back.custom, back.trim.stripe1), (owned.custom, 1));
    assert_eq!(back.colour_words(e), [0xFF3F_BFBF, owned.colour_words(e)[1], e.paint_sets[1].packed[1], 0xFF12_3456]);
}

/// `opponent_car_image` (0x4F4FA0's two draw branches both `call 0x4eeba0`, `FECar::init`, for
/// each of the seven `OpponentCar.Car` slots — not the bare sub-object constructor 0x4EE5C0 the
/// module doc used to say): a drawn opponent car's image must carry what `FECar::init` actually
/// leaves — `+0x6A0` (`car::OWNED`) at 1 (0x4EEC7F) and the record's sorted stock parts on
/// `+0x758`/`+0x75C` (`fit_stock` 0x4EE9A0, unconditional at `FECar::init`'s end) — the same lists
/// `OwnedCar::construct` puts on a fresh garage or market car.
#[test]
fn opponent_cars_carry_fecar_inits_owned_flag_and_stock_parts() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let car_id = t.catalog.iter().find(|c| t.car(c.id).is_some_and(|r| !r.stock_parts.is_empty())).unwrap().id;
    let opp = nfs_game::save::OpponentCarSave { car_id, paint_set: 0, interior: 0, package: 0, decal: -1 };
    let image = nfs_game::save::opponent_car_image(&t, &opp);
    assert_eq!(image.field_u32(car::OWNED) & 0xFF, 1, "FECar::init 0x4EEC7F sets +0x6A0 = 1 unconditionally");
    let want = OwnedCar::construct(&t, car_id, 0, 0);
    assert!(!want.stock.is_empty(), "the chosen car has stock parts to carry");
    assert_eq!(image.removed.iter().map(|p| p.id as i16).collect::<Vec<_>>(), want.stock.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    assert_eq!(image.installed.iter().map(|p| p.id as i16).collect::<Vec<_>>(), want.parts);
    // the port's own buffer is zeroed, so the untouched half of the instance-dword rule holds and
    // the value stays 0 — settled module-wide, nothing reads it back either way
    assert_eq!(image.field_u32(0), 0);
}

/// `OwnedCar::adopt_implicit_stock` (a version-1 port career's missing stock list, rebuilt):
/// missing stock parts land at the **end** of `parts`, in the record's own stock order, and the
/// parts already there keep whatever order they arrived in — not resorted, not interleaved. That
/// is not this port-only case's own choice; it is read off 0x518510 (`install_spare` erases the
/// displaced part at its own index and pushes the new one at the end) and 0x4F8590's back-fill
/// 0x4F89D7…0x4F8C63 (a market car's unmatched stock pushed at the end in `+0x75C`'s order), the
/// two live cases that add a missing stock part to an already-populated `+0x758`.
#[test]
fn adopt_implicit_stock_appends_missing_parts_at_the_end_in_stock_order() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let car_id = t
        .catalog
        .iter()
        .find(|c| {
            t.car(c.id).is_some_and(|r| {
                r.stock_parts.len() >= 3
                    && r.stock_parts.iter().filter_map(|id| t.parts.iter().find(|p| p.id == *id)).map(|p| (p.kind, &p.kind_name)).collect::<std::collections::HashSet<_>>().len() == r.stock_parts.len()
            })
        })
        .expect("a car with three or more stock parts in distinct slots")
        .id;
    let stock_ids = t.car(car_id).unwrap().stock_parts.clone();
    let last = stock_ids.len() - 1;

    // deliberately out of the record's own order (last slot first), to show
    // adopt_implicit_stock does not touch what is already there
    let mut owned = OwnedCar::new(car_id, 0, 0);
    owned.parts = vec![stock_ids[last], stock_ids[0]];
    owned.part_state = owned.parts.iter().map(|id| (*id, 0.0)).collect();
    assert!(owned.stock.is_empty(), "no stock list — the version-1 state this migrates");
    owned.adopt_implicit_stock(&t);

    // every other slot is open; the two given parts stay exactly as given, and the rest lands
    // after them, in the record's own stock order
    let mut want = vec![stock_ids[last], stock_ids[0]];
    want.extend(&stock_ids[1..last]);
    assert_eq!(owned.parts, want);
    assert_eq!(owned.stock.iter().map(|(id, _)| *id).collect::<Vec<_>>(), stock_ids);
}

/// 0x4EE853…0x4EE8AE: the car loader exchange-sorts the installed list by 0x49C350 whatever
/// order the file had it in.
#[test]
fn the_loader_sorts_a_cars_installed_parts() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let car_id = t.catalog.iter().find(|c| !c.sim.is_empty() && t.car(c.id).is_some_and(|r| r.stock_parts.len() > 2)).unwrap().id;
    let owned = OwnedCar::construct(&t, car_id, 0, 10_000);
    let mut c = Career::default();
    c.garage.push(owned.clone());
    let mut sav = career_to_sav(&c, &t, &Profile::new("Hans", 0));
    sav.player.as_mut().unwrap().cars[0].installed.reverse();
    let back = career_from_sav(&SaveFile::parse(&sav.write()).unwrap(), &t);
    assert_ne!(sav.player.as_ref().unwrap().cars[0].installed.iter().map(|p| p.id as i16).collect::<Vec<_>>(), owned.parts);
    assert_eq!(back.garage[0].parts, owned.parts, "fit_stock's order is 0x49C350's, and the loader restores it");
}

/// §4.4, 0x4F7A10 / 0x4F7750: the used-car market goes out as its own module — the year, then the
/// generated cars with `+0x6A0 = 0` and their price at `+0x68C` — and comes back from it.
#[test]
fn the_used_car_market_rides_in_its_module() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut c = Career::default();
    c.year = 1975;
    let mut seed = 7u32;
    let mut rand = move || {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        seed >> 8
    };
    c.refresh_market(&t, &mut rand, &mut || 1);
    assert!(!c.market.is_empty());
    let sav = career_to_sav(&c, &t, &Profile::new("Hans", 0));
    let bytes = sav.write();
    let parsed = SaveFile::parse(&bytes).unwrap();
    assert_eq!(parsed.write(), bytes, "the file re-writes byte for byte");
    let m = parsed.used_cars.as_ref().expect("the module");
    assert_eq!(m.year, 1975);
    assert_eq!(m.cars.len(), c.market.len());
    for (img, car) in m.cars.iter().zip(&c.market) {
        assert_eq!(img.field_u8(car::OWNED), 0, "a car the market generated (0x4F853E)");
        assert_eq!(img.field_u8(car::LISTED), 0, "the listed byte is the player's own flag");
        assert_eq!(img.field_u32(car::ASKING_PRICE), car.asking_price.unwrap());
    }
    let back = career_from_sav(&parsed, &t);
    assert_eq!(back.market.len(), c.market.len());
    for (a, b) in back.market.iter().zip(&c.market) {
        assert_eq!((a.car_id, a.mileage, a.wins, a.asking_price), (b.car_id, b.mileage, b.wins, b.asking_price));
        // the generator 0x4F8590 does not sort the parts it fits; the loader does (0x4EE853)
        let (mut x, mut y) = (a.parts.clone(), b.parts.clone());
        x.sort_unstable();
        y.sort_unstable();
        assert_eq!(x, y);
    }
}

/// What `LoadInfo` hands the front end's own loaders comes back: `Knockout` 0x4DDD60 (the whole
/// 0x84-byte block), `Personalities` 0x4E2CA0 (the last draw), `CDPlayer` 0x4BE170 (the list) and
/// the option accessors (0x4BC480), each as the index the accessor stores — `Track1.Laps` one
/// below the lap count (0x4FC1BC) and `Game.NumKOOpponents` one below the count (minimum 1,
/// 0x4E556A) — clamped into the list by `SetSelection` 0x4BC240.
#[test]
fn the_front_end_modules_and_options_come_back() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    use nfs_game::save::{side_from_sav, KnockoutProgress, SaveSide, SavedSettings};
    let ko = KnockoutProgress {
        active: true,
        race_index: 2,
        opponents: 3,
        alive: vec![true, true, false, true],
        start_pos: vec![2, 1, u32::MAX, 3],
        personalities: vec![11, 17, 23],
        last_pos: vec![3, 1, 4, 2, -1, -1, -1, -1, -1],
    };
    let side = SaveSide {
        knockout: Some(ko),
        personalities: Some(vec![40, 41]),
        playlist: vec![3, 1, 4],
        settings: SavedSettings { laps: 5, opponents: 6, traffic: 2, ko_opponents: 7, music: 90, options: vec![("Display.SpeedUnits".into(), 1), ("Graphics.CarDetail".into(), 2)], ..SavedSettings::default() },
        tournament: nfs_game::save::TournamentFe { selected: 4, next_event: 2, current_event: 1, current_tournament: 4 },
        opponent_cars: None,
        owned_car_selection: 1,
    };
    // `OpponentCar.Car`'s seven (0x4F56C0 / 0x4F56F0): a record with paint sets and interiors
    let with_paint = t.catalog.iter().find(|c| c.paint_sets.len() > 2 && c.interior_colours.len() > 1).expect("a painted car");
    let opp = nfs_game::save::OpponentCarSave { car_id: with_paint.id, paint_set: 2, interior: 1, package: 3, decal: -1 };
    let mut side = side;
    side.opponent_cars = Some((4, (0..7).map(|_| nfs_game::save::opponent_car_image(&t, &opp)).collect()));
    let sav = career_to_sav_with(&Career::default(), &t, &Profile::new("Hans", 0), &side);
    let bytes = sav.write();
    let parsed = SaveFile::parse(&bytes).unwrap();
    assert_eq!(parsed.write(), bytes, "the file re-writes byte for byte");
    let acc = parsed.accessors.as_ref().unwrap();
    let dword = |n: &str| acc.entries.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone());
    // the indices the accessors hold, not the counts
    assert_eq!(dword("Track1.Laps"), Some(nfs_formats::sav::AccessorValue::Dword(4)));
    assert_eq!(dword("Game.NumKOOpponents"), Some(nfs_formats::sav::AccessorValue::Dword(6)));
    assert_eq!(dword("Game.NumOpponents"), Some(nfs_formats::sav::AccessorValue::Dword(6)));
    assert_eq!(dword("Display.SpeedUnits"), Some(nfs_formats::sav::AccessorValue::Dword(1)));
    // a name the fixed fields carry is written once
    assert_eq!(acc.entries.iter().filter(|(k, _)| k == "Audio.MusicLevel").count(), 1);

    let back = side_from_sav(&parsed);
    // 0x4DDD60 copies the block whole: all sixteen `alive` bytes, eight personalities, nine places
    let k = back.knockout.as_ref().expect("the block");
    assert_eq!((k.active, k.race_index, k.opponents), (true, 2, 3));
    assert_eq!(k.alive[..4], [true, true, false, true]);
    assert!(k.alive[4..].iter().all(|a| !a) && k.alive.len() == 16);
    assert_eq!(k.start_pos[..4], [2, 1, u32::MAX, 3]);
    assert_eq!(k.personalities[..3], [11, 17, 23]);
    assert_eq!(k.last_pos, side.knockout.as_ref().unwrap().last_pos);
    assert_eq!(back.personalities, side.personalities);
    assert_eq!(back.playlist, side.playlist);
    // `OwnedCars.Car`: 0x4F5B30 writes the index, 0x4F5B70 reads it back
    assert!(matches!(dword("OwnedCars.Car"), Some(nfs_formats::sav::AccessorValue::OwnedCarSel { selection: 1, .. })));
    assert_eq!(back.owned_car_selection, 1);
    // `Tournament`'s four front-end dwords, in the writer's order 0x502AE0 and back through 0x5029F0
    let tm = parsed.tournament.as_ref().unwrap();
    assert_eq!((tm.remaining_tracks, tm.selected, tm.current_event, tm.current_tournament), (2, 4, 1, 4));
    assert_eq!(back.tournament, side.tournament);
    let st = &back.settings;
    assert_eq!((st.laps, st.opponents, st.traffic, st.ko_opponents, st.music), (5, 6, 2, 7, 90));
    assert!(st.options.contains(&("Display.SpeedUnits".to_string(), 1)));
    assert!(st.options.contains(&("Graphics.CarDetail".to_string(), 2)));
    let (sel, cars) = back.opponent_cars.as_ref().expect("OpponentCar.Car");
    assert_eq!((*sel, cars.len()), (4, 7));
    assert!(cars.iter().all(|c| nfs_game::save::opponent_car_of(c) == opp), "record, paint set, interior, package and decal back");
    // and a second save of what was loaded writes the same file
    assert_eq!(career_to_sav_with(&Career::default(), &t, &Profile::new("Hans", 0), &back).write(), bytes);

    // `SetSelection` 0x4BC240: above the list → its last row, negative (signed) → row 0
    assert_eq!(nfs_game::save::clamp_index(9, 4), 3);
    assert_eq!(nfs_game::save::clamp_index(u32::MAX, 4), 0);
    assert_eq!(nfs_game::save::clamp_index(2, 4), 2);
}

/// All nineteen `Controller.*` rows the factory 0x51E030 builds ride in the profile, six dwords
/// each (0x51D060), and come back by name (0x51D080).
#[test]
fn all_nineteen_control_rows_are_saved() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    use nfs_game::save::{saved_controls, SaveSide, SavedSettings, CONTROL_ROWS};
    const NAMES: [&str; CONTROL_ROWS] = [
        "Controller.Accel", "Controller.Brake", "Controller.HandBrake", "Controller.SteerLeft", "Controller.SteerRight", "Controller.ShiftUp", "Controller.ShiftDown",
        "Controller.Horn", "Controller.LookBehind", "Controller.Lights", "Controller.ResetCar", "Controller.CameraView", "Controller.Pause", "Controller.LeftTurnSignal",
        "Controller.RightTurnSignal", "Controller.Hazards", "Controller.LookLeft", "Controller.LookRight", "Controller.LookForward",
    ];
    let mut settings = SavedSettings::default();
    for (i, n) in NAMES.iter().enumerate() {
        assert_eq!(nfs_formats::sav::accessor_kind(n), Some(nfs_formats::sav::AccessorKind::ControllerInput), "{n}");
        settings.controls[i] = Some((NAMES[i], [3, 0, 0x10 + i as u32, 0, 0, b'A' as u32 + i as u32]));
    }
    let sav = career_to_sav_with(&Career::default(), &t, &Profile::new("Hans", 0), &SaveSide { settings, ..Default::default() });
    let back = saved_controls(&SaveFile::parse(&sav.write()).unwrap());
    assert_eq!(back.len(), CONTROL_ROWS);
    for (i, n) in NAMES.iter().enumerate() {
        let f = back.iter().find(|(k, _)| k == n).unwrap_or_else(|| panic!("{n} saved")).1;
        assert_eq!(f[2], 0x10 + i as u32);
    }
}

/// `PlayerInfo`'s tail (0x5018B0 / 0x501610): the three first-time bytes `[0x691DD5]`,
/// `[0x691D94]`, `[0x691DF5]` and the eight custom colours `[0x691D98]` go out and come back.
#[test]
fn the_first_time_flags_and_custom_colours_come_back() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut c = Career::default();
    c.seen_factory_welcome = true;
    c.seen_first_mission = true;
    c.custom_colours = [0xFF10_2030, 0, 0xFF00_FF00, 0, 0, 0, 0, 0xFFFF_FFFF];
    let bytes = career_to_sav(&c, &t, &Profile::new("Hans", 0)).write();
    let parsed = SaveFile::parse(&bytes).unwrap();
    let p = parsed.player.as_ref().unwrap();
    assert_eq!((p.seen_evolution_intro, p.seen_factory_welcome, p.seen_first_mission), (0, 1, 1));
    assert_eq!(p.colours, c.custom_colours);
    let back = career_from_sav(&parsed, &t);
    assert!(back.seen_factory_welcome && back.seen_first_mission && !back.seen_evolution_intro);
    assert_eq!(back.custom_colours, c.custom_colours);
}

/// The ten body-damage zones a garage car was parked with live at `FECar+0x6A8` (the park
/// 0x4EDD40 stores them, 0x4EDD83; the launch reads them for option 0x14D, 0x4EF420…0x4EF459) and
/// travel with the flat image Save 0x4EE8D0 / Load 0x4EE6A0 copy — so a front-end entry's reload
/// (0x4D3B52) keeps the dents. *(Until 2026-10-01 the port wrote and read 0 there — EV1 audit.)*
#[test]
fn the_parked_zones_ride_in_the_car_image() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let car_id = t.catalog.iter().find(|c| !c.sim.is_empty() && !c.paint_sets.is_empty()).unwrap().id;
    let mut c = Career::default();
    let mut owned = OwnedCar::new(car_id, 0, 0);
    owned.damage = [18.05, 17.5, 16.9, 10.4, 3.87, 7.74, 3.87, 10.96, 0.0, 0.25];
    c.garage.push(owned.clone());
    let parsed = SaveFile::parse(&career_to_sav(&c, &t, &Profile::new("Hans", 0)).write()).unwrap();
    let img = &parsed.player.as_ref().unwrap().cars[0];
    let f = |o: usize| f32::from_le_bytes([img.image[o], img.image[o + 1], img.image[o + 2], img.image[o + 3]]);
    assert_eq!(f(car::ZONE_DAMAGE), 18.05);
    assert_eq!(f(car::ZONE_DAMAGE + 9 * 4), 0.25);
    assert_eq!(car::ZONE_DAMAGE + 40, car::SETUP, "the ten zones end where the setup floats begin");
    assert_eq!(career_from_sav(&parsed, &t).garage[0].damage, owned.damage);
}

/// Stage 1's records passed one by one: the race that finishes the stage is the one 0x51AF90
/// offers the promotion after (`[0x69C8F0]` = that record, the first unpassed record now of
/// stage 2), and only that one.
fn finish_stage_one(c: &mut Career, t: &CareerTables) -> usize {
    let stage1: Vec<usize> = t.missions.iter().enumerate().filter(|(_, m)| m.stage == t.missions[0].stage).map(|(i, _)| i).collect();
    for (k, &i) in stage1.iter().enumerate() {
        let due = c.record_mission(t, i, true);
        assert_eq!(due.is_some(), k + 1 == stage1.len(), "offered after the stage's last record only (record {i})");
    }
    *stage1.last().unwrap()
}

/// An accepted Factory Driver promotion is `fac+0xC = −1` on the record whose race
/// finished the stage (0x51AF80 from `ACCEPTPRIZECAR` 0x51A6D0), saved and loaded with the whole
/// `nfs5.fac` image (`Missions`, 0x51A870 / 0x51A760); the offer test 0x51AF90 keys on the record
/// the last mission race filed, so after a profile reload (every front-end entry, 0x4D3B52) the
/// next mission race offers nothing — accepted or declined. *(Until 2026-10-01 the port kept the
/// accepted stages in its JSON only and offered stage 1's car again after the next race.)*
#[test]
fn a_promotion_is_offered_once_and_its_acceptance_rides_in_the_save() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    for accept in [true, false] {
        let mut c = Career::default();
        let last = finish_stage_one(&mut c, &t);
        let due = c.promotion_due(&t).expect("due after the stage's last race");
        assert_eq!(due.prize_car, t.missions[last].prize_car, "R's own `+0x10` (0x51B050)");
        let cars = c.garage.len();
        if accept {
            let car = c.accept_promotion(&t).expect("accepted");
            assert_eq!(car as i16, t.missions[last].prize_car);
            assert_eq!(c.garage.len(), cars + 1);
            assert!(c.promotion_due(&t).is_none(), "`+0xC = −1` answers no (0x51AFFA)");
        }
        // the launch save, then the next front-end entry's reload
        let sav = SaveFile::parse(&career_to_sav(&c, &t, &Profile::new("Hans", 0)).write()).unwrap();
        let mut back = career_from_sav(&sav, &t);
        assert_eq!(back.prize_taken.get(last).copied().unwrap_or(false), accept);
        assert_eq!(back.last_mission, None, "`[0x69C8F0]` is not in the save");
        assert!(back.promotion_due(&t).is_none(), "nothing raced since the load");
        // the next mission race: stage 2's first record
        let next = back.current_mission(&t).unwrap();
        assert_ne!(t.missions[next].stage, t.missions[last].stage);
        back.record_mission(&t, next, true);
        assert!(back.promotion_due(&t).is_none(), "stage 1's promotion is not offered again (accept = {accept})");
        back.record_mission(&t, next, false);
        assert!(back.promotion_due(&t).is_none(), "nor after a failed race");
    }
}

/// The original's own corner, ported as it is: with every record passed, 0x51AE80 relaunches
/// record 0 and 0x51AEF0 files it as R; with no unpassed record left 0x51AF90 answers yes unless
/// record 0's `+0xC` is −1 — so a passed replay of record 0 offers record 0's prize car once.
#[test]
fn every_mission_passed_replays_record_zero_with_its_own_offer() {
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut c = Career::default();
    for i in 0..t.missions.len() {
        c.record_mission(&t, i, true);
    }
    assert!(c.current_mission(&t).is_none());
    c.record_mission(&t, 0, true);
    let due = c.promotion_due(&t).expect("record 0's +0xC is a text id");
    assert_eq!(due.prize_car, t.missions[0].prize_car);
    c.accept_promotion(&t);
    c.record_mission(&t, 0, true);
    assert!(c.promotion_due(&t).is_none(), "taken once");
}

fn legacy_json(c: &Career, stages: &[i16]) -> Vec<u8> {
    let mut v = serde_json::to_value(c).unwrap();
    v["promotions_accepted"] = serde_json::json!(stages);
    v["eras_completed"] = serde_json::json!([]);
    v["finale"] = serde_json::Value::Null;
    serde_json::to_vec(&v).unwrap()
}

/// The one-time migration of the retired `career.json` (`save::migrate_legacy_json`): the accepted
/// stages fold into the profile's `.sav` as `fac+0xC = −1` on each stage's last record, every
/// other byte of the file stays, the JSON is set aside, and a second start finds nothing to do; a
/// JSON of another career is not folded; a profile without a `.sav` gets one from the JSON.
#[test]
fn the_legacy_json_is_folded_into_the_sav_once() {
    use nfs_game::save::{migrate_legacy_json, read_profile, write_profile, Migration};
    let Some(t) = tables() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = std::env::temp_dir().join(format!("nfs-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = Profile::new("Hans", 8);
    let json = dir.join("career.json");
    let mut c = Career::default();
    let last = finish_stage_one(&mut c, &t);
    c.last_mission = None;
    c.money = 54_321;
    // the profile as the launch saved it, the JSON beside it with the accepted stage
    write_profile(&dir, &p, &career_to_sav(&c, &t, &p)).unwrap();
    let before = std::fs::read(p.path(&dir)).unwrap();
    std::fs::write(&json, legacy_json(&c, &[t.missions[last].stage])).unwrap();
    assert_eq!(migrate_legacy_json(&json, &dir, &p, &t), Migration::Folded { marks: 1 });
    assert!(!json.exists() && dir.join("career.json.migrated").exists(), "set aside, kept");
    let after = std::fs::read(p.path(&dir)).unwrap();
    assert_eq!(after.len(), before.len());
    let rec = nfs_formats::career::MISSION_RECORD_SIZE;
    // `Missions` is the first module: its name, a u32 size, then the image
    let fac_at = b"Missions\0".len() + 4;
    let differ: Vec<usize> = (0..after.len()).filter(|&i| after[i] != before[i]).collect();
    assert!(!differ.is_empty() && differ.iter().all(|&i| (fac_at + last * rec + 0xC..fac_at + last * rec + 0x10).contains(&i)), "only R's +0xC moved: {differ:?}");
    let back = career_from_sav(&read_profile(&dir, &p).unwrap(), &t);
    assert!(back.prize_taken[last]);
    assert_eq!(back.money, 54_321);
    assert_eq!(migrate_legacy_json(&json, &dir, &p, &t), Migration::None, "one time");

    // another career's JSON (different Factory Driver results) is not folded
    std::fs::write(&json, legacy_json(&Career::default(), &[1, 2])).unwrap();
    let q = Profile::new("Greta", 3);
    write_profile(&dir, &q, &career_to_sav(&c, &t, &q)).unwrap();
    assert_eq!(migrate_legacy_json(&json, &dir, &q, &t), Migration::Mismatch);
    assert!(career_from_sav(&read_profile(&dir, &q).unwrap(), &t).prize_taken.iter().all(|x| !x));

    // no `.sav` yet: built from the JSON
    std::fs::write(&json, legacy_json(&c, &[t.missions[last].stage])).unwrap();
    let r = Profile::new("Otto", 1);
    assert_eq!(migrate_legacy_json(&json, &dir, &r, &t), Migration::Created);
    let made = career_from_sav(&read_profile(&dir, &r).unwrap(), &t);
    assert_eq!((made.money, made.missions.clone(), made.prize_taken[last]), (54_321, c.missions.clone(), true));

    // an unreadable JSON is set aside unread
    std::fs::write(&json, b"{ not json").unwrap();
    assert_eq!(migrate_legacy_json(&json, &dir, &r, &t), Migration::Unreadable);
    assert!(!json.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
