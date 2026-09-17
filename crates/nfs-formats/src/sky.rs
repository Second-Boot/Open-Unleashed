//! `GameData/Track/Sky/<Track>.txt` — the layered sky definition (`RE/RENDERING.md`).
//!
//! The file is a list of `BEGIN … END` blocks, one per computer capability, and the game keeps the
//! first block whose capability matches the `Graphics.SkyRendering` setting. Everything after a `#`
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkyKind {
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
    /// `multi*` only: the cloud texture of each layer, after both regions.
    pub clouds: Vec<Option<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyFile {
    pub blocks: Vec<SkyBlock>,
}

fn name(s: &str) -> Option<String> {
    (!s.eq_ignore_ascii_case("none")).then(|| s.to_string())
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
            let blocks_sun = next!().eq_ignore_ascii_case("yes");
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
            blocks.push(SkyBlock { capability, kind, blocks_sun, regions, clouds });
        }
        Ok(Self { blocks })
    }

    /// The block the game would use for a capability, falling back to the first one.
    pub fn for_capability(&self, capability: &str) -> Option<&SkyBlock> {
        self.blocks.iter().find(|b| b.capability.eq_ignore_ascii_case(capability)).or_else(|| self.blocks.first())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC: &str = "BEGIN\n#cap\n\tHIGH\n#type\n\tbasic\n#sun\nNO\n\tst1a\n\tnone\n\tflat\n\t.5\n\t.5\n\t0\n\t0\n\t90\n\t0\n\t0\n\tst1a\n\tnone\n\tflat\n\t.5\n\t.5\n\t0\n\t0\n\t90\n\t0\n\t0\nEND\n";
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
}

impl SkyBin {
    pub const COLOURS_AT: usize = 0xCE8;
    pub const RECORD: usize = 60;

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
        Ok(Self { version, colours })
    }

    /// The sky's own colour for weather region A — what the frame is cleared to behind the layers.
    pub fn background(&self) -> Option<[f32; 3]> {
        self.colours.first().and_then(|c| *c).map(|c| c.rgb[0][0])
    }
}

#[cfg(test)]
mod bin_tests {
    use super::*;

    #[test]
    fn rejects_a_short_file() {
        assert!(SkyBin::parse(&[9, 0, 0, 0]).is_ok(), "a truncated file just has no records");
        assert_eq!(SkyBin::parse(&[9, 0, 0, 0]).unwrap().colours.iter().filter(|c| c.is_some()).count(), 0);
    }
}
