//! The front end's two track lists, **0x4FBC10** (`RE/notes/modes.md` §2, read 2026-09-26).
//!
//! `[0x691D04]` is the list every `Track<n>.Track` accessor (`Track1..7.Track`, registered by
//! 0x4F9790 through 0x4FA4F0 with the getter 0x4F9EF0) indexes, and with it `Knockout.Track` (the
//! link accessor 0x4B8580 into the selected `Track<n>`), the location screens and `Record.Track`
//! outside Quick Knockout. `[0x691D08]` is `LoopedTrack.Track`'s (getter 0x4F9F00). 0x4FBC10:
//!
//! * empties both (0x4FBC24…0x4FBC80) and walks `[0x691D00]` — every `nfs5.trk` record, file order
//!   (0x4FBD5F…0x4FBF13);
//! * a record whose `+0x6C` is **3** is kept only when some `nfs5.trn` record `t` (all of them,
//!   `[0x691E64]`, through 0x503F70) with `chronicle year >= 0x502990(t+0x04)` lists the record's
//!   id word `trk+0` among its `t+0x0A` unlock words at `t+0x0C` (0x4FBD8A…0x4FBDD5; the year test
//!   is the signed `jl` at 0x4FBDA8). 0x502990 answers 1970 / 1989 / 2000 for 0x209 / 0x20A /
//!   0x20B (the three eras) and 0 for anything else (the club events, whose lists are empty). The
//!   chronicle year is `[0x5DB764]` (0x5032C0). Every other `+0x6C` keeps the record;
//! * **all** records are kept when `Game.GameType` is 3, the network game (0x4FBC94 defaults it to
//!   3 when the accessor is missing, 0x4FBDE2 tests it);
//! * a record whose file name `trk+8` is `"Skidpad"` under `_stricmp` (0x5DA6D0, 0x4FBDFE) is
//!   dropped; the rest is pushed on `[0x691D04]` and, with `trk+0x6E == 1`, on `[0x691D08]` too.
//!
//! A selection is a plain index into the list as it stands (the accessor's `[this+0x18]`); a
//! rebuild does not move it. The record getter 0x4FA690 / the id getter 0x4FA6D0 put a selection
//! at or past the list's end back to 0 (`vtbl+0x2C(0)`) before they read, and the name getters
//! 0x4FA560 / 0x4FA5A0 read record 0 for it.

use nfs_formats::career::{Tournament, TournamentKind, Era, TrackInfo};

/// 0x502990: the year a tournament's unlock list takes effect, by its `+0x04` type.
pub fn unlock_year(t: &Tournament) -> i32 {
    match t.kind {
        TournamentKind::Era(Era::Classic) => 1970,
        TournamentKind::Era(Era::Golden) => 1989,
        TournamentKind::Era(Era::Modern) => 2000,
        TournamentKind::Club(_) => 0,
    }
}

/// The chronicle year `[0x5DB764]`'s initial value, 0x79E (`savegame.md` §4.3), which 0x4FBC10
/// sees before any profile is loaded.
pub const START_YEAR: i32 = 1950;

/// `Game.GameType` 3, `SETTYPEMULTIPLAYER` (0x510B10).
pub const GAME_TYPE_NETWORK: u8 = 3;

/// The two lists 0x4FBC10 builds, as the tracks' file names (`trk+8`) lower-cased, which is how
/// this port names a track everywhere else. `(all, looped)` = `([0x691D04], [0x691D08])`.
pub fn build(tracks: &[TrackInfo], tournaments: &[Tournament], year: i32, game_type: u8) -> (Vec<String>, Vec<String>) {
    let (mut all, mut looped) = (Vec::new(), Vec::new());
    for r in tracks {
        let open = r.lock_mode != 3
            || tournaments.iter().any(|t| year >= unlock_year(t) && t.unlock_tracks.contains(&(r.id as u16 as i16)));
        if !(open || game_type == GAME_TYPE_NETWORK) || r.file.eq_ignore_ascii_case("Skidpad") {
            continue;
        }
        let name = r.file.to_lowercase();
        if r.circuit {
            looped.push(name.clone());
        }
        all.push(name);
    }
    (all, looped)
}

/// `FEData\Data\nfs5.trk` and `nfs5.trn`, the two tables 0x4FBC10 reads, loaded once.
#[derive(Debug, Clone, Default)]
pub struct TrackListTables {
    pub tracks: Vec<TrackInfo>,
    pub tournaments: Vec<Tournament>,
}

impl TrackListTables {
    pub fn load(paths: &nfs_assets::GamePaths) -> Self {
        let dir = paths.fe_data().join("Data");
        let read = |n: &str| std::fs::read(dir.join(n)).ok();
        let tracks = read("nfs5.trk").and_then(|d| nfs_formats::career::parse_track_table(&d).ok()).unwrap_or_default();
        let tournaments = read("nfs5.trn").and_then(|d| nfs_formats::career::parse_tournaments(&d).ok()).unwrap_or_default();
        Self { tracks, tournaments }
    }

    pub fn build(&self, year: i32, game_type: u8) -> (Vec<String>, Vec<String>) {
        build(&self.tracks, &self.tournaments, year, game_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped tables: 1950 opens the four records with `+0x6C == 0` (Skidpad dropped),
    /// 1970 adds the Classic tournaments' four, 1989 the Golden ones' six; the network game has
    /// them all. File order throughout.
    #[test]
    fn shipped_lists_by_year() {
        let Ok(paths) = nfs_assets::GamePaths::locate() else { return };
        let t = TrackListTables::load(&paths);
        if t.tracks.is_empty() {
            return;
        }
        let (a, l) = t.build(1950, 0);
        assert_eq!(a, ["coastal", "foothills", "farmland", "monaco3"]);
        assert_eq!(l, ["monaco3"]);
        let (a, _) = t.build(1969, 4);
        assert_eq!(a.len(), 4);
        let (a, l) = t.build(1970, 0);
        assert_eq!(a, ["coastal", "forest", "foothills", "farmland", "industrial", "monaco1", "monaco2", "monaco3"]);
        assert_eq!(l, ["monaco1", "monaco2", "monaco3"]);
        let all = ["coastal", "forest", "foothills", "alps", "autobahn", "castle", "farmland", "industrial", "monaco1", "monaco2", "monaco3", "monaco4", "monaco5", "canyon"];
        assert_eq!(t.build(1989, 0).0, all);
        assert_eq!(t.build(1950, GAME_TYPE_NETWORK).0, all);
        assert_eq!(t.build(1950, GAME_TYPE_NETWORK).1, ["monaco1", "monaco2", "monaco3", "monaco4", "monaco5"]);
    }
}
