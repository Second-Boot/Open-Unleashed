//! One-shot speech banks from `Speech/zzzxeng.viv` (`RE/notes/sfx.md` §7): banks are decoded on
//! demand; requests go through a FIFO like `Speech_enqueue` 0x4B05A0 (no cool-down, phrases play back
//! to back, more than 32 pending requests are dropped).

use crate::mixer::{AudioOutput, Clip, VoiceId};
use nfs_formats::{bigf::Bigf, bnk::Bnk};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

/// **kTrackRecord**, speech ids 113…124 — `oneshots.txt`'s `#Phrase_kTrackRecord` in order; 0x4A6540
/// queues `113 + rand % 12` (0x4A654B…0x4A6559).
pub const TRACK_RECORD: [&str; 12] = [
    "PG-beatbesttime.bnk",
    "PG-Cbeatbesttime.bnk",
    "PG-Cgotbesttime.bnk",
    "PG-Csetnewrecord4trk.bnk",
    "PG-Csetnewtrackrec.bnk",
    "PG-gotbesttime.bnk",
    "PG-setnewrecord4trk.bnk",
    "PG-setnewtrackrec.bnk",
    "PG-Cbeattrackrec.bnk",
    "PG-Cbeatenbesttime.bnk",
    "PG-beattrackrec.bnk",
    "PG-beatenbesttime.bnk",
];

/// **kLapRecord**, speech ids 125…129; 0x4A6600 queues `125 + rand % 5` (0x4A660B…0x4A6619).
pub const LAP_RECORD: [&str; 5] = ["Record-Lap.bnk", "Record-Newlap.bnk", "Record-Youbeatlap.bnk", "PG-Cbeatenlaprec.bnk", "PG-beatenlaprec.bnk"];

/// The speech wrappers 0x4A62A0…0x4A6680 (`RE/notes/sfx.md` §7.2): each builds a speech **id** —
/// the index of a bank name in `oneshots.txt` ([`Speech::say_id`]) — and queues it through
/// 0x4B05A0. The variants are drawn from **0x538040**, the front end's generator (`draw`), one
/// call per `rand` below and in this order; `% n` is an unsigned `div`.
pub mod phrase {
    /// 0x4A6430(p), kRacePlacing: `p == 0` → `7 + (r & 1)`, `p == 7` → `15 + (r & 1)`, else
    /// `8 + p` with no draw (0x4A6437…0x4A646D).
    pub fn race_placing(p: u32, draw: &mut dyn FnMut() -> u32) -> u32 {
        match p {
            0 => 7 + (draw() & 1),
            7 => 15 + (draw() & 1),
            _ => 8 + p,
        }
    }

    /// 0x4A62A0, kLostRace: `17 + (r & 1)`.
    pub fn lost_race(draw: &mut dyn FnMut() -> u32) -> u32 {
        17 + (draw() & 1)
    }

    /// 0x4A6640(n), kLap: `132 + n + 8·(r & 1)` (0x4A6655).
    pub fn lap(n: u32, draw: &mut dyn FnMut() -> u32) -> u32 {
        132 + n + 8 * (draw() & 1)
    }

    /// 0x4A6390(p), kLapPosition: `53 + p + 9·(r & 1)`, and for `p == 7` a second draw's bit
    /// added (0x4A6394…0x4A63B4).
    pub fn lap_position(p: u32, draw: &mut dyn FnMut() -> u32) -> u32 {
        let id = 53 + p + 9 * (draw() & 1);
        if p == 7 { id + (draw() & 1) } else { id }
    }

    /// 0x4A63E0(s), kLapTimeBehind: nothing for `s <= 0`; `s` capped at 12 (`> 11` → 12), then
    /// `71 + s + 13·(r & 1)` (0x4A63E8…0x4A6408).
    pub fn lap_time_behind(s: i32, draw: &mut dyn FnMut() -> u32) -> Option<u32> {
        if s <= 0 {
            return None;
        }
        let s = if s > 11 { 12 } else { s as u32 };
        Some(71 + s + 13 * (draw() & 1))
    }

    /// 0x4A6580, kBestLap: `49 + (r & 3)`.
    pub fn best_lap(draw: &mut dyn FnMut() -> u32) -> u32 {
        49 + (draw() & 3)
    }

    /// 0x4A65C0, kFinalLap: `107 + r % 6`.
    pub fn final_lap(draw: &mut dyn FnMut() -> u32) -> u32 {
        107 + draw() % 6
    }

    /// 0x4A6540, kTrackRecord: `113 + r % 12`.
    pub fn track_record(draw: &mut dyn FnMut() -> u32) -> u32 {
        113 + draw() % 12
    }

    /// 0x4A6600, kLapRecord: `125 + r % 5`.
    pub fn lap_record(draw: &mut dyn FnMut() -> u32) -> u32 {
        125 + draw() % 5
    }

    /// 0x4A62E0, kKnockoutWon: `98 + (r & 1)`; 0x4A6320, kKnockoutQualify: 97, no draw.
    pub fn knockout_won(draw: &mut dyn FnMut() -> u32) -> u32 {
        98 + (draw() & 1)
    }

    pub const KNOCKOUT_QUALIFY: u32 = 97;

    /// 0x4A6350, kKnockoutEliminated: `100 + r % 7`.
    pub fn knockout_eliminated(draw: &mut dyn FnMut() -> u32) -> u32 {
        100 + draw() % 7
    }

    /// 0x4A6500(n), kTourPlaced: `32 + n + 3·(r & 1)`.
    pub fn tour_placed(n: u32, draw: &mut dyn FnMut() -> u32) -> u32 {
        32 + n + 3 * (draw() & 1)
    }

    /// 0x4A6490, kNextTournament: `156 + (r & 1)`; 0x4A64D0, kTournamentLeader: 158.
    pub fn next_tournament(draw: &mut dyn FnMut() -> u32) -> u32 {
        156 + (draw() & 1)
    }

    pub const TOURNAMENT_LEADER: u32 = 158;
}

pub struct Speech {
    data: Vec<u8>,
    viv: Bigf,
    /// `oneshots.txt`'s bank names in order — the speech ids (`sfx.md` §7.1)
    names: Vec<String>,
    clips: Mutex<HashMap<String, Option<Arc<Clip>>>>,
    queue: Mutex<VecDeque<(String, bool)>>,
    current: Mutex<Option<VoiceId>>,
    /// `SpeechLevel [0x5D174C]`, 0..127
    level: AtomicU8,
    /// `game.spl[1]` / `game.spl[4]` — the trims for speech ids ≥ 4 and < 4 (the countdown)
    trims: [u32; 2],
}

impl Speech {
    /// Open the speech archive; banks are decoded the first time they are requested.
    pub fn load(speech_dir: &Path) -> anyhow::Result<Self> {
        let data = std::fs::read(speech_dir.join("zzzxeng.viv"))?;
        let viv = Bigf::parse(&data)?;
        let spl = nfs_formats::spl::GameSpl::load(&speech_dir.join("..").join("Sounds"));
        let names = viv.get(&data, "oneshots.txt").map(|t| oneshot_names(&String::from_utf8_lossy(t))).unwrap_or_default();
        Ok(Self {
            data,
            names,
            viv,
            clips: Mutex::new(HashMap::new()),
            queue: Mutex::new(VecDeque::new()),
            current: Mutex::new(None),
            level: AtomicU8::new(127),
            trims: [spl.speech(false), spl.speech(true)],
        })
    }

    fn clip(&self, name: &str) -> Option<Arc<Clip>> {
        let key = name.to_lowercase();
        let mut cache = self.clips.lock().unwrap();
        if let Some(c) = cache.get(&key) {
            return c.clone();
        }
        let decoded = self.viv.get(&self.data, &key).or_else(|| self.viv.get(&self.data, name)).and_then(|bytes| {
            let bank = Bnk::parse(bytes).ok()?;
            let slot = bank.slots.iter().position(|s| s.is_some())?;
            let pcm = bank.decode_slot(bytes, slot).ok()?;
            Some(Arc::new(Clip { rate: pcm.rate, channels: pcm.channels, samples: pcm.samples, loop_range: None, gain: pcm.gain, bend_cents: pcm.bend_cents, fx_send: pcm.fx_send }))
        });
        if decoded.is_none() {
            tracing::warn!(name, "speech bank missing");
        }
        cache.insert(key, decoded.clone());
        decoded
    }

    /// The `SpeechLevel` slider, read when a phrase starts.
    pub fn set_level(&self, level: u8) {
        self.level.store(level, Ordering::Relaxed);
    }

    /// The volume byte 0x4B04E0 builds for a queued phrase of the announcer's bank (record `+0x10`
    /// equal to `[0x657328]`): `(SpeechLevel · game.spl[k] · 85) >> 14` with `k = 1` for speech ids
    /// ≥ 4 and 4 for ids 0…3 — the countdown (0x4B052A…0x4B055A; helpers 0x4A8300 / 0x4A8320) — at
    /// bend byte 0x40. At the top of the slider with the shipped trims that is 83 of 127.
    pub fn volume(level: u8, trim: u32) -> f32 {
        (((level as u32 * trim * 85) >> 14).min(127)) as f32 / 127.0
    }

    /// Enqueue a phrase (bank name, e.g. `Lap-Final.bnk`). The port had fixed volumes 0.8 / 0.9 at
    /// each call until 2026-09-24.
    pub fn say(&self, _out: &AudioOutput, name: &str) {
        self.enqueue(name, false);
    }

    /// `0x4B05A0(id)`: the bank `oneshots.txt` names for the id; an id with no entry is dropped.
    pub fn say_id(&self, id: u32) {
        match self.names.get(id as usize) {
            Some(n) => {
                let n = n.clone();
                self.enqueue(&n, id < 4);
            }
            None => tracing::debug!(id, "speech id without a bank"),
        }
    }

    /// Enqueue one of the countdown phrases (speech ids 0…3, which take `game.spl[4]`).
    pub fn say_countdown(&self, _out: &AudioOutput, name: &str) {
        self.enqueue(name, true);
    }

    fn enqueue(&self, name: &str, countdown: bool) {
        let mut q = self.queue.lock().unwrap();
        if q.len() >= 32 {
            return;
        }
        q.push_back((name.to_string(), countdown));
    }

    /// **The announcer is busy** — what race over 0x417010 asks before it lets a finished car's
    /// 0x140 ticks suffice (0x4170F5…0x41710D): the speech ring's read index `[0x656AE4]` short of
    /// its write index `[0x657300]` — a phrase queued and not yet started, here the queue not
    /// empty — or the playing phrase's two voices `[0x6573A8]` / `[0x6573AC]` not −1. Those stay
    /// set until the service 0x4B0A20 sees both voices' status below 5 (0x4B0A79…0x4B0AD4) and
    /// clears them, as `current` stays until [`Speech::update`] sees the voice gone.
    pub fn busy(&self) -> bool {
        !self.queue.lock().unwrap().is_empty() || self.current.lock().unwrap().is_some()
    }

    /// The service **0x4B0A20**: the playing phrase is let go once its voices have ended
    /// (0x4B0A20…0x4B0B11), then the scheduler 0x4B08F0 starts the next queued one (it waits while
    /// a phrase is playing, 0x4B092B…0x4B0941). The race calls it from the audio update 0x4A8120
    /// (0x4A813A), on the odd tick.
    ///
    /// `drop_queued` is a playback at double speed (`[0x5E99F4] >= 2` and `[0x606864] == 3`): the
    /// scheduler then clears the head entry and moves on without playing it (0x4B0915…0x4B0925 →
    /// 0x4B09DC), before it looks at the voices — so a phrase already playing ends, and nothing
    /// queued is ever said. The loader skips the same entries (0x4B07B3).
    pub fn update(&self, out: &AudioOutput, drop_queued: bool) {
        let mut cur = self.current.lock().unwrap();
        if let Some(id) = *cur
            && !out.is_playing(id)
        {
            *cur = None;
        }
        if drop_queued {
            self.queue.lock().unwrap().pop_front();
            return;
        }
        if cur.is_some() {
            return;
        }
        let next = self.queue.lock().unwrap().pop_front();
        if let Some((name, countdown)) = next && let Some(c) = self.clip(&name) {
            // the volume is built when the phrase starts (0x4B04E0)
            let volume = Self::volume(self.level.load(Ordering::Relaxed), self.trims[countdown as usize]);
            *cur = Some(out.play(c, volume, 1.0, false));
        }
    }
}

/// The quoted bank names of `oneshots.txt`, in order: the speech ids (`#Phrase_…` headers and
/// blank lines are not ids).
pub fn oneshot_names(text: &str) -> Vec<String> {
    text.split('"').skip(1).step_by(2).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::Speech;
    use super::phrase;

    #[test]
    fn the_wrappers_build_the_originals_ids() {
        let bits = |v: u32| move || v;
        // placing: first / last draw one bit, the others none
        assert_eq!(phrase::race_placing(0, &mut bits(1)), 8);
        assert_eq!(phrase::race_placing(7, &mut bits(2)), 15);
        assert_eq!(phrase::race_placing(3, &mut || panic!("no draw")), 11);
        // lap position: last place takes a second bit
        let mut seq = [1u32, 1].into_iter();
        assert_eq!(phrase::lap_position(7, &mut || seq.next().unwrap()), 53 + 7 + 9 + 1);
        assert_eq!(phrase::lap_position(2, &mut bits(0)), 55);
        // seconds behind: `71 + min(s,12) + 13·bit`, nothing at 0
        assert_eq!(phrase::lap_time_behind(0, &mut bits(0)), None);
        assert_eq!(phrase::lap_time_behind(1, &mut bits(0)), Some(72));
        assert_eq!(phrase::lap_time_behind(30, &mut bits(1)), Some(96));
        // the modulo kinds are unsigned remainders of the full draw
        assert_eq!(phrase::track_record(&mut bits(0xFFFF_FFFF)), 113 + 0xFFFF_FFFF % 12);
        assert_eq!(phrase::lap_record(&mut bits(7)), 127);
        assert_eq!(phrase::final_lap(&mut bits(11)), 112);
        assert_eq!(phrase::best_lap(&mut bits(7)), 52);
        assert_eq!(phrase::lap(1, &mut bits(1)), 141);
        assert_eq!(phrase::tour_placed(2, &mut bits(1)), 37);
    }

    #[test]
    fn the_shipped_ids_name_the_record_banks() {
        let Ok(paths) = nfs_assets::GamePaths::locate() else { return };
        let Ok(sp) = Speech::load(&paths.game_data().join("Speech")) else { return };
        assert_eq!(sp.names.len(), 166);
        for (k, n) in super::TRACK_RECORD.iter().enumerate() {
            assert_eq!(&sp.names[113 + k], n);
        }
        for (k, n) in super::LAP_RECORD.iter().enumerate() {
            assert_eq!(&sp.names[125 + k], n);
        }
        assert_eq!(sp.names[132], "Lap-your1st.bnk");
    }

    #[test]
    fn oneshots_ids_are_the_quoted_names_in_order() {
        let t = "#Phrase_kCountdown\r\n    \"SOR-a-3.bnk\"\r\n    \"SOR-a-2.bnk\"\r\n\r\n#Phrase_kX\r\n    \"A.bnk\"\r\n";
        assert_eq!(super::oneshot_names(t), ["SOR-a-3.bnk", "SOR-a-2.bnk", "A.bnk"]);
    }

    #[test]
    fn the_announcer_is_busy_while_a_phrase_is_queued() {
        let sp = Speech { data: Vec::new(), names: Vec::new(), viv: nfs_formats::bigf::Bigf { declared_size: 0, header_size: 0, entries: Vec::new() }, clips: Default::default(), queue: Default::default(), current: Default::default(), level: 127.into(), trims: [127, 127] };
        assert!(!sp.busy());
        sp.enqueue("Lap-Final.bnk", false);
        assert!(sp.busy());
        sp.queue.lock().unwrap().clear();
        *sp.current.lock().unwrap() = Some(crate::VoiceId(3));
        assert!(sp.busy());
    }

    #[test]
    fn announcer_volume_is_the_speech_category() {
        // (127 · 127 · 85) >> 14 = 83 of 127 at the top of the slider with the shipped trim
        assert_eq!(Speech::volume(127, 127), 83.0 / 127.0);
        assert_eq!(Speech::volume(0, 127), 0.0);
    }
}
