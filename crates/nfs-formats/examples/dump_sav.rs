//! Scratch tool: dump a `.sav`'s module table (offset/length per module, `PlayerInfo` fields,
//! accessor block) for the 2026-09-28 native-save round-trip task. Usage: `dump_sav <path>`.
use nfs_formats::sav::SaveFile;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_sav <path>");
    let data = std::fs::read(&path).expect("read");
    println!("file: {path} ({} bytes)", data.len());

    let save = SaveFile::parse(&data).expect("SaveFile::parse");
    println!("module order on disk: {:?}", save.order);
    println!();

    // Offset table: build incremental subsets in MODULE_ORDER and measure each module's contribution.
    {
        let mut prev_len = 0usize;
        let mut acc = SaveFile::default();
        for name in save.order.iter().filter(|n| n.as_str() != "accessor") {
            match name.as_str() {
                "Missions" => acc.missions = save.missions.clone(),
                "PartsMarket" => acc.parts_market = save.parts_market.clone(),
                "Tournament" => acc.tournament = save.tournament.clone(),
                "PlayerInfo" => acc.player = save.player.clone(),
                "TrackRecords" => acc.track_records = save.track_records,
                "UsedCarMarket" => acc.used_cars = save.used_cars.clone(),
                "Personalities" => acc.personalities = save.personalities.clone(),
                "Knockout" => acc.knockout = save.knockout.clone(),
                "CDPlayer" => acc.cd_player = save.cd_player.clone(),
                other => panic!("unexpected {other}"),
            }
            let len = acc.write().len();
            println!("module {name:<14} ends at offset {len:>7}  (body {} bytes)", len - prev_len);
            prev_len = len;
        }
        if save.accessors.is_some() {
            println!("module accessor       ends at offset {:>7}  (body {} bytes)", data.len(), data.len() - prev_len);
        }
    }
    println!();

    if let Some(m) = &save.missions {
        println!("Missions: {} bytes", m.len());
    }
    if let Some(p) = &save.parts_market {
        println!("PartsMarket: {} records x 0x168", p.len());
    }
    if let Some(t) = &save.tournament {
        println!(
            "Tournament: year={} remaining_tracks={} selected={} era_type={} current_event={} current_tournament={} club_mode={} table={} bytes",
            t.year, t.remaining_tracks, t.selected, t.era_type, t.current_event, t.current_tournament, t.club_mode, t.table.len()
        );
    }
    if let Some(p) = &save.player {
        println!("PlayerInfo:");
        println!("  cars: {}", p.cars.len());
        for (i, c) in p.cars.iter().enumerate() {
            println!(
                "    car[{i}]: image={} bytes car_id={} removed_parts={} installed_parts={} +0x68C(price)={} +0x690(sold)={} +0x698(mileage)={} +0x6A0(is_player)={} +0x6A4(purchase_price)={}",
                c.image.len(),
                c.car_id(),
                c.removed.len(),
                c.installed.len(),
                c.field_u32(0x68C),
                c.field_u8(0x690),
                c.field_u32(0x698),
                c.field_u8(0x6A0),
                c.field_u32(0x6A4)
            );
        }
        println!("  money: {}", p.money);
        println!("  picture: {}", p.picture);
        println!("  player_name: {:?}", p.player_name);
        println!("  driver_name: {:?}", p.driver_name);
        println!("  seen_evolution_intro: {}", p.seen_evolution_intro);
        println!("  seen_factory_welcome: {}", p.seen_factory_welcome);
        println!("  seen_first_mission: {}", p.seen_first_mission);
        println!("  colours: {:?}", p.colours.map(|c| format!("{c:#010x}")));
    }
    println!("TrackRecords module present: {}", save.track_records);
    if let Some(m) = &save.used_cars {
        println!("UsedCarMarket: year={} cars={}", m.year, m.cars.len());
    }
    if let Some(p) = &save.personalities {
        println!("Personalities: {p:?}");
    }
    if let Some(k) = &save.knockout {
        println!("Knockout: {} bytes = {:?}", k.len(), k);
    }
    if let Some(c) = &save.cd_player {
        println!("CDPlayer: {c:?}");
    }
    if let Some(a) = &save.accessors {
        println!("accessor block: {} entries", a.entries.len());
        for (name, value) in &a.entries {
            println!("  {name} = {value:?}");
        }
    } else {
        println!("accessor block: ABSENT");
    }

    // Round-trip check.
    let out = save.write();
    println!();
    println!("re-written: {} bytes (original {} bytes, diff {})", out.len(), data.len(), out.len() as i64 - data.len() as i64);
    if out == data {
        println!("BYTE-IDENTICAL round trip");
    } else {
        let n = out.len().min(data.len());
        let mut diffs = 0usize;
        let mut first = None;
        for i in 0..n {
            if out[i] != data[i] {
                diffs += 1;
                if first.is_none() {
                    first = Some(i);
                }
            }
        }
        println!("NOT identical: {diffs} differing bytes within common length {n}; first diff at offset {:?}", first);
        if data.len() != out.len() {
            println!("length differs: original={} written={}", data.len(), out.len());
        }
    }
}
