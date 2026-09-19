//! For a `MADk` chunk, print how many bytes each of the last blocks took.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let exe = std::fs::read(&a[0]).unwrap();
    let t = nfs_formats::mad::MadTables::from_exe(&exe).unwrap();
    let mad = std::fs::read(&a[1]).unwrap();
    let want: usize = a[2].parse().unwrap();
    let video: Vec<_> = nfs_formats::mad::chunks(&mad).filter(|c| c.is_video()).map(|c| c.data.to_vec()).collect();
    let c = &video[want];
    let h = nfs_formats::mad::MadHeader::parse(c).unwrap();
    println!("{} {}x{} quant={}", String::from_utf8_lossy(&h.magic), h.width, h.height, h.quant);
    let q = nfs_formats::mad::quantisers(&t, h.quant);
    let payload = c.len() - nfs_formats::mad::MAD_HEADER_SIZE;
    let mut br = nfs_formats::mad::BitReader::new(&c[nfs_formats::mad::MAD_HEADER_SIZE..]);
    let mbs = ((h.width as usize).div_ceil(16)) * ((h.height as usize).div_ceil(16));
    let mut coef = [0i32; 64];
    let mut sizes = Vec::new();
    for _ in 0..mbs * 6 {
        let before = br.position();
        let n = nfs_formats::mad::decode_block(&mut br, &t, &q, &mut coef);
        sizes.push((br.position() - before, n));
    }
    println!("payload {payload}, ended at {}", br.position());
    let big: Vec<(usize, (usize, usize))> = sizes.iter().copied().enumerate().filter(|(_, (b, _))| *b > 40).collect();
    println!("blocks over 40 bytes: {:?}", &big[..big.len().min(8)]);
    println!("last 8 blocks: {:?}", &sizes[sizes.len() - 8..]);
    let maxn = sizes.iter().map(|(_, n)| *n).max().unwrap();
    println!("max coefficient count {maxn}");
}
