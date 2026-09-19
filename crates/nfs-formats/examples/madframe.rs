//! Decode the first frame of a `.mad` and write it as a PPM.
//!
//! `cargo run -p nfs-formats --example madframe -- <nfs5.exe> <file.mad> <out.ppm>`
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 3 {
        eprintln!("usage: madframe <nfs5.exe> <file.mad> <out.ppm>");
        return;
    }
    let exe = std::fs::read(&a[0]).expect("exe");
    let tables = nfs_formats::mad::MadTables::from_exe(&exe).expect("tables");
    let mad = std::fs::read(&a[1]).expect("mad");
    // the nth `MADk` chunk (default the first)
    let want: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    // decode every video chunk up to and including the `want`th, so an inter frame has the frames
    // it predicts from
    let video: Vec<_> = nfs_formats::mad::chunks(&mad).filter(|c| c.is_video()).map(|c| c.data.to_vec()).collect();
    let want = want.min(video.len().saturating_sub(1));
    let h = nfs_formats::mad::MadHeader::parse(&video[0]).expect("header");
    let mut d = nfs_formats::mad::MadDecoder::new(tables, &h);
    let mut f = None;
    for c in video.iter().take(want + 1) {
        f = Some(d.decode_frame(c));
    }
    let f = f.expect("a frame");
    println!("{}x{} through {} chunks", f.width, f.height, want + 1);
    let mut out = format!("P6\n{} {}\n255\n", f.width, f.height).into_bytes();
    for px in f.rgba.chunks_exact(4) {
        out.extend_from_slice(&px[..3]);
    }
    std::fs::write(&a[2], out).expect("write");
    println!("wrote {}", a[2]);
}
