//! The race-start sync of a network launch: the options the host sends, the launch settings and traffic, the race built from them.

use crate::*;

impl App {
    /// The four `Track1.*` accessors as the option table names them (`network.md` §11.6):
    /// `TRACKLOCALEID0` 0xC (the track record's `+4` name id), `TRACKDIRECTION0` 0xD (the direction
    /// row, `REVERSE + 2·MIRROR`), `TRACKTRAFFIC0` 0xE (the traffic row) and `TRACKLAPS0` 0xF (the
    /// laps row, laps − 1), with whether the track is a circuit (`trk+0x6E`, which 0x509B00 asks
    /// before it reads the laps row).
    pub(super) fn net_track_options(&self) -> Option<([(u32, u32); 4], bool)> {
        let m = self.fe.menu.as_ref()?;
        let t = self.fe.track_tables.tracks.iter().find(|r| r.file.eq_ignore_ascii_case(m.track()))?;
        let dir = usize::from(m.backwards) | (usize::from(m.mirror) << 1);
        Some((
            [
                (0xC, t.name_string_id),
                (0xD, dir as u32),
                (0xE, m.traffic.min(3)),
                (0xF, m.laps.clamp(1, LAPS_MAX) - 1),
            ],
            t.circuit,
        ))
    }

    /// The front end's half of the option records, once per tick on a lobby screen
    /// (`network.md` §11.6).
    ///
    /// **Host.** `Track1.Track`, `.Direction` and `.Laps` are mode-2 accessors: a selection change
    /// sends `(id, value)` as a `0xBC` (0x4BC240 → 0x4B7F70 → slot 1 → 0x48D550); the
    /// port sends when the value differs from what was last sent (the first look only records it,
    /// the original's accessors are set up before any session exists). `Track1.Traffic` is mode 0
    /// and goes with the launch only. The session's own descriptor (what a browser's `'d'` shows,
    /// 0x509B00) follows the same accessors.
    ///
    /// **Player.** The records that came (0x48F998 → 0x4B4B80 writes the option table) are what
    /// the launch reads: `TRACKLOCALEID0` selects the track record whose `+4` is that id — the
    /// first when none is (0x4FBF70's rule for a machine that does not host, `hud.md`) —, the rest
    /// are the direction / traffic / laps rows.
    pub(crate) fn net_sync_options(&mut self) {
        let Some(host) = self.net.session.as_ref().map(|n| n.has_authority()) else { return };
        if host {
            let Some((opts, circuit)) = self.net_track_options() else { return };
            let positional = self.net.session.as_ref().is_some_and(|n| n.positional);
            let Some(n) = self.net.session.as_mut() else { return };
            // the create screen's order in the original's capture: COMMSPOSITIONAL 0x14, then 0xD, 0xF, 0xC — the order the
            // host's own message list holds and a newcomer is replayed (0x48A390)
            n.sync_option(0x14, u32::from(positional));
            for (id, v) in [opts[1], opts[3], opts[0]] {
                n.sync_option(id, v);
            }
            // 0x509B00: laps' row only on a circuit, the direction's slot 27 (`0x5B963C[row]`), the
            // track's name id
            n.set_descriptor(
                u8::from(positional),
                if circuit { opts[3].1 as u8 } else { 0 },
                DIRECTION_OPTIONS[opts[1].1 as usize & 3] as u16,
                opts[0].1 as u16,
            );
            return;
        }
        let Some(got) = self.net.session.as_mut().map(|n| n.take_options()).filter(|g| !g.is_empty()) else { return };
        let Some(m) = self.fe.menu.as_mut() else { return };
        for (id, v) in got {
            match id {
                0xC => {
                    let at = self.fe.track_tables.tracks.iter().position(|r| r.name_string_id == v);
                    let file = at.map(|i| self.fe.track_tables.tracks[i].file.clone());
                    m.track_idx = file
                        .and_then(|f| m.tracks.iter().position(|t| t.eq_ignore_ascii_case(&f)))
                        .unwrap_or(0);
                }
                0xD => {
                    m.backwards = v & 1 != 0;
                    m.mirror = v & 2 != 0;
                }
                0xE => m.traffic = v.min(3),
                0xF => m.laps = (v + 1).min(LAPS_MAX),
                _ => {}
            }
        }
        self.bind_lists();
        self.bind_checks();
    }

    /// The host's launch settings as 0x4ED5D0's chain would build them for a network game.
    pub(super) fn net_launch_settings(&mut self) -> Option<nfs_net::options::LaunchSettings> {
        let m = self.fe.menu.as_ref()?;
        let t = self.fe.track_tables.tracks.iter().find(|r| r.file.eq_ignore_ascii_case(m.track()))?;
        let positional = self.net.session.as_ref().is_some_and(|n| n.positional);
        // `Comms.Collision`'s row 0 is "off" (FEStrings 1500), row 1 "on" (1501), the start row
        let collision = self.choice_index("Comms.Collision").map_or(1, |(i, _)| i);
        let players = self.net.session.as_ref().map_or(1, |n| n.room_len().max(1)) as u32;
        let mut s = nfs_net::options::LaunchSettings {
            players,
            traffic: Vec::new(),
            positional: u32::from(positional),
            direction_row: (usize::from(m.backwards) | (usize::from(m.mirror) << 1)) as u32,
            laps_row: m.laps.clamp(1, LAPS_MAX) - 1,
            track_name_id: t.name_string_id,
            traffic_row: m.traffic.min(3),
            playlist: self.current_playlist(),
            skill: u32::from(m.difficulty.min(2)),
            damage: u32::from(m.damage),
            track: m.track().to_string(),
            linear: !t.circuit,
            laps: fe_laps(&m.circuits, m.track(), m.laps),
            traffic_cars: TRAFFIC_COUNT[m.traffic.min(3) as usize],
            nocollision: positional && collision == 0,
            derby: false,
            // 0x4ED5D0: `0x4D8790(0)` → `RALLY` 0x19, `0x4D8790(8)` → `HEAVY` 0x1A, each `1` and only when set
            rally: self.cheat_bits & CHEAT_RALLY != 0,
            heavy: self.cheat_bits & CHEAT_HEAVY != 0,
            own_car: Vec::new(),
        };
        // the "Smash Up" cheat (cheat bit 7): 0x4ED7D9…0x4ED841 sets `DERBY`, the skid pad, damage on,
        // `NOCOLLISION` off and `LINEAR_TRACK` 0 after the mode's own options — so the stream the host's
        // launch sends says so (`derby_track`, `network.md` §11.18)
        if self.cheat_bits & CHEAT_DERBY != 0 {
            s.derby = true;
            s.track = "skidpad".to_string();
            s.linear = false;
            s.damage = 1;
            s.nocollision = false;
        }
        let mut fe_rng = self.fe_rng.clone();
        s.traffic = self.net_traffic_cars(&s.track, s.traffic_kept(), &mut || fe_rng.next_u32());
        self.fe_rng = fe_rng;
        Some(s)
    }

    /// The traffic cars 0x4ECF00 builds for a network game, as their per-slot records
    /// (0x4ED4E0…0x4ED56E → 0x4EEDA0, kind 4): car `k` is the track record's `trk+0x7C[k % 7]`
    /// (`.FILENAME`, `.FEMULTIPLAYERCARNAME`), painted with the colour the loop draws from the front
    /// end's 0x538040 — four draws per car, in order after the record 0x4F4400 (0x4ED4E4…0x4ED51A):
    /// `a << 24 | (b & 0xFF) << 16 | (c & 0xFF) << 8 | (d & 0xFF)`, alpha included, handed to the
    /// car's constructor as the exterior and all three other colours (0x4ED53B…0x4ED542) and to
    /// 0x4EEDA0 as `.COLOUR`. `draw` is that generator; a caller that only wants the model
    /// list passes one it does not mind spending.
    pub(super) fn net_traffic_cars(&self, track: &str, count: u32, draw: &mut impl FnMut() -> u32) -> Vec<nfs_net::options::TrafficCar> {
        let trk = std::fs::read(self.paths.fe_data().join("Data").join("nfs5.trk"))
            .ok()
            .and_then(|d| nfs_formats::career::parse_track_table(&d).ok())
            .unwrap_or_default();
        let ids: Vec<u16> = trk
            .iter()
            .find(|r| r.file.eq_ignore_ascii_case(track))
            .map(|r| r.traffic_cars.iter().copied().filter(|i| *i != 0).collect())
            .unwrap_or_default();
        let catalog = self.fe_car_catalog();
        (0..count as usize)
            .filter_map(|k| {
                // 0x4ED4E4…0x4ED509: the four draws come before the record is even looked at
                let (a, b, c, d) = (draw(), draw(), draw(), draw());
                let colour = a << 24 | (b & 0xFF) << 16 | (c & 0xFF) << 8 | (d & 0xFF);
                let e = catalog.iter().find(|c| Some(&c.id) == ids.get(k % ids.len().max(1)))?;
                Some(nfs_net::options::TrafficCar {
                    model: e.model.clone(),
                    name: e.display_name.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect(),
                    colour,
                })
            })
            .collect()
    }

    /// **0x4B4DA0's network half**, run by every machine once the front end has handed over: the
    /// host sends the launch stream (0x48D490 / 0x48D440) and draws the grid (0x4B4FD0), then all
    /// run the barrier 0x48D6A0, after which a player's table holds the host's records. What the
    /// launch settled goes to `net.race`, which `start_from_layout` reads.
    pub(crate) fn net_launch_sync(&mut self) {
        let host = self.net.session.as_ref().is_some_and(|n| n.has_authority());
        let mut settings = if host { self.net_launch_settings() } else { None };
        // 0x4EEDA0's record for the player (kind 1) — every machine builds it and the walk sends it
        let own_car = self.net_own_launch_car();
        let Some(n) = self.net.session.as_mut() else { return };
        // 0x4B4E28 — 0x48D3D0: the barrier, then every player's finish time and race index back to −1
        n.launch_begin();
        // `n` (`ebx` of 0x4B4DA0, the players loop 0x4B4EA7…0x4B4ED9): the players `net+0xE5` marks,
        // not the room's count — 0x4B5079 stores it as NUMPLAYERRACECARS
        let players = n.race_indices().len().max(1) as u32;
        // the own-slot arm of the walk (0x4B4F25…0x4B4F66): the slot rewritten to the race index
        let own_recs = own_car.as_ref().map(|c| n.own_launch_records(c)).unwrap_or_default();
        if let Some(s) = settings.as_mut() {
            s.own_car = own_recs.clone();
        } else {
            // a player's walk sends only its own slot's records (the host's broadcast bands are not its)
            n.send_launch(&own_recs);
        }
        let settings = settings;
        if let Some(s) = settings.as_ref() {
            // 0x4B6660 builds the list from `fe.txt` and the launch chain appends after it; 0x4B6560
            // keeps the last record of an id (`nfs_net::options::broadcast_stream`)
            let fe_text = std::fs::read_to_string(self.paths.root.join("fe.txt")).unwrap_or_default();
            let fe_recs = nfs_net::options::fe_txt_records(&fe_text);
            let mut stream = nfs_net::options::broadcast_stream(&fe_recs, &s.stream());
            // the launch chain's own `rand()` draws come first: 0x4EEDA0 starts every car's record
            // with one (0x4EEDAC) — the player's (0x4EBA4D) and each kept traffic car's
            // (0x4ED56E), none for opponents or cops in a network game
            for _ in 0..1 + s.traffic_kept() {
                self.crt_rand.next();
            }
            // 0x4B4FD0…0x4B5043: `rand() % left` out of what is left of `0…n−1`, on the C runtime's
            // own generator (`App::crt_rand`)
            let crt = &mut self.crt_rand;
            let grid = nfs_net::options::grid_draw(players, || crt.next() as u32);
            n.note_grid(&grid);
            stream.extend(grid);
            n.send_launch(&stream);
        }
        use nfs_net::options::id as opt;
        let passed = n.launch_barrier();
        // the records' slot is the **race index** — `player+0x4C`, the player's place in the room's
        // list among those in the race (0x4B4EBC, 0x4B4ED2) — not the player id
        let index = n.race_indices();
        // `player+0x4C` / `[0x65741C]`, which 0x490140 (the link classification) and the seats read
        n.note_race_indices(&index);
        let me = index.iter().find(|(id, _)| *id == n.my_player_id()).map_or(0, |(_, i)| *i);
        let got = n.launch.clone();
        let own = settings.as_ref();
        // the host's values are its own settings; a player's are the records it heard (the
        // barrier has run, so the host's stream is in), falling back on its own menu
        let m = self.fe.menu.as_ref();
        let race = NetRace {
            track: got.track.clone().or_else(|| own.map(|s| s.track.clone())),
            laps: got.laps.or(own.map(|s| s.laps)).unwrap_or_else(|| m.map_or(1, |m| m.laps)),
            backwards: got.reverse.or(own.map(|s| s.direction_row & 1)).map_or(m.is_some_and(|m| m.backwards), |v| v != 0),
            mirror: got.mirror.or(own.map(|s| (s.direction_row >> 1) & 1)).map_or(m.is_some_and(|m| m.mirror), |v| v != 0),
            damage: got.damage.or(own.map(|s| s.damage)).map_or(m.is_none_or(|m| m.damage), |v| v != 0),
            skill: got.skill.or(own.map(|s| s.skill)).unwrap_or(1).min(2) as u8,
            traffic_cars: got.num_cars.map_or_else(|| own.map_or(0, |s| s.traffic_kept()), |c| c.saturating_sub(nfs_net::options::PLAYER_SLOTS)),
            positional: got.positional.or(own.map(|s| s.positional)).is_none_or(|v| v != 0),
            nocollision: got.nocollision.or(own.map(|s| u32::from(s.nocollision))).is_some_and(|v| v != 0),
            derby: got.derby.or(own.map(|s| u32::from(s.derby))).is_some_and(|v| v != 0),
            rally: got.rally.or(own.map(|s| u32::from(s.rally))).is_some_and(|v| v != 0),
            heavy: got.heavy.or(own.map(|s| u32::from(s.heavy))).is_some_and(|v| v != 0),
            players,
            opponents: got.num_opponent_cars.or(own.map(|_| 0)).unwrap_or(0),
            // `Grid_slot` 0x410580 takes the drawn value as `g` itself (0-based here, so the value 0
            // sits one row ahead of the pole `g = 1`); `grid_pos` is `g − 1`, the 0 as `u32::MAX`
            grid: got.grid.get(&me).map(|g| g.wrapping_sub(1)),
            seat_grids: index
                .iter()
                .filter(|(id, _)| *id != n.my_player_id())
                .map(|(_, i)| got.grid.get(i).map(|g| g.wrapping_sub(1)))
                .collect(),
            playlist: got.playlist.clone(),
            seat_cars: n.remote_cars().into_iter().map(|(_, _, _, c)| c).collect(),
            // this machine's own record is in `launch.cars[me]` too (`own_launch_records`), so index
            // 0 is whoever holds it — the record the walk's relay delivered, or our own
            camera0: got.cars.get(&0).and_then(|c| c.array(nfs_net::launchcar::id::CAMERA)),
            own_words: got.cars.get(&me).map(|c| c.words.iter().map(|(i, w)| (*i, w.clone())).collect()).unwrap_or_default(),
            traffic: if host { own.map(|s| s.traffic.clone()).unwrap_or_default() } else { got.traffic.values().cloned().collect() },
            fe_txt: [("rand_seed", opt::RAND_SEED), ("time", opt::TIME), ("weather", opt::WEATHER), ("fog", opt::FOG)]
                .into_iter()
                .filter_map(|(k, id)| got.number(id).map(|v| (k, v)))
                .collect(),
        };
        tracing::info!(target: "netplay", host, passed, ?race, "network launch settled (0x4B4DA0)");
        self.net.race = Some(race);
    }

    /// The settled network race onto the launch's own set-up (`start_from_layout`'s arms have just
    /// read the menu): the same fields the original's `0x4B4B80` writes from the stream. Returns
    /// the track to load.
    pub(crate) fn net_race_apply(&mut self, track: String) -> String {
        // a live race is not a replay of one
        self.replay_net = None;
        let Some(r) = self.net.race.take() else {
            // every session re-reads `fe.txt` (0x4B6C1C): nothing a network launch wrote stays
            self.net_launch.traffic.clear();
            self.net_launch.derby = false;
            self.net_launch.flags = None;
            self.net_launch.fe_txt.clear();
            self.net_launch.nocollision = false;
            self.net_launch.seat_grids.clear();
            self.net_launch.seat_cars.clear();
            self.net_launch.own_words.clear();
            self.derby_hud_laps = None;
            return track;
        };
        // a Smash Up has no laps: the knockout arm ends every path of 0x416310 before the lap code
        // (0x4164C0), so no car ever counts one (`car+0xB20` stays 0) and none ends the race. The
        // port's lap tracking is kept from ending it by a count no race reaches (99, `RaceSetup::net`),
        // and the HUD's lap counter — which the original still draws, `1/NUM_LAPS`, 0x432F92 with
        // `[0x657494]` clear — reads the launch's real count from `derby_hud_laps`.
        self.derby_hud_laps = r.derby.then_some(r.laps.max(1));
        self.road.traffic_colours.clear();
        // **the traffic cars' records** (0x4B4B80 wrote the host's `.FILENAME` / `.COLOUR` per
        // slot): the model each is, by file name, and the colour where it is not the model's own
        let mut traffic_ids = None;
        if !r.traffic.is_empty() {
            let catalog = self.fe_car_catalog();
            let (mut ids, mut colours) = (Vec::new(), Vec::new());
            for c in &r.traffic {
                let Some(e) = catalog.iter().find(|e| e.model.eq_ignore_ascii_case(&c.model)) else {
                    tracing::warn!(model = c.model, "network traffic car: no such model in this install");
                    continue;
                };
                let own = e.paint_sets.first().map(|p| p.colours[0]).unwrap_or([0x8C, 0x8C, 0x90]);
                let got = [(c.colour >> 16) as u8, (c.colour >> 8) as u8, c.colour as u8];
                ids.push(e.id);
                colours.push((got != own).then_some(got));
            }
            let own: Vec<String> = self.net_traffic_cars(r.track.as_deref().unwrap_or(&track), r.traffic.len() as u32, &mut || 0).into_iter().map(|c| c.model).collect();
            tracing::info!(target: "netplay", traffic = ?r.traffic.iter().map(|c| (c.model.as_str(), format!("{:08X}", c.colour))).collect::<Vec<_>>(), own_table_says = ?own, "network race: the traffic cars of the host's records");
            if !ids.is_empty() {
                traffic_ids = Some(ids);
                self.road.traffic_colours = colours;
            }
        }
        if r.opponents != 0 {
            tracing::warn!(target: "netplay", opponents = r.opponents, "the stream carries NUMOPPONENTRACECARS != 0: the original numbers opponent slots after the players (0x4B5081); this port has no AI opponent in a network race");
        }
        // the host's direction, laps, damage, skill and traffic; 0x4ECF00: no cops, `nOpp` 0 in a
        // network game; the humans are the room's players (0x4B5079's `NUMPLAYERRACECARS = ebx`),
        // one of which is this machine; the drawn `.STARTINGPOS` is `Grid_slot`'s `g` as it stands
        // (the values are 0…n−1, so the 0 sits one row ahead of the pole `g = 1`, the original's own
        // arithmetic); `grid_pos` is `g − 1` (`u32::MAX` for 0, which `grid_slot` reads back as `g = 0`)
        let rs = RaceSetup::net(&NetRaceOptions::from(&r), traffic_ids, self.race_setup());
        self.apply_race_setup(rs);
        // the camera reads `car[0].camera` (`[0x658294]`, slot 0 — 0x465A60 is called with 0 by
        // 0x415AF2, 0x462F30 / 0x465790 with the camera slot): in a network race the table is by
        // **race index**, so that is the list of whoever holds index 0 — the host, in list order —
        // on every machine, and the other players' lists are stored and never read. Nothing else
        // reads `.CAMERA` (a scan of `.text` for 0x658294: 0x463226, 0x46582A, 0x465B72, 0x465B93).
        if let Some(list) = r.camera0.as_ref().filter(|l| !l.is_empty()) {
            self.camera_cycle = list.iter().map(|&v| v as u16).collect();
        }
        self.net_launch.seat_grids = r.seat_grids.clone();
        self.net_launch.seat_cars = r.seat_cars.clone();
        self.net_launch.own_words = r.own_words.clone();
        self.net_launch.traffic = r.traffic.clone();
        self.net_launch.derby = r.derby;
        self.net_launch.flags = Some((r.rally, r.heavy, r.derby));
        if let Some(n) = self.net.session.as_mut() {
            n.positional = r.positional;
        }
        if let Some(list) = r.playlist.as_ref()
            && !self.net.session.as_ref().is_some_and(|n| n.has_authority())
        {
            self.set_cd_playlist(list.iter().map(|i| *i as usize).collect());
        }
        // 0x4B4B80 wrote the host's `fe.txt` records into the option table: what this machine's own
        // file said is replaced for the ids the stream carried (a host's own values are its file's)
        self.net_launch.fe_txt = r.fe_txt.iter().copied().collect();
        self.net_launch.nocollision = r.positional && r.nocollision;
        // NOCOLLISION (0x65740C) and COMMSPOSITIONAL (0x657408): 0x498FF0's car-car half is skipped
        // while both are set (`net_nocollision`, read by the collision pass)
        tracing::info!(
            track = r.track.as_deref().unwrap_or(&track), laps = self.num_laps, reverse = self.backwards, mirror = self.mirror,
            damage = self.road.damage_on, skill = self.race_skill, traffic = self.road.traffic_count, opponents = self.num_opponents,
            startingpos = g_log(self.ko.player_grid_override), positional = r.positional, nocollision = r.nocollision,
            "network race start: settings applied"
        );
        r.track.filter(|t| !t.is_empty()).unwrap_or(track)
    }
}
