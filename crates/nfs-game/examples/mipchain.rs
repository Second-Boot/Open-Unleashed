//! Diagnostic: the mip chain each track texture carries against the one the original builds.
//! A SHPI entry's `+0xC` top nibble is the number of **extra** levels after the base — 0x44D710
//! ("ShrinkMIP") sums `nibble` quarter-size levels after level 0 and decrements it when it drops
//! the top level — and the texture build 0x44D7E0 asks the driver for `c + 1` levels with
//! `c = 1 + floor(log2(w / 4))` (0x44D95F…0x44D976; talloc 0x60008420 stores `n + 1`), i.e. down
//! to a 2-texel side. Prints, per pack, how many entries have each (w, nibble, c) and how the
//! pixel block's length (to the entry's next block, or the next entry) compares with the base
//! plus `nibble` levels (`=`), plus `nibble − 1` levels (`-1`), or neither (`?`).
use nfs_assets::GamePaths;
use nfs_formats::{fsh::Fsh, refpack};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let dir = paths.track_dir();
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fsh"))).collect();
    names.sort();
    for p in names {
        let raw = std::fs::read(&p).unwrap();
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let data = data.into_owned();
        let Ok(fsh) = Fsh::parse(&data) else { continue };
        let mut tally = std::collections::BTreeMap::<(u16, u16, u32, u32, &str), usize>::new();
        for (i, e) in fsh.entries.iter().enumerate() {
            let b = &e.blocks[0];
            let nib = (b.misc[3] >> 12) as u32;
            let mut c = 0;
            let mut x = (b.width as i32) >> 2;
            while x > 0 {
                x >>= 1;
                c += 1;
            }
            let bpp = match b.code & 0x7F { 0x7D => 4, 0x7F => 3, 0x78 | 0x7E | 0x6D => 2, 0x7B => 1, _ => 0 };
            let chain = |levels: u32| {
                let (mut w, mut h, mut n) = (b.width as usize, b.height as usize, 0usize);
                for _ in 0..levels {
                    n += w * h * bpp;
                    w = (w / 2).max(1);
                    h = (h / 2).max(1);
                }
                n
            };
            let have = match e.blocks.get(1) {
                Some(next) => next.offset - b.offset - 16,
                None => fsh.image_block_len(data.len(), i) - 16,
            };
            let kind = if b.code & 0x80 != 0 || bpp == 0 {
                "packed"
            } else if have == chain(nib + 1) {
                "="
            } else if have == chain(nib) {
                "-1"
            } else if have > chain(nib + 1) && have < chain(nib + 1) + 16 {
                "=pad"
            } else {
                "?"
            };
            *tally.entry((b.width, b.height, nib, c, kind)).or_default() += 1;
        }
        println!("# {}", p.file_name().unwrap().to_string_lossy());
        for ((w, h, nib, c, kind), n) in tally {
            println!("  {w:>4}x{h:<4} nibble {nib} c {c} (levels {}) block {kind}  x{n}", c + 1);
        }
    }
}
