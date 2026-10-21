//! `fe.txt` and the command line -- the option stream the game starts from
//! (`RE/notes/engine-audio.md` §8.4: registry head `0x5D1E40`, records `{u32 id, void* ptr, char* name}`;
//! the grammar is `RE/notes/fe-txt-grammar.md`).
//!
//! The original builds one message stream in `0x4B6660`: `fe.txt` first (`0x4B5EE0`), then every
//! `argv` string (`0x4B60D0`), and applies it in order, so a later assignment of the same option wins
//! and the command line overrides the file. Both scanners walk the bytes and react to four
//! characters only:
//!
//! * `#` (file only) -- the one byte after it is bumped until it is `\n` or 0 and then once more
//!   (0x4B5F8E), nothing else: **the rest of the line is still scanned**. Because the name of an
//!   assignment is read back to the start of its line, a `#` line can never name an option;
//! * `@` -- the rest of the line, at most 48 non-blank characters, plus `.txt`, is another file
//!   parsed in place (0x4B5FB4 / 0x4B6146);
//! * `=` and `[` -- an assignment ([`Scan::assignment`], 0x4B5470): the name is the up-to-63 characters
//!   before the character back to the line start, blanks removed, looked up case-insensitively in
//!   the 590-record registry ([`crate::fetxt_table::OPTIONS`]); an unknown name is dropped together with
//!   its line (0x4B5E64). `car[0to7].field = v` is the per-car form: the `[` after the name `CAR`.
//! * everything else is skipped. There are **no** `;` comments and no trailing comments.
//!
//! Values are integers, evaluated by 0x4B5250 ([`eval`]: decimal, `0x` hex, `-`, the symbolic table of
//! 0x5D63A0), strings (15 characters at most), comma lists of integers, or the input bindings
//! (kept as the text `device,payload`, which `nfs-input` reads). Nothing in it is required: a missing
//! key means the game's own default.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use crate::fetxt_table::{CAR_TYPES, OPTIONS, SYMBOLS};

/// The number of per-car records (`numCars`, 0x4B4DB6: 9 x 0x580 bytes from 0x65808C).
const NUM_CARS: i32 = 9;

/// `strcmpi` (0x5AE3C0) on bytes.
fn same_name(a: &[u8], b: &str) -> bool {
    a.eq_ignore_ascii_case(b.as_bytes())
}

fn blank(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r')
}

/// **0x4B5250 `eval`** -- one pass over the text, no error path.
pub fn eval(s: &[u8]) -> i32 {
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let (mut acc, mut sign, mut base, mut packed, mut comp) = (0i32, 1i32, 10i32, 0i32, 0);
    let mut i = 0usize;
    while at(i) != 0 {
        let c = at(i);
        match c {
            // 0x4B52CF: a blank resets the component
            b' ' => {
                acc = 0;
                sign = 1;
            }
            // 0x4B5298: ',' commits a component: the first two shift into place, the rest sit at bit 8
            b',' => {
                if comp < 2 {
                    packed = ((acc << 12) | packed) << 4;
                } else {
                    packed |= acc << 8;
                }
                base = 10;
                comp += 1;
                acc = 0;
                sign = 1;
            }
            b'-' => sign = -1,
            // 0x4B52E3: 'x' is hex only while the accumulator is still 0
            b'x' | b'X' => {
                if acc == 0 {
                    base = 16;
                }
            }
            _ => {
                if c.is_ascii_digit() {
                    acc = ((c - b'0') as i32).wrapping_mul(sign).wrapping_add(base.wrapping_mul(acc));
                } else if base == 16 && (b'A'..=b'F').contains(&c) {
                    acc = (acc << 4).wrapping_add((c as i32 - 0x37).wrapping_mul(sign));
                } else if base == 16 && (b'a'..=b'f').contains(&c) {
                    acc = (acc << 4).wrapping_add((c as i32 - 0x57).wrapping_mul(sign));
                } else if acc == 0 && c.is_ascii_alphabetic() {
                    // 0x4B535B: the REST of the text is the symbol; the terminator {0,0} answers 0
                    let rest = &s[i..s.iter().position(|&b| b == 0).unwrap_or(s.len())];
                    acc = SYMBOLS.iter().find(|(n, _)| same_name(rest, n)).map_or(0, |(_, v)| *v);
                    // the cursor lands on the last character and steps past it: the text is done
                    break;
                }
            }
        }
        i += 1;
    }
    packed.wrapping_add(acc)
}

/// The message stream: `(registry key, value text)` in the order the original appends them.
type Stream = Vec<(String, String)>;

/// A file reader for `@name` and `IMPORT = name`: the original opens them relative to its own
/// directory (0x59E040), the port to the game root.
type Loader<'a> = &'a dyn Fn(&str) -> Option<Vec<u8>>;

struct Scan<'a> {
    out: Stream,
    load: Loader<'a>,
    depth: u32,
}

/// The byte at `i`, 0 outside the buffer (the original reads the NUL or whatever follows its heap block).
fn at(buf: &[u8], i: isize) -> u8 {
    if i < 0 { 0 } else { buf.get(i as usize).copied().unwrap_or(0) }
}

/// The value readers of 0x4B5470 (and 0x4B51E0): non-blank characters up to `delim`, a NUL or `lim`
/// of them; the cursor ends one past where it stopped.
fn read_until(buf: &[u8], cur: &mut usize, delim: u8, lim: isize) -> Vec<u8> {
    let mut v = Vec::new();
    loop {
        let c = at(buf, *cur as isize);
        if c == delim || c == 0 || lim <= v.len() as isize {
            break;
        }
        if !blank(c) {
            v.push(c);
        }
        *cur += 1;
    }
    *cur += 1;
    v
}

/// `min(cap, bytes left)` -- `(start - cursor) + len` against the cap, every reader of 0x4B5470.
fn limit(buf: &[u8], cur: usize, cap: isize) -> isize {
    cap.min(buf.len() as isize - cur as isize)
}

impl<'a> Scan<'a> {
    /// **0x4B5EE0 `parse_text_file`** over a loaded buffer.
    fn text(&mut self, mut buf: Vec<u8>) {
        let n = buf.len();
        let mut cur = 0usize;
        while cur < n {
            let c = buf[cur];
            cur += 1;
            match c {
                b'#' => {
                    // 0x4B5F8E: bump the byte after '#' until it is '\n' or 0, then once more; the cursor stays
                    let mut al = at(&buf, cur as isize);
                    if al != b'\n' {
                        while al != 0 && cur < n {
                            al = al.wrapping_add(1);
                            buf[cur] = al;
                            al = buf[cur];
                            if al == b'\n' {
                                break;
                            }
                        }
                    }
                    if cur < n {
                        buf[cur] = buf[cur].wrapping_add(1);
                    }
                }
                b'@' => {
                    let name = self.include_name(&buf, &mut cur);
                    self.include(&name, true);
                }
                b'=' | b'[' => self.assignment(&buf, &mut cur),
                _ => {}
            }
        }
    }

    /// 0x4B5FB4: the rest of the line as a file name -- at most min(48, bytes left) non-blank characters.
    fn include_name(&self, buf: &[u8], cur: &mut usize) -> Vec<u8> {
        let lim = limit(buf, *cur, 0x30);
        read_until(buf, cur, b'\n', lim)
    }

    fn include(&mut self, name: &[u8], add_ext: bool) {
        if self.depth >= 16 {
            return; // the original has no limit (and crashes on a cycle)
        }
        let mut n = String::from_utf8_lossy(name).into_owned();
        if add_ext {
            n.push_str(".txt");
        }
        if let Some(b) = (self.load)(&n) {
            self.depth += 1;
            self.text(b);
            self.depth -= 1;
        }
    }

    /// **0x4B60D0 `parse_argv`** for one string: `=` / `[` assign, `@` includes the rest, nothing else counts.
    fn argument(&mut self, arg: &[u8]) {
        let mut cur = 0usize;
        while at(arg, cur as isize) != 0 && cur < arg.len() {
            let c = arg[cur];
            cur += 1;
            match c {
                b'=' | b'[' => self.assignment(arg, &mut cur),
                b'@' => {
                    let name = self.include_name(arg, &mut cur);
                    self.include(&name, true);
                }
                _ => {}
            }
        }
    }

    /// **0x4B5470 `parse_assignment`**: `cur` is one past the `=` / `[`.
    fn assignment(&mut self, buf: &[u8], cur: &mut usize) {
        let at_cur = *cur as isize;
        // the name: back from the character before the '=' to the line start (at most 63), blanks dropped
        let lim = (at_cur - 1).min(0x3f);
        let mut p = at_cur - 2;
        let mut count = 0isize;
        if at(buf, p) != b'\n' {
            loop {
                if at(buf, p) == 0 || count >= lim {
                    break;
                }
                p -= 1;
                count += 1;
                if at(buf, p) == b'\n' {
                    break;
                }
            }
        }
        p += 1;
        let name: Vec<u8> = (0..count).map(|k| at(buf, p + k)).filter(|&c| !blank(c)).collect();
        // the cursor is back one past the '=' (0x4B54FC)
        *cur = at_cur as usize;

        let Some(&(id, rec_name, applied)) = OPTIONS.iter().find(|(_, n, _)| same_name(&name, n)) else {
            // 0x4B5E64: dropped, and so is the rest of its line (63 characters at most)
            let lim = limit(buf, *cur, 0x3f);
            read_until(buf, cur, b'\n', lim);
            return;
        };
        let id = id as u32;
        if id == 7 {
            // FE: evaluated on the spot (0x4B55BB), it never enters the stream
            let lim = limit(buf, *cur, 0x3f);
            let v = read_until(buf, cur, b'\n', lim);
            self.out.push(("fe".into(), eval(&v).to_string()));
            return;
        }
        if id == 0x21E {
            self.car(buf, cur);
            return;
        }
        let key = rec_name.to_ascii_lowercase();
        if id > 0x14D {
            if id >= 0x21F {
                // a per-car option without `car[..]`: the original builds a malformed message; nothing reads it
                let lim = limit(buf, *cur, 0x3f);
                read_until(buf, cur, b'\n', lim);
                return;
            }
            // an input binding (0x4B5A16): `device,payload` -- the device up to the first comma (not the line!),
            // the payload to the end of the line; kept as text for `nfs-input`
            let dev = read_until(buf, cur, b',', 0x3f);
            let lim = limit(buf, *cur, 0x3f);
            let rest = read_until(buf, cur, b'\n', lim);
            let v = format!("{},{}", String::from_utf8_lossy(&dev), String::from_utf8_lossy(&rest));
            self.out.push((key, v));
            return;
        }
        if id < 0x142 {
            if id > 0x10D {
                if id >= 0x13A {
                    // per-car string without `car[..]`
                    let lim = limit(buf, *cur, 0x3f);
                    read_until(buf, cur, b'\n', lim);
                    return;
                }
                let lim = limit(buf, *cur, 0x3f);
                let v = read_until(buf, cur, b'\n', lim);
                if applied {
                    self.out.push((key, string15(&v)));
                }
            } else if id > 1 {
                let lim = limit(buf, *cur, 0x3f);
                let v = read_until(buf, cur, b'\n', lim);
                if applied {
                    self.out.push((key, eval(&v).to_string()));
                }
            } else {
                // IMPORT (0x4B5A16 -> 0x4B5EE0): the value is the name of another file, as written
                let lim = limit(buf, *cur, 0xf);
                let v = read_until(buf, cur, b'\n', lim);
                self.include(&v, false);
            }
        } else {
            // 0x142..0x14D: a list of integers
            if id >= 0x147 {
                let lim = limit(buf, *cur, 0x3ff);
                read_until(buf, cur, b'\n', lim);
                return;
            }
            let lim = limit(buf, *cur, 0x3ff);
            let v = read_until(buf, cur, b'\n', lim);
            if applied {
                self.out.push((key, int_list(&v)));
            }
        }
    }

    /// 0x4B55CB: `car[a to b].field = value` (the CAR namespace, `cur` one past the '[').
    fn car(&mut self, buf: &[u8], cur: &mut usize) {
        let lim = limit(buf, *cur, 0x3f);
        let mut body = read_until(buf, cur, b']', lim);
        // "0to7": the LAST 't' / 'T' (not in the last position) cuts the text; the second half starts two on
        let mut second = None;
        for i in 0..body.len().saturating_sub(1) {
            if body[i] == b't' || body[i] == b'T' {
                second = Some(i + 2);
                body[i] = 0;
            }
        }
        let first = body.iter().position(|&b| b == 0).map_or(&body[..], |z| &body[..z]);
        let lo = eval(first);
        let hi = match second {
            Some(s) => eval(body.get(s..).unwrap_or(&[])),
            None => lo,
        };
        // the field: up to the '=' (63 characters at most), looked up again in the same table
        let lim = limit(buf, *cur, 0x3f);
        let field = read_until(buf, cur, b'=', lim);
        let rec = OPTIONS.iter().find(|(_, n, _)| same_name(&field, n));
        let lim = limit(buf, *cur, 0x3ff);
        let value = read_until(buf, cur, b'\n', lim);
        let Some(&(id, rec_name, applied)) = rec else { return };
        let mut ev = eval(&value);
        if id == 0x21F {
            // `.TYPE`: a car's name replaces the number by its index (0x4B57EC)
            for (i, n) in CAR_TYPES.iter().enumerate() {
                if same_name(&value, n) {
                    ev = i as i32;
                }
            }
        }
        let id = id as u32;
        let text = if id >= 0x21F {
            ev.to_string()
        } else if (0x13A..=0x141).contains(&id) {
            string15(&value)
        } else if (0x147..=0x14D).contains(&id) {
            int_list(&value)
        } else {
            return; // not a per-car option: the original builds a stray message
        };
        if !applied || lo > hi {
            return;
        }
        let low = rec_name.to_ascii_lowercase();
        for n in lo.max(0)..=hi.min(NUM_CARS - 1) {
            self.out.push((format!("car[{n}]{low}"), text.clone()));
        }
    }
}

/// A string value: at most 15 characters reach the message (0x4B5A4C).
fn string15(v: &[u8]) -> String {
    String::from_utf8_lossy(&v[..v.len().min(15)]).into_owned()
}

/// A list value (0x4B5AE9): comma-separated tokens of at most 63 characters, empty ones skipped, each evaluated.
fn int_list(v: &[u8]) -> String {
    let mut out = Vec::new();
    let mut p = 0usize;
    if at(v, 0) != 0 {
        let lim = v.len().min(0x3f);
        loop {
            let mut q = p;
            let mut tok = Vec::new();
            while at(v, q as isize) != b',' && at(v, q as isize) != 0 && tok.len() < lim {
                tok.push(v[q]);
                q += 1;
            }
            p = q + 1;
            if !tok.is_empty() {
                out.push(eval(&tok).to_string());
            }
            if at(v, q as isize) == 0 || at(v, p as isize) == 0 {
                break;
            }
        }
    }
    out.join(",")
}

/// The id of an option name in the original's registry (0x5D1E40), case-insensitive; `None` for a name the
/// original drops.
pub fn option_id(name: &str) -> Option<u16> {
    OPTIONS.iter().find(|(_, n, _)| n.eq_ignore_ascii_case(name)).map(|(i, ..)| *i)
}

/// A symbol of the evaluator's table (0x5D63A0): `BumperCam2` = 1, `human` = 1, `SC_LEFT` = 75, ...
pub fn symbol(name: &str) -> Option<i32> {
    SYMBOLS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| *v)
}

/// The command-line option strings (`name=value`, `@file`, `car[..]..=v`) the process was started with.
static COMMAND_LINE: OnceLock<Vec<String>> = OnceLock::new();

/// Set once at start-up (PLATFORM GLUE): the strings every later [`Registry::load`] reads after `fe.txt`
/// (0x4B6698). The port's own `--flags` are not part of it.
pub fn set_command_line(args: Vec<String>) {
    let _ = COMMAND_LINE.set(args);
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Registry(pub HashMap<String, String>);

impl Registry {
    /// A `fe.txt` text with no files to include (`@name` finds nothing).
    pub fn parse(text: &str) -> Self {
        Self::parse_with(text.as_bytes(), &[], &|_| None)
    }

    /// **0x4B6660**: the file text, then the argument strings, applied in that order -- the last write of a key wins.
    pub fn parse_with(text: &[u8], args: &[String], load: Loader<'_>) -> Self {
        let mut s = Scan { out: Vec::new(), load, depth: 0 };
        s.text(text.to_vec());
        for a in args {
            s.argument(a.as_bytes());
        }
        Self(s.out.into_iter().collect())
    }

    /// `fe.txt` in the game root, then the command line (`fe_out.txt` is the written copy and is ignored).
    pub fn load(game_root: &Path) -> Self {
        let read = |name: &str| std::fs::read(game_root.join(name)).ok();
        let text = read("fe.txt").unwrap_or_default();
        let args = COMMAND_LINE.get().map(Vec::as_slice).unwrap_or(&[]);
        Self::parse_with(&text, args, &read)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(&key.to_lowercase()).map(|s| s.as_str())
    }

    pub fn u32(&self, key: &str, default: u32) -> u32 {
        self.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    pub fn u8(&self, key: &str, default: u8) -> u8 {
        self.get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    pub fn bool(&self, key: &str, default: bool) -> bool {
        self.get(key).map(|v| v != "0").unwrap_or(default)
    }

    /// A comma-separated list of signed integers, as the registry stores every list option.
    pub fn list_i32(&self, key: &str) -> Vec<i32> {
        self.get(key).map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default()
    }

    /// A comma-separated list of integers (`playlist = 0,1,2`).
    pub fn list(&self, key: &str) -> Vec<usize> {
        self.get(key).map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default()
    }
}

/// The race options the front end writes back into the registry (`traffic.md` §1 / the police
/// write-up §17.1 for their ids). The shipped file has all of them at 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaceSettings {
    /// `SKILL` 0..2 — the AI difficulty the catch-up band is keyed on
    pub skill: u8,
    /// `NUM_LAPS`
    pub num_laps: u32,
    /// `TRAFFIC_DENSITY` 0..3 — none / light / medium / heavy
    pub traffic_density: u8,
    /// `COPS`
    pub cops: bool,
    /// `RACE_TYPE` (0 single race, 1 pursuit, 2 tournament, 3 knockout, 4 skills test)
    pub race_type: u8,
    /// `CATCHUP_LOGIC` — a two-player option, not an AI one
    pub catchup_logic: bool,
}

impl Default for RaceSettings {
    fn default() -> Self {
        Self { skill: 0, num_laps: 1, traffic_density: 0, cops: false, race_type: 0, catchup_logic: false }
    }
}

impl RaceSettings {
    pub fn from_registry(r: &Registry) -> Self {
        Self {
            skill: r.u8("skill", 0).min(2),
            num_laps: r.u32("num_laps", 1).max(1),
            traffic_density: r.u8("traffic_density", 0).min(3),
            cops: r.bool("cops", false),
            race_type: r.u8("race_type", 0),
            catchup_logic: r.bool("catchup_logic", false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn game_dir() -> Option<std::path::PathBuf> {
        let mut dirs = Vec::new();
        if let Some(p) = std::env::var_os("NFS5_GAME_PATH") {
            dirs.push(std::path::PathBuf::from(p));
        }
        if let Some(h) = std::env::var_os("HOME") {
            dirs.push(std::path::PathBuf::from(h).join("Games/NFS-Porsche-Unleashed/game_info/data"));
        }
        dirs.into_iter().find(|d| d.join("fe.txt").is_file())
    }

    /// `text` parsed with `files` as the only readable files, and the names asked for.
    fn parse(text: &str, args: &[&str], files: &[(&str, &str)]) -> (Registry, Vec<String>) {
        let asked = RefCell::new(Vec::new());
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let load = |n: &str| {
            asked.borrow_mut().push(n.to_string());
            files.iter().find(|(f, _)| *f == n).map(|(_, c)| c.as_bytes().to_vec())
        };
        let r = Registry::parse_with(text.as_bytes(), &args, &load);
        (r, asked.into_inner())
    }

    #[test]
    fn reads_the_shipped_shape() {
        let r = Registry::parse("fe=1\nSKILL = 2\nnum_laps=4\n traffic_density = 3 \ncops=1\nplaylist = 0,1, 2 ,3\nbogus=\n");
        assert_eq!(r.get("skill"), Some("2"));
        assert_eq!(r.get("SKILL"), Some("2"), "keys are case-insensitive");
        assert_eq!(r.list("playlist"), vec![0, 1, 2, 3]);
        assert_eq!(r.u32("missing", 7), 7);
        assert_eq!(r.get("bogus"), None, "a name the registry lacks is dropped (0x4B5537)");
        let s = RaceSettings::from_registry(&r);
        assert_eq!(s.skill, 2);
        assert_eq!(s.num_laps, 4);
        assert_eq!(s.traffic_density, 3);
        assert!(s.cops);
        // an empty registry gives the game's own defaults
        let d = RaceSettings::from_registry(&Registry::default());
        assert_eq!(d, RaceSettings::default());
    }

    #[test]
    fn the_last_assignment_wins_and_the_command_line_comes_after_the_file() {
        let (r, _) = parse("skill=1\nSKILL = 0\nnum_laps=3\n", &[], &[]);
        assert_eq!((r.get("skill"), r.get("num_laps")), (Some("0"), Some("3")));
        let (r, _) = parse("skill=1\nnum_laps=3\n", &["skill=2", "car[0to2].abs=1"], &[]);
        assert_eq!(r.get("skill"), Some("2"));
        assert_eq!(r.get("num_laps"), Some("3"));
        for n in 0..=2 {
            assert_eq!(r.get(&format!("car[{n}].abs")), Some("1"));
        }
        assert_eq!(r.get("car[3].abs"), None);
        // one argv string is one scan: blanks inside it are dropped like in the file
        let (r, _) = parse("", &["skill = 2"], &[]);
        assert_eq!(r.get("skill"), Some("2"));
    }

    #[test]
    fn there_are_no_semicolon_comments() {
        // the value is everything to the end of the line, blanks removed, then evaluated: ';' is ignored by 0x4B5250
        let (r, _) = parse("skill = 2 ; later\n", &[], &[]);
        assert_eq!(r.get("skill"), Some("2"));
        let (r, _) = parse("track_name = coastal ; x\n", &[], &[]);
        assert_eq!(r.get("track_name"), Some("coastal;x"));
    }

    #[test]
    fn a_hash_line_never_names_an_option_but_its_text_is_still_scanned() {
        let (r, _) = parse("# skill=1\n#skill=1\nnum_laps=2\n", &[], &[]);
        assert_eq!((r.get("skill"), r.get("num_laps")), (None, Some("2")));
        // 0x4B5F8E bumps only the byte after '#': an empty comment turns its '\n' into 0x0B and the next
        // line's name then reaches back over it to the '#'
        let (r, _) = parse("#\nskill=1\nnum_laps=2\n", &[], &[]);
        assert_eq!((r.get("skill"), r.get("num_laps")), (None, Some("2")));
        // '@' in a comment still includes
        let (_, asked) = parse("# see @other\n", &[], &[]);
        assert_eq!(asked, ["other.txt"]);
        // the byte after '#' is 0x01: a name that starts there is different, but a line break is never skipped
        let (r, _) = parse("#a\nskill=1\n", &[], &[]);
        assert_eq!(r.get("skill"), Some("1"));
    }

    #[test]
    fn at_includes_a_file_in_place_and_import_does_too() {
        let files = [("inc.txt", "num_laps = 5\nskill=1\n"), ("other.rpl", "cops=1\n")];
        let (r, asked) = parse("skill=2\n@ inc \nskill=0\nimport = other.rpl\n", &[], &files);
        assert_eq!(asked, ["inc.txt", "other.rpl"], "@ appends .txt, IMPORT does not");
        assert_eq!(r.get("num_laps"), Some("5"));
        assert_eq!(r.get("skill"), Some("0"), "the include sits where the @ is");
        assert_eq!(r.get("cops"), Some("1"));
        // a command-line @file
        let (r, _) = parse("skill=2\n", &["@inc"], &files);
        assert_eq!((r.get("skill"), r.get("num_laps")), (Some("1"), Some("5")));
        // an include that includes itself stops (the original would run out of stack)
        let (_, asked) = parse("@loop\n", &[], &[("loop.txt", "@loop\n")]);
        assert_eq!(asked.len(), 16);
    }

    #[test]
    fn an_unknown_name_takes_its_whole_line_with_it() {
        let (r, asked) = parse("bogus=1 @inc\nskill=2\n", &[], &[]);
        assert!(asked.is_empty());
        assert_eq!(r.get("skill"), Some("2"));
        // without a '=' nothing is an assignment
        let (r, _) = parse("skill 2\n", &[], &[]);
        assert_eq!(r.get("skill"), None);
    }

    #[test]
    fn options_with_no_global_are_dropped_like_the_original_discards_them() {
        // RAMP, DEBUGUSER, IMPORT's siblings have a NULL pointer (0x4B4CC7)
        let (r, _) = parse("ramp=1\ndebuguser=1\nskill=1\n", &[], &[]);
        assert_eq!((r.get("ramp"), r.get("debuguser"), r.get("skill")), (None, None, Some("1")));
    }

    #[test]
    fn values_are_evaluated_like_0x4b5250() {
        let (r, _) = parse("num_laps=0x1F\nskill=-1\ncomm_mode = Network\nrace_type=KnockOut\ntraffic_density=bogus\ncops=1x\n", &[], &[]);
        assert_eq!(r.get("num_laps"), Some("31"));
        assert_eq!(r.get("skill"), Some("-1"));
        assert_eq!(r.get("comm_mode"), Some("4"));
        assert_eq!(r.get("race_type"), Some("3"));
        assert_eq!(r.get("traffic_density"), Some("0"), "an unknown symbol is 0");
        assert_eq!(r.get("cops"), Some("1"), "'x' is hex only while the accumulator is 0");
        assert_eq!(eval(b"1,2"), (1 << 16) | 2, "a pair packs as (a<<16)|b");
        assert_eq!(eval(b"1,2,3,4"), (1 << 20) | (2 << 16) | (3 << 8) | 4);
        assert_eq!(eval(b"0xff"), 255);
        assert_eq!(eval(b"-15"), -15);
        assert_eq!(eval(b"SC_LEFT"), 75);
        assert_eq!(eval(b""), 0);
    }

    #[test]
    fn strings_lists_flags_and_bindings() {
        let (r, _) = parse(
            "track_name = abcdefghijklmnopqrstuvwxyz\nplaylist = 0, 1,,2\nfe = 0\nc1_left = key, SC_LEFT\nreplay_filename=replay.rpl\n",
            &[],
            &[],
        );
        assert_eq!(r.get("track_name"), Some("abcdefghijklmno"), "15 characters reach the message (0x4B5A4C)");
        assert_eq!(r.get("playlist"), Some("0,1,2"));
        assert_eq!(r.get("fe"), Some("0"), "FE is evaluated on the spot (0x4B55BB)");
        assert_eq!(r.get("c1_left"), Some("key,SC_LEFT"));
        assert_eq!(r.get("replay_filename"), Some("replay.rpl"));
    }

    #[test]
    fn the_car_namespace() {
        let (r, _) = parse(
            "car[0].abs = 1\ncar [ 1 to 3 ] . class = ai\nCAR[7].transmission=auto\ncar[9].abs=1\ncar[2].aispecname = default\ncar[0].type = 11\ncar[1].type = 993C2\ncar[4 to 2].abs=1\ncar[0].bogus=1\ncar[5].skill=2\n",
            &[],
            &[],
        );
        assert_eq!(r.get("car[0].abs"), Some("1"));
        for n in 1..=3 {
            assert_eq!(r.get(&format!("car[{n}].class")), Some("2"), "{n}");
        }
        assert_eq!(r.get("car[7].transmission"), Some("1"));
        assert_eq!(r.get("car[9].abs"), None, "numCars = 9: slots 0..8");
        assert_eq!(r.get("car[2].aispecname"), Some("default"));
        assert_eq!(r.get("car[0].type"), Some("11"));
        assert_eq!(r.get("car[1].type"), Some("31"), "a car name becomes its index (0x4B57EC)");
        assert_eq!(r.get("car[4].abs"), None, "an empty range");
        assert_eq!(r.get("car[0].bogus"), None);
        assert_eq!(r.get("car[5].skill"), None, "not a per-car option");
    }

    #[test]
    fn the_shipped_fe_txt() {
        let Some(dir) = game_dir() else { return };
        let text = std::fs::read(dir.join("fe.txt")).unwrap();
        let r = Registry::parse_with(&text, &[], &|_| None);
        // the thirty-six keys the original drops (options.md §9.8) and the two NULL-pointer ones
        for k in [
            "shockmode0", "shockimpact0", "shockmode1", "shockimpact1", "pinkslipsforfeit", "display", "easter", "track",
            "tournament_mult", "song", "audiomode", "bestlap", "checkpointhud0", "checkpointhud1", "numperps", "stageoffset",
            "checkpointtype", "tachometer", "ip", "port", "car[0].colour_sv", "car[0].enginemod", "car[0].suspensionmod",
            "car[0].downforce", "car[0].colourchange", "car[0].hudcolour", "car[0].tachcolour", "car[0].tiretype", "car[0].speed",
            "car[0].tach", "car[0].map", "car[0].opponentid", "car[0].time", "car[0].lapnum", "car[0].position", "car[0].mirror",
            "ramp", "debuguser",
        ] {
            assert_eq!(r.get(k), None, "{k}");
        }
        assert_eq!(r.get("rand_seed"), Some("8165"));
        assert_eq!(r.get("replay_filename"), Some("replay.rpl"));
        assert_eq!(r.get("hudtachometer"), None, "the file has `tachometer`, which is no option");
        assert_eq!(r.get("car[0].camera"), Some("1,0,2,7"), "BumperCam2, BumperCam, TailCam0, HeliCam2");
        assert_eq!(r.get("car[0].class"), Some("1"));
        assert_eq!(r.get("car[1].class"), Some("2"));
        assert_eq!(r.get("c1_left"), Some("key,SC_LEFT"));
        assert_eq!(r.get("car[0].skillslow"), Some("1065353216"));
        assert_eq!(r.get("track_name"), Some("coastal"));
        // the commented-out bindings are no keys
        assert!(r.0.keys().all(|k| !k.starts_with('#')));
    }

    /// The tables are replayed from the image by `RE/tools/option_table.py`; here every name is looked up in it.
    #[test]
    fn the_tables_match_the_image() {
        let Some(dir) = game_dir() else { return };
        let Ok(exe) = std::fs::read(dir.join("nfs5.exe")) else { return };
        let has = |n: &str| {
            let mut pat = vec![0u8];
            pat.extend_from_slice(n.as_bytes());
            pat.push(0);
            exe.windows(pat.len()).any(|w| w == &pat[..])
        };
        assert_eq!(OPTIONS.len(), 590);
        let mut ids: Vec<u16> = OPTIONS.iter().map(|o| o.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 590, "ids are unique");
        assert_eq!((ids[0], ids[589]), (1, 0x256));
        for (_, n, _) in OPTIONS {
            assert!(has(n), "{n}");
        }
        assert_eq!(SYMBOLS.len(), 143);
        for (n, _) in SYMBOLS {
            assert!(has(n), "{n}");
        }
        // the `.TYPE` names sit in 7-byte cells from 0x5D61E8 (the first has no NUL before it)
        for n in CAR_TYPES.iter() {
            let mut cell = n.as_bytes().to_vec();
            cell.resize(7, 0);
            assert!(exe.windows(7).any(|w| w == &cell[..]), "{n}");
        }
        // spot checks of the records the parser branches on
        assert_eq!(option_id("FE"), Some(7));
        assert_eq!(option_id("car"), Some(0x21E));
        assert_eq!(option_id(".TYPE"), Some(0x21F));
        assert_eq!(option_id("IMPORT"), Some(1));
        assert_eq!(option_id("tachometer"), None);
    }
}
