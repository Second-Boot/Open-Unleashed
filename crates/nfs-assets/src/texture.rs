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

/// What the original's key-3 texture cache (the player's car, `carcmn.fsh`; keys 2 and 5 take the same path) keeps of
/// a **32-bit** source — `RE/notes/thrash.md` §6d.8 / `REFERENCE-2026-10-24-PA-TEX4444.md`.
///
/// 0x44DFE0 hands the build 0x44D7E0 a descriptor (`0x5CE59C + 4·(key + 6·TEXTUREDETAIL)`) whose cache depth byte is
/// **0x10** for keys 2, 3 and 5 at every `TEXTUREDETAIL` (and at every screen depth: the byte is the table's, not
/// the mode's), so 0x44D7E0 installs the 16-bit lists for the opaque class (`0x5CE48C`) and, `bl` = 1 with the
/// descriptor's flag 0x20 clear (0x44D80A…0x44D817), the alpha class (`0x5CE4AC`, 4444 first) through
/// 0x554950(4 / 6, list). The cache (0x554820 over the driver's formats, which dx7z lists with 4444 and wine's
/// ddraw too) picks 4444 for every 8888 source; 0x5350E0 takes the pair from its table at 0x5DE2FC /
/// 0x5DE4FC (source type 6, target 7) and runs **0x570540**: each byte `>> 4`, packed `A R G B` — truncation, no
/// rounding, no dither — and the sampler reads a nibble back as `x/15`, i.e. `x · 17`. Measured live on the
/// original (gdb at the call 0x555708, 2026-10-24): the player's `SPC2`, `SPC3`, `FE  `, `FESH` and the car's 32-bit
/// pages (901: pages 4, 5, 8) come out type 7; 1555, 4444 and 565 pages stay as they are.
///
/// Applied to the texels of an entry whose file format `code` is `0x7D` (ARGB8888) — what 0x44D7E0 receives; other
/// formats keep their bytes (the port already carries their own quantisation).
pub fn cache_4444_from_8888(rgba: &mut [u8]) {
    for c in rgba.iter_mut() {
        let x = *c >> 4;
        *c = (x << 4) | x;
    }
}

/// What the same 16-bit cache does to a **565** source — `RE/notes/thrash.md` §6d.9 / `REFERENCE-2026-10-24-PA-TEX1555.md`.
/// The opaque list 0x5CE48C (installed by 0x44D7E0 through 0x554950(4, list), [`cache_4444_from_8888`]) starts with 1555
/// (type 3), so a 565 entry (type 4) is converted by `0x5350E0`: its table 0x5DE2FC at `[src * 8 + dst]` (types mapped
/// through 0x5DE574: 565 -> 3, 1555 -> 2) holds **0x5702A0**, a single hop (the hop table 0x5DE4FC names the target).
/// 0x5702A0 reads the texel word `p` and, **`p == 0` kept as 0** (`test ax, ax` / `je`, 0x5702A6...0x5702BA), writes
/// `(p & 0x1F) | ((p >> 1) & 0xFFE0) | 0x8000`: red and blue keep their 5 bits, the green's lowest bit is dropped
/// (truncation, no rounding), alpha becomes 1 - and the one colour the word 0 stays a transparent black. Read live on the
/// original (gdb at 0x5702A0 and the call 0x555708, `nfs5-second`, dx7): `common.fsh` `HEAD`, `particle.fsh` `SPRK`
/// and the 27 `bpp = 565` pages of the front end's room models (`FEData/Models/*.tpg`).
/// `rgba` holds the decoded 565 texels (`r5 << 3 | r5 >> 2`, `g6 << 2 | g6 >> 4`, ...), from which the word is rebuilt.
pub fn cache_1555_from_565(rgba: &mut [u8]) {
    for t in rgba.chunks_exact_mut(4) {
        let p = ((t[0] >> 3) as u16) << 11 | ((t[1] >> 2) as u16) << 5 | (t[2] >> 3) as u16;
        if p == 0 {
            t.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let q = (p & 0x1F) | ((p >> 1) & 0xFFE0) | 0x8000;
        let (r, g, b) = (((q >> 10) & 0x1F) as u8, ((q >> 5) & 0x1F) as u8, (q & 0x1F) as u8);
        t.copy_from_slice(&[r << 3 | r >> 2, g << 3 | g >> 2, b << 3 | b >> 2, 255]);
    }
}

impl TextureImage {
    /// The entry's texels as the original's 16-bit texture cache holds them, for a pack the cache is told to keep in 16
    /// bits (descriptor depth 0x10 of 0x44DFE0: keys 2, 3, 4 and 5 at every `TEXTUREDETAIL`; the front end builds
    /// everything as key 5): an 8888 entry (code `0x7D`) becomes 4444 ([`cache_4444_from_8888`]), a 565 entry (`0x78`) 1555
    /// ([`cache_1555_from_565`]), every other format stays as decoded. The sky (key 1) and the track (key 0) are depth
    /// 0x20 at `TEXTUREDETAIL` 3, the tier the port runs, and are not touched by this.
    ///
    /// PORT-DEVIATION D42: the port always runs that tier (the original's 32-bit screen); at a 16-bit screen the original
    /// drops to tier 2 and keeps the track and sky 16-bit too (`RE/notes/PORT-DEVIATIONS.md`).
    pub fn cache_mips(&self) -> Vec<Vec<u8>> {
        let mut mips = self.mips.clone();
        match self.code {
            0x7D => mips.iter_mut().for_each(|m| cache_4444_from_8888(m)),
            0x78 => mips.iter_mut().for_each(|m| cache_1555_from_565(m)),
            _ => {}
        }
        mips
    }
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

#[cfg(test)]
mod cache_565_tests {
    use super::*;

    /// 0x5702A0: blue and red keep 5 bits, green drops its low bit, alpha 1; the word 0 stays 0.
    #[test]
    fn a_565_texel_becomes_1555_by_0x5702a0() {
        let mut t = [255u8, 255, 255, 255, 0, 0, 0, 255, 0x08, 0x04, 0x08, 255];
        cache_1555_from_565(&mut t);
        assert_eq!(&t[..4], [255, 255, 255, 255]);
        // the word 0 is kept as 0: alpha 0
        assert_eq!(&t[4..8], [0, 0, 0, 0]);
        // r5 = 1, g6 = 1 (low bit dropped -> g5 = 0), b5 = 1
        assert_eq!(&t[8..], [1 << 3 | 1 >> 2, 0, 1 << 3 | 1 >> 2, 255]);
    }

    /// Every 565 word goes through the routine's formula.
    #[test]
    fn every_word_matches_the_routine() {
        for p in 0..=0xFFFFu32 {
            let (r, g, b) = ((p >> 11) as u8, ((p >> 5) & 0x3F) as u8, (p & 0x1F) as u8);
            let mut t = [r << 3 | r >> 2, g << 2 | g >> 4, b << 3 | b >> 2, 255];
            cache_1555_from_565(&mut t);
            if p == 0 {
                assert_eq!(t, [0; 4]);
                continue;
            }
            let q = (p & 0x1F) | ((p >> 1) & 0xFFE0) | 0x8000;
            let (r5, g5, b5) = (((q >> 10) & 0x1F) as u8, ((q >> 5) & 0x1F) as u8, (q & 0x1F) as u8);
            assert_eq!(t, [r5 << 3 | r5 >> 2, g5 << 3 | g5 >> 2, b5 << 3 | b5 >> 2, 255], "p = {p:#06x}");
        }
    }
}
