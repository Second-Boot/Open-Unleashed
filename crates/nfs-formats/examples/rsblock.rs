//! Diagnostic: every car `mt` record's layer table (`{offset, size, 0, key}` entries in the
//! extension at +76) and the six dwords of its key −1 block — the `Rs` render states
//! `{6, 0x0A, 0x38, 2, 0x18, ?}` (`RE/notes/thrash.md` §6) — tallied per render method.
use nfs_formats::refpack;

fn main() {
    let mut tally = std::collections::BTreeMap::<(String, Vec<i32>), usize>::new();
    let mut bad = 0;
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        for e in crp.material_entries.iter().filter(|e| e.is_part(b"mt")) {
            let (off, len) = (e.data_offset(), e.data_len());
            let Some(b) = data.get(off..off + len) else { continue };
            if b.len() < 312 {
                continue;
            }
            let rm: String = b[16..32].iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
            let w: Vec<u32> = b[76..312].chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
            let Some(i) = w.windows(4).position(|q| q == [0, 0x18, 0, 0xFFFF_FFFF]) else {
                bad += 1;
                continue;
            };
            let mut j = i;
            while j + 4 <= w.len() && w[j + 2] == 0 && w[j + 1] > 0 && w[j + 1] <= 0x40 && w[j] < 0x100 {
                j += 4;
            }
            let rs: Vec<i32> = w[j..j + 6].iter().map(|&v| v as i32).collect();
            *tally.entry((rm, rs)).or_default() += 1;
        }
    }
    for ((rm, rs), n) in tally {
        println!("{rm:12} {rs:?} x{n}");
    }
    println!("no table: {bad}");
}
