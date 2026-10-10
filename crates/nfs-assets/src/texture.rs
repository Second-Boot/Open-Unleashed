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

/// How many levels the texture build 0x44D7E0 gives a texture whose pack descriptor asks for a
/// chain (flag 2 of the 0x5CE59C entry): `c + 1` with `c = 1 + floor(log2(width / 4))`
/// (0x44D95F…0x44D976 counts the halvings of `width >> 2`; the Thrash `talloc` 0x60008420 makes
/// `n + 1` levels of the `n` it is handed), so the chain stops at a 2-texel side — one level short
/// of the file's, which runs to 1×1. Every shipped track texture is square.
pub fn build_levels(width: u32) -> u32 {
    let mut c = 0;
    let mut x = width >> 2;
    while x > 0 {
        x >>= 1;
        c += 1;
    }
    c + 1
}

/// Decode every entry of a SHPI pack, **the base level only**. `path` is used for diagnostics only.
///
/// The original samples every texture but a track's with the mip filter `NONE`, i.e. its top level
/// (`RE/notes/thrash.md` §7, "The texture build"): the mip filter is set only by the material loader
/// 0x44D550, only when the pack's descriptor (0x44DFE0 → 0x5CE59C, by the pack's key and
/// `TEXTUREDETAIL`) carries flag 4, and only key 0 — the track's `.fsh` — has it; the sky (key 1),
/// the track's `.env` and the cars (2 / 3), `common.fsh` (4) and the front end (5) do not, and the
/// packs loaded whole (0x44D270) keep the default block 0x5379D0, mip `NONE`, anyway.
pub fn decode_pack(path: &Path, data: &[u8]) -> Result<Vec<TextureImage>, AssetError> {
    decode_pack_levels(path, data, |_, _| 1)
}

/// Decode a track's `.fsh` (pack key 0) with the chain the original uploads for it: the file's
/// levels, [`build_levels`] of them (0x44D7E0 → 0x535BC0 → the driver's per-level copy
/// 0x6000C390, which reads the levels one after another from the converted file data — nothing
/// is filtered or generated).
pub fn decode_pack_mipped(path: &Path, data: &[u8]) -> Result<Vec<TextureImage>, AssetError> {
    decode_pack_levels(path, data, |file, width| file.min(build_levels(width)))
}

fn decode_pack_levels(path: &Path, data: &[u8], pick: impl Fn(u32, u32) -> u32 + Sync) -> Result<Vec<TextureImage>, AssetError> {
    let fsh = Fsh::parse(data).map_err(fmt_err(path))?;
    // every entry decodes on its own: a big pack (a track's 450 textures, mips and all) is cut
    // into runs over worker threads and joined in file order
    let decode_entry = |i: usize| -> Result<TextureImage, AssetError> {
        let e = &fsh.entries[i];
        let levels = pick(e.blocks[0].mip_levels(), e.blocks[0].width as u32).max(1);
        // the whole chain at once: a compressed block is decompressed once, not once per level
        let imgs = fsh.decode_levels(data, i, levels).map_err(fmt_err(path))?;
        let (width, height) = (imgs[0].width, imgs[0].height);
        let has_alpha = imgs[0].rgba.chunks_exact(4).any(|p| p[3] < 255);
        let mips: Vec<Vec<u8>> = imgs.into_iter().map(|img| img.rgba).collect();
        let b = &e.blocks[0];
        Ok(TextureImage {
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
            text: e.blocks.iter().find(|b| b.code == nfs_formats::fsh::block_code::TEXT).and_then(|b| {
                let start = b.offset + 8;
                let end = data.get(start..)?.iter().position(|c| *c == 0).map(|n| start + n)?;
                std::str::from_utf8(&data[start..end]).ok().map(str::to_string)
            }),
        })
    };
    let n = fsh.entries.len();
    let workers = if n < 32 { 1 } else { std::thread::available_parallelism().map_or(1, |w| w.get()).min(8) };
    if workers <= 1 {
        return (0..n).map(decode_entry).collect();
    }
    let chunk = n.div_ceil(workers);
    let decode_entry = &decode_entry;
    // PORT-DEVIATION D7 (RE/notes/PORT-DEVIATIONS.md): a big pack decodes on worker threads, joined in file order; the original loads sequentially (load steps 0x4A4A70).
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|lo| s.spawn(move || (lo..(lo + chunk).min(n)).map(decode_entry).collect::<Result<Vec<_>, _>>()))
            .collect();
        let mut out = Vec::with_capacity(n);
        for h in handles {
            out.extend(h.join().map_err(|_| AssetError::Worker { path: path.to_path_buf() })??);
        }
        Ok(out)
    })
}
