//! Diagnostic: every slot of a `.bnk` — rate, channels, length in seconds, loop range, the patch
//! volume (code 0x0E) and code 0x0A.
use nfs_formats::bnk::Bnk;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(data) = std::fs::read(&p) else { continue };
        let Ok(bank) = Bnk::parse(&data) else { continue };
        println!("# {p}: {} slots", bank.slot_count());
        for s in 0..bank.slot_count() {
            match bank.decode_slot(&data, s) {
                Ok(pcm) => {
                    let frames = pcm.samples.len() / pcm.channels.max(1) as usize;
                    let h = bank.header(s);
                    let (vol, coarse) = h.map_or((0, 0), |h| (h.patch_volume(), h.coarse_tune()));
                    println!("{s:#04x} {} Hz x{} {:.3} s loop {:?} vol {vol} code0x0A {coarse}", pcm.rate, pcm.channels, frames as f32 / pcm.rate.max(1) as f32, pcm.loop_range);
                }
                Err(e) => println!("{s:#04x} -- {e}"),
            }
        }
    }
}
