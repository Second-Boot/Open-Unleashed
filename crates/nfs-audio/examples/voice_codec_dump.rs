//! TEST GLUE: reads 16-bit PCM from argv[1], writes the port's packets to argv[2] and the decoded
//! float bits (of those packets) to argv[3]; with argv[4], decodes that file of packets instead.
use nfs_audio::voicechat::codec::{Decoder, Encoder, FRAME, PACKET};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pcm: Vec<i16> = std::fs::read(&a[1]).unwrap().chunks(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
    let mut enc = Encoder::new();
    let mut pk = Vec::new();
    for f in pcm.chunks_exact(FRAME) {
        let mut out = [0xEEu8; PACKET];
        enc.encode_into(f.try_into().unwrap(), &mut out);
        pk.extend_from_slice(&out);
    }
    std::fs::write(&a[2], &pk).unwrap();
    let src = if a.len() > 4 { std::fs::read(&a[4]).unwrap() } else { pk };
    let mut dec = Decoder::new();
    let mut o = Vec::new();
    for p in src.chunks_exact(PACKET) {
        let mut out = [0f32; FRAME];
        dec.decode(p, &mut out);
        for v in out { o.extend_from_slice(&v.to_bits().to_le_bytes()); }
    }
    std::fs::write(&a[3], &o).unwrap();
    let w = enc.window();
    let ws: Vec<String> = w.iter().take(4).map(|v| format!("{:#x}", v.to_bits())).collect();
    let cs: Vec<String> = enc.coefficients().iter().map(|v| format!("{:#x}", v.to_bits())).collect();
    println!("window {ws:?} coefs {cs:?}");
    let wh = w.iter().fold(0u64, |h, v| (h ^ v.to_bits() as u64).wrapping_mul(0x100000001b3));
    println!("window hash {wh:#x}");
}
