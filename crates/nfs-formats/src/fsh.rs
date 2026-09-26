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
//!   nibble when it drops the top level. *Corrected 2026-09-26: this said "including the base".*) Palette attachments: `0x2A` 32-bit, `0x29`/`0x2D` 16-bit (HYPOTHESIS on
//!   channel order). `0x6F` = text comment, `0x7C` = hot-spot table.
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
            0x7B => Self::Indexed8,
            0x7D => Self::Argb8888,
            0x78 => Self::Rgb565,
            0x7E => Self::Argb1555,
            0x6D => Self::Argb4444,
            0x7F => Self::Bgr888,
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
        self.code & 0x7F
    }
    /// True when the pixel payload is a RefPack stream (bit 7 of the code).
    pub fn is_compressed(&self) -> bool {
        self.code & 0x80 != 0 && self.code != 0xFF
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
        if data.get(0..4) != Some(b"SHPI") {
            return Err(FormatError::UnknownMagic {
                what: "SHPI",
                offset: 0,
                expected: b"SHPI".to_vec(),
                found: data.iter().take(4).copied().collect(),
            });
        }
        let declared_size = u32_le(data, 4, "SHPI size")?;
        let count = u32_le(data, 8, "SHPI count")? as usize;
        let directory_id: [u8; 4] = data[12..16].try_into().unwrap();
        let mut entries = Vec::with_capacity(count);
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
        Ok(Self { declared_size, directory_id, entries })
    }

    /// Byte length of the image block for `entry`, using the next entry's offset (or file end)
    /// when the block's own size field is 0.
    pub fn image_block_len(&self, data_len: usize, idx: usize) -> usize {
        let e = &self.entries[idx];
        let first = &e.blocks[0];
        if first.size != 0 {
            return first.size as usize;
        }
        let next = self
            .entries
            .iter()
            .map(|o| o.offset as usize)
            .filter(|&o| o > e.offset as usize)
            .min()
            .unwrap_or(data_len);
        next - e.offset as usize
    }

    /// Decode the base mip level of entry `idx` to RGBA8.
    pub fn decode(&self, data: &[u8], idx: usize) -> Result<DecodedImage, FormatError> {
        self.decode_mip(data, idx, 0)
    }

    /// Decode mip `level` (0 = base) of entry `idx` to RGBA8.
    pub fn decode_mip(&self, data: &[u8], idx: usize, level: u32) -> Result<DecodedImage, FormatError> {
        let e = &self.entries[idx];
        let img = &e.blocks[0];
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
        let block_len = self.image_block_len(data.len(), idx);
        let payload: std::borrow::Cow<[u8]> = if img.is_compressed() {
            let comp = data.get(px_off..img.offset + block_len).ok_or(FormatError::Truncated {
                what: "SHPI compressed pixel data",
                offset: px_off,
                needed: block_len,
            })?;
            std::borrow::Cow::Owned(crate::refpack::decompress(comp)?)
        } else {
            std::borrow::Cow::Borrowed(&data[px_off..])
        };
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
                let pal = e.blocks.iter().find(|b| matches!(b.code, 0x2A | 0x29 | 0x2D)).ok_or(FormatError::UnexpectedValue {
                    what: "SHPI indexed image",
                    offset: img.offset,
                    expected: "a 0x2A/0x29/0x2D palette block".into(),
                    found: format!("blocks {:?}", e.blocks.iter().map(|b| b.code).collect::<Vec<_>>()),
                })?;
                let ncol = pal.width as usize * pal.height.max(1) as usize;
                let pal_off = pal.offset + 16;
                let entry_size = if pal.code == 0x2A { 4 } else { 2 };
                let pal_bytes = data.get(pal_off..pal_off + ncol * entry_size).ok_or(FormatError::Truncated {
                    what: "SHPI palette",
                    offset: pal_off,
                    needed: ncol * entry_size,
                })?;
                let lookup = |ix: u8| -> [u8; 4] {
                    let c = &pal_bytes[(ix as usize).min(ncol - 1) * entry_size..][..entry_size];
                    match pal.code {
                        // Stored as B,G,R,A (little-endian ARGB). CONFIRMED visually on car textures.
                        0x2A => [c[2], c[1], c[0], c[3]],
                        // HYPOTHESIS: 0x29 = RGB565, 0x2D = ARGB1555.
                        0x29 => rgb565(u16::from_le_bytes([c[0], c[1]])),
                        _ => argb1555(u16::from_le_bytes([c[0], c[1]])),
                    }
                };
                for (i, &ix) in px.iter().enumerate() {
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&lookup(ix));
                }
            }
            PixelFormat::Argb8888 => {
                for i in 0..npix {
                    let c = &px[i * 4..i * 4 + 4];
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&[c[2], c[1], c[0], c[3]]);
                }
            }
            PixelFormat::Bgr888 => {
                for i in 0..npix {
                    let c = &px[i * 3..i * 3 + 3];
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&[c[2], c[1], c[0], 255]);
                }
            }
            PixelFormat::Rgb565 => {
                for i in 0..npix {
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&rgb565(u16::from_le_bytes([px[i * 2], px[i * 2 + 1]])));
                }
            }
            PixelFormat::Argb1555 => {
                for i in 0..npix {
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&argb1555(u16::from_le_bytes([px[i * 2], px[i * 2 + 1]])));
                }
            }
            PixelFormat::Argb4444 => {
                for i in 0..npix {
                    let v = u16::from_le_bytes([px[i * 2], px[i * 2 + 1]]);
                    let a = ((v >> 12) & 0xF) as u8;
                    let r = ((v >> 8) & 0xF) as u8;
                    let g = ((v >> 4) & 0xF) as u8;
                    let b = (v & 0xF) as u8;
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&[r * 17, g * 17, b * 17, a * 17]);
                }
            }
        }
        Ok(DecodedImage { width: w as u32, height: h as u32, rgba, source_format: fmt })
    }
}

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
        if size == 0 || blocks.len() > 16 {
            break;
        }
        off += size as usize;
    }
    Ok(blocks)
}
