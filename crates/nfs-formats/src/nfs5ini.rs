//! `nfs5.ini` — the settings file of the Verok 1.0.6 patch (`nfs5.dll`), which the shipped game reads
//! in place of the Windows registry. The exe's `main` is hooked at 0x5A2DF4 -> dll 0x10002880, which
//! calls the reader dll **0x100022B0**: the module's own path with its extension replaced by `.ini`
//! (`GetModuleFileNameA`, `strrchr '.'`, 0x100022CF…0x10002331), then one `GetPrivateProfileStringA` /
//! `GetPrivateProfileIntA` per key in the section `[NFS5]` (all dll addresses below are in that
//! function). A missing file, section or key gives the call's default; the Win32 profile calls
//! compare section and key names without regard to case. `RE/notes/nfs5-dll.md` §3.3.
//!
//! The effects live elsewhere: `Language` -> the four registry-language sites (`GamePaths::language`),
//! `NoMovies` -> 0x4DD611 (the start-up movies), `Fog` / `Filtering` -> the Thrash state setter
//! wrapper dll 0x100035F0, `CarControl` -> the `jge` at 0x49DD4C. The other keys are read for
//! completeness and are host-side in the original (window, driver, CPU affinity, error mode, clock).

use crate::ini::Ini;

/// The values of `nfs5.ini`'s `[NFS5]` section, with the dll's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nfs5Ini {
    /// `Language` (default `"English"`, 0x10002343): `_stricmp` against `ENGLISH` 0, `FRENCH` 1, `GERMAN` 2,
    /// `ITALIAN` 3, `SPANISH` 4, `SWEDISH` 5, `PORTBRZL` 6 (0x10002362…0x10002445) into dll `[0x100060A0]`
    /// (initially 0): anything else, or an empty value, leaves 0.
    pub language: u16,
    /// `NoMovies` (0x1000244F, default 0), non-zero: 0x4DD611 jumps over the start-up movies.
    pub no_movies: bool,
    /// `Fog` (0x10002561, default 1) -> dll `[0x100060C8]`: 0 drops / zeroes the fog states.
    pub fog: bool,
    /// `Filtering` (0x1000257C, default 1) -> dll `[0x100060CC]`: 0 sends `MAG`/`MINFILTER` `POINT`.
    pub filtering: bool,
    /// `CarControl` (0x100025E8, default 0), non-zero: the `jge` at 0x49DD4C is NOPped.
    pub car_control: bool,
    /// `ThrashDriver` (0x100024BE, default `dx7z`, at most 15 characters): the driver folder. Not used by the port.
    pub thrash_driver: String,
    /// `WindowedMode` (0x100024DA, default 0). Not used by the port (its window is its own).
    pub windowed_mode: bool,
    /// `SingleProcAffinity` (0x10002597, default 0): `SetProcessAffinityMask(.., 1)`. Not used.
    pub single_proc_affinity: bool,
    /// `CpuLimit` (0x100025B2, default 0): MHz override of the start-up clock estimate. Not used.
    pub cpu_limit: u32,
    /// `NoErrorReporting` (0x100025CD, default 0): `SetErrorMode(old | 0x8003)`. Not used.
    pub no_error_reporting: bool,
}

impl Default for Nfs5Ini {
    /// The dll's defaults — what a missing `nfs5.ini` gives.
    fn default() -> Self {
        Self {
            language: 0,
            no_movies: false,
            fog: true,
            filtering: true,
            car_control: false,
            thrash_driver: "dx7z".into(),
            windowed_mode: false,
            single_proc_affinity: false,
            cpu_limit: 0,
            no_error_reporting: false,
        }
    }
}

/// `GetPrivateProfileIntA`: a missing key gives `default`; a present one the leading decimal number
/// (optional sign), 0 when there is none; a negative value wraps to `UINT`.
fn profile_int(v: Option<&str>, default: u32) -> u32 {
    let Some(v) = v else { return default };
    let v = v.trim();
    let (neg, digits) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v),
    };
    let n = digits.bytes().take_while(u8::is_ascii_digit).fold(0u32, |a, d| a.wrapping_mul(10).wrapping_add((d - b'0') as u32));
    if neg { n.wrapping_neg() } else { n }
}

/// The language index of dll 0x10002362…0x10002445 (`_stricmp` against the seven names).
pub fn language_index(name: &str) -> u16 {
    const NAMES: [&str; 7] = ["ENGLISH", "FRENCH", "GERMAN", "ITALIAN", "SPANISH", "SWEDISH", "PORTBRZL"];
    NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).map_or(0, |i| i as u16)
}

impl Nfs5Ini {
    /// Parse the text of an `nfs5.ini`.
    pub fn parse(text: &str) -> Self {
        let ini = Ini::parse(text);
        let sec = ini.section("NFS5");
        let get = |k: &str| sec.and_then(|s| s.get(k));
        let int = |k: &str, d: u32| profile_int(get(k), d);
        let unquote = |s: &str| {
            let b = s.as_bytes();
            if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] { s[1..s.len() - 1].to_string() } else { s.to_string() }
        };
        let mut driver = get("ThrashDriver").map_or_else(|| "dx7z".to_string(), unquote);
        driver.truncate(15);
        Self {
            language: language_index(&get("Language").map_or_else(|| "English".to_string(), unquote)),
            no_movies: int("NoMovies", 0) != 0,
            fog: int("Fog", 1) != 0,
            filtering: int("Filtering", 1) != 0,
            car_control: int("CarControl", 0) != 0,
            thrash_driver: driver,
            windowed_mode: int("WindowedMode", 0) != 0,
            single_proc_affinity: int("SingleProcAffinity", 0) != 0,
            cpu_limit: int("CpuLimit", 0),
            no_error_reporting: int("NoErrorReporting", 0) != 0,
        }
    }

    /// Read `<dir>/nfs5.ini` (the exe's directory); a missing or unreadable file gives the defaults.
    pub fn load(dir: &std::path::Path) -> Self {
        let Some(p) = crate::ci_file::find_file(dir, "nfs5.ini") else { return Self::default() };
        std::fs::read(p).map(|b| Self::parse(&String::from_utf8_lossy(&b))).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_file() {
        let t = "[NFS5]\r\nLanguage=english\r\nThrashDriver=opengl3\r\nWindowedMode=0\r\nSingleProcAffinity=1\r\nCpuLimit=0\r\nNoErrorReporting=0\r\nNoMovies=0\r\nFog=1\r\nFiltering=1\r\nCarControl=0\r\n";
        let i = Nfs5Ini::parse(t);
        assert_eq!(i.language, 0);
        assert_eq!(i.thrash_driver, "opengl3");
        assert!(i.single_proc_affinity && i.fog && i.filtering && !i.no_movies && !i.car_control);
    }

    #[test]
    fn missing_keys_and_files_give_the_dlls_defaults() {
        assert_eq!(Nfs5Ini::parse(""), Nfs5Ini::default());
        assert_eq!(Nfs5Ini::parse("[Other]\nFog=0\n"), Nfs5Ini::default());
        assert_eq!(Nfs5Ini::load(std::path::Path::new("/nonexistent-nfs5-dir")), Nfs5Ini::default());
    }

    #[test]
    fn keys_effects_and_odd_values() {
        let i = Nfs5Ini::parse("[nfs5]\nlanguage = PortBrzl\nnomovies=1\nFOG=0\nfiltering=0\ncarcontrol=7\ncpulimit=abc\n");
        assert_eq!((i.language, i.no_movies, i.fog, i.filtering, i.car_control, i.cpu_limit), (6, true, false, false, true, 0));
        // an unknown language stays at the dll cell's initial 0; the names compare without case
        assert_eq!(Nfs5Ini::parse("[NFS5]\nLanguage=Russian\n").language, 0);
        assert_eq!(Nfs5Ini::parse("[NFS5]\nLanguage=german\n").language, 2);
        assert_eq!(Nfs5Ini::parse("[NFS5]\nLanguage=\n").language, 0);
    }
}
