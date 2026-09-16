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

    /// Decode an article's geometry: the `Base` LOD table and every primitive with its streams
    /// resolved into plain arrays.
    pub fn article_geometry(&self, data: &[u8], art: &Article) -> Result<ArticleGeometry, FormatError> {
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
                let part = art.find(tag, idx).or_else(|| art.parts.iter().find(|p| p.is_part(tag) && p.lod() == lod)).ok_or(FormatError::UnexpectedValue {
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
            let (vertex_indices, uv_indices) = if rec.prim_type == 3 && rec.index_arrays.len() == 2 {
                let ia = rec.index_area_offset;
                let vi = bytes.get(ia..ia + n).ok_or(FormatError::Truncated { what: "CRP vertex indices", offset: pr.data_offset() + ia, needed: n })?;
                let ui = bytes.get(ia + n..ia + 2 * n).ok_or(FormatError::Truncated { what: "CRP uv indices", offset: pr.data_offset() + ia + n, needed: n })?;
                for (k, &i) in vi.iter().enumerate() {
                    if i as usize >= positions.len() {
                        return Err(FormatError::UnexpectedValue {
                            what: "CRP vertex index",
                            offset: pr.data_offset() + ia + k,
                            expected: format!("< {}", positions.len()),
                            found: i.to_string(),
                        });
                    }
                }
                (vi.to_vec(), ui.to_vec())
            } else {
                (Vec::new(), Vec::new())
            };
            primitives.push(Primitive { lod, prim_type: rec.prim_type, flags: rec.flags, material: rec.material, positions, normals, uvs, colors, vertex_indices, uv_indices });
        }
        Ok(ArticleGeometry { name, base, transform, transforms, primitives })
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
        // index_count: the 'uI' array offset equals the vertex index count; fall back to remaining/2.
        let index_count = index_arrays.iter().find(|(t, _)| t == b"uI").map(|(_, o)| *o).unwrap_or(((b.len() - p) / 2) as u32);
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
