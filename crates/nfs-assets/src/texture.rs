//! SHPI → engine textures.

use crate::{fmt_err, AssetError};
use nfs_formats::fsh::Fsh;
use std::path::Path;

/// A decoded texture with its full mip chain (level 0 first), RGBA8.
#[derive(Debug, Clone)]
pub struct TextureImage {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
    /// True when any decoded texel has alpha < 255.
    pub has_alpha: bool,
    /// The entry's pixel-format code (`0x7D` ARGB8888, `0x7E` 1555, `0x78` 565, `0x7F` 888, …) —
    /// what 0x556690 classifies, e.g. for the sky's "has no alpha" test 0x460D40.
    pub code: u8,
    /// Where the entry says it belongs: `misc[0]` (the page a car's day/night pack blits onto) and
    /// the blit position `misc[2] & 0xFFF` / `misc[3] & 0xFFF` (`car-model.md` §2.2).
    pub page: usize,
    pub x: u32,
    pub y: u32,
    /// `misc[0]`, `misc[1]` read as signed — where a front-end widget that places its art from the
    /// pack rather than from the `.lay` puts it. `FETrackSmall2dSpline::draw` takes the map's
    /// position from the image object alone: 0x4FCE58 and 0x4FCE66 push `-[img+0x1C]` and
    /// `-[img+0x20]` to the blit (0x4D9AF0), and the two other fields the blit reads, `+0x14` and
    /// `+0x18`, are the entry's width and height (0x4D9B04/0x4D9B0E `fild` them). Measured against
    /// the shipped art that pair is `misc[0]`, `misc[1]`: `alpsSpSm.fsh`'s map carries 170,299,
    /// inside `tournament.lay`'s `pos=140,280 size=342,152` box, and each Monaco pack gives its
    /// `s` and `f` markers **one** position because a circuit starts and finishes in the same
    /// place. The sign the pack loader puts between the header and `+0x1C` was not traced —
    /// `CnstLyr.fsh` stores the same kind of value negated — so this is the position, not an
    /// offset. HIGH.
    pub place: [i32; 2],
    /// The text of the entry's `0x6F` attachment block, if it has one. The chronicle packs use it
    /// to name the car logo that belongs with the picture: `MA601a.fsh`'s `ci` entry carries
    /// `"04finallogos2"`, and `FECarLogo::update` with `othersource=Chron` (0x4C1747) takes entry
    /// **1**, reads the string at `+0x24`, wants at least three characters (0x4C1763) and splits it
    /// into the two-digit logo pair — 0x4C17F6 computes `2·(10·s[0] + s[1]) − 0x420`, which is
    /// twice the number the two ASCII digits spell — and the pack name from `+2` (0x4C17F9).
    pub text: Option<String>,
}

/// Decode every entry of a SHPI pack. `path` is used for diagnostics only.
pub fn decode_pack(path: &Path, data: &[u8]) -> Result<Vec<TextureImage>, AssetError> {
    let fsh = Fsh::parse(data).map_err(fmt_err(path))?;
    let mut out = Vec::with_capacity(fsh.entries.len());
    for (i, e) in fsh.entries.iter().enumerate() {
        let levels = e.blocks[0].mip_levels();
        let mut mips = Vec::with_capacity(levels as usize);
        let mut width = 0;
        let mut height = 0;
        let mut has_alpha = false;
        for level in 0..levels {
            let img = fsh.decode_mip(data, i, level).map_err(fmt_err(path))?;
            if level == 0 {
                width = img.width;
                height = img.height;
                has_alpha = img.rgba.chunks_exact(4).any(|p| p[3] < 255);
            }
            mips.push(img.rgba);
        }
        let b = &e.blocks[0];
        out.push(TextureImage {
            name: e.name.clone(),
            width,
            height,
            mips,
            has_alpha,
            code: b.code,
            page: b.misc[0] as usize,
            x: (b.misc[2] & 0xFFF) as u32,
            y: (b.misc[3] & 0xFFF) as u32,
            place: [b.misc[0] as i16 as i32, b.misc[1] as i16 as i32],
            text: e.blocks.iter().find(|b| b.code == 0x6F).and_then(|b| {
                let start = b.offset + 8;
                let end = data[start..].iter().position(|c| *c == 0).map(|n| start + n)?;
                std::str::from_utf8(&data[start..end]).ok().map(str::to_string)
            }),
        });
    }
    Ok(out)
}
