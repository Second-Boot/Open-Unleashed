//! CRP containers (`.crp`): car models (`GameData/CarModel`), track geometry (`GameData/Track`)
//! and frontend rooms (`FEData/Models`).
//!
//! Status: container framing, article part runs, vertex/uv/normal/colour streams, primitive
//! records, LOD records and material → texture references are **CONFIRMED** (see
//! `RE/notes/crp-geometry.md`, validated by exporting every article of three files with zero
//! out-of-range indices). Fields whose meaning is unknown are exposed as `unknown_*`.
//!
//! Files on disk are usually RefPack-compressed; run [`crate::refpack::maybe_decompress`] first.
//!
//! ```text
//! +0   4 bytes  magic, stored byte-reversed: "karT" => "Trak", " raC" => "Car "
//! +4   u32 LE   (article_count << 5) | 26   — the header is itself a {tag,a,b,c} container entry:
//!                a>>5 = number of index-addressed children (articles), low bits = flags (CONFIRMED,
//!                loader 0x4876E0 / bundle API 0x59F4D0, see RE/notes/loaders.md)
//! +8   u32 LE   count of the remaining level entries incl. mt/sn/sf (alps 286 = 81 + 204 + 1)
//! +12  u32 LE   1 = child table offset in 16-byte units (table at +16)
//! +16  table of 16-byte entries (entry i at file offset 16 + 16*i):
//!        [level entries]   4-char reversed tag: Arti, Desc, RMth, CmAn, SimD, SimT, "Vis ", BndP, Wndo
//!                          Arti: a = 26, b = part count, c = first part entry RELATIVE to this entry
//!                          others: a = (len << 8) | kind, c = data offset RELATIVE to this entry
//!        [materials]       2-char entries 'mt' (+ 'sn' texture file name on tracks, 'sf' embedded SHPI on cars)
//!        [article runs]    per Arti: Base, Name, then 2-char parts sorted by tag then index
//!        [data]            16-byte aligned blobs
//!      2-char part entry: u16 index | char[2] tag (reversed) | u32 a=(len<<8)|kind | u32 b=count | u32 rel_offset
//!      data = file[16 + 16*i + rel_offset .. + (a >> 8)]
//!      LOD: kind = 0x03 | lod<<3 ; vt/uv/df.index = lod ; pr.index = lod<<12 | n
//! ```

use crate::{f32_le, u16_le, u32_le, FormatError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrpKind {
    Track,
    Car,
    Other([u8; 4]),
}

/// One 16-byte table entry, with its own table index (needed to resolve relative offsets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrpEntry {
    Level { entry_index: u32, tag: [u8; 4], a: u32, b: u32, c: u32 },
    Part { entry_index: u32, index: u16, tag: [u8; 2], a: u32, b: u32, c: u32 },
}

impl CrpEntry {
    pub fn tag_str(&self) -> String {
        match self {
            CrpEntry::Level { tag, .. } => tag.iter().map(|&b| b as char).collect(),
            CrpEntry::Part { tag, .. } => tag.iter().map(|&b| b as char).collect(),
        }
    }
    pub fn entry_index(&self) -> u32 {
        match self {
            CrpEntry::Level { entry_index, .. } | CrpEntry::Part { entry_index, .. } => *entry_index,
        }
    }
    fn a(&self) -> u32 {
        match self {
            CrpEntry::Level { a, .. } | CrpEntry::Part { a, .. } => *a,
        }
    }
    fn c(&self) -> u32 {
        match self {
            CrpEntry::Level { c, .. } | CrpEntry::Part { c, .. } => *c,
        }
    }
    /// Byte length of the referenced data (`a >> 8`).
    pub fn data_len(&self) -> usize {
        (self.a() >> 8) as usize
    }
    /// Low byte of `a`: `0x03 | lod << 3` for geometry parts.
    pub fn kind_byte(&self) -> u8 {
        (self.a() & 0xFF) as u8
    }
    /// LOD level for vt/uv/df/nm/pr parts (bits 3..6 of the kind byte).
    pub fn lod(&self) -> u8 {
        (self.kind_byte() >> 3) & 0x0F
    }
    /// Absolute file offset of the referenced data.
    pub fn data_offset(&self) -> usize {
        16 + 16 * self.entry_index() as usize + self.c() as usize
    }
    pub fn part_index(&self) -> u16 {
        match self {
            CrpEntry::Part { index, .. } => *index,
            CrpEntry::Level { .. } => 0,
        }
    }
    pub fn is_part(&self, tag: &[u8; 2]) -> bool {
        matches!(self, CrpEntry::Part { tag: t, .. } if t == tag)
    }
    pub fn is_level(&self, tag: &[u8; 4]) -> bool {
        matches!(self, CrpEntry::Level { tag: t, .. } if t == tag)
    }
}

#[derive(Debug, Clone)]
pub struct Crp {
    pub kind: CrpKind,
    /// `(article_count << 5) | flags`.
    pub header_a: u32,
    /// Number of non-article level entries including material entries.
    pub header_b: u32,
    /// Child table offset in 16-byte units (1).
    pub header_c: u32,
    /// Leading 4-char-tag entries (articles and other top-level records).
    pub level: Vec<CrpEntry>,
    /// `mt` / `sn` / `sf` entries following the level table.
    pub material_entries: Vec<CrpEntry>,
    file_len: usize,
}

/// One `Arti` entry and the run of part entries it references.
#[derive(Debug, Clone)]
pub struct Article {
    pub level_index: usize,
    pub parts: Vec<CrpEntry>,
}

impl Article {
    pub fn find(&self, tag: &[u8; 2], index: u16) -> Option<&CrpEntry> {
        self.parts.iter().find(|p| p.is_part(tag) && p.part_index() == index)
    }
    pub fn level(&self, tag: &[u8; 4]) -> Option<&CrpEntry> {
        self.parts.iter().find(|p| p.is_level(tag))
    }
}

fn is_ascii_tag(b: &[u8]) -> bool {
    b.iter().all(|&c| (0x20..0x7F).contains(&c))
}

fn parse_entry(data: &[u8], idx: usize) -> Result<CrpEntry, FormatError> {
    let off = 16 + idx * 16;
    let e = data.get(off..off + 16).ok_or(FormatError::Truncated { what: "CRP entry", offset: off, needed: 16 })?;
    let a = u32_le(e, 4, "CRP entry a")?;
    let b = u32_le(e, 8, "CRP entry b")?;
    let c = u32_le(e, 12, "CRP entry c")?;
    let ent = if is_ascii_tag(&e[0..4]) {
        CrpEntry::Level { entry_index: idx as u32, tag: [e[3], e[2], e[1], e[0]], a, b, c }
    } else {
        CrpEntry::Part { entry_index: idx as u32, index: u16_le(e, 0, "CRP part index")?, tag: [e[3], e[2]], a, b, c }
    };
    if !ent.is_level(b"Arti") {
        let end = ent.data_offset() + ent.data_len();
        if end > data.len() {
            return Err(FormatError::InvalidOffset { what: "CRP entry data", offset: end, len: data.len() });
        }
    }
    Ok(ent)
}

impl Crp {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let magic = data.get(0..4).ok_or(FormatError::Truncated { what: "CRP header", offset: 0, needed: 16 })?;
        let m = [magic[3], magic[2], magic[1], magic[0]];
        let kind = match &m {
            b"Trak" => CrpKind::Track,
            b"Car " => CrpKind::Car,
            other if is_ascii_tag(other) => CrpKind::Other(*other),
            _ => {
                return Err(FormatError::UnknownMagic { what: "CRP", offset: 0, expected: b"karT / raC ".to_vec(), found: magic.to_vec() })
            }
        };
        let header_a = u32_le(data, 4, "CRP +4")?;
        let header_b = u32_le(data, 8, "CRP +8")?;
        let header_c = u32_le(data, 12, "CRP +12")?;
        let mut level = Vec::new();
        let mut idx = 0usize;
        while let Some(t) = data.get(16 + idx * 16..20 + idx * 16)
            && is_ascii_tag(t)
        {
            level.push(parse_entry(data, idx)?);
            idx += 1;
        }
        if level.is_empty() {
            return Err(FormatError::UnexpectedValue {
                what: "CRP",
                offset: 16,
                expected: "at least one 4-char-tag level entry".into(),
                found: format!("{:02x?}", &data[16..32.min(data.len())]),
            });
        }
        // Material table: 2-char entries up to the first article run.
        let first_run = level
            .iter()
            .filter_map(|e| match e {
                CrpEntry::Level { entry_index, tag, c, .. } if tag == b"Arti" => Some(*entry_index as usize + *c as usize),
                _ => None,
            })
            .min()
            .unwrap_or(idx);
        let mut material_entries = Vec::new();
        for i in idx..first_run {
            let e = parse_entry(data, i)?;
            match &e {
                CrpEntry::Part { tag, .. } if matches!(tag, b"mt" | b"sn" | b"sf") => material_entries.push(e),
                other => {
                    return Err(FormatError::UnexpectedValue {
                        what: "CRP material table",
                        offset: 16 + 16 * i,
                        expected: "mt/sn/sf entry".into(),
                        found: other.tag_str(),
                    })
                }
            }
        }
        let n_arti = level.iter().filter(|e| e.is_level(b"Arti")).count() as u32;
        if header_a >> 5 != n_arti {
            return Err(FormatError::UnexpectedValue {
                what: "CRP header article count",
                offset: 4,
                expected: format!("{n_arti} << 5 | flags"),
                found: format!("{header_a:#x}"),
            });
        }
        Ok(Self { kind, header_a, header_b, header_c, level, material_entries, file_len: data.len() })
    }

    fn check(&self, data: &[u8]) -> Result<(), FormatError> {
        if data.len() != self.file_len {
            return Err(FormatError::SizeMismatch { what: "CRP data buffer", declared: self.file_len, actual: data.len() });
        }
        Ok(())
    }

    /// Bytes referenced by any entry.
    /// `BndP` — the car's **collision body points** (`RE/notes/car-model.md` §12). Each record is
    /// `{u32 style slot, u32 required value}` followed by one `vec4` per point (`w = 1`), and the
    /// loader 0x442080 keeps a record only when `carInfo[0x56C + slot] == value`, so a car's body is
    /// the union of the records its style selects. The points are in the model frame with `y = 0` at
    /// the wheel contact plane; the original then shifts them by `−carInfo[0x3AC]`.
    pub fn bound_points(&self, data: &[u8]) -> Result<Vec<BoundPoints>, FormatError> {
        self.point_records(data, b"BndP")
    }

    /// `Wndo` — the windscreen quad, same record shape as `BndP` and the same style filter
    /// (loader 0x442200). The original draws its first four points with the `INTGLASS` texture in
    /// the cockpit view (0x43A490).
    pub fn window_points(&self, data: &[u8]) -> Result<Vec<BoundPoints>, FormatError> {
        self.point_records(data, b"Wndo")
    }

    fn point_records(&self, data: &[u8], tag: &[u8; 4]) -> Result<Vec<BoundPoints>, FormatError> {
        let mut out = Vec::new();
        for e in self.level.iter().filter(|e| e.is_level(tag)) {
            let b = self.entry_data(data, e)?;
            if b.len() < 8 {
                continue;
            }
            let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            let f_at = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            let n = (b.len() - 8) / 16;
            out.push(BoundPoints {
                style_slot: u32_at(0),
                style_value: u32_at(4),
                points: (0..n).map(|i| [f_at(8 + i * 16), f_at(12 + i * 16), f_at(16 + i * 16)]).collect(),
            });
        }
        Ok(out)
    }

    pub fn entry_data<'a>(&self, data: &'a [u8], e: &CrpEntry) -> Result<&'a [u8], FormatError> {
        let off = e.data_offset();
        data.get(off..off + e.data_len()).ok_or(FormatError::InvalidOffset { what: "CRP entry data", offset: off + e.data_len(), len: data.len() })
    }

    /// Resolve all `Arti` articles and their part runs.
    pub fn articles(&self, data: &[u8]) -> Result<Vec<Article>, FormatError> {
        self.check(data)?;
        let mut out = Vec::new();
        for (i, e) in self.level.iter().enumerate() {
            if let CrpEntry::Level { entry_index, tag, b, c, .. } = e
                && tag == b"Arti"
            {
                let start = *entry_index as usize + *c as usize;
                let mut parts = Vec::with_capacity(*b as usize);
                for j in 0..*b as usize {
                    parts.push(parse_entry(data, start + j)?);
                }
                out.push(Article { level_index: i, parts });
            }
        }
        Ok(out)
    }

    /// NUL-terminated string data of an entry (`Name`, `Desc`, `sn`).
    pub fn entry_string(&self, data: &[u8], e: &CrpEntry) -> Result<String, FormatError> {
        let b = self.entry_data(data, e)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        Ok(b[..end].iter().map(|&c| c as char).collect())
    }

    /// Materials in table order (`pr.material` indexes this list).
    pub fn materials(&self, data: &[u8]) -> Result<Vec<Material>, FormatError> {
        self.check(data)?;
        self.material_entries.iter().filter(|e| e.is_part(b"mt")).map(|e| Material::parse(self.entry_data(data, e)?, e.data_offset())).collect()
    }

    /// Name of the external SHPI texture file (`sn` entry), tracks only.
    pub fn texture_file_name(&self, data: &[u8]) -> Result<Option<String>, FormatError> {
        match self.material_entries.iter().find(|e| e.is_part(b"sn")) {
            Some(e) => Ok(Some(self.entry_string(data, e)?)),
            None => Ok(None),
        }
    }

    /// Embedded SHPI blobs (`sf` entries), cars only.
    pub fn embedded_texture_files<'a>(&self, data: &'a [u8]) -> Result<Vec<&'a [u8]>, FormatError> {
        self.material_entries.iter().filter(|e| e.is_part(b"sf")).map(|e| self.entry_data(data, e)).collect()
    }

    /// An article's `Anim` / `Anqt` animation track, if it has one (`RE/notes/crp-geometry.md` §7).
    /// `Anim`'s first dword is the frame count and `Anqt` is exactly 52 bytes a frame: a unit
    /// quaternion, a position with a trailing 1.0, a scale and two words the sampler does not read.
    pub fn article_animation(&self, data: &[u8], art: &Article) -> Option<AnimTrack> {
        let anim = art.level(b"Anim")?;
        let anqt = art.level(b"Anqt")?;
        let (ao, al) = (anim.data_offset(), anim.data_len());
        if al < 4 || ao + al > data.len() {
            return None;
        }
        let count = u32::from_le_bytes([data[ao], data[ao + 1], data[ao + 2], data[ao + 3]]) as usize;
        let (qo, ql) = (anqt.data_offset(), anqt.data_len());
        if count == 0 || ql < count * 52 || qo + ql > data.len() {
            return None;
        }
        let f = |o: usize| f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
        let frames = (0..count)
            .map(|i| {
                let b = qo + i * 52;
                AnimFrame {
                    rotation: [f(b), f(b + 4), f(b + 8), f(b + 12)],
                    position: [f(b + 16), f(b + 20), f(b + 24)],
                    scale: [f(b + 32), f(b + 36), f(b + 40)],
                }
            })
            .collect();
        Some(AnimTrack { frames })
    }

    /// Decode an article's geometry: the `Base` LOD table and every primitive with its streams
    /// resolved into plain arrays.
    pub fn article_geometry(&self, data: &[u8], art: &Article) -> Result<ArticleGeometry, FormatError> {
        self.article_geometry_frame(data, art, 0)
    }

    /// The geometry at animation frame `frame`: each primitive's `vt` stream taken from the part
    /// `frame·0x10 + ` the stream's own index (0x43E242…0x43E254, 0x43E2A4…0x43E2B4), everything
    /// else as frame 0. `None`-like failure when that frame's `vt` is missing.
    pub fn article_geometry_frame(&self, data: &[u8], art: &Article, frame: u16) -> Result<ArticleGeometry, FormatError> {
        self.check(data)?;
        let name = match art.level(b"Name") {
            Some(e) => self.entry_string(data, e)?,
            None => String::new(),
        };
        let base = match art.level(b"Base") {
            Some(e) => Some(Base::parse(self.entry_data(data, e)?, e.data_offset())?),
            None => None,
        };
        let mut transforms = Vec::new();
        for e in art.parts.iter().filter(|p| p.is_part(b"tr")) {
            let b = self.entry_data(data, e)?;
            if b.len() >= 64 {
                let mut m = [0f32; 16];
                for (i, v) in m.iter_mut().enumerate() {
                    *v = f32_le(b, i * 4, "CRP tr matrix")?;
                }
                transforms.push((e.part_index() as u8, m));
            }
        }
        let transform = transforms.first().map(|t| t.1);
        let mut primitives = Vec::new();
        for pr in art.parts.iter().filter(|p| p.is_part(b"pr")) {
            let lod = pr.lod();
            let bytes = self.entry_data(data, pr)?;
            let rec = PrimitiveRecord::parse(bytes, pr.data_offset())?;
            let stream_bytes = |kind: u16| -> Result<Option<&[u8]>, FormatError> {
                let Some(sd) = rec.streams.iter().find(|s| s.kind == kind && s.slot != 0xFFFF_FFFF) else { return Ok(None) };
                let tag: &[u8; 2] = match kind {
                    0 => b"vt",
                    1 => b"nm",
                    2 => b"uv",
                    3 => b"df",
                    _ => return Ok(None),
                };
                let idx = if tag == b"pr" { 0 } else { sd.lod };
                let idx = if kind == 0 && frame > 0 { frame * 0x10 + idx } else { idx };
                let part = art.find(tag, idx).or_else(|| if frame > 0 && kind == 0 { None } else { art.parts.iter().find(|p| p.is_part(tag) && p.lod() == lod) }).ok_or(FormatError::UnexpectedValue {
                    what: "CRP primitive stream",
                    offset: pr.data_offset(),
                    expected: format!("a '{}' part for lod {lod}", String::from_utf8_lossy(tag)),
                    found: "none".into(),
                })?;
                let pb = self.entry_data(data, part)?;
                let s = sd.byte_offset as usize;
                let e = s + sd.byte_len as usize;
                Ok(Some(pb.get(s..e).ok_or(FormatError::InvalidOffset { what: "CRP stream window", offset: part.data_offset() + e, len: part.data_offset() + pb.len() })?))
            };
            let vt = stream_bytes(0)?;
            let nm = stream_bytes(1)?;
            let uv = stream_bytes(2)?;
            let df = stream_bytes(3)?;
            let read_vec = |b: Option<&[u8]>, stride: usize, n: usize| -> Vec<[f32; 3]> {
                let Some(b) = b else { return Vec::new() };
                b.chunks_exact(stride)
                    .map(|c| {
                        let mut v = [0f32; 3];
                        for (i, x) in v.iter_mut().enumerate().take(n) {
                            *x = f32::from_le_bytes([c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]]);
                        }
                        v
                    })
                    .collect()
            };
            let positions = read_vec(vt, 16, 3);
            let normals = read_vec(nm, 16, 3);
            let uvs: Vec<[f32; 2]> = read_vec(uv, 8, 2).into_iter().map(|v| [v[0], v[1]]).collect();
            let colors: Vec<u32> = df.map(|b| b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()).unwrap_or_default();
            let n = rec.index_count as usize;
            // Any primitive kind may carry indices: `vI`+`uI` (both indexed), `vI` alone (uv and
            // colour run per corner) or neither (everything sequential).
            let ia = rec.index_area_offset;
            let (vertex_indices, uv_indices) = match rec.index_arrays.len() {
                2 => {
                    let vi = bytes.get(ia..ia + n).ok_or(FormatError::Truncated { what: "CRP vertex indices", offset: pr.data_offset() + ia, needed: n })?;
                    let ui = bytes.get(ia + n..ia + 2 * n).ok_or(FormatError::Truncated { what: "CRP uv indices", offset: pr.data_offset() + ia + n, needed: n })?;
                    (vi.to_vec(), ui.to_vec())
                }
                1 => {
                    let vi = bytes.get(ia..ia + n).ok_or(FormatError::Truncated { what: "CRP vertex indices", offset: pr.data_offset() + ia, needed: n })?;
                    (vi.to_vec(), Vec::new())
                }
                _ => (Vec::new(), Vec::new()),
            };
            for (k, &i) in vertex_indices.iter().enumerate() {
                if i as usize >= positions.len() {
                    return Err(FormatError::UnexpectedValue {
                        what: "CRP vertex index",
                        offset: pr.data_offset() + ia + k,
                        expected: format!("< {}", positions.len()),
                        found: i.to_string(),
                    });
                }
            }
            primitives.push(Primitive { lod, prim_type: rec.prim_type, flags: rec.flags, material: rec.material, positions, normals, uvs, colors, vertex_indices, uv_indices });
        }
        Ok(ArticleGeometry { name, base, transform, transforms, primitives })
    }
}

/// One frame of an article's `Anqt` rotation track (52 bytes; `RE/notes/crp-geometry.md` §7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimFrame {
    /// unit quaternion, x y z w
    pub rotation: [f32; 4],
    pub position: [f32; 3],
    pub scale: [f32; 3],
}

/// An article's animation: `Anim`'s frame count and `Anqt`'s per-frame track.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimTrack {
    pub frames: Vec<AnimFrame>,
}

impl AnimTrack {
    /// The frame pair and blend at `seconds`, by `AnimPlayer::sample` 0x470E90: a looping track
    /// wraps at `frames − 2`, a one-shot holds its last frame.
    pub fn sample(&self, seconds: f32, seconds_per_frame: f32, looping: bool) -> (usize, usize, f32) {
        let n = self.frames.len();
        if n < 2 || seconds_per_frame <= 0.0 {
            return (0, 0, 0.0);
        }
        let f = (seconds.max(0.0) / seconds_per_frame).max(0.0);
        let mut i = f as usize;
        let mut frac = f - i as f32;
        if looping {
            i %= n - 2;
        } else if i >= n - 2 {
            i = n - 1;
            frac = 0.0;
        }
        (i, (i + 1).min(n - 1), frac)
    }
}

/// `Base` record of an article (`RE/notes/crp-geometry.md` §2.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Base {
    /// Bit 15 (0x8000) = library object (instanced by `.scn` GEOM elements, not placed itself).
    /// CONFIRMED: all 70 skidpad library articles have it, world-placed signs on alps (0x601) do not.
    pub flags: u32,
    /// Scene-object tag at +68 (e.g. "CONE", "ARW1", "SIGN", "FNSH"); `.scn` GEOM_ELEMENT ids are
    /// this FourCC read as a little-endian u32. Zero when absent. CONFIRMED.
    pub tag: [u8; 4],
    /// `Base+72` — the article's **translation** channel index, signed; `-1` (0xFF) for none.
    /// `R3DCarLOD::draw` reads it at 0x43C945 and offsets `object + 0x6D0 + 12n`.
    pub move_channel: i8,
    /// `Base+75` — the article's **rotation** channel index, `0` for none. 0x43C98A
    /// (`mov al, byte ptr [edx+7]` over `[part+0x44]`, the `Base` record) picks it, and
    /// 0x43C9B0 fetches `0x443F10(n)` = `object + 0x2DC + 12n`: three floats that are rotations
    /// **in turns** about X, Y and Z (0x43C9C0 / 0x43C9D7 / 0x43C9F8 each multiply by
    /// `[0x5B25CC]` = 360.0 before 0x539BD0's axis-angle build). Channels 6 and 7 are special:
    /// 0x43CA5A rotates about the `.tpg` `dooraxis` by `chan[1]` instead, x negated for 7.
    pub rot_channel: u8,
    /// `Base+77` — the **model animation** the article's vertices follow, 0 for none
    /// (`[[part+0x44] + 9]`, 0x43E195 / 0x43C541). Animation 1 is the convertible's roof, the one
    /// the front end's 0x4D1990 drives; its frames are `vt` parts at `frame·0x10 + stream index`.
    pub anim: u8,
    /// `Base+76` — the frame count those `vt` parts run to.
    pub anim_frames: u8,
    pub lod_count: u32,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    pub centre: [f32; 3],
    pub radius: f32,
    pub lods: Vec<LodRecord>,
}

impl Base {
    pub fn is_library(&self) -> bool {
        self.flags & 0x8000 != 0
    }
    pub fn tag_u32(&self) -> u32 {
        u32::from_le_bytes(self.tag)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LodRecord {
    pub first_pr: u16,
    pub last_pr: u16,
    pub lod: u16,
    /// Switch distance (40/60/400/10000; 0xFFFF sentinel).
    pub distance: u16,
}

impl Base {
    pub fn parse(b: &[u8], off: usize) -> Result<Self, FormatError> {
        if b.len() < 100 {
            return Err(FormatError::Truncated { what: "CRP Base", offset: off, needed: 100 - b.len() });
        }
        let f = |o: usize| f32_le(b, o, "CRP Base");
        let lod_count = u32_le(b, 12, "CRP Base lod_count")?;
        let mut lods = Vec::new();
        for i in 0..lod_count as usize {
            let o = 100 + i * 12;
            if o + 12 > b.len() {
                break;
            }
            lods.push(LodRecord { first_pr: u16_le(b, o + 4, "lod")?, last_pr: u16_le(b, o + 6, "lod")?, lod: u16_le(b, o + 8, "lod")?, distance: u16_le(b, o + 10, "lod")? });
        }
        Ok(Self {
            flags: u32_le(b, 0, "CRP Base flags")?,
            tag: [b[68], b[69], b[70], b[71]],
            move_channel: if b.len() > 72 { b[72] as i8 } else { -1 },
            rot_channel: if b.len() > 75 { b[75] } else { 0 },
            anim: if b.len() > 77 { b[77] } else { 0 },
            anim_frames: if b.len() > 76 { b[76] } else { 0 },
            lod_count,
            bbox_min: [f(20)?, f(24)?, f(28)?],
            bbox_max: [f(32)?, f(36)?, f(40)?],
            centre: [f(44)?, f(48)?, f(52)?],
            radius: f(60)?,
            lods,
        })
    }
}

/// Material (`mt`, 76+ bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub flags: u32,
    pub class: u32,
    /// e.g. "Opaque", "Normal", "NormMatState", "FakeSort", "FullAmbient"; cars "CarExtEnv"...
    pub render_method: String,
    /// SHPI entry FourCC in the file named by `sn` (tracks); garbage/empty when untextured.
    pub texture: [u8; 4],
    pub unknown_20: u32,
    /// The `R3DLightLayer` (0x8104) parameters a car material carries — `RE/notes/lighting.md` §6.
    pub light: Option<LayerLight>,
    /// `R3DSpecularTextureLayer`'s **reflection strength** — the first of the four 4-byte
    /// parameters at data offsets 0x18…0x24 (record +284…+296), which 0x445520 buckets at 0.05 and
    /// 0.95 to pick one of the three environment textures (`RE/notes/lighting.md` §3.1). Every
    /// shipped `mt` of every car and track keeps the four equal — 0.0, 0.5 or 1.0, and non-zero
    /// only on `CarExtEnv` and `CarWindow` — so which one the layer reads cannot be told apart and
    /// does not matter.
    pub reflection: f32,
}

/// `R3DLightLayer`'s parameter block `{f32 p0, f32 p1, u32 slot}` (read at 0x446600, 0x446886 and
/// 0x4465D8): the light block's `slot` scaled by `p0`, plus its `slot + 4` partner by `p1` along
/// the light direction. A car `mt` keeps it as its last three dwords, `+300…+311`, the entry of
/// size 0xC at data offset 0x28 that the record's extension table names at `+248` / `+252`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerLight {
    pub p0: f32,
    pub p1: f32,
    pub slot: u32,
}

/// 0x445520's buckets: under `[0x5B38B0]` = 0.05 the first environment texture, under
/// `[0x5B38B4]` = 0.95 the second, else the third (`RE/notes/lighting.md` §3.1).
pub fn env_variant(reflection: f32) -> usize {
    if reflection < 0.05 {
        0
    } else if reflection < 0.95 {
        1
    } else {
        2
    }
}

impl Material {
    pub fn parse(b: &[u8], off: usize) -> Result<Self, FormatError> {
        if b.len() < 76 {
            return Err(FormatError::Truncated { what: "CRP material", offset: off, needed: 76 - b.len() });
        }
        let name_end = b[16..32].iter().position(|&c| c == 0).unwrap_or(16);
        Ok(Self {
            flags: u32_le(b, 0, "mt flags")?,
            class: u32_le(b, 4, "mt class")?,
            render_method: b[16..16 + name_end].iter().map(|&c| c as char).collect(),
            texture: [b[40], b[41], b[42], b[43]],
            unknown_20: u32_le(b, 32, "mt +32")?,
            light: (b.len() >= 312 && u32_le(b, 248, "mt +248")? == 0x28 && u32_le(b, 252, "mt +252")? == 0xC).then(|| LayerLight {
                p0: f32::from_bits(u32::from_le_bytes([b[300], b[301], b[302], b[303]])),
                p1: f32::from_bits(u32::from_le_bytes([b[304], b[305], b[306], b[307]])),
                slot: u32::from_le_bytes([b[308], b[309], b[310], b[311]]),
            }),
            reflection: if b.len() >= 288 { f32::from_bits(u32::from_le_bytes([b[284], b[285], b[286], b[287]])) } else { 0.0 },
        })
    }
    pub fn texture_name(&self) -> String {
        self.texture.iter().map(|&c| c as char).collect()
    }
    /// Cars: `+40` is a 0-based texture-page index (`.tpg` `[tpage(N+1).details]`). CONFIRMED.
    pub fn page_index(&self) -> u32 {
        u32::from_le_bytes(self.texture)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamDesc {
    pub slot: u32,
    pub byte_offset: u32,
    pub byte_len: u16,
    /// 0 = vt, 1 = nm, 2 = uv, 3 = df, 0x246E = `$n`.
    pub kind: u16,
    pub lod: u16,
    pub fmt: u16,
}

/// Parsed `pr` header (`RE/notes/crp-geometry.md` §4).
#[derive(Debug, Clone, PartialEq)]
pub struct PrimitiveRecord {
    pub prim_type: u16,
    pub flags: u16,
    pub material: u16,
    pub sphere: [f32; 4],
    pub streams: Vec<StreamDesc>,
    /// (tag, byte offset in index area)
    pub index_arrays: Vec<([u8; 2], u32)>,
    pub index_area_offset: usize,
    pub index_count: u32,
}

impl PrimitiveRecord {
    pub fn parse(b: &[u8], off: usize) -> Result<Self, FormatError> {
        if b.len() < 48 {
            return Err(FormatError::Truncated { what: "CRP pr", offset: off, needed: 48 - b.len() });
        }
        let n_streams = u32_le(b, 40, "pr n_streams")? as usize;
        let n_ia = u32_le(b, 44, "pr n_index_arrays")? as usize;
        let mut streams = Vec::with_capacity(n_streams);
        let mut p = 48;
        for _ in 0..n_streams {
            streams.push(StreamDesc {
                slot: u32_le(b, p, "pr stream slot")?,
                byte_offset: u32_le(b, p + 4, "pr stream offset")?,
                byte_len: u16_le(b, p + 8, "pr stream len")?,
                kind: u16_le(b, p + 10, "pr stream kind")?,
                lod: u16_le(b, p + 12, "pr stream lod")?,
                fmt: u16_le(b, p + 14, "pr stream fmt")?,
            });
            p += 16;
        }
        let mut index_arrays = Vec::with_capacity(n_ia);
        for _ in 0..n_ia {
            let t = b.get(p + 2..p + 4).ok_or(FormatError::Truncated { what: "pr index array", offset: off + p, needed: 8 })?;
            index_arrays.push(([t[1], t[0]], u32_le(b, p + 4, "pr index array offset")?));
            p += 8;
        }
        // With the usual `vI`+`uI` pair the `uI` offset is the index count (the two arrays are equally
        // long). Some primitives carry **only** `vI`, and then the whole remaining index area is that
        // one array, with the uv and colour streams running per corner instead of being indexed.
        // the tags are stored reversed in the file, so the uv array reads back as `Iu`
        let index_count = match index_arrays.iter().find(|(t, _)| t == b"Iu") {
            Some((_, o)) => *o,
            None if index_arrays.len() == 1 => (b.len() - p) as u32,
            None => 0,
        };
        Ok(Self {
            prim_type: u16_le(b, 0, "pr type")?,
            flags: u16_le(b, 2, "pr flags")?,
            material: u16_le(b, 4, "pr material")?,
            sphere: [f32_le(b, 8, "pr")?, f32_le(b, 12, "pr")?, f32_le(b, 16, "pr")?, f32_le(b, 20, "pr")?],
            streams,
            index_arrays,
            index_area_offset: p,
            index_count,
        })
    }
}

/// One `BndP` record: the collision body points a style selects.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundPoints {
    /// Index into the car-info style array at `+0x56C`.
    pub style_slot: u32,
    /// The value that slot must hold for these points to be part of the body.
    pub style_value: u32,
    /// Model-frame points, `y = 0` at the wheel contact plane.
    pub points: Vec<[f32; 3]>,
}

/// A primitive with its stream windows resolved. Indices are relative to `positions`/`uvs`.
/// Triangle winding in the file is clockwise seen from +Y (D3D); `colors` has one RGBA u32 per
/// index (per corner), not per vertex.
#[derive(Debug, Clone, PartialEq)]
pub struct Primitive {
    pub lod: u8,
    pub prim_type: u16,
    pub flags: u16,
    pub material: u16,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<u32>,
    pub vertex_indices: Vec<u8>,
    pub uv_indices: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArticleGeometry {
    pub name: String,
    pub base: Option<Base>,
    /// First `tr` matrix (4×4 row-major, translation in row 3) — placement of tracks' placed objects.
    pub transform: Option<[f32; 16]>,
    /// All `tr` parts by index; cars carry one per LOD (`tr#lod`), see RE/notes/car-model.md.
    pub transforms: Vec<(u8, [f32; 16])>,
    pub primitives: Vec<Primitive>,
}

impl ArticleGeometry {
    /// `tr` for a given LOD (exact index, else the first one).
    pub fn transform_for_lod(&self, lod: u8) -> Option<[f32; 16]> {
        self.transforms.iter().find(|(i, _)| *i == lod).or(self.transforms.first()).map(|t| t.1)
    }
}

impl Crp {
    /// Article count declared in the header (`+4 >> 5`).
    pub fn article_count(&self) -> u32 {
        self.header_a >> 5
    }
}
