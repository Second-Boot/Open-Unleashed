//! `fe.txt` — the front-end registry the game persists its settings in
//! (`RE/notes/engine-audio.md` §8.4: registry head `0x5D1E40`, records `{u32 id, void* ptr, char* name}`,
//! written back as `fe_out.txt`).
//!
//! The file is plain `key = value` lines, case-insensitively named, with a few list values
//! (`playlist = 0,1,2,…`) and a few indexed keys (`car[0].traction`). Nothing in it is required: a
//! missing key means the game's own default.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Registry(pub HashMap<String, String>);

impl Registry {
    pub fn parse(text: &str) -> Self {
        let mut m = HashMap::new();
        for line in text.lines() {
            let line = line.split(';').next().unwrap_or(line);
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim().to_lowercase();
                if !k.is_empty() {
                    m.insert(k, v.trim().to_string());
                }
            }
        }
        Self(m)
    }

    /// `fe.txt` in the game root (`fe_out.txt` is the written copy and is ignored).
    pub fn load(game_root: &Path) -> Self {
        std::fs::read_to_string(game_root.join("fe.txt")).map(|t| Self::parse(&t)).unwrap_or_default()
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

    #[test]
    fn reads_the_shipped_shape() {
        let r = Registry::parse("fe=1\nSKILL = 2\nnum_laps=4\n traffic_density = 3 \ncops=1\nplaylist = 0,1, 2 ,3\nempty=\n");
        assert_eq!(r.get("skill"), Some("2"));
        assert_eq!(r.get("SKILL"), Some("2"), "keys are case-insensitive");
        assert_eq!(r.list("playlist"), vec![0, 1, 2, 3]);
        assert_eq!(r.u32("missing", 7), 7);
        assert_eq!(r.get("empty"), Some(""));
        let s = RaceSettings::from_registry(&r);
        assert_eq!(s.skill, 2);
        assert_eq!(s.num_laps, 4);
        assert_eq!(s.traffic_density, 3);
        assert!(s.cops);
        // an empty registry gives the game's own defaults
        let d = RaceSettings::from_registry(&Registry::default());
        assert_eq!(d, RaceSettings::default());
    }
}
