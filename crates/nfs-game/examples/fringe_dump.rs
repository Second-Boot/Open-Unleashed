//! Diagnostic (FRINGE): PPM of an entry's mip: raw RGB | alpha | RGB over magenta by alpha.
use nfs_assets::GamePaths;
use nfs_formats::{fsh::Fsh, refpack};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let raw = std::fs::read(paths.track_dir().join(format!("{}.fsh", a[0]))).unwrap();
    let data = refpack::maybe_decompress(&raw).unwrap().into_owned();
    let fsh = Fsh::parse(&data).unwrap();
    let i = fsh.entries.iter().position(|e| e.name == a[1]).unwrap();
    let l: u32 = a[2].parse().unwrap();
    let d = fsh.decode_mip(&data, i, l).unwrap();
    let (w, h) = (d.width as usize, d.height as usize);
    let mut out = format!("P6\n{} {}\n255\n", w * 3, h).into_bytes();
    for y in 0..h {
        for pass in 0..3 {
            for x in 0..w {
                let p = &d.rgba[(y * w + x) * 4..][..4];
                let al = p[3] as u32;
                match pass {
                    0 => out.extend_from_slice(&p[..3]),
                    1 => out.extend_from_slice(&[p[3]; 3]),
                    _ => {
                        let m = [255u32, 0, 255];
                        for k in 0..3 {
                            out.push(((p[k] as u32 * al + m[k] * (255 - al)) / 255) as u8)
                        }
                    }
                }
            }
        }
    }
    std::fs::write(&a[3], out).unwrap();
}
