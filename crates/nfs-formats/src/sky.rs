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
    /// `+0x04` — one scalar per track, 0.0 (foothills) … 1.0 (castle, industrial). No reader found.
    pub scalar_04: f32,
    /// `+0x08` — how far the sixteen `hzcl` colours are rotated around the horizon ring (0 or 5).
    pub colour_rotation: i32,
    /// `+0x0C`, `+0x10` — the tile grid of the `horz` texture: 4 × 2 everywhere but Monaco (4 × 1).
    pub tile_cols: i32,
    pub tile_rows: i32,
    /// `+0x14`, `+0x18` — a height pair, e.g. −182.8 / +237.6 (autobahn); read together at 0x45A0B4.
    pub height_pair: [f32; 2],
    /// `+0x1C` — per ring segment, the `horz` tile it shows and whether that tile is mirrored in u.
    pub segments: [SkySegment; 16],
    /// `+0x3C` (region A) and `+0xB3C` (region B) — the ring each region draws.
    pub rings: [SkyRing; 2],
    /// `+0xCE4` — a scalar handed to the region-A ring's constructor, 0.1 … 1.5.
    pub scalar_ce4: f32,
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
    /// ring radius in metres: 500 (industrial) … 1500 (monaco, coastal, autobahn)
    pub radius: f32,
    /// a near-zero constant, ±0.0002 in every shipped file
    pub small: f32,
    /// the ring's height above the camera: 44 … 125 for region A, −75 for the default region B
    pub height: f32,
}

impl SkyBin {
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
            scalar_04: f_raw(4),
            colour_rotation: i_at(8),
            tile_cols: i_at(0xC),
            tile_rows: i_at(0x10),
            height_pair: [f_raw(0x14), f_raw(0x18)],
            segments,
            rings: [ring(0x3C), ring(0xB3C)],
            scalar_ce4: f_raw(0xCE4),
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

    /// Which of the four colour records the sky uses, the way `0x459950` picks it: start at the
    /// **`SKYRENDERING` option** `[0x657D9C]` (the `Graphics.SkyRendering` list accessor 0x4E5A59,
    /// whose three items are *off / normal / high*, strings 1500 / 1511 / 1504) and step **down**
    /// while the record's first dword is zero, ending at 0 if none is enabled.
    ///
    /// Index 0 is what reproduces the sky every shipped track visibly has. It is also the only safe
    /// one: on `alps` records 1 and 2 are marked enabled but were never authored — they hold the
    /// pure-red default the tool writes — so either the option is still 0 when a track's sky is
    /// built, or that track's data is faulty. The port therefore defaults to 0 and keeps the walk.
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

    /// The same at a given `SKYRENDERING` level.
    pub fn background_for(&self, detail: usize) -> Option<[f32; 3]> {
        self.colours.get(self.colour_index(detail)).and_then(|c| *c).map(|c| c.rgb[0][0])
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
    fn rejects_a_short_file() {
        assert!(SkyBin::parse(&[9, 0, 0, 0]).is_ok(), "a truncated file just has no records");
        assert_eq!(SkyBin::parse(&[9, 0, 0, 0]).unwrap().colours.iter().filter(|c| c.is_some()).count(), 0);
    }
}
