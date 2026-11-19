//! **The enhanced game's own front-end pieces** (not the original):
//! storage of the switches, the «Улучшения» sub-tab's generated rows and its string table.
//! `RE/notes/REMASTER-PLAN.md` stage 1, `ENHANCEMENTS.md`.
//!
//! Nothing here reads the original's binary: the panel is a port-own layout
//! (`enhanced/layouts/enhancements.lay`) built from widget types the original's layouts use, read
//! by the same layout engine from a second directory ([`layouts_dir`]). It is the **third
//! sub-tab of Options › графика**: `graphicstabs_entry.lay` (one `NFS5.TABBUTTON`) is spliced into
//! `GraphicsTabs.lay` and `graphics_entry.lay` (the panel the button names, `ID_ENHPANEL`, and the
//! tab strip's third separator) into `graphics.lay`, at load (`layout::load_one`).
//!
//! **Hard original.** `NFS_ENH=off` is the test mode "this is the original": every switch off
//! *and* none of this module's front-end additions present (no third graphics sub-tab,
//! no extra strings, no A/B key, nothing saved) — what the FE goldens compare against `master`'s. The
//! menu's «как в оригинале» action turns every switch off but leaves the tab, so the player can
//! come back.

use nfs_assets::paths::GamePaths;
use nfs_formats::locale::{Strings, typed_char};
use nfs_renderer::{ENHANCEMENT_REGISTRY, EnhancementStored, EnhancementSwitch, Enhancements, MAX_ENHANCEMENT_LEVELS};
use nfs_renderer::{E1, E2, E3, E4, E5, E6, E7, E8, E9, E10, E11, E12, E13, E14, E15, E16, E17, E18, E19, E21, E22, E23, E24, E26, E27, E28, E29, E31, E32, E33, E34, E35, E36, E37, E38, E39, E40, E41, E42, E43, E44, E47, E48, E49, E50, E51};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::path::PathBuf;

mod panel;
mod settings;
mod strings;

pub use panel::*;
pub use settings::*;
pub use strings::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// ENHANCEMENT E41: with no pack found (nothing sets the flag in a test run) the row is disabled, its hint the reason, its level reads off.
    #[test]
    fn the_textures_row_is_grey_without_a_pack() {
        let e = Enhancements::all();
        let i = nfs_renderer::E41.0 as usize;
        assert_eq!(e.level(nfs_renderer::E41), 1);
        assert_eq!(e.shown_level(nfs_renderer::E41), 0);
        let info = line_info(Item::Entry(i), &e).unwrap();
        assert!(info.inactive);
        let b = e.blocked("E41").unwrap();
        assert_eq!(info.help_id, STR_REASON + b.dep as u32 * 3 + b.by as u32);
        assert!(reason_text(&nfs_renderer::ENHANCEMENT_DEPENDS[b.dep], b.by, true).contains("hd_textures"));
    }

    #[test]
    fn an_old_file_with_the_photo_switch_still_loads() {
        // 2026-10-27: E30 left the registry and every later index moved; the file names tokens, so the other levels
        // load as saved and "E30" is ignored
        let e = parse_file(r#"{"version":6,"levels":{"E7":2,"E29":1,"E30":1,"E31":1,"E40":0,"E44":0}}"#);
        assert_eq!((e.level(E7), e.level(E29), e.level(E31), e.level(E40), e.level(E44)), (2, 1, 1, 0, 0));
        assert_eq!(e.level(E32), ENHANCEMENT_REGISTRY[E32.0 as usize].default_level);
        assert!(!file_text(&e).contains("\"E30\""));
    }

    /// the ids the layouts name literally and a sample of every computed kind, as they were before the
    /// compile-time range checks (a saved layout or a string table must not move).
    #[test]
    fn the_string_ids_are_pinned() {
        assert_eq!([STR_TITLE, STR_RESET, STR_NONE_YET, STR_HINT, STR_ENTRY, STR_DEFAULTS, STR_MORE, STR_FIRST, STR_LITE], [900_001, 900_002, 900_003, 900_004, 900_005, 900_006, 900_007, 900_008, 900_009]);
        assert_eq!((STR_NAME + 3, STR_HELP + 43), (900_103, 900_243));
        assert_eq!(level_ids(0), &[900_300, 900_301, 900_302, 900_303]);
        assert_eq!(level_ids(2), &[900_316, 900_317, 900_318, 900_319, 900_320, 900_321]);
        // the per-row ids follow the registry index (no layout names them): E38 is 34 since E30 (2026-10-27; it was 37 with ids
        // 900_596..), E20 (2026-11-13; 36, ids 900_588..) and E25 (2026-11-16; 35, ids 900_580..) left the registry
        let e38 = nfs_renderer::E38.0 as usize;
        assert_eq!(e38, 34);
        let want: &[u32] = if cfg!(target_os = "macos") { &[900_572, 900_573, 900_574] } else { &[900_572, 900_574] };
        assert_eq!(level_ids(e38), want);
        assert_eq!((VRow::MfxMode.name_id(), VRow::FsrScale.help_id(), VRow::FsrScale.level_id(2)), (903_000, 903_012, 903_122));
        assert_eq!((STR_GROUP + 1, STR_REASON + 5 * 3 + 2), (903_301, 903_417));
    }

    #[test]
    fn env_over_file_over_default() {
        // empty registry: every set is the same set, but the mode tells which rule fired
        assert_eq!(load_with(None, None).1, Mode::Normal);
        assert_eq!(load_with(Some("off"), None), (Enhancements::ORIGINAL, Mode::HardOriginal));
        assert_eq!(load_with(Some("all"), None).1, Mode::Normal);
        assert_eq!(load_with(Some("E1,E3"), None).1, Mode::Normal);
        assert_eq!(load_with(Some(""), None).1, Mode::Normal);
    }

    /// A file written while E45 (normal maps, removed 2026-10-05) existed still loads: its token is unknown now and ignored, every
    /// other entry keeps its stored level.
    #[test]
    fn a_stored_removed_token_is_ignored() {
        let with = parse_file(r#"{"version": 6, "levels": {"E41": 2, "E45": 3, "E42": 0}}"#);
        let without = parse_file(r#"{"version": 6, "levels": {"E41": 2, "E42": 0}}"#);
        assert_eq!(with, without);
        assert_eq!(with.level(nfs_renderer::E41), 1);
        assert_eq!(with.level(nfs_renderer::E42), 0);
        // E20 (lightning lights the scene, removed 2026-11-13): a file written while it existed, with it on and off, loads as the
        // same file without it; the switches after it keep their stored levels (the file names tokens, not indices)
        for e20 in [0, 1] {
            let old = parse_file(&format!(r#"{{"version": 6, "levels": {{"E19": 0, "E20": {e20}, "E21": 0, "E23": 1, "E51": 0}}}}"#));
            let new = parse_file(r#"{"version": 6, "levels": {"E19": 0, "E21": 0, "E23": 1, "E51": 0}}"#);
            assert_eq!(old, new);
            assert_eq!((old.level(nfs_renderer::E19), old.level(nfs_renderer::E21), old.level(nfs_renderer::E23), old.level(nfs_renderer::E51)), (0, 0, 1, 0));
        }
        // E25 (soft shadows when overcast, removed 2026-11-16): the same; the switches after it (E22 and on, one index down now)
        // keep their stored levels
        for e25 in [0, 1] {
            let old = parse_file(&format!(r#"{{"version": 6, "levels": {{"E24": 0, "E25": {e25}, "E22": 2, "E27": 1, "E50": 1}}}}"#));
            let new = parse_file(r#"{"version": 6, "levels": {"E24": 0, "E22": 2, "E27": 1, "E50": 1}}"#);
            assert_eq!(old, new);
            assert_eq!((old.level(nfs_renderer::E24), old.level(nfs_renderer::E22), old.level(nfs_renderer::E27), old.level(nfs_renderer::E50)), (0, 2, 1, 1));
        }
    }

    /// ENH-SAVE: changed levels survive a save and a fresh load through a real file — every entry (every panel
    /// page, the platform-only ones where available) at its highest level, then each at 0, then a mixed set.
    #[test]
    fn changed_levels_survive_save_and_load() {
        let dir = std::env::temp_dir().join(format!("nfs-enh-save-{}", std::process::id()));
        let path = dir.join("enhancements.json");
        let mut top = Enhancements::all();
        let mut zero = Enhancements::all();
        let mut mixed = Enhancements::all();
        for (i, en) in ENHANCEMENT_REGISTRY.iter().enumerate() {
            let s = nfs_renderer::EnhancementSwitch(i as u8);
            if en.available() {
                top.set_level(s, en.levels.len() as u8 - 1);
                zero.set_level(s, 0);
                // every second entry off, every third at its top: differs from the defaults in both directions
                mixed.set_level(s, if i % 2 == 0 { 0 } else if i % 3 == 0 { en.levels.len() as u8 - 1 } else { en.default_level });
            }
        }
        for e in [top, zero, mixed] {
            save_to(&path, &e).unwrap();
            let (back, mode) = load_with(None, std::fs::read_to_string(&path).ok().as_deref());
            assert_eq!((back, mode), (e, Mode::Normal));
            // a second save of what was loaded is the same file (no drift to the defaults)
            save_to(&path, &back).unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), file_text(&e));
        }
        // every entry is in the file (none falls back to its default for want of a key)
        let text = file_text(&Enhancements::all());
        for en in ENHANCEMENT_REGISTRY {
            assert!(text.contains(&format!("\"{}\"", en.token)), "{} missing from the file", en.token);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ENH-SAVE: the A/B key is a view — off, then back to the player's own levels, not to the defaults; and from an all-off
    /// player set it shows the defaults and returns to off.
    #[test]
    fn the_ab_key_returns_the_players_set() {
        let mut player = Enhancements::all();
        player.set_level(nfs_renderer::E1, 3);
        player.set_level(nfs_renderer::E9, 0);
        let (shown, stash) = ab_toggle(player, None);
        assert_eq!(shown, Enhancements::ORIGINAL);
        let (shown, stash) = ab_toggle(shown, stash);
        assert_eq!((shown, stash), (player, None));
        let (shown, stash) = ab_toggle(Enhancements::ORIGINAL, None);
        assert_eq!(shown, Enhancements::all());
        assert_eq!(ab_toggle(shown, stash), (Enhancements::ORIGINAL, None));
    }

    #[test]
    fn e34_old_spatial_levels_are_renumbered() {
        // ENHANCEMENT E34: a file written before the temporal levels (version 2) has the spatial 1..3; they are 5..7 now
        let get = |text: &str| parse_file(text).level(nfs_renderer::E34);
        let mac = cfg!(target_os = "macos");
        for (old, new) in [(0u8, 0u8), (1, 5), (2, 6), (3, 7)] {
            assert_eq!(get(&format!(r#"{{"version":2,"levels":{{"E34":{old}}}}}"#)), if mac { new } else { 0 });
            assert_eq!(get(&format!(r#"{{"levels":{{"E34":{old}}}}}"#)), if mac { new } else { 0 });
        }
        // version 3 is stored as it is (1 = the temporal smoothing-only level)
        assert_eq!(get(r#"{"version":3,"levels":{"E34":1}}"#), if mac { 1 } else { 0 });
        assert_eq!(get(r#"{"version":3,"levels":{"E34":7}}"#), if mac { 7 } else { 0 });
        // the writer is version 7
        assert!(file_text(&Enhancements::all()).contains("\"version\": 7"));
        // ENHANCEMENT E41: a file written before off / on (version <= 6): any level above 0 is on, 0 stays off
        let e41 = |t: &str| parse_file(t).level(nfs_renderer::E41);
        assert_eq!(e41(r#"{"version":5,"levels":{"E41":1}}"#), 1);
        assert_eq!(e41(r#"{"version":5,"levels":{"E41":0}}"#), 0);
        assert_eq!(e41(r#"{"version":6,"levels":{"E41":1}}"#), 1);
        assert_eq!(e41(r#"{"version":6,"levels":{"E41":2}}"#), 1);
        assert_eq!(e41(r#"{"version":7,"levels":{"E41":0}}"#), 0);
        assert_eq!(e41(r#"{"version":5,"levels":{"E1":1}}"#), 1); // not named: the default (on)
        // ENHANCEMENT ENH-COAT: E8's stored levels of a file written before the clear coat (version <= 4): low (1) -> normal (2), high (2) -> new (1);
        // 0 stays off, a version-5 file is read as it is
        let e8 = |text: &str| parse_file(text).level(nfs_renderer::E8);
        assert_eq!(e8(r#"{"version":4,"levels":{"E8":1}}"#), 2);
        assert_eq!(e8(r#"{"version":4,"levels":{"E8":2}}"#), 1);
        assert_eq!(e8(r#"{"version":4,"levels":{"E8":0}}"#), 0);
        assert_eq!(e8(r#"{"version":3,"levels":{"E8":2}}"#), 1);
        assert_eq!(e8(r#"{"version":5,"levels":{"E8":1}}"#), 1);
        assert_eq!(e8(r#"{"version":5,"levels":{"E8":3}}"#), 3);
        // a file with no E8 is at the default (normal)
        assert_eq!(e8(r#"{"version":4,"levels":{"E1":1}}"#), 2);
    }

    #[test]
    fn the_file_round_trips() {
        let e = Enhancements::all();
        assert_eq!(parse_file(&file_text(&e)), e);
        assert_eq!(parse_file("not json"), Enhancements::all());
        assert_eq!(parse_file("{}"), Enhancements::all());
        // every level of every entry survives
        // (an entry this platform has no backend for - E33 / E34 off macOS, E37 on macOS - cannot be set: it stays 0, see `Entry::available`)
        for (i, en) in ENHANCEMENT_REGISTRY.iter().enumerate().filter(|(_, e)| e.available()) {
            for l in 0..en.levels.len() as u8 {
                let mut x = Enhancements::all();
                x.set_level(nfs_renderer::EnhancementSwitch(i as u8), l);
                assert_eq!(parse_file(&file_text(&x)), x);
            }
        }
        // an absent token is at its default
        let d = parse_file(r#"{"version":2,"levels":{"E1":0}}"#);
        let mut want = Enhancements::all();
        want.set_level(nfs_renderer::E1, 0);
        assert_eq!(d, want);
        // the first (boolean) format: true = default level, false = off
        let old = parse_file(r#"{"version":1,"switches":{"E1":false,"E2":true,"E3":true}}"#);
        assert_eq!(old.level(nfs_renderer::E1), 0);
        assert_eq!(old.level(nfs_renderer::E2), ENHANCEMENT_REGISTRY[1].default_level);
        assert_eq!(old.level(nfs_renderer::E3), ENHANCEMENT_REGISTRY[2].default_level);
        assert_eq!(old.level(nfs_renderer::E4), ENHANCEMENT_REGISTRY[3].default_level);
        // a level past the choices is clamped
        assert_eq!(parse_file(r#"{"levels":{"E1":99}}"#).level(nfs_renderer::E1) as usize, ENHANCEMENT_REGISTRY[0].levels.len() - 1);
    }

    #[test]
    fn nfs_enh_override_forms() {
        use nfs_renderer::{E1, E3};
        assert_eq!(load_with(Some("all"), None).0, Enhancements::all());
        let (e, m) = load_with(Some("E1,E3"), Some(r#"{"levels":{"E2":1}}"#));
        assert_eq!(m, Mode::Normal);
        assert_eq!((e.level(E1), e.level(E3)), (ENHANCEMENT_REGISTRY[0].default_level, ENHANCEMENT_REGISTRY[2].default_level));
        assert!(!e.is_on(nfs_renderer::E2)); // the file is not read under an override
        let (e, _) = load_with(Some("E1=3,E3=0"), None);
        assert_eq!((e.level(E1), e.level(E3)), (3, 0));
        // no override: the file, else the defaults
        assert_eq!(load_with(None, Some(r#"{"levels":{"E1":1}}"#)).0.level(E1), 1);
        assert_eq!(load_with(None, None).0, Enhancements::all());
    }

    #[test]
    fn combo_rows_are_the_level_captions() {
        for (i, e) in ENHANCEMENT_REGISTRY.iter().enumerate() {
            // the upscaler row lists only this platform's backends (no MetalFX off macOS)
            let want = if i == nfs_renderer::E38.0 as usize { nfs_renderer::upscaler_choices().len() } else { e.levels.len() };
            assert_eq!(level_ids(i).len(), want);
        }
        let mut s = Strings::default();
        s.fe_by_id.insert(1500, "off".into());
        let s = with_extras(s);
        assert_eq!(s.get(level_ids(0)[0]), Some("off"));
        assert_eq!(s.get(level_ids(0)[2]), Some("4x"));
        assert!(s.get(STR_DEFAULTS).is_some());
    }

    /// The sets the panel is checked for: every upscaler choice (MetalFX only where the platform has it), with HDR off too.
    fn panel_sets() -> Vec<Enhancements> {
        let mut v = Vec::new();
        for &up in nfs_renderer::upscaler_choices() {
            for hdr in [1u8, 0] {
                let mut e = Enhancements::all();
                e.set_level(nfs_renderer::E38, up);
                e.set_level(nfs_renderer::E7, hdr);
                v.push(e);
            }
        }
        v
    }

    fn lines_of(pages: &[Vec<Item>]) -> Vec<Item> {
        pages.iter().flatten().copied().collect()
    }

    #[test]
    fn every_entry_has_a_row_or_is_an_upscaler_setting() {
        let mut seen: Vec<&str> = GROUPS.iter().flat_map(|(_, t)| t.iter().map(|s| s.token())).chain(SETTINGS_OF_E38.iter().map(|s| s.token())).collect();
        for e in ENHANCEMENT_REGISTRY.iter() {
            assert_eq!(seen.iter().filter(|t| **t == e.token).count(), 1, "{} is in the panel exactly once", e.token);
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), ENHANCEMENT_REGISTRY.len(), "a token of the panel that is not in the registry");
    }

    #[test]
    fn only_the_chosen_upscaler_has_settings_rows() {
        let rows = |up: u8| {
            let mut e = Enhancements::all();
            e.set_level(nfs_renderer::E38, up);
            lines_of(&panel_pages(&e))
        };
        let has = |l: &[Item], it: Item| l.contains(&it);
        let e39 = Item::Entry(nfs_renderer::E39.0 as usize);
        // off: neither backend's settings; E34 / E37 / E39 never have a row of their own
        let off = rows(nfs_renderer::UP_OFF);
        assert!(!has(&off, Item::Virtual(VRow::MfxMode)) && !has(&off, Item::Virtual(VRow::MfxScale)) && !has(&off, Item::Virtual(VRow::FsrScale)) && !has(&off, e39));
        for k in [nfs_renderer::E34, nfs_renderer::E37] {
            assert!(!off.contains(&Item::Entry(k.0 as usize)));
        }
        // FSR: its scale and sharpness, not MetalFX's
        let fsr = rows(nfs_renderer::UP_FSR);
        assert!(has(&fsr, Item::Virtual(VRow::FsrScale)) && has(&fsr, e39) && !has(&fsr, Item::Virtual(VRow::MfxMode)));
        // the settings follow the choice row at once
        let at = |l: &[Item], it: Item| l.iter().position(|x| *x == it).unwrap();
        let up = Item::Entry(nfs_renderer::E38.0 as usize);
        assert_eq!((at(&fsr, Item::Virtual(VRow::FsrScale)), at(&fsr, e39)), (at(&fsr, up) + 1, at(&fsr, up) + 2));
        if cfg!(target_os = "macos") {
            let mfx = rows(nfs_renderer::UP_METALFX);
            assert!(has(&mfx, Item::Virtual(VRow::MfxMode)) && has(&mfx, Item::Virtual(VRow::MfxScale)) && !has(&mfx, Item::Virtual(VRow::FsrScale)) && has(&mfx, e39));
            assert_eq!((at(&mfx, Item::Virtual(VRow::MfxScale)), at(&mfx, e39)), (at(&mfx, up) + 2, at(&mfx, up) + 3));
        } else {
            // no MetalFX off macOS: a stored choice 1 shows no settings
            assert!(!has(&rows(nfs_renderer::UP_METALFX), Item::Virtual(VRow::MfxMode)));
        }
    }

    /// Every available entry shows once, a group is never split over two pages while it fits on one, no page is over the limit.
    #[test]
    fn pages_keep_groups_whole_and_cover_every_row_once() {
        for e in panel_sets() {
            let groups = group_items(&e);
            let pages = panel_pages(&e);
            assert!(pages.iter().all(|p| !p.is_empty() && p.len() <= MAX_ROWS));
            // every line once, in group order
            assert_eq!(lines_of(&pages), groups.iter().flatten().copied().collect::<Vec<_>>());
            for g in &groups {
                if g.len() <= MAX_ROWS {
                    let on: Vec<usize> = pages.iter().enumerate().filter(|(_, p)| p.contains(&g[0])).map(|(i, _)| i).collect();
                    assert_eq!(on.len(), 1);
                    assert!(g.iter().all(|it| pages[on[0]].contains(it)), "group {:?} is split", g[0]);
                }
            }
            // every available entry that has a row appears; the headers are the groups'
            let lines = lines_of(&pages);
            for (i, en) in ENHANCEMENT_REGISTRY.iter().enumerate().filter(|(_, e)| e.available() && !SETTINGS_OF_E38.iter().any(|s| s.token() == e.token)) {
                assert_eq!(lines.iter().filter(|it| **it == Item::Entry(i)).count(), 1, "{}", en.token);
            }
            assert_eq!(lines.iter().filter(|it| matches!(it, Item::Header(_))).count(), GROUPS.len());
        }
        // a group longer than a page is split, a short one starts a new page instead
        let long: Vec<Item> = (0..20).map(Item::Entry).collect();
        let short: Vec<Item> = (0..5).map(Item::Entry).collect();
        assert_eq!(paginate(&[long.clone()], 16).iter().map(Vec::len).collect::<Vec<_>>(), vec![16, 4]);
        assert_eq!(paginate(&[short.clone(), short.clone(), short.clone(), short.clone()], 16).iter().map(Vec::len).collect::<Vec<_>>(), vec![15, 5]);
        assert_eq!(paginate(&[short.clone(), long], 16).iter().map(Vec::len).collect::<Vec<_>>(), vec![5, 16, 4]);
        assert_eq!(paginate(&[], 16), vec![Vec::<Item>::new()]);
        assert_eq!(pages_for(MAX_ROWS * 2 + 6), 3);
        assert_eq!(pages_for(0), 1);
    }

    #[test]
    fn the_rows_go_ahead_of_the_backgrounds() {
        let file = "[section.0]\nid=REAL.BLACK_640x480\n\n[section.1]\nid=NFS5.GOURAUDBOX\n";
        let t = with_rows(file, "\n[section.900]\nid=NFS5.ComboBoxSlider\n");
        assert!(t.find("ComboBoxSlider").unwrap() < t.find("GOURAUDBOX").unwrap());
        assert!(t.starts_with("[section.0]"));
    }

    #[test]
    fn generated_rows_have_the_lines_of_the_page() {
        for e in panel_sets() {
            let pages = panel_pages(&e);
            let mut combos = 0;
            for (page, lines) in pages.iter().enumerate() {
                let rows = generated_rows_in(&e, page);
                let n = lines.iter().filter(|it| !matches!(it, Item::Header(_))).count();
                assert_eq!(rows.matches("NFS5.ComboBoxSlider").count(), n);
                assert_eq!(rows.matches("helptext=").count(), n);
                // a caption label per line (headers too), a fade bar per combo
                assert_eq!(rows.matches("id=REAL.LABEL").count(), lines.len());
                assert_eq!(rows.matches("NFS5.GOURAUDBOX").count(), n);
                // the page button exactly when there is more than one page, walking on and back to the first
                assert_eq!(rows.contains("ID_ENHMOREBUTTON"), pages.len() > 1);
                if pages.len() > 1 {
                    assert!(rows.contains(&format!("callback={CB_PAGE}{}", if page + 1 < pages.len() { page + 2 } else { 1 })));
                }
                combos += n;
            }
            assert!(combos > 0);
        }
        // the accessors: entries `Enh.<token>`, the settings rows their own
        let mut e = Enhancements::all();
        e.set_level(nfs_renderer::E38, nfs_renderer::UP_FSR);
        let all: String = (0..panel_pages(&e).len()).map(|p| generated_rows_in(&e, p)).collect();
        assert!(all.contains("accessor=Enh.FSRSCALE") && all.contains("accessor=Enh.E39") && all.contains("accessor=Enh.E38") && !all.contains("accessor=Enh.E34") && !all.contains("accessor=Enh.E37"));
    }

    /// An inactive row is the engine's disabled state with the reason as its hover help; its stored level is kept and it comes back.
    #[test]
    fn inactive_rows_say_why_and_keep_their_level() {
        let mut e = Enhancements::all();
        let bloom = ENHANCEMENT_REGISTRY.iter().position(|x| x.token == "E10").unwrap();
        let info = |e: &Enhancements| line_info(Item::Entry(bloom), e).unwrap();
        assert!(!info(&e).inactive);
        assert_eq!(info(&e).help_id, STR_HELP + bloom as u32);
        let level = e.level(nfs_renderer::E10);
        e.set_level(nfs_renderer::E7, 0);
        let i = info(&e);
        assert!(i.inactive);
        assert!(i.help_id >= STR_REASON && i.help_id != STR_HELP + bloom as u32);
        assert_eq!(e.level(nfs_renderer::E10), level, "the stored level is kept");
        // the page shows it disabled, grey, with the reason id as its help
        let page: String = (0..panel_pages(&e).len()).map(|p| generated_rows_in(&e, p)).collect();
        assert!(page.contains("state=disabled") && page.contains(INACTIVE_COLOR) && page.contains(&format!("helptext={}", i.help_id)));
        // switching the cause back restores the row
        e.set_level(nfs_renderer::E7, 1);
        assert!(!info(&e).inactive);
        // the hint names the setting that disables it
        let mut s = Strings::default();
        s.fe_by_id.insert(1500, "off".into());
        let s = with_extras(s);
        e.set_level(nfs_renderer::E7, 0);
        assert!(s.get(info(&e).help_id).is_some_and(|t| t.contains("HDR and tone mapping")), "{:?}", s.get(info(&e).help_id));
        // the upscaler overrides name the backend
        let (render_scale, msaa) = (ENHANCEMENT_REGISTRY.iter().position(|x| x.token == "E3").unwrap(), ENHANCEMENT_REGISTRY.iter().position(|x| x.token == "E1").unwrap());
        let mut f = Enhancements::all();
        f.set_level(nfs_renderer::E38, nfs_renderer::UP_FSR);
        assert!(line_info(Item::Entry(render_scale), &f).unwrap().inactive && line_info(Item::Entry(msaa), &f).unwrap().inactive);
        assert!(s.get(line_info(Item::Entry(render_scale), &f).unwrap().help_id).is_some_and(|t| t.contains("FSR 3.1")));
        // the lines change with the set (a rebuild is needed) exactly when they differ
        assert_ne!(panel_signature(&Enhancements::all()), panel_signature(&f));
        assert_eq!(panel_signature(&f), panel_signature(&f));
    }

    #[test]
    fn every_reason_and_virtual_row_has_a_string() {
        let mut s = Strings::default();
        s.fe_by_id.insert(1500, "off".into());
        let s = with_extras(s);
        for k in 0..nfs_renderer::ENHANCEMENT_DEPENDS.len() as u32 * 3 {
            assert!(s.get(STR_REASON + k).is_some_and(|t| !t.is_empty()), "reason {k}");
        }
        for v in VRow::ALL {
            assert!(s.get(v.name_id()).is_some() && s.get(v.help_id()).is_some());
            assert!(vrow_ids(v).iter().all(|id| s.get(*id).is_some()));
        }
        for g in 0..GROUPS.len() as u32 {
            assert!(s.get(STR_GROUP + g).is_some());
        }
        // the upscaler row lists this platform's backends, off first
        let ids = level_ids(nfs_renderer::E38.0 as usize);
        assert_eq!(ids.len(), nfs_renderer::upscaler_choices().len());
        assert_eq!(s.get(ids[0]), Some("off"));
        assert_eq!(s.get(*ids.last().unwrap()), Some("FSR 3.1"));
        // the simple MetalFX mode lists the three scales after 1:1
        assert_eq!(vrow_ids(VRow::MfxScale).len(), 4);
        assert_eq!(vrow_ids(VRow::MfxMode).len(), 2);
    }

    /// The settings file: a file written before the upscaler choice (version <= 3) turns its stored backend on; version 4 is read as it is.
    #[test]
    fn old_files_choose_their_upscaler() {
        use nfs_renderer::{E34, E37, E38, E39, UP_FSR, UP_METALFX, UP_OFF};
        let mac = cfg!(target_os = "macos");
        // MetalFX stored on (v3: temporal 59 %)
        let m = parse_file(r#"{"version":3,"levels":{"E34":3}}"#);
        assert_eq!((m.level(E38), m.level(E34)), if mac { (UP_METALFX, 3) } else { (UP_OFF, 0) });
        assert_eq!(m.effective().level(E34), if mac { 3 } else { 0 });
        // the first spatial levels (v2: 1..3 = 5..7) and the choice
        let sp = parse_file(r#"{"version":2,"levels":{"E34":2}}"#);
        assert_eq!((sp.level(E38), sp.level(E34)), if mac { (UP_METALFX, 6) } else { (UP_OFF, 0) });
        // FSR stored on (v3): the choice is FSR, the scale stays, sharpness = the old constant
        let f = parse_file(r#"{"version":3,"levels":{"E37":4}}"#);
        assert_eq!((f.level(E38), f.level(E37), f.level(E39)), (UP_FSR, 4, 2));
        assert_eq!(nfs_renderer::fsr_sharpness(f.level(E39)), Some(0.3));
        // neither on: the choice stays off
        let off = parse_file(r#"{"version":3,"levels":{"E34":0,"E37":0}}"#);
        assert_eq!(off.level(E38), UP_OFF);
        // version 4: E38 as stored, hidden settings kept (FSR chosen, E34 left over)
        let v4 = parse_file(r#"{"version":4,"levels":{"E38":2,"E34":3,"E37":3,"E39":4}}"#);
        assert_eq!((v4.level(E38), v4.level(E37), v4.level(E39)), (UP_FSR, 3, 4));
        assert_eq!(v4.level(E34), if mac { 3 } else { 0 });
        assert_eq!((v4.effective().level(E37), v4.effective().level(E34)), (3, 0));
        let off4 = parse_file(r#"{"version":4,"levels":{"E38":0,"E37":3}}"#);
        assert_eq!((off4.level(E38), off4.level(E37), off4.effective().level(E37)), (UP_OFF, 3, 0));
        // a version-4 file written from a set round-trips exactly (E38 / E39 in it)
        let mut e = Enhancements::all();
        e.set_level(E38, UP_FSR);
        e.set_level(E37, 3);
        e.set_level(E39, 3);
        assert_eq!(parse_file(&file_text(&e)), e);
    }

    #[test]
    fn the_strings_follow_the_install_language() {
        let mut s = Strings::default();
        s.fe_by_id.insert(1500, "выкл".chars().map(|c| typed_char(c).unwrap()).collect());
        assert!(is_russian(&s));
        let s = with_extras(s);
        assert_eq!(s.get(STR_RESET).map(|t| t.chars().count()), Some("как в оригинале".chars().count()));
        // Windows-1251 bytes carried as cp1252 chars: «к» is 0xEA
        assert_eq!(s.get(STR_RESET).and_then(|t| t.chars().next()), Some(nfs_formats::locale::cp1252(0xEA)));
        let mut e = Strings::default();
        e.fe_by_id.insert(1500, "off".into());
        assert!(!is_russian(&e));
        assert_eq!(with_extras(e).get(STR_RESET), Some("as in the original"));
    }

    #[test]
    fn screen_names() {
        assert!(is_screen("Enhancements") && is_screen("enhancements.lay") && !is_screen("options"));
        assert_eq!(splice_file("GraphicsTabs"), Some("graphicstabs_entry.lay"));
        assert_eq!(splice_file("graphics.lay"), Some("graphics_entry.lay"));
        assert_eq!(splice_file("graphicsettings"), None);
        assert_eq!(splice_file("options"), None);
    }
}
