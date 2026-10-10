//! `.mad` — EA's MAD video (`RE/notes/mad.md`).
//!
//! An intra-only MPEG-1-style DCT codec: the MPEG-1 default intra quantiser matrix, the MPEG-1
//! zig-zag scan (transposed), an AAN inverse transform, and EA's own variable-length code tables.
//! Everything here is transcribed from `nfs5.exe`; nothing is invented.
//!
//! The VLC tables are 222 sixteen-byte records in the executable's `.rdata` and are **not** copied
//! into this repository — [`MadTables::from_exe`] reads them out of the player's own `nfs5.exe`,
//! the same installation every other asset comes from.
//!
//! **State.** The container and the intra (`MADk`) decoder are confirmed against the original's own
//! data: `chunks` walks every shipped `.mad` exactly to its last byte, and every keyframe's
//! bitstream is consumed to **exactly** its payload length — `examples/madcheck.rs` checks that.
//! The inter (`MADm`) path, which needs the mode mask, the motion vectors and a reference frame, is
//! not written yet.

use crate::error::FormatError;
use crate::bytes::{get_u16, read_i16, read_i32, read_u32};
use crate::{u16_le, u32_le};

// -------------------------------------------------------------------------------------------
// container
// -------------------------------------------------------------------------------------------

/// One chunk of a `.mad` file. The container is EA's usual `{char[4] tag, u32 size}`, with `size`
/// counting the eight header bytes. CONFIRMED against `EArts-av.mad`, whose chunks walk exactly to
/// the end of the file.
///
/// Four tags appear: **`MADk`** — an intra video frame, **`MADm`** — an inter one, and `SCHl`,
/// `SCCl`, `SCDl` — EA's SCHl audio stream, which `crates/nfs-formats/src/schl.rs` already reads.
/// `MADe` is the third magic 0x4DC8D9 accepts but does not appear in the shipped files. The player
/// skips anything that is not one of the three MAD tags (0x4DC8F1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chunk<'a> {
    pub tag: [u8; 4],
    /// the chunk's bytes, header included
    pub data: &'a [u8],
}

impl<'a> Chunk<'a> {
    pub fn is_video(&self) -> bool {
        matches!(&self.tag, b"MADk" | b"MADm" | b"MADe")
    }
}

/// Walk a `.mad` file's chunks.
pub fn chunks(d: &[u8]) -> impl Iterator<Item = Chunk<'_>> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        let h = d.get(at..at + 8)?;
        let size = read_u32(h, 4) as usize;
        if size < 8 || at + size > d.len() {
            return None;
        }
        let c = Chunk { tag: [h[0], h[1], h[2], h[3]], data: &d[at..at + size] };
        at += size;
        Some(c)
    })
}

/// The movie's sound track: its `SCHl`, `SCCl`, `SCDl` and `SCEl` chunks in file order, which
/// together are one ordinary `SCHl` stream ([`crate::schl`]).
///
/// The start-up player 0x4DC850 never looks at them itself: it registers the stream's `SC` chunks
/// with the sound library (0x56B230(stream, 1, 0xFFFF, 0x4353 `"SC"`, 2) at 0x4DC8AC), hands every
/// chunk that is not a video one back through 0x56C620 as it walks the file, and starts the
/// library's player on the stream once (0x56CA20 at 0x4DCBCF, 0x56CA00 → 0x567550 at 0x4DCBFB —
/// no level argument). Every shipped movie's header is 2 channels, no 0x84 rate (22050) and no
/// 0xA0 codec (EA-XA), the same as the music's.
pub fn audio_track(d: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for c in chunks(d) {
        if c.tag[..2] == *b"SC" {
            out.extend_from_slice(c.data);
        }
    }
    out
}

/// The header a video chunk carries (0x4DC8D9 … 0x4DCA1D).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MadHeader {
    /// `MADk` — intra — or `MADm` — inter. 0x4DC8D9 also accepts `MADe`.
    pub magic: [u8; 4],
    /// `+0x04`, the chunk's own size, header included.
    pub size: u32,
    /// `+0x0C`, copied to `[0x65E268]` at 0x4DC952 — the frame period.
    pub period: u32,
    /// `+0x10` / `+0x12`, read with `movsx word` at 0x4DC958 and 0x4DC964.
    pub width: i16,
    pub height: i16,
    /// `+0x15`, the quantiser the player hands the decoder's setup (0x4DCA1D).
    pub quant: i8,
}

/// The video header is 0x18 bytes; the bitstream starts right after it (0x4DCA22).
pub const MAD_HEADER_SIZE: usize = 0x18;

impl MadHeader {
    pub fn parse(d: &[u8]) -> Result<Self, FormatError> {
        let what = "mad";
        if d.len() < MAD_HEADER_SIZE {
            return Err(FormatError::Truncated { what, offset: 0, needed: MAD_HEADER_SIZE });
        }
        let magic = [d[0], d[1], d[2], d[3]];
        if !matches!(&magic, b"MADk" | b"MADm" | b"MADe") {
            return Err(FormatError::UnknownMagic { what, offset: 0, expected: b"MADk".to_vec(), found: magic.to_vec() });
        }
        Ok(Self {
            magic,
            size: read_u32(d, 4),
            period: read_u32(d, 0xC),
            width: read_i16(d, 0x10),
            height: read_i16(d, 0x12),
            quant: d[0x15] as i8,
        })
    }

    /// `MADk` hands the decoder 0 and the other two magics 1 (0x4DCA25 against 0x4DCA32). With 0
    /// every block of every macroblock is coded and the mode and motion-vector reads at
    /// 0x56CD02…0x56CD56 never run — so `MADk` is the intra frame and `MADm` the inter one.
    pub fn kind(&self) -> u32 {
        u32::from(&self.magic != b"MADk")
    }
}

// -------------------------------------------------------------------------------------------
// the tables the executable carries
// -------------------------------------------------------------------------------------------

/// Where each table the decoder needs lives in the **patched** `nfs5.exe` this project reads
/// (Verok 1.0.6 of EA v3.5). See `RE/notes/mad.md` §3 and §5.
mod va {
    /// the MPEG-1 default intra quantiser matrix, row-major (`0x56CAA4`, `0x56CABC`)
    pub const QUANT: u32 = 0x5B_ED70;
    /// the AAN prescale table that goes with it
    pub const PRESCALE: u32 = 0x5E_283C;
    /// scan position → raster **byte** offset: the MPEG-1 zig-zag, transposed (`0x5933D2`)
    pub const SCAN: u32 = 0x5E_2940;
    /// the first VLC record array, 94 records (`0x56CB61` … `0x56CBF1`)
    pub const VLC_A: u32 = 0x5B_DF90;
    pub const VLC_A_END: u32 = 0x5B_E570;
    /// the second, 128 records (`0x56CBF9` … `0x56CC7C`)
    pub const VLC_B: u32 = 0x5B_E570;
    pub const VLC_B_END: u32 = 0x5B_ED70;
}

/// A minimal PE virtual-address → file-offset map, so the tables can be read out of the player's
/// own executable rather than transcribed into this repository.
pub struct PeImage<'a> {
    data: &'a [u8],
    /// `(virtual address, virtual size, file offset)` per section
    sections: Vec<(u32, u32, u32)>,
}

impl<'a> PeImage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, FormatError> {
        let what = "nfs5.exe";
        // every offset below comes out of the file: read through the checked readers
        let u16at = |o: usize| u16_le(data, o, what);
        let u32at = |o: usize| u32_le(data, o, what);
        if data.len() < 0x40 || &data[..2] != b"MZ" {
            return Err(FormatError::UnknownMagic { what, offset: 0, expected: b"MZ".to_vec(), found: data[..data.len().min(2)].to_vec() });
        }
        let pe = u32at(0x3C)? as usize;
        if data.len() < pe.saturating_add(0x18) || &data[pe..pe + 4] != b"PE\0\0" {
            return Err(FormatError::UnknownMagic { what, offset: pe, expected: b"PE\0\0".to_vec(), found: data.get(pe..).unwrap_or_default().iter().take(4).copied().collect() });
        }
        let base = u32at(pe + 0x34)?;
        let n = u16at(pe + 6)? as usize;
        let opt = u16at(pe + 0x14)? as usize;
        let first = pe + 0x18 + opt;
        let mut sections = Vec::with_capacity(n);
        for i in 0..n {
            let s = first + i * 40;
            if s + 40 > data.len() {
                return Err(FormatError::Truncated { what, offset: s, needed: 40 });
            }
            sections.push((base.wrapping_add(u32at(s + 12)?), u32at(s + 8)?, u32at(s + 20)?));
        }
        Ok(Self { data, sections })
    }

    /// `n` bytes at virtual address `va`, or `None` when that address is not mapped.
    pub fn at(&self, va: u32, n: usize) -> Option<&'a [u8]> {
        let (v, size, off) = *self.sections.iter().find(|(v, size, _)| va >= *v && va - *v < *size)?;
        let _ = size;
        let start = (off as usize).checked_add((va - v) as usize)?;
        self.data.get(start..start.checked_add(n)?)
    }

    fn i32s(&self, va: u32, n: usize) -> Option<Vec<i32>> {
        let d = self.at(va, n * 4)?;
        Some(d.chunks_exact(4).map(|c| read_i32(c, 0)).collect())
    }
}

/// One record of a VLC array: `{i32 bits, i32 code, i32 0, i32 index}` (`RE/notes/mad.md` §5).
#[derive(Debug, Clone, Copy)]
struct VlcRecord {
    bits: i32,
    code: i32,
    index: i32,
}

/// The lookup tables 0x56CB00 builds, plus the ones the setup needs.
///
/// The three code tables are **one contiguous block** in the original, and that matters: a run of
/// entries can spill out of one and into the next, and the reader then finds it there. `0x6B51A0`
/// holds 512 entries indexed by nine bits, `0x6B59A0` the 256 the `0x0F` escape reaches with eight
/// more, `0x6B5DA0` the 256 the `0x1F` escape reaches, and `0x6B61A0` the 64 motion-vector ones —
/// 1088 dwords end to end, which is how [`MadTables::flat`] keeps them.
///
/// An entry packs, exactly as the executable's does: the number of bits to consume in the low byte,
/// the signed **level** in `entry >> 22` and the **run** in `(entry >> 16) & 0x3F`.
#[derive(Clone)]
pub struct MadTables {
    /// `0x6B51A0 … 0x6B62A0` as one array of dwords
    pub flat: Vec<u32>,
    /// the MPEG-1 default intra quantiser matrix, row-major
    pub quant: [i32; 64],
    /// the AAN prescale table
    pub prescale: [i32; 64],
    /// scan position → raster **element** index (the executable stores it ×4)
    pub scan: [usize; 64],
}

/// Where each table starts inside [`MadTables::flat`], in dwords from `0x6B51A0`.
mod tbl {
    pub const PRIMARY: usize = 0;
    pub const ESCAPE_A: usize = 512;
    pub const ESCAPE_B: usize = 768;
    pub const MOTION: usize = 1024;
    pub const LEN: usize = 1088;
}

impl MadTables {
    /// The 9-bit primary lookup.
    pub fn primary(&self, idx: u32) -> u32 {
        self.flat[tbl::PRIMARY + idx as usize]
    }
    /// The table the `0x0F` escape reaches, with eight more bits.
    pub fn escape_a(&self, idx: u32) -> u32 {
        self.flat[tbl::ESCAPE_A + idx as usize]
    }
    /// The table the `0x1F` escape reaches.
    pub fn escape_b(&self, idx: u32) -> u32 {
        self.flat[tbl::ESCAPE_B + idx as usize]
    }
    /// A motion-vector component's six-bit lookup.
    pub fn motion(&self, idx: u32) -> u32 {
        self.flat[tbl::MOTION + idx as usize]
    }

    /// Read them out of the player's own `nfs5.exe`.
    pub fn from_exe(exe: &[u8]) -> Result<Self, FormatError> {
        let what = "nfs5.exe";
        let pe = PeImage::parse(exe)?;
        let miss = |va: u32| FormatError::UnexpectedValue { what, offset: va as usize, expected: "a mapped .rdata address".into(), found: "unmapped".into() };

        let quant_v = pe.i32s(va::QUANT, 64).ok_or_else(|| miss(va::QUANT))?;
        let prescale_v = pe.i32s(va::PRESCALE, 64).ok_or_else(|| miss(va::PRESCALE))?;
        let scan_v = pe.i32s(va::SCAN, 64).ok_or_else(|| miss(va::SCAN))?;
        // the one sanity check that the addresses still point at what the note says they do: the
        // matrix starts with the MPEG-1 default intra matrix's first row
        if quant_v[..8] != [8, 16, 19, 22, 26, 27, 29, 34] {
            return Err(FormatError::UnexpectedValue {
                what,
                offset: va::QUANT as usize,
                expected: "the MPEG-1 default intra quantiser matrix".into(),
                found: format!("{:?}", &quant_v[..8]),
            });
        }
        let mut quant = [0i32; 64];
        quant.copy_from_slice(&quant_v);
        let mut prescale = [0i32; 64];
        prescale.copy_from_slice(&prescale_v);
        let mut scan = [0usize; 64];
        for (s, v) in scan.iter_mut().zip(&scan_v) {
            *s = (*v as usize) / 4;
        }

        let read = |from: u32, to: u32| -> Result<Vec<VlcRecord>, FormatError> {
            let n = ((to - from) / 16) as usize;
            let raw = pe.i32s(from, n * 4).ok_or_else(|| miss(from))?;
            Ok(raw.chunks_exact(4).map(|r| VlcRecord { bits: r[0], code: r[1], index: r[3] }).collect())
        };
        let a = read(va::VLC_A, va::VLC_A_END)?;
        let b = read(va::VLC_B, va::VLC_B_END)?;

        let mut flat = vec![0u32; tbl::LEN];

        // the escape markers, written before the records are expanded (0x56CB37 … 0x56CB77): their
        // length fields 0x0F, 0x1F, 0x2F and 0x3F are longer than any real code
        flat[0] = 0x0F;
        flat[1..8].fill(0x1F);
        flat[8..16].fill(0x2F);
        flat[256..384].fill(0x3F);

        // the entry the builder stores (0x56CB9A … 0x56CBA9)
        let pack = |code: i32, len: i32| -> u32 { (((code as u32 & 0xFC00) | ((code as u32) << 16)) << 6) | (len as u32 & 0xFF) };
        // `shl ebx, cl` then `test ebx, ebx; jle` — a shift count the x86 masks to five bits, and a
        // run that is skipped when the result is not positive. That is not the same as clamping the
        // count to one: a code longer than the table's index writes **nothing** (0x56CBAB,
        // 0x56CBDF, 0x56CC36, 0x56CC6A).
        let fill = |t: &mut [u32], base: usize, shift: i32, e: u32| {
            let n = 1i32.wrapping_shl(shift as u32 & 31);
            if n <= 0 {
                return;
            }
            let end = base.saturating_add(n as usize).min(t.len());
            if base < end {
                t[base..end].fill(e);
            }
        };

        // array A → the 9-bit primary, or the second-level table for the long codes
        for r in &a {
            if r.index & 0xFC00 != 0 {
                fill(&mut flat, tbl::PRIMARY.saturating_add((r.index >> 7) as usize), 9i32.wrapping_sub(r.bits), pack(r.code, r.bits));
            } else {
                // `add esi, -6` at 0x56CBD7: the reader reaches this table after consuming the
                // six-bit prefix, so what the entry stores is what is left
                fill(&mut flat, tbl::ESCAPE_B.saturating_add((r.index >> 2) as usize), 0xEi32.wrapping_sub(r.bits), pack(r.code, r.bits.wrapping_sub(6)));
            }
        }
        // array B → the two escape tables; the record's length counts the prefix, which the entry
        // does not (0x56CC2E subtracts 9, 0x56CC62 subtracts 6)
        for r in &b {
            let total = r.bits.wrapping_add(8);
            if r.index & 0x8000 != 0 {
                fill(&mut flat, tbl::ESCAPE_B.saturating_add((r.index >> 10) as usize), 0xEi32.wrapping_sub(total), pack(r.code, total.wrapping_sub(6)));
            } else {
                fill(&mut flat, tbl::ESCAPE_A.saturating_add((r.index >> 7) as usize), 0x11i32.wrapping_sub(total), pack(r.code, total.wrapping_sub(9)));
            }
        }

        // the motion-vector table, written by hand at 0x56CC91 and 0x56CCAB
        flat[tbl::MOTION..tbl::MOTION + 32].fill(1);
        for i in 0..16u32 {
            flat[tbl::MOTION + 32 + i as usize] = 0xFC00_0000u32.wrapping_add(i * 0x40_0000).wrapping_add(0x440_0000) | 6;
            flat[tbl::MOTION + 48 + i as usize] = 0xFC00_0000u32.wrapping_add(i * 0x40_0000) | 6;
        }

        Ok(Self { flat, quant, prescale, scan })
    }

    /// The name of the small table file a self-contained app bundle carries instead of the whole
    /// executable (`packaging/make_app.sh --with-data`, written by the `extract_mad_tables` example).
    pub const FILE: &'static str = "madtables.bin";
    const MAGIC: &'static [u8; 8] = b"NFSMADT1";

    /// These tables as the [`Self::FILE`] blob: the magic, then `flat`, `quant`, `prescale` and
    /// `scan` as little-endian dwords. Only derived data — what [`Self::from_exe`] built — so a
    /// bundle can drop `nfs5.exe`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + 4 * (tbl::LEN + 3 * 64));
        out.extend_from_slice(Self::MAGIC);
        out.extend(self.flat.iter().flat_map(|v| v.to_le_bytes()));
        out.extend(self.quant.iter().flat_map(|v| v.to_le_bytes()));
        out.extend(self.prescale.iter().flat_map(|v| v.to_le_bytes()));
        out.extend(self.scan.iter().flat_map(|v| (*v as u32).to_le_bytes()));
        out
    }

    /// The inverse of [`Self::to_bytes`].
    pub fn from_bytes(d: &[u8]) -> Result<Self, FormatError> {
        let what = Self::FILE;
        let need = 8 + 4 * (tbl::LEN + 3 * 64);
        if d.len() != need {
            return Err(FormatError::Truncated { what, offset: 0, needed: need });
        }
        if &d[..8] != Self::MAGIC {
            return Err(FormatError::UnknownMagic { what, offset: 0, expected: Self::MAGIC.to_vec(), found: d[..8].to_vec() });
        }
        let mut words = d[8..].chunks_exact(4).map(|c| read_u32(c, 0));
        let flat: Vec<u32> = words.by_ref().take(tbl::LEN).collect();
        let mut quant = [0i32; 64];
        let mut prescale = [0i32; 64];
        let mut scan = [0usize; 64];
        quant.iter_mut().for_each(|q| *q = words.next().unwrap_or(0) as i32);
        prescale.iter_mut().for_each(|p| *p = words.next().unwrap_or(0) as i32);
        scan.iter_mut().for_each(|s| *s = words.next().unwrap_or(0) as usize);
        Ok(Self { flat, quant, prescale, scan })
    }

    /// The tables for the game at `root`: [`Self::FILE`] when a bundle carries it, otherwise read
    /// out of the installation's own `nfs5.exe`.
    pub fn load(root: &std::path::Path) -> Result<Self, FormatError> {
        if let Ok(d) = std::fs::read(root.join(Self::FILE)) {
            return Self::from_bytes(&d);
        }
        let exe = std::fs::read(root.join("nfs5.exe"))
            .map_err(|_| FormatError::Truncated { what: "nfs5.exe", offset: 0, needed: 1 })?;
        Self::from_exe(&exe)
    }
}

// -------------------------------------------------------------------------------------------
// the decoder
// -------------------------------------------------------------------------------------------

/// The bitstream: little-endian 16-bit units whose bits are consumed most-significant first
/// (0x56CA69 packs the first two, 0x56D040 refills).
pub struct BitReader<'a> {
    d: &'a [u8],
    pos: usize,
    /// the 32-bit window, MSB-aligned (`[0x6B4F60]`)
    w: u32,
    /// how many of its bits are valid (`[0x6B5084]`)
    avail: i32,
}

impl<'a> BitReader<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        // past the end the reader zero-fills (format behaviour, keep)
        let le16 = |o: usize| -> u32 { get_u16(d, o).map_or(0, u32::from) };
        Self { d, pos: 4, w: (le16(0) << 16) | le16(2), avail: 0x20 }
    }

    /// `0x56D040`.
    pub fn consume(&mut self, n: i32) {
        self.w = self.w.wrapping_shl(n as u32);
        self.avail -= n;
        if self.avail < 16 {
            let next = get_u16(self.d, self.pos).map_or(0, u32::from);
            self.pos += 2;
            self.w |= next.wrapping_shl((16 - self.avail) as u32);
            self.avail += 16;
        }
    }

    /// The window itself, for callers stepping the decode by hand.
    pub fn window(&self) -> u32 {
        self.w
    }

    fn peek(&self, n: u32) -> u32 {
        self.w >> (32 - n)
    }

    /// How far into the data the reader has got, in bytes.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// True once the reader has run past the end of the data it was given.
    pub fn exhausted(&self) -> bool {
        self.pos > self.d.len()
    }
}

/// The 64 quantisers the setup builds (`0x56CA50`): `[0]` is the DC one and `[1..]` the AC ones,
/// indexed by the **raster** position the scan table gives.
pub fn quantisers(t: &MadTables, quant: i8) -> [i32; 64] {
    // `shl edx, 0x10; shr eax, 0x10; adc eax, edx` is a 64-bit `>> 16` that rounds on bit 15
    let mulshift = |a: i64, b: i64| -> i32 { (((a * b) + 0x8000) >> 16) as i32 };
    let mut q = [0i32; 64];
    q[0] = mulshift((t.quant[0] as i64) << 15, t.prescale[0] as i64);
    for i in 1..64 {
        q[i] = mulshift(((t.quant[i] as i64) * quant as i64) << 12, t.prescale[i] as i64);
    }
    q
}

/// Decode one 8×8 block's coefficients (`0x593260`). Returns the number of coefficients; **1**
/// means the block is DC only, which is what makes 0x56D0D0's flat fill correct.
pub fn decode_block(br: &mut BitReader, t: &MadTables, q: &[i32; 64], out: &mut [i32; 64]) -> usize {
    let dc = (br.w as i32) >> 24;
    out.fill(0);
    out[0] = dc.wrapping_mul(q[0]);
    br.consume(8);
    let mut i = 1usize;
    loop {
        let mut e = t.primary(br.peek(9));
        let len = e & 0xFF;
        if len > 9 {
            if len < 0x10 {
                br.consume(9);
                e = t.escape_a(br.peek(8));
            } else if len < 0x20 {
                br.consume(6);
                e = t.escape_b(br.peek(8));
            } else if len < 0x30 {
                br.consume(6);
                e = (br.w & 0xFFFF_0000) | 0x10;
            } else {
                br.consume(2);
                return i;
            }
        }
        let run = ((e >> 16) & 0x3F) as usize;
        let level = (e as i32) >> 22;
        i += run;
        br.consume((e & 0xFF) as i32);
        // The original has no bound here: 0x5933D2 indexes the scan table with the running count
        // and 0x5933DE increments it, and only the end-of-block escape leaves the loop. A block
        // that runs past 63 would read past the table, which the shipped streams never do — so the
        // write is guarded and the loop is not, because leaving early would desynchronise it.
        if let Some(&off) = t.scan.get(i) {
            out[off] = level.wrapping_mul(q[off]);
        }
        i += 1;
        if br.exhausted() {
            return i.min(64);
        }
    }
}

/// AAN constants (`RE/notes/mad.md` §8).
const C_SQRT2_FIX: i64 = 1_518_500_250;
const C_0_5412: f64 = 0.541_196_107_864_379_9;
const C_1_3066: f64 = 1.306_563_019_752_502_4;
const C_0_3827: f64 = 0.382_683_426_141_738_9;
/// `1.5 · 2^52` — adding it to a `f64` puts the rounded integer in the low 32 bits of the pattern,
/// which is how the original gets its integer back (`fadd [0x5E2618]`, `fstp qword`).
const ROUND_MAGIC: f64 = 6_755_399_441_055_744.0;

fn round_magic(v: f64) -> i32 {
    (v + ROUND_MAGIC).to_bits() as u32 as i32
}

/// One 1-D pass of the inverse transform (`0x592E60` / `0x592FBD` — the same butterfly, only the
/// output stride and the all-zero shortcut differ). `src` is eight consecutive coefficients and the
/// eight results go to `out[0]`, `out[stride]`, … .
fn idct_pass(src: &[i32], out: &mut [i32], first: usize, stride: usize, shortcut: bool) {
    let (i0, i1, i2, i3) = (src[0], src[1], src[2], src[3]);
    let (i4, i5, i6, i7) = (src[4], src[5], src[6], src[7]);
    if shortcut && (i1 | i2 | i3 | i4 | i5 | i6 | i7) == 0 {
        for k in 0..8 {
            out[first + k * stride] = i0;
        }
        return;
    }
    // the odd part (0x592E87 … 0x592F12)
    let z1 = i5 - i3;
    let z2 = i1 - i7;
    let t = (i3 + i5) + (i1 + i7);
    let u = (i1 + i7) - (i3 + i5);
    let s = (z1 as f64 + z2 as f64) * C_0_3827;
    let a = round_magic(z1 as f64 * C_0_5412 + s);
    let b = round_magic(z2 as f64 * C_1_3066 - s);
    let u_sqrt2 = ((((u as i64) * C_SQRT2_FIX) >> 32) as i32) << 1;
    let c1 = a.wrapping_add(u_sqrt2);
    let c2 = u_sqrt2.wrapping_add(b);
    let c3 = b.wrapping_add(t);
    // the even part (0x592F18 … 0x592F8A)
    let e26 = (((((i2 - i6) as i64) * C_SQRT2_FIX) >> 32) as i32) << 1;
    let p = (i2 + i6).wrapping_add(e26);
    let x0 = e26.wrapping_add(i0 - i4);
    let x1 = (i0 - i4).wrapping_sub(e26);
    let x2 = p.wrapping_add(i0 + i4);
    let x3 = (i0 + i4).wrapping_sub(p);
    let put = |out: &mut [i32], k: usize, v: i32| out[first + k * stride] = v;
    put(out, 2, x1.wrapping_add(c1));
    put(out, 5, x1.wrapping_sub(c1));
    put(out, 3, x3.wrapping_add(a));
    put(out, 4, x3.wrapping_sub(a));
    put(out, 1, c2.wrapping_add(x0));
    put(out, 6, x0.wrapping_sub(c2));
    put(out, 0, x2.wrapping_add(c3));
    put(out, 7, x2.wrapping_sub(c3));
}

/// The full inverse transform (`0x5930C7`): eight row passes into a transposing intermediate of
/// stride 9, then eight passes back out.
pub fn idct(block: &[i32; 64], out: &mut [i32; 64]) {
    let mut mid = [0i32; 72];
    for r in 0..8 {
        idct_pass(&block[r * 8..r * 8 + 8], &mut mid, r, 9, true);
    }
    for r in 0..8 {
        idct_pass(&mid[r * 9..r * 9 + 8], out, r * 8, 1, false);
    }
}

// -------------------------------------------------------------------------------------------
// frames
// -------------------------------------------------------------------------------------------

/// One decoded frame, RGBA8.
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// A motion-vector component (`0x56D0A0` through the 64-entry table at `0x6B61A0`): a one-bit `0`
/// for zero, else six bits for −16…+16.
fn motion_component(br: &mut BitReader, t: &MadTables) -> i32 {
    let e = t.motion(br.peek(6));
    br.consume((e & 0xFF) as i32);
    (e as i32) >> 22
}

/// A `.mad` decoder. It keeps the pair of surfaces the player alternates (`[0x65B9D4 + idx·8]`,
/// toggled by `[0x65BA64]`): `MADm` predicts out of one and composes into the other.
pub struct MadDecoder {
    tables: MadTables,
    width: usize,
    height: usize,
    /// the surface is packed 4:2:2, two pixels a dword — `width/2` dwords a row (0x4DCA54 halves
    /// the pixel index, 0x56CCEF halves the stride)
    row_dwords: usize,
    surfaces: [Vec<u32>; 2],
    cur: usize,
    luma: [i32; 256],
    chroma: [i32; 128],
}

impl MadDecoder {
    pub fn new(tables: MadTables, header: &MadHeader) -> Self {
        let (width, height) = (header.width.max(0) as usize, header.height.max(0) as usize);
        let row_dwords = width / 2;
        Self {
            tables,
            width,
            height,
            row_dwords,
            surfaces: [vec![0u32; row_dwords * height], vec![0u32; row_dwords * height]],
            cur: 0,
            luma: [0; 256],
            chroma: [0; 128],
        }
    }

    /// Decode one video chunk — header included — into the current surface. Returns how many bytes
    /// of the payload the bit reader took.
    ///
    /// For a whole frame that is the payload itself, **plus up to four**: the reader keeps its
    /// 32-bit window full, so `8·pos − avail` is the bit count and `avail` is 16…32 after every
    /// refill. A frame read at the wrong length lands well outside that. Over the twelve shipped
    /// movies 3346 of 3357 frames land inside it, and in every one of the other eleven the whole
    /// excess is the **last macroblock**, which the bitstream does not reach: the reader runs onto
    /// zeros, the all-zero prefix is the `0x0F` escape, and the entry it lands on is one of the
    /// thirty-two the builder never fills. `RE/notes/mad.md` §10 has it.
    pub fn decode_chunk(&mut self, chunk: &[u8]) -> Result<usize, FormatError> {
        let h = MadHeader::parse(chunk)?;
        let q = quantisers(&self.tables, h.quant);
        let kind = h.kind();
        let (w, rows) = (self.width, self.height);
        let mut br = BitReader::new(&chunk[MAD_HEADER_SIZE..]);
        let mut coef = [0i32; 64];
        let mut block = [0i32; 64];
        for my in (0..rows).step_by(16) {
            for mx in (0..w).step_by(16) {
                let mb = my * self.row_dwords + mx / 2;
                // the mode in front of the macroblock (0x56CCFD…0x56CD56); `MADk` has none
                let (mask, base, parity) = if kind == 0 {
                    (0u32, mb as i32, 0usize)
                } else {
                    let win = br.window();
                    if win & 0xC000_0000 == 0 {
                        br.consume(2);
                        (0, mb as i32, 0)
                    } else {
                        let mask = if win & 0x8000_0000 != 0 {
                            br.consume(1);
                            0x3FF
                        } else {
                            br.consume(8);
                            win >> 24
                        };
                        let mvx = motion_component(&mut br, &self.tables);
                        let mvy = motion_component(&mut br, &self.tables);
                        (mask, mb as i32 + mvy * self.row_dwords as i32 + (mvx >> 1), (mvx & 1) as usize)
                    }
                };
                // four luma blocks then two chroma, at the places 0x56CD68, 0x56CDAB, 0x56CDF1,
                // 0x56CE64, 0x56CE87 and 0x56CECB give them
                let stride = self.row_dwords as i32;
                for (bit, buf_at, ref_at) in [(1u32, 0usize, 0i32), (2, 8, 4), (4, 128, stride * 8), (8, 136, stride * 8 + 4)] {
                    if mask & bit != 0 {
                        let dc = motion_component(&mut br, &self.tables) - 0x40;
                        self.predict_luma(base + ref_at, parity, buf_at, dc);
                    } else {
                        let n = decode_block(&mut br, &self.tables, &q, &mut coef);
                        Self::place(&mut self.luma, buf_at, 16, n, &coef, &mut block);
                    }
                }
                for (bit, buf_at, lane) in [(0x10u32, 0usize, 1u32), (0x20, 64, 0)] {
                    if mask & bit != 0 {
                        let dc = motion_component(&mut br, &self.tables) - 0x40;
                        self.predict_chroma(base, lane, buf_at, dc);
                    } else {
                        let n = decode_block(&mut br, &self.tables, &q, &mut coef);
                        Self::place(&mut self.chroma, buf_at, 8, n, &coef, &mut block);
                    }
                }
                self.compose(mx, my);
            }
        }
        self.cur ^= 1;
        Ok(br.position())
    }

    /// `0x56D110`: 8×8 luma bytes out of the reference into the block buffer's own lane, plus a DC.
    /// `parity` picks the dword's `Y0` lane (byte 3) or `Y1` (byte 2) — an odd motion-vector `x` is
    /// a byte-lane shift, not a half-pel, because the surface packs two pixels a dword.
    fn predict_luma(&mut self, base: i32, parity: usize, buf_at: usize, dc: i32) {
        let prev = &self.surfaces[self.cur ^ 1];
        for r in 0..8usize {
            for k in 0..8usize {
                let pix = parity + k;
                let at = base + r as i32 * self.row_dwords as i32 + (pix / 2) as i32;
                let d = usize::try_from(at).ok().and_then(|i| prev.get(i).copied()).unwrap_or(0);
                let y = if pix.is_multiple_of(2) { d >> 24 } else { d >> 16 } as u8;
                self.luma[buf_at + r * 16 + k] = (y.wrapping_add(dc as u8) as i32) << 16;
            }
        }
    }

    /// `0x56D1F0`: the same for a chroma block. `lane` is 1 for U and 0 for V — the byte inside the
    /// dword — and the row step is two luma rows, because the chroma is 4:2:0.
    fn predict_chroma(&mut self, base: i32, lane: u32, buf_at: usize, dc: i32) {
        let prev = &self.surfaces[self.cur ^ 1];
        for r in 0..8usize {
            for k in 0..8usize {
                let at = base + (r as i32) * 2 * self.row_dwords as i32 + k as i32;
                let d = usize::try_from(at).ok().and_then(|i| prev.get(i).copied()).unwrap_or(0);
                let c = (d >> (8 * lane)) as u8;
                self.chroma[buf_at + r * 8 + k] = (c.wrapping_add(dc as u8) as i32) << 16;
            }
        }
    }

    /// 0x56CF00: sixteen rows through the colour conversion, the chroma row advancing every second.
    fn compose(&mut self, mx: usize, my: usize) {
        let (row_dwords, h) = (self.row_dwords, self.height);
        for r in 0..16usize {
            let dst_row = my + r;
            if dst_row >= h {
                break;
            }
            for k in 0..8usize {
                let px = mx / 2 + k;
                if px >= row_dwords {
                    break;
                }
                let y0 = Self::pixel(self.luma[r * 16 + k * 2]);
                let y1 = Self::pixel(self.luma[r * 16 + k * 2 + 1]);
                let cu = Self::pixel(self.chroma[(r / 2) * 8 + k]);
                let cv = Self::pixel(self.chroma[64 + (r / 2) * 8 + k]);
                // 0x59345B packs them `y0 << 24 | y1 << 16 | u << 8 | v`
                self.surfaces[self.cur][dst_row * row_dwords + px] = (y0 as u32) << 24 | (y1 as u32) << 16 | (cu as u32) << 8 | cv as u32;
            }
        }
    }

    /// Decode one whole frame and hand back its pixels. `chunk` is a video chunk, header included.
    pub fn decode_frame(&mut self, chunk: &[u8]) -> Frame {
        let _ = self.decode_chunk(chunk);
        Frame { width: self.width, height: self.height, rgba: self.rgba() }
    }

    /// The surface the last decode produced, as RGBA8.
    pub fn rgba(&self) -> Vec<u8> {
        Self::to_rgba(&self.surfaces[self.cur ^ 1], self.width, self.height)
    }

    /// Run the inverse transform and drop the 8×8 result into a macroblock buffer of `stride`
    /// dwords. `n == 1` is the DC-only shortcut 0x56D0D0 takes.
    fn place(buf: &mut [i32], base: usize, stride: usize, n: usize, coef: &[i32; 64], block: &mut [i32; 64]) {
        if n == 1 {
            for r in 0..8 {
                for c in 0..8 {
                    buf[base + r * stride + c] = coef[0];
                }
            }
            return;
        }
        idct(coef, block);
        for r in 0..8 {
            for c in 0..8 {
                buf[base + r * stride + c] = block[r * 8 + c];
            }
        }
    }

    /// The pixel inside a transform output: **byte 2** of the dword (0x593427 reads `[esi+2]`),
    /// through the clamp table at 0x6B50A0, which is `clamp(v, −64, 63) + 64` — so the surface is
    /// 7-bit YUV. That `+64` is also why a predicted block's DC is `level − 0x40` (0x56CD88): the
    /// subtraction takes the stored value back to the signed range the table then re-biases, so a
    /// zero-level prediction reproduces the reference exactly.
    fn pixel(v: i32) -> u8 {
        let b = ((v >> 16) & 0xFF) as u8 as i8 as i32;
        (b.clamp(-64, 63) + 64) as u8
    }

    /// The software colour conversion, **0x56A920** — which every movie goes through (its callers
    /// are the intro player 0x4DC710, the `MOVIE` widget 0x4E27FF and the overlay fallback
    /// 0x5690FE) — with the tables **0x56AD70** builds once per display depth.
    ///
    /// A surface dword is read as bytes: byte 0 `V`, byte 1 `U`, byte 3 the first `Y` and byte 2
    /// the second (0x592628…0x592676). Each is 7-bit with a +64 bias ([`pixel`](Self::pixel)).
    /// The tables are JFIF's full-range coefficients, as `f32` (0x5BD8AC…0x5BD8B4), applied to the
    /// chroma **plus a half**: for `c = byte − 64`, `k(c) = rne(f32(c + 0.5) · coef)` (`fistp` in
    /// the default rounding mode, after the product is stored as an `f32` at 0x56ADA1 / 0x56ADE8).
    /// The luma table's three fields start at 89 (R), 67 (G) and 113 (B) and step one per `Y`
    /// (`0x2C821871 + y · 0x800801`, 0x56AD7F / 0x56AE2A); the sums index clamp tables whose entry
    /// `k` is `start + 2k` clamped to 0…255, `start` −177 (R, 0x56AE46), −133 (G, 0x56AEC6) and
    /// −225 (B, 0x56AF46). So, with `y` the stored byte:
    ///
    /// ```text
    /// R = clamp(−177 + 2·(89  + y + rne(1.402·(v−63.5))))
    /// G = clamp(−133 + 2·(67  + y + rne(−0.3441·(u−63.5)) + rne(−0.7141·(v−63.5))))
    /// B = clamp(−225 + 2·(113 + y + rne(1.772·(u−63.5))))
    /// ```
    ///
    /// The fields are nine bits apiece and the sums never leave the tables (R 0…305 of 306, G
    /// 0…261 of 262, B 0…353 of 354). This is the 32-bit row writer 0x5926EF; on a 15/16-bit
    /// display 0x592610 takes the same tables with each entry holding two quantisations —
    /// `(c + 4) >> 3` (G: `(c + 2) >> 2`) capped, for the first pixel of the pair, and `c >> 3`
    /// (`c >> 2`) for the second — a horizontal dither this true-colour port does not reproduce.
    fn to_rgba(yuv: &[u32], w: usize, h: usize) -> Vec<u8> {
        let k = |c: u32, coef: f32| (((c as i32 - 64) as f32 + 0.5) * coef).round_ties_even() as i32;
        let row_dwords = w / 2;
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..row_dwords {
                let p = yuv[y * row_dwords + x];
                let (u, v) = ((p >> 8) & 0xFF, p & 0xFF);
                let rv = k(v, 1.402);
                let g = k(u, -0.3441) + k(v, -0.7141);
                let bu = k(u, 1.772);
                for (j, yv) in [(p >> 24) & 0xFF, (p >> 16) & 0xFF].into_iter().enumerate() {
                    let yv = yv as i32;
                    let o = (y * w + x * 2 + j) * 4;
                    if o + 4 <= out.len() {
                        out[o] = (-177 + 2 * (89 + yv + rv)).clamp(0, 255) as u8;
                        out[o + 1] = (-133 + 2 * (67 + yv + g)).clamp(0, 255) as u8;
                        out[o + 2] = (-225 + 2 * (113 + yv + bu)).clamp(0, 255) as u8;
                        out[o + 3] = 255;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod colour_tests {
    /// 0x56AD70's tables at a few points: the neutral grey, full-scale luma, and the chroma
    /// extremes that set the tables' lengths.
    #[test]
    fn the_software_conversion_matches_the_tables() {
        let px = |y0: u32, y1: u32, u: u32, v: u32| {
            let rgba = super::MadDecoder::to_rgba(&[(y0 << 24) | (y1 << 16) | (u << 8) | v], 2, 1);
            ([rgba[0], rgba[1], rgba[2]], [rgba[4], rgba[5], rgba[6]])
        };
        // c = 0 → c + 0.5: R +1 (0.701), G 0 and 0, B +1 (0.886)
        assert_eq!(px(64, 0, 64, 64), ([131, 129, 131], [3, 1, 3]));
        assert_eq!(px(127, 127, 64, 64).0, [255, 255, 255]);
        // v = 127: 1.402·63.5 = 89.03 → 89; u = 0: 1.772·−63.5 → −113
        assert_eq!(px(64, 64, 0, 127).0, [255, 83, 0]);
    }
}
