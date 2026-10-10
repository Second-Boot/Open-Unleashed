//! `GameData/Track/animdefs.txt` — the per-tag object property definitions.
//!
//! One `SECTION:ANIM` per scene-object tag (the FourCC in an article's `Base` record and in a `.scn`
//! GEOM element), carrying what the object is made of physically — `collideType`
//! (`kCylinder` / `kBox` / `kSmackable`), `audioType`, `mass`, `radius` — and, for the animated ones,
//! a list of `SECTION:PIECE` grouped under `speed=` values. A piece names the article whose rotation
//! track (`QuatGroup`) and vertex track (`VertGroup`) it plays, its `playbackrate` and its
//! `properties` (`LOOP`, `LANDFLAT`, `DIRECTED`).
//!
//! The parser is 0x47D6B0 / 0x47E7C0 (`RE/notes/crp-geometry.md` §7). Two facts from it that the file
//! itself does not state: a piece record starts zeroed with **`playbackrate` = 1** (0x47DE21), and the
//! animation's frame time is `0.1 / playbackrate` seconds (0x476576), so the default is ten frames a
//! second and the shipped 3, 5 and 8 are 33, 50 and 80.
//!
//! `SECTION:Audio` blocks (the ambience patches) are parsed by `engine-audio.md` §7 and skipped here.


/// `properties=` flags of a piece. `LOOP` is bit 0 — the sampler's looping flag (0x47EB21) —,
/// `DIRECTED` bit 8 and `LANDFLAT` bit 0x10 (`[0x5B45AC]` / `[0x5B45B0]`, 0x47EB43…0x47EB72), the
/// two turns 0x470F70 gives a piece about y by the hit's heading.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PieceFlags {
    pub looping: bool,
    pub landflat: bool,
    pub directed: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Piece {
    /// the article whose `Anqt` track this piece plays
    pub quat_group: Option<String>,
    /// the article whose vertex frames this piece plays
    pub vert_group: Option<String>,
    pub base_group: Option<String>,
    /// frames per tenth of a second; 1 unless the file says otherwise (0x47DE21)
    pub playback_rate: f32,
    pub flags: PieceFlags,
}

impl Piece {
    /// Seconds per animation frame: `0.1 / playbackrate` (0x476576).
    pub fn seconds_per_frame(&self) -> f32 {
        if self.playback_rate > 0.0 { 0.1 / self.playback_rate } else { 0.1 }
    }
}

/// `SECTION:Audio { tag=…; SECTION:Sound { … } }` — the animated-object ambient-sound emitter
/// record (`RE/notes/engine-audio.md` §7.3/§7.4, `COVERAGE-2026-09-26.md` §14.38). Parser
/// `0x47D6B0` (`AnimDefs_parseAudioSection`, the `tag=` line) and `0x47D980`
/// (`AnimDefs_parseSound`, the eight fields below, each read inline except the last two, which go
/// through the shared field-match-and-parse helper `0x47D3F0`). All eight offsets are exact —
/// read directly off `0x47D980`'s disassembly (2026-09-30): `PatchID`→word `[ebp+0]`,
/// `Range`→float `[ebp+4]`, `FadeIn`→float `[ebp+8]`, `Trigger`→word `[ebp+0xC]`,
/// `FadeOut`→word `[ebp+0xE]`, `MinRepeat`→word `[ebp+0x10]` (all four inline, `0x47D9FE…0x47DC41`),
/// `RandomRepeat`→word `[ebp+0x12]` and `Volume`→word `[ebp+0x14]` (both via `0x47D3F0`,
/// `0x47DC46…0x47DC78`) — the exact `SoundDef` layout `engine-audio.md` §7.3 already gives. The
/// shipped file has exactly one block, `tag=TRAN` → `PatchID 833` (`train1_833.bnk`), `Range 300`,
/// `FadeIn 17`, `Volume 100`, every other field 0.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SoundDef {
    pub patch_id: u16,
    pub range: f32,
    pub fade_in: f32,
    pub trigger: u16,
    pub fade_out: u16,
    pub min_repeat: u16,
    pub random_repeat: u16,
    pub volume: u16,
}

/// One `SECTION:ANIM`. The parser is **0x47DCF0**, and the record it fills is
/// `+0x00 mass, +0x08 radius, +0x0C audioType, +0x10 collideType, +0x14 drivethru,
/// +0x15 triggerable, +0x16 dynamicLight, +0x18 piece count, +0x80 velocityFactor,
/// +0x84 frameTickOffset, +0x88 ticksPerVertFrame`, zeroed at 0x47DD5D except **radius = 0.5**
/// (0x47DD62) and **ticksPerVertFrame = 8** (0x47DD8D).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimDef {
    /// the scene-object tag, e.g. `DOC2`, `CONE`, `RHEL`
    pub tag: String,
    /// `collideType` — 0x47E0A2/0x47E0EF/0x47E13E store `kSmackable` 3, `kCylinder` 1 and
    /// `kBox` 2 at `+0x10`; anything else leaves the 0 the record was zeroed with.
    pub collide_type: String,
    /// `audioType` — `+0x0C`, the thirteen `kAud…` names in order: `kAudSign` 0, `kAudMetalSign`
    /// 1, `kAudGlass` 2, `kAudWoodbox` 3, `kAudPylon` 4, `kAudTree` 5, `kAudBush` 6,
    /// `kAudCarBody` 7, `kAudWood` 8, `kAudSnow` 9, `kAudIce` 10, `kAudEarth` 11, `kAudRock` 12.
    pub audio_type: String,
    pub mass: f32,
    /// `radius` — **0.5** when the file does not say (0x47DD62).
    pub radius: f32,
    /// `ticksPerVertFrame` — **8** when the file does not say (0x47DD8D).
    pub ticks_per_vert_frame: Option<u32>,
    pub triggerable: bool,
    pub dynamic_light: bool,
    /// `drivethru` / `drivethrough` — both spellings write the same byte `+0x14` (0x47E6CE and
    /// 0x47E6F3). Only `BUSH` carries it in the shipped file, and no scene places a `BUSH`.
    pub drive_through: bool,
    /// `velocityFactor` — `+0x80`, **scaled by a thousandth on the way in** (0x47E618 multiplies
    /// the stored float by `[0x5B3060]` = 0.001), so `BBOX`'s `velocityFactor=18` is **0.018**.
    /// Only `BBOX` carries it — the crates the tracks place as world articles (coastal, castle,
    /// industrial), whose pieces drift by it (0x476641).
    pub velocity_factor: f32,
    /// `frameTickOffset` — `+0x84`. No shipped definition sets it.
    pub frame_tick_offset: i32,
    /// pieces with the `speed=` they were listed under
    pub pieces: Vec<(f32, Piece)>,
    /// The `speed=` values in file order: every `speed=` line stores its value at `+0x60 + 4·n`
    /// and counts `+0x5C` up (0x47DFAC / 0x47DFCD).
    pub bands: Vec<f32>,
    /// `+0x70…`, the pieces per band: every piece counts up `+0x70[n − 1]`, or `+0x70[0]` before
    /// the first band (0x47DE5F…0x47DE77).
    pub band_pieces: Vec<usize>,
}

impl Default for AnimDef {
    fn default() -> Self {
        Self {
            tag: String::new(),
            collide_type: String::new(),
            audio_type: String::new(),
            mass: 0.0,
            radius: 0.5,
            ticks_per_vert_frame: Some(8),
            triggerable: false,
            dynamic_light: false,
            drive_through: false,
            velocity_factor: 0.0,
            frame_tick_offset: 0,
            pieces: Vec::new(),
            bands: Vec::new(),
            band_pieces: Vec::new(),
        }
    }
}

impl AnimDef {
    /// The `audioType` as the parser stores it at `+0x0C`: its index among the thirteen `kAud…`
    /// names (the order in [`AnimDef::audio_type`]'s doc); `None` for a name that is not one.
    pub fn audio_type_index(&self) -> Option<u32> {
        const NAMES: [&str; 13] = [
            "kAudSign", "kAudMetalSign", "kAudGlass", "kAudWoodbox", "kAudPylon", "kAudTree", "kAudBush", "kAudCarBody", "kAudWood", "kAudSnow", "kAudIce", "kAudEarth", "kAudRock",
        ];
        NAMES.iter().position(|n| n.eq_ignore_ascii_case(&self.audio_type)).map(|i| i as u32)
    }

    /// The pieces 0x476380 starts for a hit at `speed` (`|v|`, 0x532B20), as a range of `pieces`:
    /// the bands are walked while `speed` is above the band's value (0x4763ED…0x476400); none passed
    /// gives the first piece alone, else the last band passed with the pieces of the bands before
    /// it skipped (0x476402…0x476422).
    pub fn group(&self, speed: f32) -> std::ops::Range<usize> {
        band_group(&self.bands, &self.band_pieces, speed)
    }

    /// The pieces of the lowest `speed=` group — the one an object at rest plays.
    pub fn resting_pieces(&self) -> impl Iterator<Item = &Piece> {
        let lowest = self.pieces.iter().map(|(s, _)| *s).fold(f32::MAX, f32::min);
        self.pieces.iter().filter(move |(s, _)| *s == lowest).map(|(_, p)| p)
    }
}

/// [`AnimDef::group`] over a definition's `speed=` values and pieces per band.
pub fn band_group(bands: &[f32], band_pieces: &[usize], speed: f32) -> std::ops::Range<usize> {
    let passed = bands.iter().take_while(|v| speed > **v).count();
    if passed == 0 {
        return 0..1;
    }
    let count = |i: usize| band_pieces.get(i).copied().unwrap_or(0);
    let start: usize = (0..passed - 1).map(count).sum();
    start..start + count(passed - 1)
}

/// The definitions **in file order**: 0x47DCF0 appends each `SECTION:ANIM` through 0x525AE0, and
/// the lookup 0x47ED90 is a linear search that returns the **first** record whose tag matches
/// (so of `WELD`'s two definitions the `kCylinder` one wins), and — for a tag it does not find —
/// the vector's first record, `ENDW` (0x47EDCC…0x47EDD1).
#[derive(Debug, Clone, Default)]
pub struct AnimDefs {
    defs: Vec<AnimDef>,
    /// `SECTION:Audio` blocks, by their `tag=`, in file order (`0x47ED4C`'s own, separate vector —
    /// `RE/notes/crp-geometry.md` §7.1's "`SECTION:Audio` goes to a second vector"). Not merged
    /// into `defs`: an `Audio` tag need not have a matching `ANIM` definition (the shipped `TRAN`
    /// has none), so this is looked up on its own by [`AnimDefs::sound_by_tag`]/[`AnimDefs::sound`].
    sounds: Vec<(String, SoundDef)>,
}

impl AnimDefs {
    /// The first definition with this tag (0x47ED90's search), `None` when there is none.
    pub fn get(&self, tag: &str) -> Option<&AnimDef> {
        self.defs.iter().find(|d| d.tag.eq_ignore_ascii_case(tag))
    }

    /// 0x47ED90 in full: the tag's first definition, else the file's first — `None` only for a zero
    /// tag (0x47ED9E) or an empty file.
    pub fn lookup(&self, tag: u32) -> Option<&AnimDef> {
        if tag == 0 {
            return None;
        }
        self.by_fourcc(tag).or_else(|| self.defs.first())
    }

    /// By the FourCC an article's `Base` record carries, little-endian.
    pub fn by_fourcc(&self, tag: u32) -> Option<&AnimDef> {
        let b = tag.to_le_bytes();
        let s: String = b.iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
        self.get(s.trim())
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &AnimDef> {
        self.defs.iter()
    }

    /// The first `SECTION:Audio`'s `SoundDef` with this tag, `0x47ED4C`'s own linear search
    /// (unlike `defs`'/`0x47ED90`'s "unknown tag → the file's first" fallback, an unmatched Audio
    /// tag has no fallback in the original — no code path there needs one, since only a scene
    /// object whose own `Base` tag is looked up gets here at all).
    pub fn sound_by_tag(&self, tag: &str) -> Option<&SoundDef> {
        self.sounds.iter().find(|(t, _)| t.eq_ignore_ascii_case(tag)).map(|(_, s)| s)
    }

    /// By the FourCC an article's `Base` record carries, little-endian — the same decoding
    /// [`AnimDefs::by_fourcc`] uses for `ANIM` definitions.
    pub fn sound_by_fourcc(&self, tag: u32) -> Option<&SoundDef> {
        let b = tag.to_le_bytes();
        let s: String = b.iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
        self.sound_by_tag(s.trim())
    }

    /// Every `SECTION:Audio` block, in file order.
    pub fn sounds(&self) -> impl Iterator<Item = (&str, &SoundDef)> {
        self.sounds.iter().map(|(t, s)| (t.as_str(), s))
    }

    pub fn parse(text: &str) -> AnimDefs {
        let mut out = AnimDefs::default();
        let mut def: Option<AnimDef> = None;
        let mut piece: Option<Piece> = None;
        // `SECTION:Audio`'s own two-level nest (0x47D6B0/0x47D980): `audio_tag` is set while inside
        // the outer `Audio{}` (its own `tag=` line), `sound` while inside the nested `Sound{}`.
        let mut audio_tag: Option<String> = None;
        let mut sound: Option<SoundDef> = None;
        let mut speed = 0.0f32;
        // depth 1 = inside the section's braces, 2 = inside a piece's/sound's
        let mut depth = 0usize;
        let mut pending: Option<&'static str> = None;
        for raw in text.lines() {
            let line = raw.split("//").next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(kind) = line.strip_prefix("SECTION:") {
                let kind = kind.trim().to_ascii_uppercase();
                pending = match kind.as_str() {
                    "ANIM" => Some("anim"),
                    "PIECE" => Some("piece"),
                    "AUDIO" => Some("audio"),
                    "SOUND" => Some("sound"),
                    _ => Some("other"),
                };
                continue;
            }
            if line.starts_with('{') {
                depth += 1;
                match pending.take() {
                    Some("anim") => def = Some(AnimDef::default()),
                    Some("piece") => piece = Some(Piece { playback_rate: 1.0, ..Piece::default() }),
                    Some("audio") => audio_tag = Some(String::new()),
                    Some("sound") => sound = Some(SoundDef::default()),
                    _ => {}
                }
                continue;
            }
            if line.starts_with('}') {
                depth = depth.saturating_sub(1);
                if let Some(p) = piece.take()
                    && let Some(d) = def.as_mut()
                {
                    d.pieces.push((speed, p));
                    let band = d.bands.len().saturating_sub(1);
                    if d.band_pieces.len() <= band {
                        d.band_pieces.resize(band + 1, 0);
                    }
                    d.band_pieces[band] += 1;
                } else if let Some(d) = def.take()
                    && !d.tag.is_empty()
                {
                    out.defs.push(d);
                } else if let Some(s) = sound.take() {
                    // closing `Sound{}`: file it under the enclosing `Audio{}`'s tag
                    if let Some(tag) = audio_tag.clone()
                        && !tag.is_empty()
                    {
                        out.sounds.push((tag, s));
                    }
                } else {
                    // closing the outer `Audio{}` itself (or any other unhandled section)
                    audio_tag = None;
                }
                continue;
            }
            let Some((k, v)) = line.trim_end_matches(';').split_once('=') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().trim_end_matches(';').trim());
            if let Some(s) = sound.as_mut() {
                match k.as_str() {
                    "patchid" => s.patch_id = v.parse().unwrap_or(0),
                    "range" => s.range = v.parse().unwrap_or(0.0),
                    "fadein" => s.fade_in = v.parse().unwrap_or(0.0),
                    "trigger" => s.trigger = v.parse().unwrap_or(0),
                    "fadeout" => s.fade_out = v.parse().unwrap_or(0),
                    "minrepeat" => s.min_repeat = v.parse().unwrap_or(0),
                    "randomrepeat" => s.random_repeat = v.parse().unwrap_or(0),
                    "volume" => s.volume = v.parse().unwrap_or(0),
                    _ => {}
                }
                continue;
            }
            if audio_tag.is_some() {
                if k == "tag" {
                    audio_tag = Some(v.to_string());
                }
                continue;
            }
            if let Some(p) = piece.as_mut() {
                match k.as_str() {
                    "quatgroup" => p.quat_group = Some(v.to_string()),
                    "vertgroup" => p.vert_group = Some(v.to_string()),
                    "basegroup" => p.base_group = Some(v.to_string()),
                    "playbackrate" => p.playback_rate = v.parse().unwrap_or(1.0),
                    "properties" => match v.to_ascii_uppercase().as_str() {
                        "LOOP" => p.flags.looping = true,
                        "LANDFLAT" => p.flags.landflat = true,
                        "DIRECTED" => p.flags.directed = true,
                        _ => {}
                    },
                    _ => {}
                }
                continue;
            }
            let Some(d) = def.as_mut() else { continue };
            match k.as_str() {
                "tag" => d.tag = v.to_string(),
                "collidetype" => d.collide_type = v.to_string(),
                "audiotype" => d.audio_type = v.to_string(),
                "mass" => d.mass = v.parse().unwrap_or(0.0),
                "radius" => d.radius = v.parse().unwrap_or(0.0),
                "tickspervertframe" => d.ticks_per_vert_frame = v.parse().ok(),
                "triggerable" => d.triggerable = v != "0",
                "dynamiclight" => d.dynamic_light = v != "0",
                "drivethru" | "drivethrough" => d.drive_through = v != "0",
                "velocityfactor" => d.velocity_factor = v.parse().unwrap_or(0.0) * 0.001,
                "frametickoffset" => d.frame_tick_offset = v.parse().unwrap_or(0),
                "speed" => {
                    speed = v.parse().unwrap_or(0.0);
                    d.bands.push(speed);
                }
                _ => {}
            }
        }
        let _ = depth;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defaults the record is zeroed with (0x47DD5D) — radius 0.5, eight ticks a vertex
    /// frame — and the three keys the parser gained with §7.2: `drivethrough`, `velocityFactor`
    /// and `frameTickOffset`.
    #[test]
    fn the_defaults_and_the_three_late_keys() {
        let d = AnimDefs::parse(SAMPLE);
        let cone = d.get("CONE").unwrap();
        assert_eq!(cone.radius, 0.5, "unset radius is 0.5, not 0");
        assert_eq!(cone.ticks_per_vert_frame, Some(8));
        assert!(!cone.drive_through);
        assert_eq!(cone.velocity_factor, 0.0);
        let bush = d.get("BUSH").unwrap();
        assert_eq!(bush.radius, 1.0);
        assert!(bush.drive_through, "drivethrough= writes the same byte as drivethru=");
        // 0x47E618 scales it by 0.001 on the way in
        assert!((bush.velocity_factor - 0.018).abs() < 1e-6, "{}", bush.velocity_factor);
        assert_eq!(bush.frame_tick_offset, 0);
        assert_eq!(d.get("DOC2").unwrap().ticks_per_vert_frame, Some(3));
    }

    const SAMPLE: &str = r#"
// a comment
SECTION:ANIM
{
	tag=CONE;
	collideType=kSmackable;
	audioType=kAudPylon;
	dynamicLight=1;
	mass=20;
}

SECTION:ANIM
{
	tag=BUSH;
	CollideType=kCylinder;
	Radius=1;
	AudioType=kAudBush;
	drivethrough=1;
	velocityFactor=18;
}

SECTION:ANIM	//	Docker just walking
{
	tag=DOC2;
	collideType=0;
	ticksPerVertFrame=3;

	speed=0;
	SECTION:PIECE
	{
		properties=LOOP;
		//BaseGroup=SGN0;
		QuatGroup=DOC2;
		VertGroup=DOC2;
	}
}

SECTION:ANIM
{
	tag=TEMP;
	collideType=kCylinder;
	mass=5;

	speed=0;
	SECTION:PIECE
	{
		playbackrate=3;
		properties=LANDFLAT;
		properties=DIRECTED;
		QuatGroup=SIGN;
	}

	speed=5;
	SECTION:PIECE
	{
		playbackrate=8;
		properties=LANDFLAT;
		QuatGroup=SIGN;
	}
}
"#;

    #[test]
    fn sections_and_their_pieces() {
        let d = AnimDefs::parse(SAMPLE);
        assert_eq!(d.len(), 4);

        let cone = d.get("CONE").unwrap();
        assert_eq!((cone.collide_type.as_str(), cone.audio_type.as_str(), cone.mass), ("kSmackable", "kAudPylon", 20.0));
        assert!(cone.dynamic_light && cone.pieces.is_empty());

        // the docker loops, and with no `playbackrate` of its own it runs at the default ten
        // frames a second (0x47DE21 seeds the piece record with 1.0)
        let doc = d.get("doc2").unwrap();
        assert_eq!(doc.ticks_per_vert_frame, Some(3));
        let p = doc.resting_pieces().next().unwrap();
        assert!(p.flags.looping);
        assert_eq!(p.quat_group.as_deref(), Some("DOC2"));
        assert_eq!(p.playback_rate, 1.0);
        assert!((p.seconds_per_frame() - 0.1).abs() < 1e-6);

        // two speed groups, and the resting one is the slower piece
        let temp = d.get("TEMP").unwrap();
        assert_eq!(temp.pieces.len(), 2);
        assert_eq!(temp.pieces[0].0, 0.0);
        assert_eq!(temp.pieces[1].0, 5.0);
        let rest = temp.resting_pieces().next().unwrap();
        assert_eq!(rest.playback_rate, 3.0);
        assert!((rest.seconds_per_frame() - 0.1 / 3.0).abs() < 1e-6);
        assert!(rest.flags.landflat && rest.flags.directed && !rest.flags.looping);
        // 0x476380's group: at rest (no band passed) the first piece; above 0 the first band;
        // above 5 the second
        assert_eq!((temp.bands.clone(), temp.band_pieces.clone()), (vec![0.0, 5.0], vec![1, 1]));
        assert_eq!(temp.group(0.0), 0..1);
        assert_eq!(temp.group(3.0), 0..1);
        assert_eq!(temp.group(5.0), 0..1);
        assert_eq!(temp.group(9.0), 1..2);
    }

    #[test]
    fn a_fourcc_finds_its_definition() {
        let d = AnimDefs::parse(SAMPLE);
        assert_eq!(d.by_fourcc(u32::from_le_bytes(*b"CONE")).map(|x| x.tag.as_str()), Some("CONE"));
        assert_eq!(d.by_fourcc(u32::from_le_bytes(*b"DOC2")).map(|x| x.tag.as_str()), Some("DOC2"));
        assert!(d.by_fourcc(0).is_none());
    }

    /// `SECTION:Audio{tag=TRAN;SECTION:Sound{...}}` — the exact bytes of the shipped
    /// `GameData/Track/animdefs.txt`'s only `SECTION:Sound` block (verified against the installed
    /// file 2026-09-30). `0x47D980`/`0x47D3F0` field-by-field, `COVERAGE-2026-09-26.md` §14.38.
    const AUDIO_SAMPLE: &str = "\
SECTION:Audio\n\
{\n\
\ttag=TRAN;\n\
\tSECTION:Sound\n\
\t{\n\
\t\tPatchID=833;\n\
\t\tRange=300;\n\
\t\tFadeIn=17;\n\
\t\tTrigger=0;\n\
\t\tFadeOut=0;\n\
\t\tMinRepeat=0;\n\
\t\tRandomRepeat=0;\n\
\t\tVolume=100;\t\t\t\t// Percent\n\
\t}\n\
}\n\
\n\
SECTION:ANIM\n\
{\n\
\ttag=ENDW;\n\
\tcollideType=kBox;\n\
}\n";

    #[test]
    fn the_one_shipped_sound_def() {
        let d = AnimDefs::parse(AUDIO_SAMPLE);
        // the `Audio` block does not become an `ANIM` definition — `TRAN` has no `ANIM` of its own
        assert!(d.get("TRAN").is_none());
        assert_eq!(d.get("ENDW").map(|x| x.tag.as_str()), Some("ENDW"));
        let s = d.sound_by_tag("TRAN").expect("the shipped TRAN SoundDef");
        assert_eq!(s.patch_id, 833);
        assert_eq!(s.range, 300.0);
        assert_eq!(s.fade_in, 17.0);
        assert_eq!(s.trigger, 0);
        assert_eq!(s.fade_out, 0);
        assert_eq!(s.min_repeat, 0);
        assert_eq!(s.random_repeat, 0);
        assert_eq!(s.volume, 100);
        assert_eq!(d.sound_by_fourcc(u32::from_le_bytes(*b"TRAN")).map(|s| s.patch_id), Some(833));
        assert!(d.sound_by_tag("NOPE").is_none());
        assert_eq!(d.sounds().count(), 1);
        // case-insensitive, matching `ANIM`'s own tag lookup
        assert_eq!(d.sound_by_tag("tran").map(|s| s.patch_id), Some(833));
    }
}
