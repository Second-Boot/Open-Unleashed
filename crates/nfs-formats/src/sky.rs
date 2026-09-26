//! `GameData/Track/Sky/<Track>.txt` — the layered sky definition (`RE/RENDERING.md`).
//!
//! The file is a list of `BEGIN … END` blocks, one per computer capability, and the game keeps
//! **every** block whose capability line holds the `SKYRENDERING` level's word or `all`
//! ([`SkyFile::for_level`]). Everything after a `#`
//! is a comment — and the shipped files document their own fields in those comments. The loader
//! 0x45FB80 reads the whole file, folds it to lower case (`toupper`'s counterpart at 0x5A2248) and
//! walks the non-comment lines in order, so the parse below is positional, exactly as the game's is.
//!
//! The **type** keyword selects the level class and, for the `multi*` family, the blend the layers
//! are drawn with. The executable compares against, in order: `basic`, `basicwater`, `freecloud`,
//! `additive`, `multiadditive`, `multiadditivewater`, `multiattenuate`, `multiattenuatewater`,
//! `multimodulate`, `multimodulatewater`, `multinormal`, `multinormalwater` (strings at 0x45Fxxx).
//! A `basic` block carries **one** layer per weather region, a `multi*` block **three**.

use crate::FormatError;

/// The `type` keyword. 0x45FD57 onwards compares it against thirteen strings and turns it into a
/// **bitmask** — `config` 0x01, `basic` 0x02, `multinormal` 0x04, `multiadditive` 0x08,
/// `multiattenuate` 0x10, `multimodulate` 0x20, `freecloud` 0x40, `additive` 0x100, the `…water`
/// spellings the same with **0x200** added, and anything unrecognised **0x800**.
///
/// The factory at 0x45D920 then tests the mask in order — `& 2` → `SkyLevel_BasicSky` (0x45ADF0,
/// 0x5C bytes), `& 4`/`& 8`/`& 0x10`/`& 0x20` → `SkyLevel_MultiSky` (0x45B5A0, 0x88 bytes) with
/// blend 0, 1, 2 and 3, `& 0x40` → `SkyLevel_FreeClouds` (0x45BF80, 0xFC bytes) — and **stops**.
/// Nothing tests 0x100, 0x01 or 0x800, so an `additive` block, a `config` block and a misspelt
/// one all build **no level at all**: `[`SkyKind::builds_a_level`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkyKind {
    Config,
    Basic { water: bool },
    FreeCloud,
    Additive,
    Multi { blend: SkyBlend, water: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkyBlend {
    Normal,
    Additive,
    Modulate,
    Attenuate,
}

impl SkyKind {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "config" => Self::Config,
            "basic" => Self::Basic { water: false },
            "basicwater" => Self::Basic { water: true },
            "freecloud" => Self::FreeCloud,
            "additive" => Self::Additive,
            "multinormal" => Self::Multi { blend: SkyBlend::Normal, water: false },
            "multinormalwater" => Self::Multi { blend: SkyBlend::Normal, water: true },
            "multiadditive" => Self::Multi { blend: SkyBlend::Additive, water: false },
            "multiadditivewater" => Self::Multi { blend: SkyBlend::Additive, water: true },
            "multimodulate" => Self::Multi { blend: SkyBlend::Modulate, water: false },
            "multimodulatewater" => Self::Multi { blend: SkyBlend::Modulate, water: true },
            "multiattenuate" => Self::Multi { blend: SkyBlend::Attenuate, water: false },
            "multiattenuatewater" => Self::Multi { blend: SkyBlend::Attenuate, water: true },
            _ => return None,
        })
    }

    /// How many sky-map layers a region of this kind lists.
    pub fn layers(&self) -> usize {
        match self {
            Self::Multi { .. } => 3,
            _ => 1,
        }
    }

    /// Whether 0x45D920's chain builds a level for this keyword at all. `additive` and `config`
    /// are parsed and then dropped on the floor — `default.txt`'s `HIGH` block is `ADDITIVE`, so
    /// at high sky detail that file contributes nothing.
    pub fn builds_a_level(&self) -> bool {
        !matches!(self, Self::Config | Self::Additive)
    }

    /// The mask 0x45FD57's chain assigns, for comparing against a disassembly.
    pub fn mask(&self) -> u32 {
        match self {
            Self::Config => 0x001,
            Self::Basic { water } => 0x002 | if *water { 0x200 } else { 0 },
            Self::Additive => 0x100,
            Self::FreeCloud => 0x040,
            Self::Multi { blend, water } => {
                (match blend {
                    SkyBlend::Normal => 0x004,
                    SkyBlend::Additive => 0x008,
                    SkyBlend::Attenuate => 0x010,
                    SkyBlend::Modulate => 0x020,
                }) | if *water { 0x200 } else { 0 }
            }
        }
    }
}

/// One layer of one weather region.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkyLayer {
    /// SHPI entry in `Sky/<Track>.fsh`; `none` becomes `None`.
    pub map: Option<String>,
    /// `basic` only: the separate alpha map (`none` or `noise`).
    pub alpha_map: Option<String>,
    /// UV repeats across the layer.
    pub scale: [f32; 2],
    /// 0 = still, "10 for lots".
    pub wind_speed: f32,
    /// Degrees, north 0, east 90.
    pub wind_direction: f32,
}

/// One of the two weather regions of a block.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SkyRegion {
    pub layers: Vec<SkyLayer>,
    /// "0 for no movement at all, otherwise 50 is fast and 400 is slow".
    pub move_ratio: f32,
    /// "0 doesn't move, 9 is super super fast, about 0.3 for mild warpage".
    pub warp_speed: f32,
    /// "maximum proportion of UV to warp (0 to 1.0)".
    pub warp_amount: f32,
    /// `basic` only: the cloud texture of this region.
    pub cloud: Option<String>,
    /// `freecloud` only: the three cloud amounts, relative weights rather than counts —
    /// 0x45C4BA divides each by their sum and multiplies by `[0x5B243C]` = 15.0, so `{1,1,1}`
    /// and `{10,10,10}` both give five instances each. **Region B's three are read and then
    /// never used**: 0x45C376 copies only region A's into `this+0xBC…+0xC4`.
    pub cloud_weights: [f32; 3],
}

/// One `BEGIN … END` block.
#[derive(Debug, Clone, PartialEq)]
pub struct SkyBlock {
    /// `HIGH`, `MEDIUM` or `LOW`, as written.
    pub capability: String,
    pub kind: SkyKind,
    /// "Does this layer block the sun".
    pub blocks_sun: bool,
    pub regions: [SkyRegion; 2],
    /// `multi*` and `freecloud`: the cloud texture of each layer, after both regions.
    pub clouds: Vec<Option<String>>,
    /// `freecloud` only: the two integers that follow each cloud name (`this+0xC8+4k` and
    /// `this+0xD4+4k`, atoi'd at 0x45C384 / 0x45C3CD, and passed on to 0x45F470 as its fifth
    /// and sixth arguments). What they mean is not read.
    pub cloud_params: Vec<[i32; 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyFile {
    pub blocks: Vec<SkyBlock>,
}

/// A map name, or `None` for `none`. The original's test is `strncmp(tok, "none", 4)` (0x45B746),
/// a **prefix** compare, so anything beginning `none` counts.
fn name(s: &str) -> Option<String> {
    (s.len() < 4 || !s[..4].eq_ignore_ascii_case("none")).then(|| s.to_string())
}

impl SkyFile {
    pub fn parse(text: &str) -> Result<Self, FormatError> {
        let toks: Vec<&str> = text
            .lines()
            .map(|l| l.split('#').next().unwrap_or("").trim())
            .filter(|l| !l.is_empty())
            .collect();
        let bad = |what: &'static str| FormatError::Truncated { what, offset: 0, needed: 1 };
        let mut blocks = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            if !toks[i].eq_ignore_ascii_case("begin") {
                i += 1;
                continue;
            }
            i += 1;
            macro_rules! next {
                () => {{
                    let t = toks.get(i).copied().ok_or(bad("sky field"))?;
                    i += 1;
                    t
                }};
            }
            let capability = next!().to_string();
            let type_word = next!();
            let kind = SkyKind::parse(type_word).ok_or(FormatError::UnexpectedValue {
                what: "sky level type",
                offset: 0,
                expected: "basic | freecloud | additive | multi*".into(),
                found: type_word.to_string(),
            })?;
            // 0x45B6FE is `strncmp(tok, "no", 2)`: the flag goes **on** unless the word starts
            // with `no`, so it is "not no" rather than "yes"
            // **`freecloud` has no sun token.** 0x45AEBC (`basic`) and 0x45B6FE (`multi*`) do
            // the `strncmp(tok, "no", 2)` test; the freecloud constructor 0x45BF80 contains no
            // such test at all and consumes its opening token as the first sky-map name
            // (0x45BFC2). Reading a sun token here puts the whole block one out.
            let blocks_sun = if kind == SkyKind::FreeCloud {
                true
            } else {
                let sun_word = next!();
                sun_word.len() < 2 || !sun_word[..2].eq_ignore_ascii_case("no")
            };
            if kind == SkyKind::FreeCloud {
                // 0x45BFCC…0x45C02A: eight tokens per weather region, twice, then
                // 0x45C02E…0x45C05A: three per cloud, three times. Twenty-five in all.
                let mut regions: [SkyRegion; 2] = Default::default();
                for region in regions.iter_mut() {
                    let mut l = SkyLayer::default();
                    l.map = name(next!());
                    l.alpha_map = name(next!());
                    let c: f32 = next!().parse().unwrap_or(1.0);
                    l.scale = [c, c]; // one token, kept as 1/c for both axes at 0x45F8C9
                    l.wind_speed = next!().parse().unwrap_or(0.0);
                    l.wind_direction = next!().parse().unwrap_or(0.0);
                    region.layers = vec![l];
                    for w in region.cloud_weights.iter_mut() {
                        *w = next!().parse().unwrap_or(0.0);
                    }
                }
                let mut clouds = Vec::new();
                let mut cloud_params = Vec::new();
                for _ in 0..3 {
                    clouds.push(name(next!()));
                    cloud_params.push([next!().parse().unwrap_or(0), next!().parse().unwrap_or(0)]);
                }
                while i < toks.len() && !toks[i].eq_ignore_ascii_case("end") {
                    i += 1;
                }
                i += 1;
                blocks.push(SkyBlock { capability, kind, blocks_sun, regions, clouds, cloud_params });
                continue;
            }
            let n = kind.layers();
            let mut regions: [SkyRegion; 2] = Default::default();
            for region in regions.iter_mut() {
                let mut layers = vec![SkyLayer::default(); n];
                if n == 1 {
                    layers[0].map = name(next!());
                    layers[0].alpha_map = name(next!());
                    region.cloud = name(next!());
                    layers[0].scale = [next!().parse().unwrap_or(1.0), next!().parse().unwrap_or(1.0)];
                    region.move_ratio = next!().parse().unwrap_or(0.0);
                    layers[0].wind_speed = next!().parse().unwrap_or(0.0);
                    layers[0].wind_direction = next!().parse().unwrap_or(0.0);
                } else {
                    for l in layers.iter_mut() {
                        l.map = name(next!());
                    }
                    for l in layers.iter_mut() {
                        l.scale[0] = next!().parse().unwrap_or(1.0);
                    }
                    for l in layers.iter_mut() {
                        l.scale[1] = next!().parse().unwrap_or(1.0);
                    }
                    region.move_ratio = next!().parse().unwrap_or(0.0);
                    for l in layers.iter_mut() {
                        l.wind_speed = next!().parse().unwrap_or(0.0);
                    }
                    for l in layers.iter_mut() {
                        l.wind_direction = next!().parse().unwrap_or(0.0);
                    }
                }
                region.warp_speed = next!().parse().unwrap_or(0.0);
                region.warp_amount = next!().parse().unwrap_or(0.0);
                region.layers = layers;
            }
            let mut clouds = Vec::new();
            if n > 1 {
                for _ in 0..n {
                    clouds.push(name(next!()));
                }
            }
            // the block ends at END; skip anything left over so a longer file still parses
            while i < toks.len() && !toks[i].eq_ignore_ascii_case("end") {
                i += 1;
            }
            i += 1;
            blocks.push(SkyBlock { capability, kind, blocks_sun, regions, clouds, cloud_params: Vec::new() });
        }
        Ok(Self { blocks })
    }

    /// The first usable block whose capability is exactly `capability` (a diagnostic's view; the
    /// game picks by [`SkyFile::for_level`]).
    ///
    /// "Usable" is [`SkyKind::builds_a_level`]: 0x45D920's chain has no branch for `additive`,
    /// `config` or an unrecognised keyword, so such a block builds nothing and the game draws no
    /// layer for it. `Sky/default.txt`'s only `HIGH` block is `ADDITIVE`, which is exactly that
    /// case. *(Until 2026-09-25 this fell back to the first usable block of any capability — the
    /// port's own; the original has no level then.)*
    pub fn for_capability(&self, capability: &str) -> Option<&SkyBlock> {
        self.blocks.iter().filter(|b| b.kind.builds_a_level()).find(|b| b.capability.eq_ignore_ascii_case(capability))
    }

    /// The usable blocks the game builds at `SKYRENDERING` level `level` (`[0x657D9C]`): the loader
    /// 0x45FB80 lower-cases the file and keeps every block whose capability line — the first
    /// non-comment line after `begin` — contains, anywhere, the level's word from the table at
    /// 0x5CF3A0 (20 bytes apart: `low`, `medium`, `high`, `amazing`) or `all` (0x45FCA5…0x45FD18),
    /// walking on to the next `begin` to the end of the file (0x4600DB…0x460147); 0x45D8F0 then
    /// builds a level for each. The shipped tracks carry `HIGH` and `MEDIUM` blocks only, so at
    /// level 0 (the option's "off", the shipped value) they build none; `default.txt` has two `LOW`.
    pub fn for_level(&self, level: usize) -> Vec<&SkyBlock> {
        const WORDS: [&str; 4] = ["low", "medium", "high", "amazing"];
        let Some(word) = WORDS.get(level) else { return Vec::new() };
        self.blocks
            .iter()
            .filter(|b| b.kind.builds_a_level())
            .filter(|b| {
                let c = b.capability.to_ascii_lowercase();
                c.contains(word) || c.contains("all")
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freecloud_has_no_sun_token_and_twenty_five_fields() {
        let f = SkyFile::parse(FREE).unwrap();
        let b = &f.blocks[0];
        assert_eq!(b.kind, SkyKind::FreeCloud);
        // no `NO` line to read, so the flag is left on
        assert!(b.blocks_sun);
        assert_eq!(b.regions[0].layers[0].map.as_deref(), Some("st1a"));
        assert_eq!(b.regions[0].layers[0].alpha_map.as_deref(), Some("noise"));
        assert_eq!(b.regions[0].layers[0].scale, [2.0, 2.0]);
        assert_eq!(b.regions[0].layers[0].wind_speed, 3.0);
        assert_eq!(b.regions[0].layers[0].wind_direction, 90.0);
        assert_eq!(b.regions[0].cloud_weights, [1.0, 2.0, 3.0]);
        assert_eq!(b.regions[1].layers[0].map.as_deref(), Some("st1b"));
        assert_eq!(b.regions[1].layers[0].alpha_map, None);
        assert_eq!(b.regions[1].cloud_weights, [4.0, 5.0, 6.0]);
        assert_eq!(b.clouds, vec![Some("cc1a".into()), Some("cc1b".into()), None]);
        assert_eq!(b.cloud_params, vec![[7, 8], [9, 10], [11, 12]]);
    }

    const BASIC: &str = "BEGIN\n#cap\n\tHIGH\n#type\n\tbasic\n#sun\nNO\n\tst1a\n\tnone\n\tflat\n\t.5\n\t.5\n\t0\n\t0\n\t90\n\t0\n\t0\n\tst1a\n\tnone\n\tflat\n\t.5\n\t.5\n\t0\n\t0\n\t90\n\t0\n\t0\nEND\n";
    // 25 tokens and no sun line: 2 x (map, alphamap, scale, windspeed, winddir, w0, w1, w2)
    // then 3 x (cloudmap, int, int). Synthetic — no shipped .txt declares `freecloud`.
    const FREE: &str = "BEGIN\n\tHIGH\n\tfreecloud\n\tst1a\n\tnoise\n\t2\n\t3\n\t90\n\t1\n\t2\n\t3\n\tst1b\n\tnone\n\t4\n\t5\n\t270\n\t4\n\t5\n\t6\n\tcc1a\n\t7\n\t8\n\tcc1b\n\t9\n\t10\n\tnone\n\t11\n\t12\nEND\n";

    const MULTI: &str = "BEGIN\n\tHIGH\n\tMULTINORMAL\nNO\n\tst1a\n\tsc1a\n\tnone\n\t1\n\t2\n\t0\n\t1\n\t2\n\t0\n\t400\n\t0\n\t2\n\t0\n\t90\n\t40\n\t0\n\t0\n\t0\n\tst1a\n\tnone\n\tnone\n\t4\n\t0\n\t0\n\t4\n\t0\n\t0\n\t400\n\t0\n\t0\n\t0\n\t90\n\t0\n\t0\n\t0\n\t0\n\tflat\n\tcc1a\n\tnone\nEND\n";

    #[test]
    fn basic_block() {
        let f = SkyFile::parse(BASIC).unwrap();
        assert_eq!(f.blocks.len(), 1);
        let b = &f.blocks[0];
        assert_eq!(b.kind, SkyKind::Basic { water: false });
        assert!(!b.blocks_sun);
        let a = &b.regions[0];
        assert_eq!(a.layers.len(), 1);
        assert_eq!(a.layers[0].map.as_deref(), Some("st1a"));
        assert_eq!(a.layers[0].alpha_map, None);
        assert_eq!(a.cloud.as_deref(), Some("flat"));
        assert_eq!(a.layers[0].scale, [0.5, 0.5]);
        assert_eq!(a.layers[0].wind_direction, 90.0);
        // both regions are read, not just the first
        assert_eq!(b.regions[1].layers[0].map.as_deref(), Some("st1a"));
    }

    #[test]
    fn multi_block() {
        let f = SkyFile::parse(MULTI).unwrap();
        let b = &f.blocks[0];
        assert_eq!(b.kind, SkyKind::Multi { blend: SkyBlend::Normal, water: false });
        let a = &b.regions[0];
        assert_eq!(a.layers.len(), 3);
        assert_eq!(a.layers[1].map.as_deref(), Some("sc1a"));
        assert_eq!(a.layers[1].scale, [2.0, 2.0]);
        assert_eq!(a.layers[1].wind_speed, 2.0);
        assert_eq!(a.layers[1].wind_direction, 40.0);
        assert_eq!(a.move_ratio, 400.0);
        assert_eq!(b.clouds, vec![Some("flat".into()), Some("cc1a".into()), None]);
        // the MEDIUM-style second region keeps only its first layer
        assert_eq!(b.regions[1].layers[1].map, None);
    }

    #[test]
    fn every_level_type_name() {
        for (s, k) in [
            ("basic", SkyKind::Basic { water: false }),
            ("basicwater", SkyKind::Basic { water: true }),
            ("freecloud", SkyKind::FreeCloud),
            ("additive", SkyKind::Additive),
            ("multimodulatewater", SkyKind::Multi { blend: SkyBlend::Modulate, water: true }),
            ("MULTINORMAL", SkyKind::Multi { blend: SkyBlend::Normal, water: false }),
        ] {
            assert_eq!(SkyKind::parse(s), Some(k), "{s}");
        }
        assert_eq!(SkyKind::parse("nonsense"), None);
    }

    /// The masks 0x45FD57's chain assigns, and the three keywords 0x45D920 then has no branch for.
    #[test]
    fn the_type_masks_and_what_builds_a_level() {
        for (s, m) in [
            ("config", 0x001),
            ("basic", 0x002),
            ("multinormal", 0x004),
            ("multiadditive", 0x008),
            ("multiattenuate", 0x010),
            ("multimodulate", 0x020),
            ("freecloud", 0x040),
            ("additive", 0x100),
            ("basicwater", 0x202),
            ("multinormalwater", 0x204),
            ("multiadditivewater", 0x208),
            ("multiattenuatewater", 0x210),
            ("multimodulatewater", 0x220),
        ] {
            assert_eq!(SkyKind::parse(s).unwrap().mask(), m, "{s}");
        }
        // 0x45D920 tests 2, 4, 8, 0x10, 0x20 and 0x40 and nothing else
        assert!(SkyKind::parse("basic").unwrap().builds_a_level());
        assert!(SkyKind::parse("multinormal").unwrap().builds_a_level());
        assert!(SkyKind::parse("freecloud").unwrap().builds_a_level());
        assert!(!SkyKind::parse("additive").unwrap().builds_a_level());
        assert!(!SkyKind::parse("config").unwrap().builds_a_level());
    }

    /// `blocks_sun` is "not `no`", not "`yes`" (0x45B6FE), and `none` is a four-character prefix
    /// test (0x45B746).
    #[test]
    fn the_sun_flag_and_the_none_test() {
        let f = SkyFile::parse(&BASIC.replace("NO\n", "MAYBE\n")).unwrap();
        assert!(f.blocks[0].blocks_sun, "anything not starting `no` sets the flag");
        assert!(!SkyFile::parse(BASIC).unwrap().blocks[0].blocks_sun);
        assert_eq!(name("NONEXISTENT"), None, "the compare is four characters, not the whole word");
        assert_eq!(name("non"), Some("non".to_string()));
    }
}

/// `GameData/Track/Sky/<Track>.bin` — the sky's runtime configuration, dumped to disk.
///
/// The loader 0x459650 reads the whole file into a fixed **0x71DC-byte** buffer called `SkyCnfig`
/// (falling back to `Sky/default.bin`) and checks its leading `u32` version; every shipped track is
/// version 9 except `Monaco.bin`, which is 7. Most of the file is never written — 22 700 of
/// Coastal's 29 148 bytes are the allocator's `0xCB` fill — so only the parts a consumer reads are
/// meaningful.
///
/// The one this type exposes is the colour array at **`+0xCE8`**, addressed by 0x459970 and
/// 0x4599E0 as 60-byte records (`buf + 0xCE8 + 60·i`, a record counting as used when its first
/// dword is non-zero). Each record is two colour **pairs**, one per weather region:
///
/// ```text
/// i32 used                 i32 gradient      f32 rgb_top[3]  f32 rgb_bottom[3]   (region A)
/// i32 gradient             f32 rgb_top[3]    f32 rgb_bottom[3]                   (region B)
/// ```
///
/// with the components on a 0..254 scale. Record 0 reads as the sky's own colour — black on the five
/// Monaco circuits, sky blue on forest and skidpad, a warm 254/210/196 on coastal's sunset, and the
/// only one with a real gradient is alps (128/128/144 over 75/75/144, `gradient = 1`). Records 1 and
/// 2 carry two further colour pairs whose role is not established.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyColours {
    /// `gradient != 0`: the pair is a top-to-bottom ramp rather than one flat colour.
    pub gradient: [bool; 2],
    /// `[region][0] = top, [region][1] = bottom`, each 0..1.
    pub rgb: [[[f32; 3]; 2]; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyBin {
    pub version: u32,
    /// Records 0..3 of the array at `+0xCE8`, in file order; `None` where the record is unused.
    pub colours: Vec<Option<SkyColours>>,
    /// `+0x04` — the horizon ring's start azimuth, **in turns** (0.0 foothills … 1.0 castle,
    /// industrial): 0x459D30 turns its first point `(0, 0, 400)` by it (0x459FA2…0x45A006) and
    /// 0x45A1A0 takes it off the camera's heading to pick the quads to draw (0x45A1BD).
    /// *(Until 2026-09-26 this said "no reader found".)*
    pub ring_turn: f32,
    /// `+0x08` — how far the sixteen `hzcl` colours are rotated around the horizon ring (0 or 5).
    pub colour_rotation: i32,
    /// `+0x0C`, `+0x10` — the tile grid of the `horz` texture: 4 × 2 everywhere but Monaco (4 × 1).
    pub tile_cols: i32,
    pub tile_rows: i32,
    /// `+0x14`, `+0x18` — the horizon ring's bottom `y` and its height in metres, e.g. −182.8 /
    /// +237.6 (autobahn): the rows sit at `+0x14` and `+0x14 + +0x18` (0x45A0B4…0x45A0C3).
    pub height_pair: [f32; 2],
    /// `+0x1C` — per ring segment, the `horz` tile it shows and whether that tile is mirrored in u.
    pub segments: [SkySegment; 16],
    /// `+0x3C` (region A) and `+0xB3C` (region B) — the ring each region draws.
    pub rings: [SkyRing; 2],
    /// `+0xCE4` — a scalar handed to the region-A ring's constructor, 0.1 … 1.5; the sun's scale.
    pub scalar_ce4: f32,
    /// `+0xCDC`, `+0xCE0` — where the sun sits on region A's dome before its turn (0x461090's
    /// first two arguments, pushed at 0x46092A / 0x46092B); 0, 0 on the tracks whose sun is overhead.
    pub sun_offset: [f32; 2],
    /// `+0xDD8` — non-zero enables the sun's screen read-back (the lens flare, 0x4604B8).
    pub sun_readback: bool,
    /// `+0xDDC` — a per-slice scalar track, handed to every layer's update (0x45D077).
    pub track_a: Vec<(i32, f32)>,
    /// `+0x1A5C` — a per-slice track of eleven floats (0x45A560).
    pub track_b: Vec<(i32, [f32; 11])>,
    /// `+0x655C` — a per-slice byte track (0x45A970).
    pub track_c: Vec<(i32, u8)>,
}

/// One of the sixteen quads of the horizon ring (`SkyBin+0x1C`, two bytes each).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SkySegment {
    /// which tile of the `horz` grid this quad shows
    pub tile: u8,
    /// non-zero swaps the quad's two u coordinates
    pub mirror: u8,
}

/// The four floats a weather region's ring is built from (`+0x3C` for A, `+0xB3C` for B), passed to
/// 0x460EC0 at 0x45AFF0 / 0x45B051.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SkyRing {
    /// wind / ring rotation, **in turns** — the original multiplies by 2π before `fsin`/`fcos`
    /// (0x5327E0, 0x5327F0, `[0x5BB2C0] = 6.28318452835083`)
    pub angle_turns: f32,
    /// the layer grid's side in metres, 500 (industrial) … 1500 (monaco, coastal, autobahn) — not
    /// the horizon ring's radius, which is a fixed 400 (0x459F92)
    pub radius: f32,
    /// a near-zero constant, ±0.0002 in every shipped file
    pub small: f32,
    /// the ring's height above the camera: 44 … 125 for region A, −75 for the default region B
    pub height: f32,
}

impl SkyBin {
    /// The sun, as the lens-flare object 0x460780 places it at `+0x11C` through **0x461090**:
    /// the offset `(a, b)` turned by region A's `angle_turns` (`c`, times 2π, `fcos` / `fsin`),
    /// raised on the ring's dome `height − small·(a² + b²)`, all scaled by `+0xCE4`:
    ///
    /// ```text
    /// x = (cos c · a + sin c · b) · g
    /// y = (height − small · (a² + b²)) · g
    /// z = (cos c · b − sin c · a) · g
    /// ```
    ///
    /// The race car's light travels along its **negation**, normalised (0x43AC8D…0x43AD36).
    pub fn sun_position(&self) -> [f32; 3] {
        let [a, b] = self.sun_offset;
        let r = &self.rings[0];
        let t = (r.angle_turns as f64 * 6.28318452835083) as f32;
        let (c, s) = (t.cos(), t.sin());
        let g = self.scalar_ce4;
        [(c * a + s * b) * g, (r.height - r.small * (a * a + b * b)) * g, (c * b - s * a) * g]
    }

    /// Whether the sun object 0x460780 is on (`+0x140`, 0x4607CE…0x460822): off only when both
    /// offsets lie strictly inside ±0.02 (`[0x5B3F54]` / `[0x5B3F50]`) — the overhead suns of
    /// `Autobahn`, `Canyon`, `Forest`, `foothills` and the Monaco circuits.
    pub fn sun_active(&self) -> bool {
        let inside = |v: f32| v > -0.02 && v < 0.02;
        !(inside(self.sun_offset[0]) && inside(self.sun_offset[1]))
    }

    pub const COLOURS_AT: usize = 0xCE8;
    pub const RECORD: usize = 60;
    /// `+0xDDC`, `+0x1A5C`, `+0x655C` — three per-slice keyframe tracks, each at most 400 entries and
    /// ended by the first entry whose slice is negative (uninitialised memory is `0xCB` filler,
    /// which reads as one). The upgrade path 0x459700 writes the same bound and the `−1` terminator.
    pub const TRACK_A_AT: usize = 0xDDC;
    pub const TRACK_B_AT: usize = 0x1A5C;
    pub const TRACK_C_AT: usize = 0x655C;
    pub const MAX_KEYS: usize = 0x190;

    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let u32_at = |o: usize| -> Result<u32, FormatError> {
            data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or(FormatError::Truncated { what: "sky .bin", offset: o, needed: 4 })
        };
        let f_at = |o: usize| -> f32 { data.get(o..o + 4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0.0) / 254.0 };
        let version = u32_at(0)?;
        let mut colours = Vec::new();
        for i in 0..4 {
            let o = Self::COLOURS_AT + Self::RECORD * i;
            if o + Self::RECORD > data.len() || u32_at(o)? == 0 {
                colours.push(None);
                continue;
            }
            let rgb = |o: usize| [f_at(o), f_at(o + 4), f_at(o + 8)];
            colours.push(Some(SkyColours {
                gradient: [u32_at(o + 4)? != 0, u32_at(o + 32)? != 0],
                rgb: [[rgb(o + 8), rgb(o + 20)], [rgb(o + 36), rgb(o + 48)]],
            }));
        }
        let f_raw = |o: usize| -> f32 { data.get(o..o + 4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(0.0) };
        let i_at = |o: usize| -> i32 { data.get(o..o + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(-1) };
        let mut segments = [SkySegment::default(); 16];
        for (i, s) in segments.iter_mut().enumerate() {
            *s = SkySegment { tile: data.get(0x1C + i * 2).copied().unwrap_or(0), mirror: data.get(0x1D + i * 2).copied().unwrap_or(0) };
        }
        let ring = |o: usize| SkyRing { angle_turns: f_raw(o), radius: f_raw(o + 4), small: f_raw(o + 8), height: f_raw(o + 12) };
        // each track ends at the first negative slice, and never runs past 400 entries
        let mut track_a = Vec::new();
        let mut track_b = Vec::new();
        let mut track_c = Vec::new();
        for k in 0..Self::MAX_KEYS {
            let o = Self::TRACK_A_AT + k * 8;
            if o + 8 > data.len() || i_at(o) < 0 {
                break;
            }
            track_a.push((i_at(o), f_raw(o + 4)));
        }
        for k in 0..Self::MAX_KEYS {
            let o = Self::TRACK_B_AT + k * 48;
            if o + 48 > data.len() || i_at(o) < 0 {
                break;
            }
            let mut v = [0f32; 11];
            for (i, x) in v.iter_mut().enumerate() {
                *x = f_raw(o + 4 + i * 4);
            }
            track_b.push((i_at(o), v));
        }
        for k in 0..Self::MAX_KEYS {
            let o = Self::TRACK_C_AT + k * 8;
            if o + 8 > data.len() || i_at(o) < 0 {
                break;
            }
            track_c.push((i_at(o), data[o + 4]));
        }
        Ok(Self {
            version,
            colours,
            ring_turn: f_raw(4),
            colour_rotation: i_at(8),
            tile_cols: i_at(0xC),
            tile_rows: i_at(0x10),
            height_pair: [f_raw(0x14), f_raw(0x18)],
            segments,
            rings: [ring(0x3C), ring(0xB3C)],
            scalar_ce4: f_raw(0xCE4),
            sun_offset: [f_raw(0xCDC), f_raw(0xCE0)],
            sun_readback: i_at(0xDD8) != 0,
            track_a,
            track_b,
            track_c,
        })
    }

    /// The `horz` UVs of ring segment `k`, as `[u_left, u_right, v_top, v_bottom]` (0x459E71).
    ///
    /// The tile index is split over the grid, `u = (tile mod cols)/cols`, `row = tile / cols`, and
    /// the v range is inset by the original's own 0.01 / 0.98 (`[0x5B27A8]`, `[0x5B25F4]`) so the
    /// strip never samples its neighbours. `mirror` swaps the two u's.
    pub fn segment_uv(&self, k: usize) -> [f32; 4] {
        let s = self.segments[k % 16];
        let (cols, rows) = (self.tile_cols.max(1) as f32, self.tile_rows.max(1) as f32);
        let col = (s.tile as i32 % self.tile_cols.max(1)) as f32;
        let row = (s.tile as i32 / self.tile_cols.max(1)) as f32;
        let (u0, u1) = if s.mirror == 0 { (col / cols, (col + 1.0) / cols) } else { ((col + 1.0) / cols, col / cols) };
        [u0, u1, (row + 0.01) / rows, (row + 0.98) / rows]
    }

    /// Which of the sixteen `hzcl` colours segment `k` starts at (0x459D7D). The ring is rotated by
    /// `colour_rotation`, so segment `k` spans colours `k + rot` and `k + rot + 1`, both mod 16.
    pub fn segment_colour(&self, k: usize) -> [usize; 2] {
        let r = self.colour_rotation.rem_euclid(16) as usize;
        [(k + r) % 16, (k + r + 1) % 16]
    }

    /// Evaluate a per-slice track at `slice`, the way 0x45A350 does: find the pair of keys the slice
    /// falls between and interpolate linearly, with every slice difference wrapped into ±half a lap
    /// (`[0x628BC0]`'s count) so a track that starts before the finish line still reads correctly.
    pub fn sample_track_a(&self, slice: i32, lap_slices: i32) -> Option<f32> {
        sample(&self.track_a, slice, lap_slices, |v| *v)
    }

    /// Track B's float `field` (the entry's `+4 + 4·field`) at `slice`, as the sky manager's
    /// accessors read it (the object `[0x627E48]+0x48`, built at 0x45D8B0 by **0x45A490** over
    /// `+0x1A5C`): **0x45A560** finds the keys `b`, `c = b + 1` (0 past the last) with `key[b] ≤
    /// slice < key[c]` — keeping the last pair while it still holds, else searching from 0 and
    /// ending on the last key and key 0 — and returns `1 − t`, `t` the slice's wrapped distance past
    /// `key[b]` over the wrapped span to `key[c]` (both wrapped into ±half of `[[0x628BC0]]`, the
    /// slice count, and a negative one taken a lap on); `1.0` when `b == c` or the two keys share a
    /// slice. The field accessor (**0x45A8E0** for `+0x18`, field 5; 0x45A910 `+0x1C`, 0x45A940
    /// `+0x20`) returns `(1 − s)·key[c] + s·key[b]`. With fewer than two authored keys 0x45A490
    /// writes two zero keys (slices 0 and 4), so every field reads 0. The cursor's fast path gives
    /// the search's answer on keys in ascending slice order, which every shipped track's are.
    ///
    /// Field 5 (`+0x18`, 0x45D000) is what the car's glare scales its road reflection 0x451D60 by
    /// (0x43D486), what the wheel spray 0x43EF80 gates on (> 0.1, 0x43EFDE) and what 0x459020 hands
    /// its two weather emitters; it is 0 on every shipped track but **Autobahn** (0…1 along the lap).
    pub fn sample_track_b(&self, field: usize, slice: i32, lap_slices: i32) -> f32 {
        let keys = &self.track_b;
        let n = keys.len();
        if n < 2 || field >= 11 {
            return 0.0;
        }
        let (mut b, mut c) = (n - 1, 0);
        for k in 0..n - 1 {
            if keys[k].0 <= slice && keys[k + 1].0 > slice {
                (b, c) = (k, k + 1);
                break;
            }
        }
        let s = if keys[c].0 == keys[b].0 {
            1.0
        } else {
            let lap = lap_slices.max(1);
            let span = wrap(keys[c].0 - keys[b].0, lap);
            let span = if span < 0 { span + lap } else { span };
            let into = wrap(slice - keys[b].0, lap);
            let into = if into < 0 { into + lap } else { into };
            if span == 0 { 1.0 } else { 1.0 - into as f32 / span as f32 }
        };
        (1.0 - s) * keys[c].1[field] + s * keys[b].1[field]
    }

    /// Which of the four colour records the sky uses, the way `0x459950` picks it: start at the
    /// **`SKYRENDERING` option** `[0x657D9C]` (the `Graphics.SkyRendering` list accessor 0x4E5A59,
    /// whose three items are *off / normal / high*, strings 1500 / 1511 / 1504) and step **down**
    /// while the record's first dword is zero, ending at 0 if none is enabled.
    ///
    /// The shipped option is 0 (no `fe.txt` key), so index 0 is what the game shows. A record's
    /// pure-red `rgb` is not faulty data: it is the placeholder behind a set `gradient` flag, under
    /// which the colour comes from track B ([`Self::background_at`], 0x459A70) — `Autobahn`'s
    /// record 0 and `alps`' 0…2 are such. *(Until 2026-09-26 this said alps' records 1 and 2 were
    /// "never authored".)*
    pub fn colour_index(&self, detail: usize) -> usize {
        let mut i = detail.min(self.colours.len().saturating_sub(1));
        while i > 0 && self.colours.get(i).map(|c| c.is_none()).unwrap_or(true) {
            i -= 1;
        }
        i
    }

    /// The sky's own colour for weather region A — what the frame is cleared to behind the layers.
    pub fn background(&self) -> Option<[f32; 3]> {
        self.background_for(0)
    }

    /// The same at a given `SKYRENDERING` level — the record's flat top colour, which is only what
    /// the frame shows when the record's `gradient` is clear and track A reads 0 there; the race
    /// takes [`Self::background_at`].
    pub fn background_for(&self, detail: usize) -> Option<[f32; 3]> {
        self.colours.get(self.colour_index(detail)).and_then(|c| *c).map(|c| c.rgb[0][0])
    }

    /// Track B's **colour** — the dword at the key's `+0x10` (the float field 3's bits, an
    /// `0x00RRGGBB`), at `slice`: **0x45A720**(`s`, 1) over the pair 0x45A560 picks, lerped a byte
    /// at a time in 16.16 — `((B·s·65536) >> 16) + ((C·(65536 − s·65536)) >> 16)` per byte, with
    /// `s` the sampler's `1 − t` weighting key `b` — and `0xFF` on top. With fewer than two keys
    /// 0x45A490's two zero keys give black.
    pub fn track_b_colour(&self, slice: i32, lap_slices: i32) -> [f32; 3] {
        match self.track_b_pair(slice, lap_slices) {
            Some(p) => Self::rgb(self.track_b_blend(p, 3)),
            None => [0.0; 3],
        }
    }

    /// 0x45A560's pair at `slice`: the keys `b`, `c` and the weight `s` of `b` ([`Self::sample_track_b`]
    /// has the rule); `None` with fewer than two keys (0x45A490's two zero keys, which read 0).
    fn track_b_pair(&self, slice: i32, lap_slices: i32) -> Option<(usize, usize, f32)> {
        let keys = &self.track_b;
        let n = keys.len();
        if n < 2 {
            return None;
        }
        let (mut b, mut c) = (n - 1, 0);
        for k in 0..n - 1 {
            if keys[k].0 <= slice && keys[k + 1].0 > slice {
                (b, c) = (k, k + 1);
                break;
            }
        }
        let s = if keys[c].0 == keys[b].0 {
            1.0
        } else {
            let lap = lap_slices.max(1);
            let span = wrap(keys[c].0 - keys[b].0, lap);
            let span = if span < 0 { span + lap } else { span };
            let into = wrap(slice - keys[b].0, lap);
            let into = if into < 0 { into + lap } else { into };
            if span == 0 { 1.0 } else { 1.0 - into as f32 / span as f32 }
        };
        Some((b, c, s))
    }

    /// The colour field `field`'s dwords of keys `b` and `c` blended a byte at a time in 16.16, as
    /// 0x45A720 / 0x45A800 do: `w = fistp(s · 65536)` (0x45A727…0x45A736, round to nearest),
    /// `(B·w >> 16) + (C·(65536 − w) >> 16)` per byte (0x45A77F…0x45A7ED), `0xFF` on top.
    /// *(Until 2026-09-26 the port truncated `w`.)*
    fn track_b_blend(&self, (b, c, s): (usize, usize, f32), field: usize) -> u32 {
        let wb = ((s * 65536.0) as f64).round_ties_even() as i64;
        let wc = 65536 - wb;
        let (cb, cc) = (self.track_b[b].1[field].to_bits(), self.track_b[c].1[field].to_bits());
        let byte = |shift: u32| (((((cb >> shift) & 0xFF) as i64 * wb) >> 16) + ((((cc >> shift) & 0xFF) as i64 * wc) >> 16)) as u32 & 0xFF;
        0xFF00_0000 | byte(16) << 16 | byte(8) << 8 | byte(0)
    }

    fn rgb(c: u32) -> [f32; 3] {
        [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0]
    }

    /// The weather fog at `slice` — the sky pass's (`sky`, **0x45D2E0**, set at 0x45D695 before the
    /// levels draw) or the scene's (**0x45D3B0**, set at 0x45D7A2 as the sky pass ends, so over the
    /// track, the cars, the particles and the weather after it) — as `(colour, density)`, or `None`
    /// when the pass turns the fog off:
    ///
    /// ```text
    /// x = (1 − s)·key[c].f + s·key[b].f        f = field 0 (sky, 0x45A6A0) / 1 (scene, 0x45A6E0)
    /// density = x³                             ; fld st0, fmul st1, fmulp (0x45A6CE…0x45A6D2)
    /// on while density > 3e-6 ([0x5B3E64]; `test ah, 0x41`, 0x45D30F / 0x45D3DF), then
    ///   Thrash 0x15 ← 4 (EXP) and ← 1 (on), 0x0E ← density (0x537920), 0x0F ← colour (0x537980)
    /// colour = key[b]'s dword f+3 when key[c].f ≤ 3e-6 (0x45A74E…0x45A775, the sky's call passes
    ///          0 for 0x45A720's flag at 0x45D358), else the two blended a byte at a time
    /// ```
    ///
    /// `dx7z.dll` makes that `FOGTABLEMODE` `EXP`, `FOGTABLEDENSITY` the float as it is (0x60004A93:
    /// a value with bits above 0xFFF is not an integer divisor), `FOGCOLOR` the colour's low 24 bits
    /// (0x60004AFA) and `FOGENABLE` 1 (0x600048B7, 0x60004A26) — on a card with table fog
    /// (`[0x60033F6C]`; without it state 4 fails and the fog stays off). *(Until 2026-09-26 the notes
    /// held that the original has no fog at all — H34, which looked at the draw distance only.)*
    pub fn fog_at(&self, sky: bool, slice: i32, lap_slices: i32) -> Option<([f32; 3], f32)> {
        let f = if sky { 0 } else { 1 };
        let (b, c, s) = self.track_b_pair(slice, lap_slices)?;
        let (kb, kc) = (self.track_b[b].1[f], self.track_b[c].1[f]);
        let x = kc as f64 * (1.0 - s as f64) + kb as f64 * s as f64;
        let density = x * x * x;
        if density <= 3.000_000_106_112_566e-6 {
            return None;
        }
        let colour = if kc as f64 <= 3.000_000_106_112_566e-6 { self.track_b[b].1[f + 3].to_bits() } else { self.track_b_blend((b, c, s), f + 3) };
        Some((Self::rgb(colour), density as f32))
    }

    /// What the race clears the frame to — **0x459A70** on the record `0x459950` picks
    /// ([`Self::colour_index`]), group A (`[0x627F43] & 0x10` is never set on a shipped track):
    /// with the group's `gradient` flag set, the sky manager's track-B colour
    /// ([`Self::track_b_colour`], 0x45D2B0); else `(1 − t) · top + t · bottom` with `t` track A's
    /// value at the camera's slice (0x45CFC0 → 0x45A350, 0x459AC3…0x459B1E), each channel through
    /// `fistp`. *(Until 2026-09-26 the port used the record's top colour alone, which on
    /// `Autobahn` — and `alps`' records 1 and 2 — is the tool's pure-red placeholder behind a set
    /// `gradient` flag: the sky showed red through the trees.)*
    pub fn background_at(&self, detail: usize, slice: i32, lap_slices: i32) -> Option<[f32; 3]> {
        let c = self.colours.get(self.colour_index(detail)).and_then(|c| *c)?;
        if c.gradient[0] {
            return Some(self.track_b_colour(slice, lap_slices));
        }
        let t = self.sample_track_a(slice, lap_slices).unwrap_or(0.0);
        let [top, bot] = c.rgb[0];
        let ch = |k: usize| (((1.0 - t) * top[k] * 254.0 + t * bot[k] * 254.0).round() / 254.0).clamp(0.0, 1.0);
        Some([ch(0), ch(1), ch(2)])
    }
}

/// `x − y` wrapped into ±half a lap (0x45A3E2).
fn wrap(d: i32, lap: i32) -> i32 {
    if lap <= 0 {
        return d;
    }
    let half = lap / 2;
    if d > half { d - lap } else if d < -half { d + lap } else { d }
}

/// A scalar per-slice track evaluated at `slice` the way 0x45A350 does ([`SkyBin::sample_track_a`]).
pub fn sample_track(keys: &[(i32, f32)], slice: i32, lap_slices: i32) -> Option<f32> {
    sample(keys, slice, lap_slices, |v| *v)
}

fn sample<T: Copy>(keys: &[(i32, T)], slice: i32, lap: i32, get: impl Fn(&T) -> f32) -> Option<f32> {
    if keys.is_empty() {
        return None;
    }
    if keys.len() == 1 {
        return Some(get(&keys[0].1));
    }
    // the pair whose wrapped span contains the slice
    for w in 0..keys.len() {
        let (a, b) = (&keys[w], &keys[(w + 1) % keys.len()]);
        let span = wrap(b.0 - a.0, lap);
        let into = wrap(slice - a.0, lap);
        if span > 0 && (0..=span).contains(&into) {
            let t = into as f32 / span as f32;
            return Some(get(&a.1) + (get(&b.1) - get(&a.1)) * t);
        }
    }
    Some(get(&keys[0].1))
}

#[cfg(test)]
mod bin_tests {
    use super::*;

    #[test]
    fn a_gradient_record_takes_track_bs_colour() {
        // a bin with record 0 enabled, gradient set, the placeholder red, and two track-B keys
        let mut d = vec![0u8; SkyBin::TRACK_C_AT + 8];
        let put = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, SkyBin::COLOURS_AT, 1);
        put(&mut d, SkyBin::COLOURS_AT + 4, 1);
        put(&mut d, SkyBin::COLOURS_AT + 8, 254f32.to_bits());
        put(&mut d, SkyBin::TRACK_A_AT, u32::MAX);
        put(&mut d, SkyBin::TRACK_B_AT, 0);
        put(&mut d, SkyBin::TRACK_B_AT + 0x10, 0xFF41556F);
        put(&mut d, SkyBin::TRACK_B_AT + 48, 100);
        put(&mut d, SkyBin::TRACK_B_AT + 48 + 0x10, 0xFF000000);
        put(&mut d, SkyBin::TRACK_B_AT + 96, u32::MAX);
        put(&mut d, SkyBin::TRACK_C_AT, u32::MAX);
        let bin = SkyBin::parse(&d).unwrap();
        let c0 = bin.background_at(0, 0, 400).unwrap();
        assert!((c0[0] - 0x41 as f32 / 255.0).abs() < 1e-6 && (c0[2] - 0x6F as f32 / 255.0).abs() < 1e-6, "key 0's colour, not the red: {c0:?}");
        let c50 = bin.background_at(0, 50, 400).unwrap();
        assert!((c50[0] - (0x41 / 2) as f32 / 255.0).abs() < 1.5 / 255.0, "half way to black: {c50:?}");
    }

    #[test]
    fn fog_is_the_cube_of_the_lerped_field() {
        // key 0 at slice 0: sky 0.2, scene 0.3, colours 3 / 4; key 1 at slice 100: sky 0, scene 0.1
        let mut d = vec![0u8; SkyBin::TRACK_C_AT + 8];
        let put = |d: &mut Vec<u8>, o: usize, v: u32| d[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, SkyBin::TRACK_A_AT, u32::MAX);
        let k0 = SkyBin::TRACK_B_AT;
        put(&mut d, k0, 0);
        put(&mut d, k0 + 4, 0.2f32.to_bits());
        put(&mut d, k0 + 8, 0.3f32.to_bits());
        put(&mut d, k0 + 0x10, 0xFF8B6757);
        put(&mut d, k0 + 0x14, 0xFF204060);
        let k1 = k0 + 48;
        put(&mut d, k1, 100);
        put(&mut d, k1 + 8, 0.1f32.to_bits());
        put(&mut d, k1 + 0x10, 0xFF000000);
        put(&mut d, k1 + 0x14, 0xFF000000);
        put(&mut d, k1 + 48, u32::MAX);
        put(&mut d, SkyBin::TRACK_C_AT, u32::MAX);
        let bin = SkyBin::parse(&d).unwrap();
        // at key 0 (s = 1): 0.3³ and key 0's colour
        let (c, dens) = bin.fog_at(false, 0, 400).unwrap();
        assert!((dens - 0.027).abs() < 1e-6 && (c[0] - 0x20 as f32 / 255.0).abs() < 1e-6, "{c:?} {dens}");
        // half way: x = 0.2, 0.008; the colour a byte-wise half
        let (c, dens) = bin.fog_at(false, 50, 400).unwrap();
        assert!((dens - 0.008).abs() < 1e-6 && (c[2] - (0x60 / 2) as f32 / 255.0).abs() < 1e-6, "{c:?} {dens}");
        // the sky's: 0.1³ at slice 50, and key 1's field 0 is 0, so the colour is key 0's unblended
        let (c, dens) = bin.fog_at(true, 50, 400).unwrap();
        assert!((dens - 0.001).abs() < 1e-6 && (c[0] - 0x8B as f32 / 255.0).abs() < 1e-6, "{c:?} {dens}");
        // at or under 3e-6 the fog is off: 0.01³ = 1e-6
        assert!(bin.fog_at(true, 95, 400).is_none());
    }

    #[test]
    fn rejects_a_short_file() {
        assert!(SkyBin::parse(&[9, 0, 0, 0]).is_ok(), "a truncated file just has no records");
        assert_eq!(SkyBin::parse(&[9, 0, 0, 0]).unwrap().colours.iter().filter(|c| c.is_some()).count(), 0);
    }
}
