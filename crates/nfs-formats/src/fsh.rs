//! SHPI ("FSH") texture containers.
//!
//! Status (see `RE/FORMATS.md` for evidence):
//! - Container layout: **CONFIRMED** (`CarModel/356_1D.fsh`, decompressed `Track/alps.fsh`).
//! - Entry code `0x7B` = 8-bit indexed + `0x2A` 32-bit palette attachment: **CONFIRMED by size**.
//! - Entry code `0x7D` = 32-bit ARGB: **HIGH CONFIDENCE by size** (256×256 entries spaced
//!   0x40000 + mip chain apart); channel order is a HYPOTHESIS until rendered.
//! - Codes `0x78`, `0x7E`, `0x6D` = 2 bytes/pixel, `0x7F` = 3 bytes/pixel: **CONFIRMED by block size**
//!   across all 566 SHPI files (channel layouts RGB565 / ARGB1555 / ARGB4444 / BGR are HYPOTHESES
//!   until rendered). Bit 7 of the code (`0xF8`, `0xFB`, `0xED`) marks **RefPack-compressed pixel
//!   data** (CONFIRMED: decompressed size = w·h·bpp). `misc[3] >> 12` = number of mip levels
//!   **after** the base (0x8000 for 256², 0x7000 for 128²: the chain runs down to 1×1, and the
//!   pixel block is the base plus that many quarter-size levels, padded to 16 — `mipchain`
//!   example; the game's own reader 0x44D710 sums the levels the same way and decrements the
//!   nibble when it drops the top level. *Corrected 2026-09-26: this said "including the base".*) Palette attachments: `0x2A` 32-bit, `0x29`/`0x2D` 16-bit. `0x6F` = text comment, `0x7C` = hot-spot
//!   table.
//! - Palette attachment `0x29` = RGB565, `0x2D` = ARGB1555: **CONFIRMED 2026-09-29** by reading the
//!   blitter's palette-converter dispatch `0x5346D0` (`RE/notes/car-model.md` ~589) and its two
//!   16-bit targets: `0x29 → 0x570820`, `0x2D → 0x570780` (`0x24`'s sibling, `0x571160`, uses an
//!   unrelated `imul`-based 6-bit→8-bit rounding expansion — contrast, not this format). Both
//!   0x570820 and 0x570780 read each raw `u16` little-endian and, unless it equals the colour-key
//!   word at `[0x6A6420]` (never written anywhere in `.text`/`PACODE` in the shipped exe, so a
//!   fixed **0** — this is what makes the check equivalent to "raw value is exactly 0x0000"),
//!   expand it with the exact shift/OR bit-replication `rgb565`/`argb1555` below already used
//!   (`r << 3 | r >> 2` etc. — no gamma, no LUT) and set alpha 255 (opaque); on a match they write
//!   an all-zero dword (fully transparent). For `0x2D` (ARGB1555) this colour-key check changes
//!   nothing already-correct — `0x0000` already has its own alpha bit (bit 15) clear, which
//!   `argb1555` already treats as transparent — but for `0x29` (RGB565, which has no alpha bit of
//!   its own) it is the *only* source of transparency, and matters a lot: scanning every shipped
//!   `FEData/Art/*.fsh`'s `0x29` palettes, most entries used by name-indexed pictures (e.g.
//!   `ScreenFullOval.lay`'s 45 `FullOval.fsh` tiles, `shapeIndex` 0..44) carry 60-255 of their 256
//!   slots as raw `0x0000` — a tiled-oval mask needs most of each square tile transparent outside
//!   the oval curve. The port previously had no such rule (every `0x29` entry decoded opaque),
//!   which is now fixed below.
//! - **Investigated and NOT ported (2026-09-29):** a hypothesised 32→16-bit truncation on the FE
//!   picture builder's "hardware DEFAULT" texture format (`[0x5DE65C]`/`[0x5DE65D]`, set by
//!   `nfs5.exe`'s `0x534360` via a real driver `THRASH_lockwindow` probe, classified by `0x556880`)
//!   was traced end to end across both driver DLLs on disk (`dx7z.dll`, `opengl3z.dll`) — see
//!   `RE/notes/thrash.md` §11 / `RE/notes/layouts.md` "16-bit texture-format lead, closed". On
//!   both drivers' actual shipped configuration it resolves to 32-bit/no-truncation, not
//!   4444/1555/565, so no truncate-then-re-expand step was added here; the `carcompare`/
//!   `chronicle` background brightness gap that motivated the search remains open, root cause
//!   elsewhere.
//!
//! ```text
//! +0   "SHPI"
//! +4   u32 LE  file size
//! +8   u32 LE  entry count
//! +12  4 bytes directory id ("GIMX" observed)
//! +16  entries: { 4-byte name, u32 LE offset }
//!
//! entry block:
//! +0   u8  code
//! +1   u24 LE size of this block (0 = last block of the entry)
//! +4   u16 LE width, u16 LE height
//! +8   u16 LE misc[4]
//! +16  pixel data; further blocks (e.g. palette, code 0x2A) follow at +size
//! ```

use crate::{u16_le, u32_le, FormatError};

/// The file magic.
pub const MAGIC: &[u8; 4] = b"SHPI";

/// `FshBlock::code` values (see the module doc for the evidence behind each).
pub mod block_code {
    // image formats (before the compressed bit is applied)
    /// 8-bit indexed, palette in a following block
    pub const INDEXED8: u8 = 0x7B;
    /// 32 bits per pixel
    pub const ARGB8888: u8 = 0x7D;
    pub const RGB565: u8 = 0x78;
    pub const ARGB1555: u8 = 0x7E;
    pub const ARGB4444: u8 = 0x6D;
    /// 24 bits per pixel
    pub const BGR888: u8 = 0x7F;
    // attachments
    /// 32-bit palette
    pub const PALETTE_ARGB8888: u8 = 0x2A;
    /// 16-bit RGB565 palette
    pub const PALETTE_RGB565: u8 = 0x29;
    /// 16-bit ARGB1555 palette
    pub const PALETTE_ARGB1555: u8 = 0x2D;
    /// text comment
    pub const TEXT: u8 = 0x6F;
    /// hot-spot table
    pub const HOT_SPOTS: u8 = 0x7C;
    /// bit 7 of an image code: the pixel data is a RefPack stream
    pub const COMPRESSED_BIT: u8 = 0x80;
    /// the code without [`COMPRESSED_BIT`]
    pub const FORMAT_MASK: u8 = 0x7F;
    /// the one code with bit 7 set that is not a compressed block
    pub const NOT_COMPRESSED: u8 = 0xFF;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8 bits per pixel, palette in a following `0x2A` block. Code `0x7B`.
    Indexed8,
    /// 32 bits per pixel. Code `0x7D`.
    Argb8888,
    /// 16 bits per pixel RGB565. Code `0x78` (HYPOTHESIS).
    Rgb565,
    /// 16 bits per pixel ARGB1555. Code `0x7E` (HYPOTHESIS).
    Argb1555,
    /// 16 bits per pixel ARGB4444. Code `0x6D` (HYPOTHESIS).
    Argb4444,
    /// 24 bits per pixel. Code `0x7F` (channel order HYPOTHESIS: B,G,R).
    Bgr888,
}

impl PixelFormat {
    pub fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            block_code::INDEXED8 => Self::Indexed8,
            block_code::ARGB8888 => Self::Argb8888,
            block_code::RGB565 => Self::Rgb565,
            block_code::ARGB1555 => Self::Argb1555,
            block_code::ARGB4444 => Self::Argb4444,
            block_code::BGR888 => Self::Bgr888,
            _ => return None,
        })
    }
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Indexed8 => 1,
            Self::Argb8888 => 4,
            Self::Rgb565 | Self::Argb1555 | Self::Argb4444 => 2,
            Self::Bgr888 => 3,
        }
    }
}

impl FshBlock {
    /// Image format code with the "compressed" bit removed.
    pub fn format_code(&self) -> u8 {
        self.code & block_code::FORMAT_MASK
    }
    /// True when the pixel payload is a RefPack stream (bit 7 of the code).
    pub fn is_compressed(&self) -> bool {
        self.code & block_code::COMPRESSED_BIT != 0 && self.code != block_code::NOT_COMPRESSED
    }
    /// Number of mip levels in the file, the base included: `misc[3] >> 12` counts the levels
    /// **after** the base (0x44D710), so 0 ⇒ base only.
    pub fn mip_levels(&self) -> u32 {
        (self.misc[3] >> 12) as u32 + 1
    }
}

#[derive(Debug, Clone)]
pub struct FshBlock {
    pub offset: usize,
    pub code: u8,
    pub size: u32,
    pub width: u16,
    pub height: u16,
    pub misc: [u16; 4],
}

#[derive(Debug, Clone)]
pub struct FshEntry {
    pub name: String,
    pub offset: u32,
    /// The first block (the image itself) followed by any attachments.
    pub blocks: Vec<FshBlock>,
}

#[derive(Debug, Clone)]
pub struct Fsh {
    pub declared_size: u32,
    pub directory_id: [u8; 4],
    pub entries: Vec<FshEntry>,
    /// Every entry's offset, ascending (duplicates kept): [`Fsh::image_block_len`] finds "the next
    /// entry after this one" by binary search instead of scanning the directory per call.
    sorted_offsets: Vec<u32>,
}

/// A decoded image, always expanded to RGBA8 (row-major, top-left origin).
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub source_format: PixelFormat,
}

impl Fsh {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        if data.get(0..4) != Some(MAGIC) {
            return Err(FormatError::UnknownMagic {
                what: "SHPI",
                offset: 0,
                expected: MAGIC.to_vec(),
                found: data.iter().take(4).copied().collect(),
            });
        }
        let declared_size = u32_le(data, 4, "SHPI size")?;
        let count = u32_le(data, 8, "SHPI count")? as usize;
        let directory_id: [u8; 4] = data
            .get(12..16)
            .and_then(|b| b.try_into().ok())
            .ok_or(FormatError::Truncated { what: "SHPI directory id", offset: 12, needed: 4 })?;
        let mut entries = Vec::with_capacity(crate::cap_hint(count, data.len(), 8));
        for i in 0..count {
            let p = 16 + i * 8;
            let name_b = data.get(p..p + 4).ok_or(FormatError::Truncated { what: "SHPI directory", offset: p, needed: 8 })?;
            let name = name_b.iter().map(|&b| b as char).collect();
            let offset = u32_le(data, p + 4, "SHPI entry offset")?;
            if offset as usize + 16 > data.len() {
                return Err(FormatError::InvalidOffset { what: "SHPI entry", offset: offset as usize, len: data.len() });
            }
            let blocks = parse_blocks(data, offset as usize)?;
            entries.push(FshEntry { name, offset, blocks });
        }
        let mut sorted_offsets: Vec<u32> = entries.iter().map(|e| e.offset).collect();
        sorted_offsets.sort_unstable();
        Ok(Self { declared_size, directory_id, entries, sorted_offsets })
    }

    /// Byte length of the image block for `entry`, using the next entry's offset (or file end)
    /// when the block's own size field is 0.
    pub fn image_block_len(&self, data_len: usize, idx: usize) -> usize {
        let e = &self.entries[idx];
        let first = &e.blocks[0];
        if first.size != 0 {
            return first.size as usize;
        }
        // the smallest entry offset above this one's, else the end of the file
        let above = self.sorted_offsets.partition_point(|&o| o <= e.offset);
        let next = self.sorted_offsets.get(above).map_or(data_len, |&o| o as usize);
        next - e.offset as usize
    }

    /// The `0x6F` "text comment" attachment trailing an entry's image/palette blocks, if it has
    /// one — the chronicle timeline's caption above a picture cell, `RE/notes/controls.md`
    /// §4.118/§4.126/§4.173. Confirmed 2026-09-29 by hand-parsing `FEData/Chron/icons356.fsh`'s
    /// directory byte for byte: `pE001a`/`pE003a`'s two `"48p "`/`"50p "` entries carry the
    /// literal ASCII `"356 ROADSTER NO1"` / `"356 Roadster No1"` and `"356"` right after their
    /// `0x7B` image and `0x29` palette blocks, matching `chronicle.lay`'s on-screen caption at
    /// 1948/1950 (`cmp_chronicle.jpg`) exactly.
    ///
    /// A `0x6F` block does **not** follow the ordinary 16-byte `code + size(u24) + width(u16) +
    /// height(u16) + misc(4×u16)` header that every image/palette block uses: the 3-byte `size`
    /// field is always zero (which is why the generic block walk in [`parse_blocks`] correctly
    /// treats it as the entry's last block — it always is, in every shipped file), and the field
    /// at `+4` that an image block would read as `width` is instead the **text length**, with
    /// that many bytes — null-padded — starting at `+8` (`FshBlock::offset + 8`). This reads that
    /// length back out of the already-parsed [`FshBlock::width`] rather than reparsing anything.
    pub fn entry_caption(&self, data: &[u8], idx: usize) -> Option<String> {
        let e = self.entries.get(idx)?;
        let b = e.blocks.iter().find(|b| b.code == block_code::TEXT)?;
        let len = b.width as usize;
        let start = b.offset + 8;
        let raw = data.get(start..start + len)?;
        let text: String = raw.iter().take_while(|&&c| c != 0).map(|&c| crate::locale::cp1252(c)).collect();
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    /// Decode the base mip level of entry `idx` to RGBA8.
    pub fn decode(&self, data: &[u8], idx: usize) -> Result<DecodedImage, FormatError> {
        self.decode_mip(data, idx, 0)
    }

    /// Decode mip `level` (0 = base) of entry `idx` to RGBA8.
    pub fn decode_mip(&self, data: &[u8], idx: usize, level: u32) -> Result<DecodedImage, FormatError> {
        self.decode_level(data, idx, level, &mut None)
    }

    /// Decode mip levels `0..levels` of entry `idx` to RGBA8. The same images as `levels` calls of
    /// [`Self::decode_mip`] (and the same first error), but a RefPack-compressed chain is
    /// decompressed once instead of once per level.
    pub fn decode_levels(&self, data: &[u8], idx: usize, levels: u32) -> Result<Vec<DecodedImage>, FormatError> {
        let mut payload = None;
        (0..levels).map(|level| self.decode_level(data, idx, level, &mut payload)).collect()
    }

    /// `decode_mip` with the entry's raw payload (inline or decompressed) kept in `payload`
    /// between calls: filled by the first level that gets that far.
    fn decode_level<'a>(&self, data: &'a [u8], idx: usize, level: u32, payload: &mut Option<std::borrow::Cow<'a, [u8]>>) -> Result<DecodedImage, FormatError> {
        let e = self.entries.get(idx).ok_or(FormatError::UnexpectedValue {
            what: "SHPI entry index",
            offset: 0,
            expected: format!("< {}", self.entries.len()),
            found: idx.to_string(),
        })?;
        let img = e.blocks.first().ok_or(FormatError::Truncated { what: "SHPI entry blocks", offset: e.offset as usize, needed: 16 })?;
        let fmt = PixelFormat::from_code(img.format_code()).ok_or(FormatError::UnknownRecordType {
            what: "SHPI pixel format",
            offset: img.offset,
            code: img.code as u32,
        })?;
        if level >= img.mip_levels() {
            return Err(FormatError::UnexpectedValue {
                what: "SHPI mip level",
                offset: img.offset,
                expected: format!("< {}", img.mip_levels()),
                found: level.to_string(),
            });
        }
        let bpp = fmt.bytes_per_pixel();
        let (bw, bh) = (img.width as usize, img.height as usize);
        // Raw payload: either inline or a RefPack stream covering the whole mip chain.
        let px_off = img.offset + 16;
        if payload.is_none() {
            *payload = Some(if img.is_compressed() {
                let block_len = self.image_block_len(data.len(), idx);
                let comp = data.get(px_off..img.offset + block_len).ok_or(FormatError::Truncated {
                    what: "SHPI compressed pixel data",
                    offset: px_off,
                    needed: block_len,
                })?;
                std::borrow::Cow::Owned(crate::refpack::decompress(comp)?)
            } else {
                std::borrow::Cow::Borrowed(data.get(px_off..).ok_or(FormatError::Truncated { what: "SHPI pixel data", offset: px_off, needed: 1 })?)
            });
        }
        let payload: &[u8] = payload.as_deref().unwrap_or_default();
        // Skip preceding mip levels.
        let mut start = 0usize;
        let (mut w, mut h) = (bw, bh);
        for _ in 0..level {
            start += w * h * bpp;
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        let npix = w * h;
        let px = payload.get(start..start + npix * bpp).ok_or(FormatError::Truncated {
            what: "SHPI pixel data",
            offset: px_off + start,
            needed: npix * bpp,
        })?;
        let mut rgba = vec![0u8; npix * 4];
        match fmt {
            PixelFormat::Indexed8 => {
                let pal = e.blocks.iter().find(|b| matches!(b.code, block_code::PALETTE_ARGB8888 | block_code::PALETTE_RGB565 | block_code::PALETTE_ARGB1555)).ok_or(FormatError::UnexpectedValue {
                    what: "SHPI indexed image",
                    offset: img.offset,
                    expected: "a 0x2A/0x29/0x2D palette block".into(),
                    found: format!("blocks {:?}", e.blocks.iter().map(|b| b.code).collect::<Vec<_>>()),
                })?;
                let ncol = pal.width as usize * pal.height.max(1) as usize;
                let pal_off = pal.offset + 16;
                let entry_size = if pal.code == block_code::PALETTE_ARGB8888 { 4 } else { 2 };
                let pal_bytes = data.get(pal_off..pal_off + ncol * entry_size).ok_or(FormatError::Truncated {
                    what: "SHPI palette",
                    offset: pal_off,
                    needed: ncol * entry_size,
                })?;
                if ncol == 0 && !px.is_empty() {
                    return Err(FormatError::UnexpectedValue {
                        what: "SHPI indexed image",
                        offset: pal.offset,
                        expected: "a palette with at least one colour".into(),
                        found: "0 colours".into(),
                    });
                }
                let lookup = |ix: u8| -> [u8; 4] {
                    let c = &pal_bytes[(ix as usize).min(ncol - 1) * entry_size..][..entry_size];
                    match pal.code {
                        // Stored as B,G,R,A (little-endian ARGB). CONFIRMED visually on car textures.
                        block_code::PALETTE_ARGB8888 => [c[2], c[1], c[0], c[3]],
                        // CONFIRMED (0x570820, see the module doc above): RGB565, transparent
                        // (0,0,0,0) when the raw u16 is the fixed colour-key 0x0000.
                        block_code::PALETTE_RGB565 => {
                            let raw = u16::from_le_bytes([c[0], c[1]]);
                            if raw == PALETTE_565_COLORKEY { [0, 0, 0, 0] } else { rgb565(raw) }
                        }
                        // CONFIRMED (0x570780): ARGB1555; its own colour-key check against the
                        // same fixed 0x0000 changes nothing here since alpha's own bit (15) is
                        // already clear on 0x0000.
                        _ => argb1555(u16::from_le_bytes([c[0], c[1]])),
                    }
                };
                if px.len() > 256 {
                    // more texels than palette slots: convert each slot once
                    let mut lut = [[0u8; 4]; 256];
                    for (ix, slot) in lut.iter_mut().enumerate() {
                        *slot = lookup(ix as u8);
                    }
                    for (&ix, out) in px.iter().zip(rgba.chunks_exact_mut(4)) {
                        out.copy_from_slice(&lut[ix as usize]);
                    }
                } else {
                    for (&ix, out) in px.iter().zip(rgba.chunks_exact_mut(4)) {
                        out.copy_from_slice(&lookup(ix));
                    }
                }
            }
            PixelFormat::Argb8888 => {
                for (c, out) in px.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
                    out.copy_from_slice(&[c[2], c[1], c[0], c[3]]);
                }
            }
            PixelFormat::Bgr888 => {
                for (c, out) in px.chunks_exact(3).zip(rgba.chunks_exact_mut(4)) {
                    out.copy_from_slice(&[c[2], c[1], c[0], 255]);
                }
            }
            PixelFormat::Rgb565 => {
                for (c, out) in px.chunks_exact(2).zip(rgba.chunks_exact_mut(4)) {
                    out.copy_from_slice(&rgb565(u16::from_le_bytes([c[0], c[1]])));
                }
            }
            PixelFormat::Argb1555 => {
                for (c, out) in px.chunks_exact(2).zip(rgba.chunks_exact_mut(4)) {
                    out.copy_from_slice(&argb1555(u16::from_le_bytes([c[0], c[1]])));
                }
            }
            PixelFormat::Argb4444 => {
                for (c, out) in px.chunks_exact(2).zip(rgba.chunks_exact_mut(4)) {
                    let v = u16::from_le_bytes([c[0], c[1]]);
                    let a = ((v >> 12) & 0xF) as u8;
                    let r = ((v >> 8) & 0xF) as u8;
                    let g = ((v >> 4) & 0xF) as u8;
                    let b = (v & 0xF) as u8;
                    out.copy_from_slice(&[r * 17, g * 17, b * 17, a * 17]);
                }
            }
        }
        Ok(DecodedImage { width: w as u32, height: h as u32, rgba, source_format: fmt })
    }
}

/// The 16-bit palette converters' (0x570820/0x570780) fixed colour-key word, read once from
/// `[0x6A6420]` and never written anywhere in `.text`/`PACODE` in the shipped exe — CONFIRMED 0.
const PALETTE_565_COLORKEY: u16 = 0x0000;

fn rgb565(v: u16) -> [u8; 4] {
    let r = ((v >> 11) & 0x1F) as u8;
    let g = ((v >> 5) & 0x3F) as u8;
    let b = (v & 0x1F) as u8;
    [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2, 255]
}

fn argb1555(v: u16) -> [u8; 4] {
    let a = if v & 0x8000 != 0 { 255 } else { 0 };
    let r = ((v >> 10) & 0x1F) as u8;
    let g = ((v >> 5) & 0x1F) as u8;
    let b = (v & 0x1F) as u8;
    [r << 3 | r >> 2, g << 3 | g >> 2, b << 3 | b >> 2, a]
}

/// The longest block chain an entry may have (image, palette, text, … — the shipped ones have at
/// most three).
const MAX_BLOCKS: usize = 16;

fn parse_blocks(data: &[u8], mut off: usize) -> Result<Vec<FshBlock>, FormatError> {
    let mut blocks = Vec::new();
    loop {
        let hdr = data.get(off..off + 16).ok_or(FormatError::Truncated { what: "SHPI block header", offset: off, needed: 16 })?;
        let code = hdr[0];
        let size = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], 0]);
        let width = u16_le(data, off + 4, "SHPI block width")?;
        let height = u16_le(data, off + 6, "SHPI block height")?;
        let misc = [
            u16_le(data, off + 8, "SHPI misc")?,
            u16_le(data, off + 10, "SHPI misc")?,
            u16_le(data, off + 12, "SHPI misc")?,
            u16_le(data, off + 14, "SHPI misc")?,
        ];
        blocks.push(FshBlock { offset: off, code, size, width, height, misc });
        if size == 0 {
            break;
        }
        // no shipped entry chains more than a handful of blocks; a longer chain is a broken file,
        // not something to cut off silently
        if blocks.len() > MAX_BLOCKS {
            return Err(FormatError::UnexpectedValue {
                what: "SHPI block chain",
                offset: off,
                expected: format!("at most {MAX_BLOCKS} blocks"),
                found: "a longer chain".into(),
            });
        }
        off += size as usize;
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal synthetic SHPI: one `0x7B` (8-bit indexed) 2×1 image over a `0x29` (RGB565)
    /// palette whose entry 0 is the fixed colour-key `0x0000` and entry 1 is `0x63f0` — the exact
    /// value at `FullOval.fsh`'s `fb40` palette slot 63 (`RE/notes/car-model.md`, converter
    /// `0x570820`). No shipped `FEData/Art/*.fsh` pixel data actually indexes a `0x0000` palette
    /// slot (scanned exhaustively 2026-09-29 — every such slot in every shipped file is unused
    /// filler), so this is the only way to exercise the colour-key branch on real numbers.
    fn synthetic_indexed_565() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"SHPI");
        d.extend_from_slice(&0u32.to_le_bytes()); // declared size, unused by parse/decode
        d.extend_from_slice(&1u32.to_le_bytes()); // entry count
        d.extend_from_slice(b"GIMX");
        d.extend_from_slice(b"test"); // directory: name
        d.extend_from_slice(&24u32.to_le_bytes()); // directory: entry offset
        assert_eq!(d.len(), 24);
        // Image block: code 0x7B, size 18 (16-byte header + 2 pixel bytes), 2x1, misc zero.
        d.push(0x7B);
        d.extend_from_slice(&18u32.to_le_bytes()[..3]);
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&[0u8; 8]);
        d.push(0); // pixel 0 -> palette index 0 (colour-key, transparent)
        d.push(1); // pixel 1 -> palette index 1 (opaque RGB565)
        assert_eq!(d.len(), 24 + 18);
        // Palette block: code 0x29, size 0 (last block), 2x1 entries, misc zero.
        d.push(0x29);
        d.extend_from_slice(&[0u8; 3]);
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&[0u8; 8]);
        d.extend_from_slice(&0x0000u16.to_le_bytes());
        d.extend_from_slice(&0x63f0u16.to_le_bytes());
        d
    }

    #[test]
    fn block_codes_keep_their_literal_values() {
        use block_code::*;
        assert_eq!(MAGIC, b"SHPI");
        assert_eq!([INDEXED8, ARGB8888, RGB565, ARGB1555, ARGB4444, BGR888], [0x7B, 0x7D, 0x78, 0x7E, 0x6D, 0x7F]);
        assert_eq!([PALETTE_ARGB8888, PALETTE_RGB565, PALETTE_ARGB1555, TEXT, HOT_SPOTS], [0x2A, 0x29, 0x2D, 0x6F, 0x7C]);
        assert_eq!([COMPRESSED_BIT, FORMAT_MASK, NOT_COMPRESSED], [0x80, 0x7F, 0xFF]);
        // every format code round-trips through PixelFormat, with and without the compressed bit
        for c in [INDEXED8, ARGB8888, RGB565, ARGB1555, ARGB4444, BGR888] {
            assert!(PixelFormat::from_code(c).is_some());
            let b = FshBlock { offset: 0, code: c | COMPRESSED_BIT, size: 0, width: 0, height: 0, misc: [0; 4] };
            assert_eq!(b.format_code(), c);
            assert_eq!(b.is_compressed(), c | COMPRESSED_BIT != NOT_COMPRESSED);
        }
    }

    #[test]
    fn palette_0x29_colorkey_zero_is_transparent() {
        let d = synthetic_indexed_565();
        let f = Fsh::parse(&d).unwrap();
        let img = f.decode(&d, 0).unwrap();
        assert_eq!(&img.rgba[0..4], &[0, 0, 0, 0], "raw palette word 0x0000 must decode fully transparent (0x570820)");
        assert_eq!(&img.rgba[4..8], &[99, 125, 132, 255], "0x63f0 -> (r<<3|r>>2, g<<2|g>>4, b<<3|b>>2, 255)");
    }

    #[test]
    fn rgb565_bit_replication_matches_0x570820() {
        // A few hand-picked words, checked against the 0x570820 read (car-model.md ~589,
        // `RE/notes/layouts.md` "carcompare, read 2026-09-29"): shift-and-OR replication, no
        // gamma, no LUT.
        assert_eq!(rgb565(0x0000), [0, 0, 0, 255]);
        assert_eq!(rgb565(0xFFFF), [255, 255, 255, 255]);
        assert_eq!(rgb565(0xF800), [255, 0, 0, 255]); // R5=31, G6=0, B5=0
        assert_eq!(rgb565(0x07E0), [0, 255, 0, 255]); // G6=63
        assert_eq!(rgb565(0x001F), [0, 0, 255, 255]); // B5=31
        assert_eq!(rgb565(0x63f0), [99, 125, 132, 255]); // FullOval.fsh fb40 palette slot 63
        assert_eq!(rgb565(0x0001), [0, 0, 8, 255]); // FullOval.fsh fbr0 palette slot 0
    }
}
