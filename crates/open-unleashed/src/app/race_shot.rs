//! **`I_SCREENSHOT`** (Alt+P, 0x411760's `[0x5E991C]` = 0x4001903): the race's screenshot, `RaceFrame`
//! 0x4158A2…0x415969 (`RE/notes/controls.md` §4.170.2).
//!
//! On the press of bit 0x0D (read whatever the pause, inside the block `[0x606A8C]` == 0 gates, before the
//! F1 read) the original formats `"%sUnleashed%02d.tga"` (0x5CC7D4) over `[0x65B360]` — the save folder
//! `SaveData\` — and the counter `[0x606ADC]`, copies the screen (0x535950 at the display's
//! `[0x5DE654]` × `[0x5DE658]`, 0x534530 / 0x535620 / 0x534550), converts it to format 0x7E (0x535380(0xF),
//! 0x534D40) and writes it with 0x534310 → the TGA writer **0x534010**; then the counter goes up and back to 0
//! at 99 (0x415950…0x415963: `Unleashed00` … `Unleashed98`, an old file overwritten). `[0x606ADC]` has no
//! other writer, so it starts at 0 with the process.
//!
//! The file 0x534010 writes (0x534010…0x53419B): the 18-byte header — id length 0, no colour map, image type
//! 2, the colour-map fields 0 (a 16-bit surface), x origin `[0x5DE634]` and y origin `[0x5DE628] − [0x5DE640]`
//! (0 and 0 for a whole surface), width, height, 16 bits a pixel, descriptor 0x20 (top-left origin) — then
//! the rows top to bottom (0x5341B0, two bytes a pixel), and for a 16-bit surface every pixel through
//! **0x5702A0**: a non-zero `p` becomes `((p >> 1) & 0xFFE0) | (p & 0x1F) | 0x8000`, 0 stays 0.
//!
//! **APPROXIMATION** (not read to the pixel): what the copy holds. The port renders the frame it would show
//! (the scene with the HUD) at the window's size, where the original copies its back buffer as the race
//! frame's input block finds it; and it packs the colours straight into 0x5702A0's output, A1R5G5B5 with the
//! top bit set (a pure black pixel 0, as 0x5702A0 leaves a 0), since the pixel format the surface
//! conversion 0x534D40 / 0x5345A0 hands 0x5702A0 was not traced.

use crate::*;

/// `"%sUnleashed%02d.tga"` (0x5CC7D4) for counter `n`.
pub(crate) fn shot_name(n: u32) -> String {
    format!("Unleashed{n:02}.tga")
}

/// The counter after a shot (0x415950…0x415963): up by one, back to 0 at 99.
pub(crate) fn next_shot(n: u32) -> u32 {
    if n + 1 >= 99 { 0 } else { n + 1 }
}

/// 0x534010's file for an RGBA8 frame `w` × `h` (rows top to bottom), the pixels as 0x5702A0 leaves them.
pub(crate) fn tga_1555(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(18 + (w * h * 2) as usize);
    out.extend_from_slice(&[0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&[16, 0x20]);
    for p in rgba.chunks_exact(4).take((w * h) as usize) {
        let v = (u16::from(p[0] >> 3) << 10) | (u16::from(p[1] >> 3) << 5) | u16::from(p[2] >> 3);
        let v = if v == 0 { 0 } else { v | 0x8000 };
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

impl App {
    /// `I_SCREENSHOT` pressed in a race (see the module notes): the frame as `Unleashed<nn>.tga` in the save
    /// folder, the counter stepped.
    pub(crate) fn race_screenshot(&mut self) {
        let Some((w, h)) = self.renderer.as_ref().map(|r| r.size()) else { return };
        let cam = self.render_camera();
        let scene = self.scene.clone();
        let Some(r) = self.renderer.as_mut() else { return };
        let path = nfs_game::save::save_dir().join(shot_name(self.shot_counter));
        self.shot_counter = next_shot(self.shot_counter);
        match r.render_to_image(&scene, &cam, w, h) {
            Ok(rgba) => match std::fs::write(&path, tga_1555(&rgba, w, h)) {
                Ok(()) => tracing::info!(path = %path.display(), w, h, "I_SCREENSHOT (0x4158A2): written"),
                Err(e) => tracing::warn!(error = %e, path = %path.display(), "I_SCREENSHOT: not written"),
            },
            Err(e) => tracing::warn!(error = %e, "I_SCREENSHOT: the frame could not be rendered"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0x415950…0x415963: `Unleashed00` … `Unleashed98`, then `Unleashed00` again.
    #[test]
    fn the_counter_runs_to_98() {
        assert_eq!(shot_name(0), "Unleashed00.tga");
        assert_eq!(shot_name(7), "Unleashed07.tga");
        let mut n = 0;
        for _ in 0..98 {
            n = next_shot(n);
        }
        assert_eq!(n, 98);
        assert_eq!(next_shot(n), 0);
    }

    /// 0x534010's header and 0x5702A0's pixels.
    #[test]
    fn the_file_is_0x534010s() {
        let rgba = [255, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255];
        let t = tga_1555(&rgba, 3, 1);
        assert_eq!(&t[..18], &[0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 1, 0, 16, 0x20]);
        assert_eq!(&t[18..], &[0x00, 0xFC, 0x00, 0x00, 0xFF, 0xFF]);
    }
}
