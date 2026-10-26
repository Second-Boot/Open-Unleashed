//! PORT-DEVIATION D46 (photo mode, `RE/notes/PORT-DEVIATIONS.md`): the photo mode's own on-screen text and keys.
//! **Not in the original**: `nfs5.exe` has no photo mode, so none of these strings is in `FEStrings.loc` / `hud.loc`.
//! The text is the port's, English and Russian; every other install language gets the English one (the game's
//! own tables carry no line to translate from).

use nfs_formats::locale::{Strings, typed_char};

/// Whether F11 is a photo-mode key on this platform. On macOS F11 belongs to the system (Show Desktop / Stage
/// Manager sweeps the game window away), so photo mode there is **K only**; Windows and Linux keep F11 and K.
/// (The original's `I_VIEWDISTANCE` is `SC_F11` but names a key its key table does not have —
/// `nfs_input::default_binding` — and K is bound to nothing, so neither key takes an original action.)
pub const PHOTO_F11: bool = !cfg!(target_os = "macos");

/// The ids of [`PHOTO_STRINGS`], in its order.
pub const S_TITLE: usize = 0;
pub const S_KEYS_MOVE: usize = 1;
pub const S_KEYS_ZOOM: usize = 2;
pub const S_KEYS_SHOT: usize = 3;
pub const S_KEYS_PAD: usize = 4;
pub const S_SIZE: usize = 5;
pub const S_FOV: usize = 6;
pub const S_SAVED: usize = 7;
pub const S_ENTER: usize = 8;
pub const S_FAILED: usize = 9;

/// The photo mode's own text, English and Russian: the title, three key lines and a pad line, the words of the
/// status line, the pointer shown on the pause menu and in a replay, and the failure. `{KEY}` / `{EXIT}` are the
/// platform's keys ([`photo_keys`]).
pub const PHOTO_STRINGS: [[&str; 2]; 10] = [
    ["photo mode", "фото-режим"],
    ["WASD move, Q / E down / up, arrows or right mouse button look, Z / X roll", "WASD движение, Q / E вниз / вверх, стрелки или правая кнопка мыши обзор, Z / X крен"],
    ["wheel or - / = zoom, Shift faster, Ctrl slower, R reset the camera", "колесо или - / = масштаб, Shift быстрее, Ctrl медленнее, R вернуть камеру"],
    ["Enter / Space shoot, 6 size, H HUD, Tab hint, {EXIT} exit", "Enter / Пробел снимок, 6 размер, H индикаторы, Tab подсказка, {EXIT} выход"],
    ["pad: sticks move / look, triggers down / up, bumpers roll, d-pad up / down zoom, A shoot, B exit, Y HUD", "геймпад: стики движение / обзор, курки вниз / вверх, бамперы крен, крестовина вверх / вниз масштаб, A снимок, B выход, Y индикаторы"],
    ["size", "размер"],
    ["FOV", "угол"],
    ["saved", "сохранено"],
    ["{KEY}: photo mode", "{KEY}: фото-режим"],
    ["the still could not be saved", "снимок не удалось сохранить"],
];

/// The photo mode's text with its key placeholders filled for this platform: `{KEY}` the keys that enter it,
/// `{EXIT}` the keys that leave it (Esc, P, Backspace and the pad's B stay on every platform).
pub fn photo_keys(s: &str, russian: bool) -> String {
    let (key, exit) = match (PHOTO_F11, russian) {
        (true, false) => ("F11 or K", "F11 / K / Esc"),
        (true, true) => ("F11 или K", "F11 / K / Esc"),
        (false, _) => ("K", "K / Esc"),
    };
    s.replace("{KEY}", key).replace("{EXIT}", exit)
}

/// Whether the install's text is Russian. The Russian release keeps its text in the **English** column of
/// `FEStrings.loc` (language 0, so the language index cannot tell), as Windows-1251 bytes the port carries as
/// `cp1252` chars: a Cyrillic letter is one of U+00C0..U+00FF. FE string 1500 is then mostly such letters; a
/// German / French / Spanish / ... line has a few accented Latin letters among plain ASCII ones, so it does not count.
pub fn is_russian(strings: &Strings) -> bool {
    strings.get(1500).is_some_and(|s| {
        let high = s.chars().filter(|c| ('\u{C0}'..='\u{FF}').contains(c) && *c != '\u{D7}' && *c != '\u{F7}').count();
        let ascii = s.chars().filter(char::is_ascii_alphabetic).count();
        high > 0 && high > ascii
    })
}

/// A port string as the game's byte-keyed fonts draw it: the Russian text as its Windows-1251 bytes carried as
/// `cp1252` chars (`nfs_formats::locale::typed_char`), `?` for a char the fonts cannot have.
pub fn as_game_text(s: &str) -> String {
    s.chars().map(|c| typed_char(c).unwrap_or('?')).collect()
}

/// Line `k` of [`PHOTO_STRINGS`] in the install's language (English for every language but Russian), keys filled in.
pub fn photo_text(strings: &Strings, k: usize) -> String {
    let ru = is_russian(strings);
    PHOTO_STRINGS.get(k).map_or_else(String::new, |t| as_game_text(&photo_keys(t[ru as usize], ru)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_follow_the_platform() {
        let t = photo_keys(PHOTO_STRINGS[S_ENTER][0], false);
        if PHOTO_F11 {
            assert_eq!(t, "F11 or K: photo mode");
        } else {
            assert_eq!(t, "K: photo mode");
        }
        assert!(!photo_keys(PHOTO_STRINGS[S_KEYS_SHOT][1], true).contains('{'));
    }

    #[test]
    fn russian_is_told_from_the_accented_languages() {
        let with = |s: &str| {
            let mut t = Strings::default();
            t.fe_by_id.insert(1500, s.to_string());
            t
        };
        // "Гонка" as the Windows-1251 bytes the port holds as cp1252 chars
        assert!(is_russian(&with(&as_game_text("Гонка"))));
        assert!(!is_russian(&with("Rennen über Land")));
        assert!(!is_russian(&with("Course libérée")));
        assert!(!is_russian(&with("Race")));
        assert!(!is_russian(&Strings::default()));
        assert_eq!(photo_text(&with("Race"), S_TITLE), "photo mode");
        assert_eq!(photo_text(&with(&as_game_text("Гонка")), S_TITLE), as_game_text("фото-режим"));
    }
}
