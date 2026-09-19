//! Decode every video chunk of a `.mad` and compare the bits consumed with the chunk's payload.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let exe = std::fs::read(&a[0]).unwrap();
    let t = nfs_formats::mad::MadTables::from_exe(&exe).unwrap();
    let mad = std::fs::read(&a[1]).unwrap();
    let limit: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(400);
    let video: Vec<_> = nfs_formats::mad::chunks(&mad).filter(|c| c.is_video()).map(|c| c.data.to_vec()).collect();
    let h = nfs_formats::mad::MadHeader::parse(&video[0]).unwrap();
    let mut d = nfs_formats::mad::MadDecoder::new(t, &h);
    let (mut exact, mut off, mut shown) = (0usize, 0usize, 0);
    for (n, c) in video.iter().take(limit).enumerate() {
        let used = d.decode_chunk(c).unwrap();
        let payload = c.len() - nfs_formats::mad::MAD_HEADER_SIZE;
        // the reader keeps its window full, so it may have pulled one 16-bit unit it never used
        if used >= payload && used <= payload + 4 {
            exact += 1;
        } else {
            off += 1;
            if shown < 6 {
                println!("chunk {n} ({}): used {used} of {payload}", String::from_utf8_lossy(&c[..4]));
                shown += 1;
            }
        }
    }
    println!("exact {exact}, off {off}");
}
