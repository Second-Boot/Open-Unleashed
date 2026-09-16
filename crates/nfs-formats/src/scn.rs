//! `Track/<track>_*.scn` — text scene element lists (cameras, triggers, geometry, audio).
//!
//! Status: grammar CONFIRMED across all 78 files (see `RE/FORMATS.md` §6); the meaning of the
//! per-type parameter lines is only partially known, so they are kept as raw token lists.
//!
//! ```text
//! <KIND>_ELEMENT <version>
//! x y z                                   ; position
//! m00 m01 m02  m10 m11 m12  m20 m21 m22   ; 3×3 orientation (rows)      [absent for AUDIO]
//! ... type-specific parameter lines (numeric tokens) ...
//! <name line>                              ; optional trailing free text
//! (blank line)
//! ```
//! Observed shapes (token counts per line after the header):
//! CAMERA v4: 3, 9, 3, 9, 3, name ·  TRIGGER v5: 3, 9, 17, 6, 8[, dir 3]  ·  GEOM v5: 3, 9, 10, name ·
//! GEOM v4: 3, 9, 8, name  ·  AUDIO v4: 3, 12, name.

use crate::FormatError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementKind {
    Camera,
    Trigger,
    Geom,
    Audio,
    Particle,
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneElement {
    pub kind: ElementKind,
    pub version: u32,
    pub position: [f32; 3],
    /// Row-major 3×3 orientation matrix; `None` for AUDIO elements (which have no matrix line).
    pub orientation: Option<[[f32; 3]; 3]>,
    /// Remaining numeric lines, each as parsed tokens.
    pub params: Vec<Vec<f32>>,
    /// The same lines as raw tokens (large integers such as FourCC ids do not survive `f32`).
    pub params_text: Vec<Vec<String>>,
    /// Trailing non-numeric line (element name / description), if any.
    pub name: Option<String>,
    /// 1-based line number of the header line, for diagnostics.
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub elements: Vec<SceneElement>,
}

/// `<KIND>_ELEMENT <n>` — files do not always separate elements with a blank line.
fn is_header(line: &str) -> bool {
    match line.split_once(' ') {
        Some((k, v)) => k.ends_with("_ELEMENT") && v.trim().parse::<u32>().is_ok(),
        None => false,
    }
}

fn parse_floats(line: &str) -> Option<Vec<f32>> {
    line.split_whitespace().map(|t| t.parse::<f32>().ok()).collect()
}

impl Scene {
    pub fn parse(text: &str) -> Result<Self, FormatError> {
        let lines: Vec<&str> = text.lines().collect();
        let mut elements = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let l = lines[i].trim();
            if l.is_empty() {
                i += 1;
                continue;
            }
            let (kind_s, ver_s) = l.split_once(' ').ok_or_else(|| FormatError::UnexpectedValue {
                what: ".scn",
                offset: i + 1,
                expected: "<KIND>_ELEMENT <version>".into(),
                found: l.to_string(),
            })?;
            let kind = match kind_s {
                "CAMERA_ELEMENT" => ElementKind::Camera,
                "TRIGGER_ELEMENT" => ElementKind::Trigger,
                "GEOM_ELEMENT" => ElementKind::Geom,
                "AUDIO_ELEMENT" => ElementKind::Audio,
                "PARTICLE_ELEMENT" => ElementKind::Particle,
                other if other.ends_with("_ELEMENT") => ElementKind::Other(other.to_string()),
                other => {
                    return Err(FormatError::UnexpectedValue {
                        what: ".scn",
                        offset: i + 1,
                        expected: "<KIND>_ELEMENT".into(),
                        found: other.to_string(),
                    })
                }
            };
            let version: u32 = ver_s.trim().parse().map_err(|_| FormatError::UnexpectedValue {
                what: ".scn version",
                offset: i + 1,
                expected: "integer".into(),
                found: ver_s.to_string(),
            })?;
            let header_line = i + 1;
            i += 1;
            // collect the block until a blank line
            let mut block: Vec<&str> = Vec::new();
            while i < lines.len() && !lines[i].trim().is_empty() && !is_header(lines[i].trim()) {
                block.push(lines[i].trim());
                i += 1;
            }
            let pos = block.first().and_then(|l| parse_floats(l)).filter(|v| v.len() == 3).ok_or(FormatError::UnexpectedValue {
                what: ".scn position",
                offset: header_line + 1,
                expected: "3 floats".into(),
                found: block.first().unwrap_or(&"").to_string(),
            })?;
            let mut rest = &block[1..];
            let orientation = match rest.first().and_then(|l| parse_floats(l)) {
                Some(v) if v.len() == 9 && kind != ElementKind::Audio => {
                    rest = &rest[1..];
                    Some([[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
                }
                _ => None,
            };
            let mut params = Vec::new();
            let mut params_text = Vec::new();
            let mut name = None;
            for l in rest {
                match parse_floats(l) {
                    Some(v) => {
                        params.push(v);
                        params_text.push(l.split_whitespace().map(str::to_string).collect());
                    }
                    None => name = Some(l.to_string()),
                }
            }
            elements.push(SceneElement { kind, version, position: [pos[0], pos[1], pos[2]], orientation, params, params_text, name, line: header_line });
        }
        Ok(Self { elements })
    }

    /// Integer-exact access to a parameter token.
    pub fn param_u32(e: &SceneElement, line: usize, idx: usize) -> Option<u32> {
        e.params_text.get(line)?.get(idx)?.parse().ok()
    }

    pub fn of_kind<'a>(&'a self, kind: &'a ElementKind) -> impl Iterator<Item = &'a SceneElement> + 'a {
        self.elements.iter().filter(move |e| &e.kind == kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_trigger_and_geom() {
        let txt = "TRIGGER_ELEMENT 5\n-1 0.5 2\n1 0 0\t0 1 0\t0 0 1\n2 1 1.0 19.0 0.25 0 200 -1 -1 -1 -1 -1 -1 0 65535 0 0\n-2 0.5 2\t0 0.5 2\n0 0 0 0\t0 0 0 0\n\nGEOM_ELEMENT 5\n1 2 3\n1 0 0 0 1 0 0 0 1\n0 1213419078 0 0 -1 1 1 61.5 61.5 61.5\nGeometry Element\n";
        let s = Scene::parse(txt).unwrap();
        assert_eq!(s.elements.len(), 2);
        assert_eq!(s.elements[0].kind, ElementKind::Trigger);
        assert_eq!(s.elements[0].params.len(), 3);
        assert_eq!(s.elements[0].params[0].len(), 17);
        assert_eq!(s.elements[1].name.as_deref(), Some("Geometry Element"));
        assert_eq!(s.elements[1].orientation.unwrap()[1], [0.0, 1.0, 0.0]);
    }
}
