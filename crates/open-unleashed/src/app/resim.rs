//! **`--resim <package.json>`: an input-exchange race played again from a server's record** (`RE/notes/lobby.md` section 15, LOBBY-AC4).
//!
//! The server (the private `open-unleashed-lobby`) records, for each input-exchange race, the room's list at the go, the launch stream and
//! every frame the room emitted ([`nfs_game::resim`]). This is the run that plays them: the package becomes a network replay's setup
//! (`replay.md` section 18.15 -- the race is built by `start_race` exactly as a replay of a network race is) and the controller samples of
//! every human come from the frames ([`nfs_game::resim::Resim::sample`]) where a replay would read its lanes, so the simulation is the
//! application's own, unchanged. A scripted run (`--autotest` machinery: invisible window, unpaced, silent), ending when every human has
//! finished or left, or the frames are used up. It writes a JSON report: per human the tick the car finished at (as `0xB7` would
//! send it: the tick less the 448-tick countdown, 0x4168A9), the state hash of every 64th tick and the run's time.
//!
//! **Headless** (the default of `--resim`; `NFS_RESIM_GPU=1` brings back the windowed scripted run): no winit event loop, no window and
//! no GPU device are made -- the server's Linux box has no display server, GPU or sound card. [`App::run_headless`] builds the race
//! with [`nfs_renderer::Renderer::headless`] (the same uploads' bookkeeping, nothing drawn) through the very `start_race` and
//! `run_scripted_race` a windowed run takes; the simulation never reads a pixel, a window or a device, only the counts and the size the
//! headless renderer hands out as a device-backed one does. The sound is the muted offline mixer of every scripted run, the controllers
//! are not opened. The report of the two paths is identical (tick for tick, hash for hash).
//!
//! **TEST GLUE / server tooling, not the original's.**

use crate::*;
use std::collections::BTreeMap;

/// The report of a run that could not start (or finished without a result): written where `--resim-out` says, and on stderr.
fn not_run(reason: &str) -> String {
    format!("{{\"status\":\"not_run\",\"reason\":{}}}\n", json_str(reason))
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// The package at `path` resolved against this install's catalogue. A package that cannot be run writes the "not run" report and ends
/// the process with status 3 (the server reads the report).
pub(crate) fn load(paths: &GamePaths, path: &str, out: Option<&str>) -> nfs_game::resim::Resim {
    let fail = |reason: String| -> ! {
        let r = not_run(&reason);
        if let Some(o) = out {
            let _ = std::fs::write(o, &r);
        }
        eprint!("{r}");
        std::process::exit(3);
    };
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| fail(format!("package unreadable: {e}")));
    let pkg = nfs_game::resim::Package::from_json(&text).unwrap_or_else(|e| fail(format!("package: {e}")));
    let catalog = std::fs::read(paths.fe_data().join("Data").join("nfs5.car"))
        .ok()
        .and_then(|d| nfs_formats::fedata::parse_car_catalog(&d).ok())
        .unwrap_or_else(|| fail("this install has no car catalogue (nfs5.car)".into()));
    nfs_game::resim::resolve(&pkg, &catalog).unwrap_or_else(|e| fail(e))
}

/// The state of a re-simulation run.
pub(crate) struct ResimRun {
    pub(crate) feed: nfs_game::resim::Resim,
    out: Option<String>,
    started: std::time::Instant,
    /// `(tick, hash)` of every 64th tick
    hashes: Vec<(u64, u64)>,
    /// race index -> the finish as the `0xB7` would carry it; `None` = left the race (state 3, no time)
    finished: BTreeMap<u32, Option<u64>>,
}

impl ResimRun {
    pub(crate) fn new(feed: nfs_game::resim::Resim, out: Option<String>) -> Self {
        Self { feed, out, started: std::time::Instant::now(), hashes: Vec::new(), finished: BTreeMap::new() }
    }
}

/// FNV-1a over the bit patterns of a car's state.
fn fnv(h: &mut u64, v: &[f32]) {
    for x in v {
        for b in x.to_bits().to_le_bytes() {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
}

impl App {
    /// **The headless run** (see the module's note): what `resume_inner` does for a race with no front end -- hand a renderer (and a window,
    /// here none) to `start_race` -- with the loop handle that only remembers `exit`. The report is written by the scripted loop's end.
    pub(crate) fn run_headless(&mut self) {
        let loop_ctl = LoopCtl::headless();
        // the size the windowed scripted run's invisible window has (`shot_geometry`)
        let (w, h, _) = shot_geometry();
        let renderer = Renderer::headless((w, h));
        self.start_race(renderer, None, &loop_ctl);
        if self.resim.is_some() && !loop_ctl.exited() {
            // the scripted loop ended without a report (a race that could not start): say so
            let reason = "the race did not run to a report";
            let r = not_run(reason);
            if let Some(o) = self.resim.as_ref().and_then(|r| r.out.as_deref()) {
                let _ = std::fs::write(o, &r);
            }
            eprint!("{r}");
            std::process::exit(3);
        }
    }

    /// After a tick of the scripted loop: the hash of every 64th tick, the finishes, and whether the run is over (every human finished or
    /// left, or the frames the record has are used up).
    pub(crate) fn resim_after_tick(&mut self) -> bool {
        let Some(rn) = self.replay_net.as_ref() else { return false };
        let (local, seats): (u32, Vec<u32>) = (rn.local_index, rn.seats.clone());
        let done_tick = self.tick; // ticks run so far
        let mut states: Vec<(u32, [f32; 21])> = Vec::new();
        let pack = |v: &Vehicle| -> [f32; 21] {
            let mut a = [0f32; 21];
            a[0..3].copy_from_slice(&v.position.to_array());
            a[3..6].copy_from_slice(&v.velocity.to_array());
            a[6..15].copy_from_slice(&v.orientation.to_cols_array());
            a[15..18].copy_from_slice(&v.angular_velocity.to_array());
            a[18] = v.rpm;
            a
        };
        if let Some(v) = self.vehicle.as_ref() {
            states.push((local, pack(v)));
        }
        for (k, o) in self.road.opponents.iter().enumerate() {
            if let Some(&i) = seats.get(k).filter(|i| **i != u32::MAX) {
                states.push((i, pack(&o.vehicle)));
            }
        }
        // finishes: the local car's `race`, a seat's progress; `finish = tick - 0x1C0` as 0x4168A9 sends it
        let mut fin: Vec<(u32, Option<u64>)> = Vec::new();
        if let Some(r) = self.race.as_ref().filter(|r| r.finished) {
            fin.push((local, (!self.player_eliminated).then(|| r.tick.saturating_sub(0x1C0))));
        }
        for (k, o) in self.road.opponents.iter().enumerate() {
            if let Some(&i) = seats.get(k).filter(|i| **i != u32::MAX)
                && o.progress.finished
            {
                fin.push((i, (!o.eliminated).then(|| o.progress.tick.saturating_sub(0x1C0))));
            }
        }
        let Some(run) = self.resim.as_mut() else { return false };
        for (i, t) in fin {
            run.finished.entry(i).or_insert(t);
        }
        if done_tick % 64 == 0 {
            states.sort_by_key(|s| s.0);
            let mut h = 0xCBF2_9CE4_8422_2325u64;
            for (i, a) in &states {
                fnv(&mut h, &[f32::from_bits(*i)]);
                fnv(&mut h, a);
            }
            fnv(&mut h, &[f32::from_bits(nfs_game::rng::state())]);
            run.hashes.push((done_tick, h));
        }
        let all = run.feed.humans.iter().all(|i| run.finished.contains_key(i));
        let used_up = done_tick >= run.feed.ticks();
        all || used_up
    }

    /// The report: status, the finishes, the hashes, the run's time.
    pub(crate) fn resim_report(&mut self) {
        let Some(run) = self.resim.as_ref() else { return };
        let all = run.feed.humans.iter().all(|i| run.finished.contains_key(i));
        let fin: Vec<String> = run
            .feed
            .humans
            .iter()
            .map(|i| match run.finished.get(i) {
                Some(Some(t)) => format!("{{\"race_index\":{i},\"finish_ticks\":{t}}}"),
                Some(None) => format!("{{\"race_index\":{i},\"left\":true}}"),
                None => format!("{{\"race_index\":{i},\"finish_ticks\":null}}"),
            })
            .collect();
        let hashes: Vec<String> = run.hashes.iter().map(|(t, h)| format!("[{t},\"{h:016x}\"]")).collect();
        let notes: Vec<String> = run.feed.notes.iter().map(|n| json_str(n)).collect();
        let report = format!(
            "{{\"status\":\"ok\",\"complete\":{all},\"ticks_run\":{},\"frames_ticks\":{},\"wall_secs\":{:.3},\"local_index\":{},\"track\":{},\"laps\":{},\"finishes\":[{}],\"hash_every\":64,\"hashes\":[{}],\"notes\":[{}]}}\n",
            self.tick,
            run.feed.ticks(),
            run.started.elapsed().as_secs_f64(),
            run.feed.local_index,
            json_str(&run.feed.setup.track),
            run.feed.setup.laps,
            fin.join(","),
            hashes.join(","),
            notes.join(",")
        );
        if let Some(o) = &run.out {
            let _ = std::fs::write(o, &report);
        }
        print!("{report}");
    }
}
