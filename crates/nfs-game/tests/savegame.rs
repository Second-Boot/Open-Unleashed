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
