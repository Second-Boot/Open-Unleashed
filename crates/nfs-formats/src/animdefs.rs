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

use std::collections::HashMap;

/// `properties=` flags of a piece. `LOOP` is bit 0 — the sampler's looping flag (0x47EB21).
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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimDef {
    /// the scene-object tag, e.g. `DOC2`, `CONE`, `RHEL`
    pub tag: String,
    pub collide_type: String,
    pub audio_type: String,
    pub mass: f32,
    pub radius: f32,
    pub ticks_per_vert_frame: Option<u32>,
    pub triggerable: bool,
    pub dynamic_light: bool,
    /// pieces with the `speed=` they were listed under
    pub pieces: Vec<(f32, Piece)>,
}

impl AnimDef {
    /// The pieces of the lowest `speed=` group — the one an object at rest plays.
    pub fn resting_pieces(&self) -> impl Iterator<Item = &Piece> {
        let lowest = self.pieces.iter().map(|(s, _)| *s).fold(f32::MAX, f32::min);
        self.pieces.iter().filter(move |(s, _)| *s == lowest).map(|(_, p)| p)
    }
}

#[derive(Debug, Clone, Default)]
pub struct AnimDefs {
    by_tag: HashMap<String, AnimDef>,
}

impl AnimDefs {
    pub fn get(&self, tag: &str) -> Option<&AnimDef> {
        self.by_tag.get(&tag.to_ascii_uppercase())
    }

    /// By the FourCC an article's `Base` record carries, little-endian.
    pub fn by_fourcc(&self, tag: u32) -> Option<&AnimDef> {
        let b = tag.to_le_bytes();
        let s: String = b.iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
        self.get(s.trim())
    }

    pub fn len(&self) -> usize {
        self.by_tag.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_tag.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &AnimDef> {
        self.by_tag.values()
    }

    pub fn parse(text: &str) -> AnimDefs {
        let mut out = AnimDefs::default();
        let mut def: Option<AnimDef> = None;
        let mut piece: Option<Piece> = None;
        let mut speed = 0.0f32;
        // depth 1 = inside the section's braces, 2 = inside a piece's
        let mut depth = 0usize;
        let mut pending: Option<&'static str> = None;
        for raw in text.lines() {
            let line = raw.split("//").next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with("SECTION:") {
                let kind = line["SECTION:".len()..].trim().to_ascii_uppercase();
                pending = match kind.as_str() {
                    "ANIM" => Some("anim"),
                    "PIECE" => Some("piece"),
                    _ => Some("other"),
                };
                continue;
            }
            if line.starts_with('{') {
                depth += 1;
                match pending.take() {
                    Some("anim") => def = Some(AnimDef::default()),
                    Some("piece") => piece = Some(Piece { playback_rate: 1.0, ..Piece::default() }),
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
                } else if let Some(d) = def.take()
                    && !d.tag.is_empty()
                {
                    out.by_tag.insert(d.tag.to_ascii_uppercase(), d);
                }
                continue;
            }
            let Some((k, v)) = line.trim_end_matches(';').split_once('=') else { continue };
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().trim_end_matches(';').trim());
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
                "speed" => speed = v.parse().unwrap_or(0.0),
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
        assert_eq!(d.len(), 3);

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
    }

    #[test]
    fn a_fourcc_finds_its_definition() {
        let d = AnimDefs::parse(SAMPLE);
        assert_eq!(d.by_fourcc(u32::from_le_bytes(*b"CONE")).map(|x| x.tag.as_str()), Some("CONE"));
        assert_eq!(d.by_fourcc(u32::from_le_bytes(*b"DOC2")).map(|x| x.tag.as_str()), Some("DOC2"));
        assert!(d.by_fourcc(0).is_none());
    }
}
