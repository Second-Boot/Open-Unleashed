//! Trace the first blocks of a `.mad` frame.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let exe = std::fs::read(&a[0]).unwrap();
    let t = nfs_formats::mad::MadTables::from_exe(&exe).unwrap();
    let mad = std::fs::read(&a[1]).unwrap();
    let want: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let chunk = nfs_formats::mad::chunks(&mad).filter(|c| &c.tag == b"MADk").nth(want).unwrap();
    let mad = chunk.data.to_vec();
    let h = nfs_formats::mad::MadHeader::parse(&mad).unwrap();
    let q = nfs_formats::mad::quantisers(&t, h.quant);
    println!("q[0]={} q[1]={} q[2]={}", q[0], q[1], q[2]);
    let mut br = nfs_formats::mad::BitReader::new(&mad[nfs_formats::mad::MAD_HEADER_SIZE..]);
    let mut c = [0i32; 64];
    let mbs = (h.width as usize).div_ceil(16) * (h.height as usize).div_ceil(16);
    let mut hist = std::collections::BTreeMap::new();
    let mut dcs = std::collections::BTreeMap::new();
    for b in 0..mbs * 6 {
        let n = nfs_formats::mad::decode_block(&mut br, &t, &q, &mut c);
        *hist.entry(n).or_insert(0usize) += 1;
        if b < 6 {
            let nz: Vec<(usize, i32)> = c.iter().enumerate().filter(|(_, v)| **v != 0).map(|(i, v)| (i, *v)).collect();
            println!("block {b}: n={n} dc={} nonzero={} {:?}", c[0], nz.len(), &nz[..nz.len().min(6)]);
        }
        *dcs.entry(c[0] / q[0]).or_insert(0usize) += 1;
    }
    println!("macroblocks {mbs}, blocks {}", mbs * 6);
    println!("coefficient-count histogram (first 8): {:?}", hist.iter().take(8).collect::<Vec<_>>());
    println!("distinct DCs: {}, first few {:?}", dcs.len(), dcs.iter().take(6).collect::<Vec<_>>());
    println!("bytes consumed: {} of payload {}", br.position(), mad.len() - nfs_formats::mad::MAD_HEADER_SIZE);
}
