//! The front end's accessor answers — `App::accessor_value`, the match from an accessor's name
//! (`OwnedCars.Car(SHORT)`, `Audio.SfxLevel`, …) to the string a widget shows.
//!
//! Moved out of `main.rs` unchanged on 2026-09-21, byte for byte, as the first step of splitting
//! that file: it is a six-hundred-line lookup with no state of its own, and it is where front-end
//! answers are added and fixed. Every arm keeps the addresses it was written against.
use super::*;

impl App {
    /// Values for the front-end accessors an `NFS5.AccessorWatch` can name. Only the ones the port
    /// actually has state for are answered; anything else returns `None` and the watch is left
    /// blank rather than filled with something invented. The `(MODIFIER)` suffix the layouts use
    /// (`OwnedCars.Car(SHORT)`) selects a shorter form of the same value.
    pub(crate) fn accessor_value(&self, name: &str) -> Option<String> {
        let (base, modifier) = match name.split_once('(') {
            Some((b, m)) => (b.trim(), m.trim_end_matches(')')),
            None => (name, ""),
        };
        let menu = self.fe.menu.as_ref();
        let career = self.career.as_ref();
        // **`CARSETTINGS.CAR` is a forwarder, not an accessor.** `FECarSettingsCarAccessor`'s vtable
        // 0x5B4CE0 has 37 slots and **every one of them** is a thunk: 0x4B8820…0x4B88D0 each call
        // 0x4B88F0 for a delegate and `jmp`/`call` the same slot on it. 0x4B88F0 switches on
        // `Game.GameType`:
        //
        // * **4** (Evolution) → `OwnedCars.Car` (0x4B892E)
        // * **9** (Factory Driver) → `FactoryDriver.Car` (0x4B8958)
        // * anything else → **`zMergedList.Car`** (0x4B896F)
        //
        // The port used to treat it as a synonym of `OwnedCars.Car` throughout, which is right only
        // in a career. `Game.GameType` here is what the port can tell: a pending career event is the
        // Evolution race, a factory-driver mission is 9, and everything else is the single-race 0.
        let base = if base == "CARSETTINGS.CAR" { self.car_settings_delegate() } else { base };
        Some(match base {
            "Showcase.Text" => self.showcase_text(modifier.parse::<usize>().ok()?)?,
            // the tournament screen. Each field is one the `nfs5.trn` record already carries, and
            // `(LOCATION)` is what 0x506992 makes it: the accessor looks up
            // `Tournament.RemainingTracks`, calls its vtable +0x70 for an id and hands that to the
            // `FEStrings` lookup 0x4E06D0 — the name of the event the tournament is on.
            //
            // `(DIFFICULTY)` is `FEStrings[450 + n]` (0x506823 adds 0x1C2 to a two-byte field and
            // looks it up), and 450…452 are the three names `Game.Difficutly` uses. The port takes
            // `n` from the record's `+0xE6`, which is the difficulty it already parsed; the getter
            // reads `+0x78` of a run-time object instead, so the two are **MEDIUM**, not confirmed
            // to be the same field.
            // the bare accessor's caption is its own row 0 — slot 4 (0x4BC4E0) hands `[this+0x18]`,
            // which starts at 0 (0x4BC224), to the row-text slot. §4.105.
            "Tournament.RemainingTracks" if modifier.is_empty() => {
                let c = self.career.as_ref()?;
                let t = c.current()?;
                let done = c.career.tournaments.get(&t.id).map_or(0, |s| s.events.len());
                c.tables.track_name(t.events.get(done)?.track_id).to_uppercase()
            }
            "Tournament.Tournament" => {
                let c = self.career.as_ref()?;
                let t = c.current()?;
                match modifier {
                    "SHORT" | "" => self.fe.strings.get(t.name_string_id)?.to_uppercase(),
                    "DESCRIPTION" => self.fe.strings.get(t.description_string_id)?.to_string(),
                    "CARRESTRICTION" => self.fe.strings.get(t.restriction_string_id)?.to_string(),
                    // 0x50666D and 0x506749: both are `sprintf("%s", 0x4DBF80(value))` — the
                    // money formatter's thousands commas and **no currency sign**. Nothing in the
                    // image formats one: there is no `$%d` or `$%s` anywhere in it, and 0x4DBF80's
                    // own format strings are `FEStrings` 1700…1703, `"%d,%03d,%03d"` and their
                    // signed and quoted variants. The `$` this port printed was its own.
                    "ENTRYFEE" => with_thousands(t.entry_fee as i64),
                    "PRIZEMONEY" => with_thousands(*t.prize.first()? as i64),
                    "DIFFICULTY" => self.fe.strings.get(450 + t.difficulty_level.min(2) as u32)?.to_string(),
                    "LOCATION" => {
                        // the event the tournament is on: the first one the player has not finished
                        let done = c.career.tournaments.get(&t.id).map_or(0, |s| s.events.len());
                        let ev = t.events.get(done.min(t.events.len().saturating_sub(1)))?;
                        c.tables.track_name(ev.track_id).to_uppercase()
                    }
                    // the record on the track the tournament is on. 0x506895 asks
                    // `Tournament.RemainingTracks` for the current track id, 0x4FB120 finds that
                    // `nfs5.trk` record and answers `record + 0x1B0` — the **first** record-holder
                    // entry, the fastest — and 0x5068BB takes its `+0x48`, the time, into the
                    // formatter 0x4356E0.
                    "RECORDTIME" => {
                        let done = c.career.tournaments.get(&t.id).map_or(0, |s| s.events.len());
                        let ev = t.events.get(done.min(t.events.len().saturating_sub(1)))?;
                        format_lap_time(c.tables.track(ev.track_id)?.records.first()?.time)
                    }
                    // `"%d"` of `trn+0xD3C` (0x506929), the tournament's own completion counter
                    "TIMESCOMPLETED" => c.career.tournaments.get(&t.id).map_or(0, |s| s.completions).to_string(),
                    _ => return None,
                }
            }
            // the end-of-era lines on `VictoryMessage.lay`. Both read the era's finale tournament
            // record: `ENDERAHEADER` is `FEStrings[[rec+0x24]]` (0x506389) and `ENDERADESCRIP`
            // `FEStrings[[rec+0x28]]` (0x5063DE), and +0x24 and +0x28 are the port's
            // `title_string_id` and `description_string_id`. The record is what
            // `0x504990(0x504800())` returns; the port takes the era's hidden finale, which its
            // own `visibility == 2` marks.
            // the price of the car the dealer screen is on: 0x5062FC takes `DealerCar.Car`, asks it
            // for the record (vtable +0x90), runs it through the age curve 0x4EE220 — which is the
            // port's own `dealer_price` — and prints the result with 0x4DBF80's thousands commas.
            // `0x506250`'s first arm: `"CASH"` is `0x4DBF80(0x5012D0())` — the player's money
            // through the thousands formatter. `NFS5.ACCOUNTTHING` is its only consumer (§4.128).
            "Tournament.Data" if modifier == "CASH" => with_thousands(career?.career.money as i64),
            "Tournament.Data" if modifier == "DEALERCARPRICE" => {
                let c = self.career.as_ref()?;
                let car = self.logo_car(Some("DealerCar.Car"), None)?;
                with_thousands(car.dealer_price(c.career.year) as i64)
            }
            "Tournament.Data" if matches!(modifier, "ENDERAHEADER" | "ENDERADESCRIP") => {
                let c = self.career.as_ref()?;
                let list = c.tables.era_tournaments(c.career.era);
                let t = list.iter().find(|t| t.visibility == 2).or_else(|| list.last())?;
                let id = if modifier == "ENDERAHEADER" { t.title_string_id } else { t.description_string_id };
                self.fe.strings.get(id)?.to_string()
            }
            // the factory-drive mission the career is on (`missionbrief.lay`, `missionmap.lay`).
            // `NARRNAME` is `FEStrings[(i16)[mission+0x1E] + 0x9C4]` — 0x519DCA reads the field and
            // 0x519DCE adds 2500 — `MISSONDESC` is `FEStrings[[mission+0]]`, the id 0x519D67 loads
            // straight out of the record, and `ENDOFLEVEL` is `FEStrings[[record+0xC]]` (0x519E31).
            // All three offsets are ones the port already parses: `narrator` at +0x1E,
            // `intro_string_id` at +0 and `promotion_string_id` at +0x0C.
            "Factory.Data" if matches!(modifier, "NARRNAME" | "MISSONDESC" | "TRACKNAME" | "ENDOFLEVEL") => {
                let c = self.career.as_ref()?;
                let m = &c.tables.missions[c.career.current_mission(&c.tables)?];
                match modifier {
                    "NARRNAME" => self.fe.strings.get(2500 + m.narrator.max(0) as u32)?.to_string(),
                    "MISSONDESC" => self.fe.strings.get(m.intro_string_id)?.to_string(),
                    // the promotion line at the end of a stage: 0x519E31 reads `[record+0xC]` and
                    // looks it up, and +0x0C is the port's `promotion_string_id`
                    "ENDOFLEVEL" => self.fe.strings.get(m.promotion_string_id)?.to_string(),
                    _ => c.tables.track_name(m.track_id).to_uppercase(),
                }
            }
            // one of the factory team's profiles (`teamprofile.lay`). Each getter ends in the same
            // `FEStrings` lookup with the profile index scaled by four: `profilename` is
            // `0xDAD + 4n` (0x519F1F), `profiletitle` `0xDAE + 4n` (0x519FBD), `profileage`
            // `0xDAF + 4n` (0x519F6E) and `profiledesc` the next — 3501 "Dieter", 3502 "Porsche
            // Executive", 3503 "45", 3504 his description, then the same four for Billy.
            // CONFIRMED, and `FEStrings.csv` carries exactly 28 of them: seven profiles.
            "Factory.Data" => {
                let base = match modifier {
                    "profilename" => 3501,
                    "profiletitle" => 3502,
                    "profileage" => 3503,
                    "profiledesc" => 3504,
                    _ => return None,
                };
                self.fe.strings.get(base + 4 * self.fe.factory_profile as u32)?.to_string()
            }
            // a range setting the port has no other state for — the force-feedback volumes and
            // the gamma — shows its number
            // A range setting asked for as a per cent. `FEAccessor::text("PERCENT")` is
            // 0x4BC558…0x4BC5F0: with a list of one entry or none it answers a flat **100**
            // (0x4BC5D8), otherwise `index · 100 / (count − 1)` (0x4BC5A5…0x4BC5C1) **clamped at
            // 100** (0x4BC5C7), printed with a bare `"%d"` — no per-cent sign. An int range's
            // index is the value less its minimum and its count is `max − min + 1` (0x4BA329,
            // 0x4BA34A), so this works out to `(v − min) · 100 / (max − min)`.
            //
            // The sign on screen is the layout's, not the accessor's: `audioreal.lay` carries five
            // `text=%` labels beside its five `(PERCENT)` watches and `advgraphics.lay` one beside
            // its one, so appending a `%` here printed it twice.
            _ if modifier == "PERCENT" && RANGE_ACCESSORS.iter().any(|(n, ..)| *n == base) => {
                let (v, lo, hi) = self.range_value(base)?;
                if hi <= lo {
                    "100".to_string()
                } else {
                    ((v - lo) * 100 / (hi - lo)).min(100).to_string()
                }
            }
            "FF.TotalVolume" | "FF.RoadVolume" | "FF.EngineVolume" | "FF.CollisionVolume"
            | "FF.RoadGripVolume" | "Graphics.GammaCorrection" => self.range_value(base)?.0.to_string(),
            // a multiple-choice setting: the text is the `FEStrings` entry its chosen index names
            _ if CHOICE_ACCESSORS.iter().any(|(n, _, _)| *n == base) => {
                let (i, options) = self.choice_index(base)?;
                self.fe.strings.get(options[i])?.to_string()
            }
            // the car-compare screen's five numbers — `CarCompare.Data(<n>CarCompareCar<m>)`, the
            // same source name its bars read (`fn compare_rating`)
            // 0x50E94C prints it with **`"%02d"`** — a rating under ten reads `05`, not `5`
            "CarCompare.Data" => format!("{:02}", self.compare_rating(modifier)?),
            // the network screens (`netplay::Lobby`): the names come out of the layouts themselves
            "Comms.IP" => self.net.lobby.as_ref()?.ip.clone(),
            "Comms.Data" => {
                // once a session is open the lobby screens read from it, not from the browser
                if let Some(n) = self.net.session.as_ref() {
                    return Some(match modifier {
                        "GAMENAME" => n.game_name(),
                        "MYIPADDRESS" => n.my_address(),
                        "POSITIONALUPDATES" => if n.positional { "ON" } else { "OFF" }.to_string(),
                        "TRACKNAME" => menu.map(|m| m.track().to_uppercase())?,
                        "TRACKLAPS" => menu?.laps.to_string(),
                        "TRACKDIRECTION" => if menu?.backwards {
                            "REVERSE"
                        } else {
                            "FORWARD"
                        }
                        .to_string(),
                        _ => return None,
                    });
                }
                let l = self.net.lobby.as_ref()?;
                match modifier {
                    "GAMENAME" => l.game_name(),
                    // `[desc+0x44]` copies the socket's own record out of `sock+0x08` (0x5878F0)
                    "MYIPADDRESS" => l.my_address(),
                    // the `0xBE` broadcast of §5.2, which `Comms.Positional` switches
                    "POSITIONALUPDATES" => if l.positional { "ON" } else { "OFF" }.to_string(),
                    // the race the host chose. Which message carries it to a joiner has not been
                    // traced, so these show this machine's own settings and no more.
                    "TRACKNAME" => menu.map(|m| m.track().to_uppercase())?,
                    "TRACKLAPS" => menu?.laps.to_string(),
                    "TRACKDIRECTION" => if menu?.backwards {
                        "REVERSE"
                    } else {
                        "FORWARD"
                    }
                    .to_string(),
                    _ => return None,
                }
            }
            // **not** off/on. 0x50BB03 registers it with 0x4BB680 and the option list 0x5B9674,
            // which is `0x119, 0x11A, 0` — `FEStrings` **281 "input exchange"** and **282
            // "positional"** — and 0x50BB1C then calls the setter (slot 11) with **1**, so the
            // screen opens on "positional". §4.106.
            "Comms.Positional" => {
                // with no session yet the answer is the accessor's own starting index, 1
                let on = match self.net.session.as_ref() {
                    Some(n) => n.positional,
                    None => self.net.lobby.as_ref().map_or(true, |l| l.positional),
                };
                self.fe.strings.get(POSITIONAL_OPTIONS[usize::from(on)]).unwrap_or_default().to_uppercase()
            }
            // this port has no modem, so the modem screen's fields are blank, which is what a
            // machine without one shows
            "Comms.PhoneNumber" => String::new(),
            // chat is carried by a message the port has not traced, so the box stays empty
            "Comms.ChatInput" => String::new(),
            "Chronicle.Year" => self.chronicle_year().to_string(),
            "Graphics.Resolutions" => "640 x 480 x 16".to_string(),
            "Controller.AllControllers" | "Controller.AllPads" => {
                let pads_only = name == "Controller.AllPads";
                self.gamepads
                    .devices()
                    .into_iter()
                    .find(|(_, is_pad)| !pads_only || *is_pad)
                    .map(|(n, _)| n.to_uppercase())
                    .unwrap_or_default()
            }
            "Game.Damage" => {
                let id = BOOL_OPTIONS[usize::from(menu?.damage)];
                self.fe.strings.get(id).unwrap_or_default().to_string()
            }
            "Game.NumOpponents" => menu?.opponents.to_string(),
            "Game.NumKOOpponents" => menu?.ko_opponents.to_string(),
            "Game.Difficutly" => {
                ["EASY", "MEDIUM", "HARD"][(self.race_skill as usize).min(2)].to_string()
            }
            "Track1.Track" | "LoopedTrack.Track" | "Knockout.Track" | "Record.Track"
            | "Track.Track" => {
                let t = menu
                    .map(|m| m.track().to_string())
                    .unwrap_or_else(|| self.track.name.clone());
                match modifier {
                    // the track's own best time, which is what the port's records table holds
                    "RECORDTIME" => {
                        format_ticks(self.records.track(&t)?.entries.first()?.time as u64)
                    }
                    // the track record's `+0x78`, as `FEStrings[0x1C2 + it]` — 450…452, the same
                    // three names `Game.Difficutly` uses (0x4FB730) — and the **short** name, which
                    // is the name id plus 40 (0x4FA845 adds 0x28), not the full one
                    "DIFFICULTY" | "SHORT" => {
                        let c = career?;
                        let rec = c.tables.tracks.iter().find(|r| c.tables.strings.text(r.name_string_id).eq_ignore_ascii_case(&t))?;
                        if modifier == "SHORT" {
                            self.fe.strings.get(rec.name_string_id + 40)?.to_uppercase()
                        } else {
                            self.fe.strings.get(450 + rec.difficulty.min(2))?.to_string()
                        }
                    }
                    _ => t.to_uppercase(),
                }
            }
            // the car-setup screen: the 17 floats the original keeps on the car and reads back
            // nowhere (`RE/notes/layouts.md`), so the port shows them and changes nothing
            b if nfs_game::career::CAR_SETUP_NAMES.contains(&b) => {
                let c = career?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let k = nfs_game::career::CAR_SETUP_NAMES
                    .iter()
                    .position(|n| *n == b)?;
                let v = c.career.garage.get(idx)?.setup[k];
                // shown through the accessor's own index and format, not as the raw float —
                // `career::car_setup_text` has the chain (0x4B9C10 → 0x4BC240 → 0x4B9AB0)
                nfs_game::career::car_setup_text(k, v)
            }
            // the repair screen: what each category costs on the selected car (`RepairCost`
            // 0x518880 per installed part, summed by the QUICKFIX screen 0x50F110)
            b if b.starts_with("QUICKFIX.") => {
                let c = career?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let kinds = quickfix_kinds(&b["QUICKFIX.".len()..])?;
                with_thousands(c.career.repair_cost(&c.tables, idx, kinds))
            }
            // The two baskets' totals. `parts.lay` §19 watches `Parts.ShoppingList(TOTAL)` and
            // `advancedrepair.lay` §24 `Parts.RepairShoppingList(TOTAL)`, and both accessors
            // answer the same way: 0x516480 and 0x517500 compare the modifier against the literal
            // **`TOTAL`** (0x5DCE04) and, on a match, print `this->slot6(modifier)` with
            // **`"%d"`** (0x5CD1B0) — a bare number, no currency sign and no thousands commas.
            // The sum itself (0x517580 for the repair one) walks the basket and takes each
            // entry's repair cost (0x518900) or its price (0x518880) by the entry's own
            // `[+4]` byte, so an **empty** basket prints **`0`**, not nothing. Any other modifier
            // answers null, which is `CANTFINDSTRING` in the original (§4.10) and nothing here.
            //
            // The `$` this port used to print was its own.
            "Parts.ShoppingList" if modifier == "TOTAL" => format!("{}", career?.basket_total()),
            // **and it counts only the categories that are ticked.** The screen's own sum,
            // 0x50F110, takes the car from `OwnedCars.Car` (0x4F5A20) and then adds one
            // category's cost per `if (accessor->slot27())` — ten of them, 0x50F14A onwards, each
            // guarded by its own `QUICKFIX.*` bool and each cost from its own getter (0x4EFBC0,
            // 0x4EFC20, 0x4EFC80, …). All ten bools start false (§4.84) and 0x50F0D1 resets them
            // to false as a group, so the total starts at **0** and grows as boxes are ticked —
            // where this port used to sum every damaged part unconditionally. §4.96.
            "Parts.RepairShoppingList" if modifier == "TOTAL" => {
                let c = career?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let mut kinds: Vec<u16> = Vec::new();
                for name in &self.fe.repair_ticks {
                    kinds.extend(quickfix_kinds(name).unwrap_or(&[]));
                }
                kinds.sort_unstable();
                kinds.dedup();
                format!("{}", c.career.repair_cost(&c.tables, idx, &kinds))
            }
            // A `Parts.*` accessor with **no** modifier: `FEAccessorWatch::tick` asks vtable +0x10
            // (§4.10), which is the base's 0x4BC4E0 — `this->slot12([this+0x18])`, the text of the
            // accessor's **current row**, and that index is 0 out of the constructor and nothing
            // in this port moves it.
            //
            // For `Parts.AllParts` row 0 is the first section header (0x5152E0's `row == 0` arm),
            // whose text is `FEStrings` 2550, "PARTS AVAILABLE FOR %s", with the car (0x5157A1).
            // For `Parts.InstallList` slot 12 is 0x516B50: `"%s %s"` of the row's part name and
            // its type name, so row 0 is the first installed part.
            "Parts.AllParts" if modifier.is_empty() => {
                let c = self.career.as_ref()?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let car = c.tables.car(c.career.garage.get(idx)?.car_id)?;
                self.fe.strings.get(2550)?.replacen("%s", &car.display_name.to_uppercase(), 1)
            }
            "Parts.InstallList" if modifier.is_empty() => {
                let c = self.career.as_ref()?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let p = c.career.installed_parts(&c.tables, idx).into_iter().next()?;
                format!("{} {}", self.fe.strings.get(p.name_string_id).unwrap_or(""), p.kind_name)
            }
            // A `Parts.*` accessor **with** a modifier goes to vtable +0x14, and these two classes
            // override it with a function that never looks at the modifier at all: 0x515D60 and
            // 0x516BA0 both take the accessor's current row and answer
            // `FEStrings[part->nameStringId + 0x1F4]` — the part's **description**, 500 ids above
            // its name. So `Parts.InstallList(DESC)` is the description of the first installed
            // part, and `DESC` could have been any word.
            //
            // `Parts.AllParts(DESC)` is not portable: its current row is **0**, a section header
            // (§4.x), and 0x5152E0's header arms leave the fourth and fifth out-parameters
            // untouched (0x5152EE, 0x515346), so 0x515DCA reads an uninitialised stack slot. The
            // original's answer there is undefined; this port draws nothing.
            "Parts.InstallList" => {
                let c = self.career.as_ref()?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let p = c.career.installed_parts(&c.tables, idx).into_iter().next()?;
                self.fe.strings.get(p.name_string_id + 500)?.to_string()
            }
            // one cell of `locationsetup.lay`'s table. The modifier is `<row>TRACK<FIELD>`:
            // 0x4BA838 takes the leading digit as the row (the same shape the car-compare bars
            // use), 0x4BA786 reads `Game.NumKOOpponents` for how many rows there are, and
            // 0x4BA7CF answers the empty string for a row at or past that count. `TRACKNUMBER` is
            // the row plus one (0x4BA800 formats `ebp + 1` with `"%d"`).
            "Knockout.TrackNumber" => {
                let m = menu?;
                let (row, field) = match modifier.as_bytes().first() {
                    Some(c) if c.is_ascii_digit() => (usize::from(c - b'0'), &modifier[1..]),
                    _ => (m.ko_sel, modifier),
                };
                if row >= m.ko_opponents as usize {
                    return None;
                }
                let slot = m.ko.get(row)?;
                match field {
                    "TRACKNUMBER" => (row + 1).to_string(),
                    "TRACKNAME" => m.tracks.get(slot.track_idx)?.to_uppercase(),
                    "TRACKLAPS" => slot.laps.to_string(),
                    // **the short, capitalised forms.** 0x4BAA3B adds **0x23** to the traffic
                    // accessor's own string id and 0x4BAA03 adds **0x14** to the direction's —
                    // `FEStrings` 435…438 (`HEAVY`, `MED`, `LIGHT`, `NONE`) and 430…433 (`FWD`,
                    // `BKW`, …), the `…SHORT` rows of the sheet, not the sentence forms the
                    // combo boxes show. §4.136.
                    "TRACKTRAFFIC" => self.fe.strings.get(TRAFFIC_OPTIONS[(slot.traffic as usize).min(3)] + 0x23)?.to_string(),
                    "TRACKDIRECTION" => self.fe.strings.get(DIRECTION_OPTIONS[usize::from(slot.backwards)] + 0x14)?.to_string(),
                    // the same two the track accessor answers, for the row's own track: the
                    // record's `+0x78` as `FEStrings[0x1C2 + it]` (0x4BA8F5) and its best time
                    // (0x4BA913)
                    "DIFFICULTY" | "RECORDTIME" => {
                        let name = m.tracks.get(slot.track_idx)?;
                        if field == "RECORDTIME" {
                            format_ticks(self.records.track(name)?.entries.first()?.time as u64)
                        } else {
                            let c = career?;
                            let rec = c.tables.tracks.iter().find(|r| c.tables.strings.text(r.name_string_id).eq_ignore_ascii_case(name))?;
                            self.fe.strings.get(450 + rec.difficulty.min(2))?.to_string()
                        }
                    }
                    _ => return None,
                }
            }
            "Track1.Laps" | "Knockout.Laps" => match menu {
                Some(m) if base == "Knockout.Laps" => m.ko.get(m.ko_sel).map_or(m.laps, |k| k.laps).to_string(),
                Some(m) => m.laps.to_string(),
                None => self.num_laps.to_string(),
            },
            // the screen's own option list, not a bare number: `FEStrings` 403, 402, 401, 400.
            // (`Knockout.Traffic` belongs on this arm too — its body has always tested for it and
            // the pattern has always left it out, so `locationsetup.lay`'s combo read blank. The
            // sweep's caption axis is what showed it; the blank count never could, because the
            // combo draws its own box. §4.93.)
            "Track1.Traffic" | "Knockout.Traffic" => {
                let t = match menu {
                    Some(m) if base == "Knockout.Traffic" => m.ko.get(m.ko_sel).map_or(m.traffic, |k| k.traffic),
                    Some(m) => m.traffic,
                    None => 0,
                };
                self.fe.strings.get(TRAFFIC_OPTIONS[(t as usize).min(3)])?.to_string()
            }
            "Track1.Direction" | "Knockout.Direction" => {
                let b = match menu {
                    Some(m) if base == "Knockout.Direction" => m.ko.get(m.ko_sel).map_or(m.backwards, |k| k.backwards),
                    Some(m) => m.backwards,
                    None => self.backwards,
                };
                self.fe.strings.get(DIRECTION_OPTIONS[usize::from(b)])?.to_string()
            }
            // `"%s %s"` of the car's era and its class, as `FEStrings[era − 10]` and
            // `FEStrings[class + 13]` (0x4F0AAE…0x4F0AD4): 521 − 10 is "Classic" and 501 + 13 is
            // "3", so a `'50 356` reads "Classic 3".
            b if modifier == "CLASSLISTING" && matches!(b, "OwnedCars.Car" | "DealerCar.Car" | "GenericCar.Car" | "UsedCar.Car" | "zMergedList.Car") => {
                let car = self.logo_car(Some(base), None)?;
                let era = self.fe.strings.get(car.era_string_id.checked_sub(10)?)?;
                let class = self.fe.strings.get(car.class_string_id + 13)?;
                format!("{era} {class}")
            }
            // a car's short form is its family, the same one `zMergedList.Car(SHORT)` gives: the
            // accessor's `SHORT` hands the index to a virtual of its own (0x4BC54D) rather than
            // falling through to the display name.
            b if modifier == "SHORT" && matches!(b, "OwnedCars.Car" | "DealerCar.Car" | "GenericCar.Car" | "CARSETTINGS.CAR") => {
                self.logo_car(Some(base), None)?.family.to_uppercase()
            }
            // the dealer's and the car-select list's own car by its full name — the same car their
            // `SHORT` and `CLASSLISTING` forms above resolve. (Until 2026-09-25 these fell into the
            // garage arm below, so `dealership.lay`'s name line showed the player's own car.)
            "DealerCar.Car" | "GenericCar.Car" => self.logo_car(Some(base), None)?.display_name.to_uppercase(),
            "OwnedCars.Car" | "CARSETTINGS.CAR" => {
                // the garage screens ask the same accessor for the car's money and damage figures
                let c = career?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                match c.career.garage.get(idx).and_then(|o| self.car_field(c, o, modifier)) {
                    Some(v) => v,
                    None => c
                        .career
                        .garage
                        .get(idx)
                        .and_then(|o| c.tables.car(o.car_id))
                        .map(|e| e.display_name.to_uppercase())
                        .unwrap_or_else(|| self.specs.name.to_uppercase()),
                }
            }
            // the car the front end has selected, and (quick race puts the field in every car) the
            // opponents' car, which this port takes from the same list.
            //
            // **Only the name** for the plain and `SHORT` forms; everything else is a field of the
            // *market* record, which is an `OwnedCar` with its own mileage, parts and asking price
            // (`RE/notes/career.md` §10.5). The port generates that market now — `Career::market`,
            // filled by `refresh_market` from 0x4F80C0 — so `UsedCar.Car(MILEAGE)`,
            // `(ASKINGPRICE)`, `(…DAMAGE)` and `(…DAMAGECOST)` all come from the selected market
            // car, the one `usedcar_sel` names.
            "UsedCar.Car" if !matches!(modifier, "" | "SHORT") => {
                let c = career?;
                self.car_field(c, self.used_car()?, modifier)?
            }
            "UsedCar.Car" => {
                let c = career?;
                let e = c.tables.car(self.used_car()?.car_id)?;
                if modifier == "SHORT" { e.family.to_uppercase() } else { e.display_name.to_uppercase() }
            }
            // the same five strings the drop-down lists — `SHORT` is the same jump table
            // (0x4F57A0), so both forms answer alike
            "OpponentCar.Car" => {
                let m = menu?;
                let c = nfs_game::opponents::OpponentChoice::from_index(m.opp_car as u32);
                self.fe.strings.get(c.string_id())?.to_uppercase()
            }
            "zMergedList.Car" | "PrizeCars.Car" => match menu {
                _ if !matches!(modifier, "" | "SHORT") => return None,
                // `(SHORT)` asks for the short form, which for a car is its family name
                Some(m) if modifier == "SHORT" => (*m.car()?).family.to_uppercase(),
                Some(m) => (*m.car()?).display_name.to_uppercase(),
                None => self.specs.name.to_uppercase(),
            },
            // All seven colour combos count the same way — the paint set's number from 1, which
            // is what `combo_rows` enumerates (§4.91). The original keeps four separate values
            // per car (exterior, interior, and the two stripes) and this port keeps one, so the
            // other three read the exterior's: an APPROXIMATION, and the one this arm carries.
            "OwnedCar.ExtColour" | "OwnedCar.IntColour" | "OwnedCar.S1Colour" | "OwnedCar.S2Colour"
            | "GenericCar.Colour" | "DealerCar.Colour" | "DealerCar.IntColour" => {
                format!("{}", menu.map(|m| m.paint_idx + 1).unwrap_or(1))
            }
            // the compare screen's right-hand car, the one `GenericCar2.Car` steps with `car2_idx`
            "GenericCar2.Car" => match menu {
                _ if !matches!(modifier, "" | "SHORT") => return None,
                Some(m) if modifier == "SHORT" => m.cars.get(m.car2_idx)?.family.to_uppercase(),
                Some(m) => m.cars.get(m.car2_idx)?.display_name.to_uppercase(),
                None => return None,
            },
            "Game.GameType" => "QUICK RACE".to_string(),
            // the key-binding rows of `GameCtrl.lay` / `fluffctrl.lay`: `"keyboard %s"` over
            // 0x51C050's name for the row's key (`nfs_game::keyname`), and nothing for a binding of
            // kind 0 — what a row shows after 0x51DCC0 took its control for another (0x51C997).
            // The binding is `fe.txt`'s, rebindable on the screen (`nfs_input::bindings`);
            // `CameraView` and `LookBehind` are `I_*` bindings this port does not read, so they
            // keep the port's own map. `(ARC)` is this port's plumbing, not a layout modifier: the
            // row's draw asks it for the arc 0x51C050 picked, which the original's draw reads
            // straight off the binding.
            b if b.starts_with("Controller.") => {
                let action = controller_action(b)?;
                let (text, arc) = match nfs_input::binding_id(action).and_then(|i| self.input.bindings.get(i)) {
                    Some(b) if b.kind == 3 => nfs_game::keyname::keyboard_row(b.key, &self.fe.strings),
                    Some(b) => nfs_game::keyname::pad_row(b.kind, b.device, b.index, b.dir, b.pov, &self.fe.strings),
                    None => nfs_game::keyname::keyboard_row(nfs_input::key_code_for(action)?, &self.fe.strings),
                };
                match modifier {
                    "ARC" => arc?.to_string(),
                    _ => text,
                }
            }
            // the options screens: the levels the port actually mixes with (`fe.txt`, 0..127)
            // the CD player's now-playing line, and the sound-quality setting (`AUDIOQUALITY`)
            // `FECDPlayerAccessor`'s `STATUS` is one of two strings: `FEStrings[0x2857]`
            // "playing..." while something is playing and `[0x2858]` "stopped." otherwise
            // (0x4BF0BA, 0x4BF0CE). The front end plays nothing, so it is the second.
            "CDPlayer.Title" if modifier == "STATUS" => {
                let playing = self.snd.music.as_ref().is_some_and(|m| m.now_playing().is_some());
                self.fe.strings.get(if playing { 10327 } else { 10328 })?.to_string()
            }
            // The three timing lines. All of them end in `FEStrings[0x2859]` — `"%02d:%02d"` — over
            // a millisecond count divided by 1000, then `/60` and `%60` (0x4BED1F, 0x4BEF6E).
            //
            // * `SelectedList(TOTALPLAY)` sums `track + 0x20` over the whole playlist (0x4BED16);
            // * `Title(TRACKPLAY)` is the current track's own length;
            // * `Title(SONGTIMEREMAIN)` is the elapsed time, or the length minus it when the
            //   `CDPlayer.View` toggle is set (0x4BEF66). Nothing is playing in the front end, so
            //   the elapsed time is zero.
            "CDPlayer.SelectedList" | "CDPlayer.Title"
                if matches!(modifier, "TOTALPLAY" | "TRACKPLAY" | "SONGTIMEREMAIN") =>
            {
                let list: Vec<usize> = match self.snd.music.as_ref() {
                    Some(m) => m.playlist().to_vec(),
                    None => self.snd.audio_settings.playlist.iter().map(|i| *i as usize).collect(),
                };
                let len = |i: usize| self.snd.music_lengths.get(i).copied().unwrap_or(0) as u64;
                let ms = match modifier {
                    "TOTALPLAY" => list.iter().map(|i| len(*i)).sum::<u64>(),
                    "TRACKPLAY" => len(list.first().copied().unwrap_or(0)),
                    // the elapsed time, which is 0 with nothing playing; `CDPlayer.View` would
                    // make it the remainder instead
                    _ if self.choice_index("CDPlayer.View").is_some_and(|(i, _)| i != 0) => len(list.first().copied().unwrap_or(0)),
                    _ => 0,
                };
                let secs = ms / 1000;
                format!("{:02}:{:02}", secs / 60, secs % 60)
            }
            // the title line. 0x4BEE70 reads the song table at the accessor's own index — the song
            // `SKIPFWD` / `SKIPBK` step to (0x4BEDC0) — and answers the empty string only when the
            // table itself is empty
            "CDPlayer.Title" if modifier.is_empty() => {
                let names = self.cd_names();
                let table = Self::cd_table(&names);
                let (_, n) = table.get(self.fe.cd_title.min(table.len().checked_sub(1)?))?;
                nfs_audio::music::split_title(n).0.to_uppercase()
            }
            "Audio.Quality" => self.snd.audio_settings.audio_quality.to_string(),
            // the draw-distance setting, as the original's index into `0x5CE90C` = {0.5, .67, .83, 1}
            "Graphics.ViewDistance" => {
                let idx = [0.5f32, 0.67, 0.83, 1.0]
                    .iter()
                    .position(|v| (v - self.detail).abs() < 0.01)
                    .unwrap_or(3);
                idx.to_string()
            }
            "Audio.MusicLevel" => self.snd.audio_settings.music_level.to_string(),
            "Audio.SfxLevel" => self.snd.audio_settings.sfx_level.to_string(),
            "Audio.EngineLevel" => self.snd.audio_settings.engine_level.to_string(),
            "Audio.SpeechLevel" => self.snd.audio_settings.speech_level.to_string(),
            "Audio.AmbientLevel" => self.snd.audio_settings.ambient_level.to_string(),
            // A list accessor with no arm of its own still answers: 0x4BC4E0, the caption getter,
            // is nothing but `rowText([this+0x18])`, so the caption is the row the list is on, and
            // the empty string when the list is empty — which is what a `Comms.*` list shows
            // before a session has filled it. §4.106.
            _ => {
                let (rows, cur) = self.list_rows(base)?;
                match rows.get(cur.unwrap_or(0)) {
                    Some(ListRow::Text(t)) => t.clone(),
                    _ => String::new(),
                }
            }
        })
    }
}

/// The action a `Controller.*` binding row stands for (`GameCtrl.lay`, `fluffctrl.lay`).
pub(crate) fn controller_action(accessor: &str) -> Option<nfs_input::Action> {
    use nfs_input::Action;
    Some(match accessor.strip_prefix("Controller.")? {
        "Accel" => Action::Throttle,
        "Brake" => Action::Brake,
        "SteerLeft" => Action::SteerLeft,
        "SteerRight" => Action::SteerRight,
        "HandBrake" => Action::Handbrake,
        "ShiftUp" => Action::ShiftUp,
        "ShiftDown" => Action::ShiftDown,
        "ResetCar" => Action::Reset,
        "CameraView" => Action::CameraNext,
        "LookBehind" => Action::RearView,
        "Horn" => Action::Horn,
        _ => return None,
    })
}
