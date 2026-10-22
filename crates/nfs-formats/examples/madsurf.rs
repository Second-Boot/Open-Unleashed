//! Decode a `.mad`'s video chunks up to chunk `n` and compare the surface with one dumped from the running original at the
//! end of that frame's macroblock loop (0x4DCA8F / 0x4DD0EC): dwords that differ, and which macroblocks they fall in.
//!
//! `cargo run -p nfs-formats --example madsurf -- <nfs5.exe> <file.mad> <n> <surface dump>`
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let exe = std::fs::read(&a[0]).unwrap();
    let t = nfs_formats::mad::MadTables::from_exe(&exe).unwrap();
    let mad = std::fs::read(&a[1]).unwrap();
    let n: usize = a[2].parse().unwrap();
    let dump = std::fs::read(&a[3]).unwrap();
    let orig: Vec<u32> = dump.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    // with `RUNON=1` a chunk is handed to the decoder with the bytes that follow it in the file (the original reads the
    // whole file into one buffer, and its bit reader has no end), not zeros
    let runon = std::env::var("RUNON").is_ok();
    let video: Vec<_> = nfs_formats::mad::chunks(&mad)
        .filter(|c| c.is_video())
        .map(|c| {
            let off = c.data.as_ptr() as usize - mad.as_ptr() as usize;
            let end = if runon { (off + c.data.len() + 256).min(mad.len()) } else { off + c.data.len() };
            mad[off..end].to_vec()
        })
        .collect();
    let h = nfs_formats::mad::MadHeader::parse(&video[0]).unwrap();
    let (w, hh) = (h.width as usize, h.height as usize);
    let mut d = nfs_formats::mad::MadDecoder::new(t, &h);
    for c in video.iter().take(n + 1) {
        d.decode_chunk(c).unwrap();
    }
    let ours = d.surface();
    let row = w / 2;
    let mut diff = std::collections::BTreeMap::<(usize, usize), usize>::new();
    let mut total = 0;
    for i in 0..ours.len().min(orig.len()) {
        if ours[i] != orig[i] {
            total += 1;
            *diff.entry(((i / row) / 16, (i % row) / 8)).or_default() += 1;
        }
    }
    println!("{}x{} chunk {n}: {} of {} dwords differ; macroblocks (row, col): {:?}", w, hh, total, ours.len(), diff);
    let last = ((hh - 1) / 16, (w - 1) / 16);
    let (my, mx) = last;
    let show = |name: &str, s: &[u32]| {
        let at = (my * 16 + 8) * row + mx * 8 + 2;
        println!("{name} last macroblock {last:?}, row {} dwords: {:08x?}", my * 16 + 8, &s[at..at + 4]);
    };
    show("original", &orig);
    show("port    ", ours);
}
