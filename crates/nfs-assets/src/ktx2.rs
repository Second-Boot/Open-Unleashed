//! ENHANCEMENT E41: the HD pack's GPU-compressed container — a minimal **KTX2** (Khronos Texture 2.0) reader and writer for
//! one 2D BC7 texture with its mip chain (`vkFormat` 145 `VK_FORMAT_BC7_UNORM_BLOCK`, no supercompression, one layer, one face).
//!
//! Why KTX2: a published standard that the common tools open (`ktx`, Blender, Basis); the header is 80 bytes plus a level
//! index, so the reader here is ~60 lines and needs no dependency; the level order and 16-byte alignment follow the spec
//! (levels stored smallest first, the index lists level 0 first). The data is the GPU's own block layout (16 bytes per 4x4
//! block, rows of `ceil(w / 4)` blocks), so an upload is one `write_texture` per level. UNORM and not sRGB: the game's surface
//! is a byte surface (colours work on the stored bytes), the textures are sampled as `Rgba8Unorm` today.
//!
//! The DFD (data format descriptor) is the spec's BC7 one; the KVD is empty. A base level whose width or height is not a
//! multiple of 4 cannot be a BC7 texture: the pack tool keeps such an image as a PNG.

/// `VK_FORMAT_BC7_UNORM_BLOCK`
pub const VK_FORMAT_BC7_UNORM_BLOCK: u32 = 145;
const IDENT: [u8; 12] = [0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A];
const HEADER: usize = 80;

/// A decoded KTX2 BC7 file: `levels[0]` is the base level, every level the raw 16-byte blocks, row-major.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bc7Image {
    pub width: u32,
    pub height: u32,
    pub levels: Vec<Vec<u8>>,
}

/// Bytes of one BC7 level of `w x h` texels.
pub fn level_len(w: u32, h: u32) -> usize {
    w.div_ceil(4) as usize * h.div_ceil(4) as usize * 16
}

fn dfd() -> Vec<u8> {
    // the Khronos Data Format spec's BC7 descriptor: total size 44, one basic block of 40 bytes with one 128-bit sample
    let mut d = Vec::with_capacity(44);
    d.extend_from_slice(&44u32.to_le_bytes()); // dfdTotalSize
    d.extend_from_slice(&0u32.to_le_bytes()); // vendorId 0 | descriptorType 0 (basic)
    d.extend_from_slice(&2u16.to_le_bytes()); // versionNumber
    d.extend_from_slice(&40u16.to_le_bytes()); // descriptorBlockSize
    d.extend_from_slice(&[0x86, 1, 1, 0]); // colorModel BC7, primaries BT.709, transfer linear, flags 0
    d.extend_from_slice(&[3, 3, 0, 0]); // texelBlockDimension 4x4 (stored minus one)
    d.extend_from_slice(&[16, 0, 0, 0, 0, 0, 0, 0]); // bytesPlane0 = 16
    d.extend_from_slice(&0u16.to_le_bytes()); // sample: bitOffset
    d.extend_from_slice(&[127, 0]); // bitLength 128 (stored minus one), channelType 0
    d.extend_from_slice(&[0, 0, 0, 0]); // samplePosition
    d.extend_from_slice(&0u32.to_le_bytes()); // sampleLower
    d.extend_from_slice(&u32::MAX.to_le_bytes()); // sampleUpper
    d
}

/// Write a KTX2 file of the BC7 `levels` (level 0 first; `levels[i]` is `level_len` of its size).
pub fn write_bc7(width: u32, height: u32, levels: &[Vec<u8>]) -> Vec<u8> {
    let n = levels.len();
    let dfd = dfd();
    let dfd_off = HEADER + 24 * n;
    let data_start = (dfd_off + dfd.len()).next_multiple_of(16);
    // smallest level first in the data; every level 16-byte aligned (the block size)
    let mut offsets = vec![0u64; n];
    let mut at = data_start;
    for i in (0..n).rev() {
        at = at.next_multiple_of(16);
        offsets[i] = at as u64;
        at += levels[i].len();
    }
    let mut out = Vec::with_capacity(at);
    out.extend_from_slice(&IDENT);
    for v in [VK_FORMAT_BC7_UNORM_BLOCK, 1, width, height, 0, 0, 1, n as u32, 0] {
        out.extend_from_slice(&v.to_le_bytes()); // vkFormat, typeSize, w, h, depth, layers, faces, levels, supercompression
    }
    for v in [dfd_off as u32, dfd.len() as u32, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes()); // dfd offset / length, kvd offset / length
    }
    for v in [0u64, 0] {
        out.extend_from_slice(&v.to_le_bytes()); // sgd offset / length
    }
    for i in 0..n {
        out.extend_from_slice(&offsets[i].to_le_bytes());
        out.extend_from_slice(&(levels[i].len() as u64).to_le_bytes());
        out.extend_from_slice(&(levels[i].len() as u64).to_le_bytes()); // uncompressed length = length (no supercompression)
    }
    out.extend_from_slice(&dfd);
    out.resize(data_start, 0);
    for i in (0..n).rev() {
        out.resize(offsets[i] as usize, 0);
        out.extend_from_slice(&levels[i]);
    }
    out
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Parse a KTX2 BC7 file. `None` for anything else (another format, supercompressed, an array / cube, a short or inconsistent file).
pub fn read_bc7(b: &[u8]) -> Option<Bc7Image> {
    if b.get(..12)? != IDENT || u32_at(b, 12)? != VK_FORMAT_BC7_UNORM_BLOCK {
        return None;
    }
    let (width, height) = (u32_at(b, 20)?, u32_at(b, 24)?);
    let (depth, layers, faces, nlevels, supercompression) = (u32_at(b, 28)?, u32_at(b, 32)?, u32_at(b, 36)?, u32_at(b, 40)?, u32_at(b, 44)?);
    if width == 0 || height == 0 || depth != 0 || layers > 1 || faces != 1 || supercompression != 0 || nlevels == 0 || nlevels > 16 {
        return None;
    }
    let (mut w, mut h) = (width, height);
    let mut levels = Vec::with_capacity(nlevels as usize);
    for i in 0..nlevels as usize {
        let (off, len) = (u64_at(b, HEADER + 24 * i)? as usize, u64_at(b, HEADER + 24 * i + 8)? as usize);
        if len != level_len(w, h) {
            return None;
        }
        levels.push(b.get(off..off.checked_add(len)?)?.to_vec());
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    Some(Bc7Image { width, height, levels })
}

/// Decode BC7 blocks of a `w x h` level to RGBA8 (the CPU fallback of a device without BC support, and the car pages).
pub fn decode_level(blocks: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (bw, bh) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    let (w, h) = (w as usize, h as usize);
    let mut out = vec![0u8; w * h * 4];
    let mut tile = [0u8; 4 * 4 * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let o = (by * bw + bx) * 16;
            bcdec_rs::bc7(&blocks[o..o + 16], &mut tile, 16);
            for ty in 0..4.min(h - by * 4) {
                let n = 4.min(w - bx * 4);
                let dst = ((by * 4 + ty) * w + bx * 4) * 4;
                out[dst..dst + n * 4].copy_from_slice(&tile[ty * 16..ty * 16 + n * 4]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_round_trips() {
        // 16x8, levels 16x8 / 8x4 / 4x2 / 2x1 / 1x1: block counts 4x2, 2x1, 1x1, 1x1, 1x1
        let sizes = [(16u32, 8u32), (8, 4), (4, 2), (2, 1), (1, 1)];
        let levels: Vec<Vec<u8>> = sizes.iter().enumerate().map(|(i, &(w, h))| (0..level_len(w, h)).map(|k| (k * 7 + i) as u8).collect()).collect();
        let f = write_bc7(16, 8, &levels);
        let r = read_bc7(&f).expect("parses");
        assert_eq!((r.width, r.height), (16, 8));
        assert_eq!(r.levels, levels);
        // every level offset is 16-byte aligned
        for i in 0..levels.len() {
            let off = u64::from_le_bytes(f[HEADER + 24 * i..HEADER + 24 * i + 8].try_into().unwrap());
            assert_eq!(off % 16, 0);
        }
        // a wrong length or format is refused
        let mut bad = f.clone();
        bad[12] = 146;
        assert!(read_bc7(&bad).is_none());
        assert!(read_bc7(&f[..f.len() - 1]).is_none());
        assert!(read_bc7(b"\x89PNG").is_none());
    }

    #[test]
    fn the_decoder_loop_covers_a_level_that_is_not_a_multiple_of_four() {
        let px = decode_level(&[0u8; 16], 3, 2);
        assert_eq!(px.len(), 3 * 2 * 4);
    }
}
