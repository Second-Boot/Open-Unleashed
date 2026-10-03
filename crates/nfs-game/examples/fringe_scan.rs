//! Diagnostic (FRINGE): per alpha-carrying entry of a track `.fsh`, the colour of the alpha-0 texels
//! against the opaque ones, per mip level — what a bilinear / mip sample blends in at a cut edge.
use nfs_assets::GamePaths;
use nfs_formats::{fsh::Fsh, refpack};
fn main() {
    let name = std::env::args().nth(1).unwrap_or("autobahn".into());
    let paths = GamePaths::locate().unwrap();
    let p = paths.track_dir().join(format!("{name}.fsh"));
    let raw = std::fs::read(&p).unwrap();
    let data = refpack::maybe_decompress(&raw).unwrap().into_owned();
    let fsh = Fsh::parse(&data).unwrap();
    for (i, e) in fsh.entries.iter().enumerate() {
        let b = &e.blocks[0];
        let nl = b.mip_levels();
        let mut line = String::new();
        let mut any = false;
        for l in 0..nl {
            let Ok(d) = fsh.decode_mip(&data, i, l) else { break };
            let (mut n0, mut nm, mut nf, mut s0, mut sf) = (0u32, 0u32, 0u32, [0u64; 3], [0u64; 3]);
            for px in d.rgba.chunks_exact(4) {
                if px[3] == 0 {
                    n0 += 1;
                    for k in 0..3 {
                        s0[k] += px[k] as u64
                    }
                } else if px[3] == 255 {
                    nf += 1;
                    for k in 0..3 {
                        sf[k] += px[k] as u64
                    }
                } else {
                    nm += 1
                }
            }
            if n0 + nm > 0 {
                any = true
            }
            let m = |s: [u64; 3], n: u32| if n == 0 { "-".to_string() } else { format!("{},{},{}", s[0] / n as u64, s[1] / n as u64, s[2] / n as u64) };
            line += &format!(" L{l}[{}x{} a0={} mid={} full={} rgb0={} rgbF={}]", d.width, d.height, n0, nm, nf, m(s0, n0), m(sf, nf));
            if l >= 2 {
                break;
            }
        }
        if any {
            println!("{i} {} fmt={:#x} {}", e.name, b.code, line);
        }
    }
}
