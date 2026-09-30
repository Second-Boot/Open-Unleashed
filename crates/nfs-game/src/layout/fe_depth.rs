//! The front end's 2D kit depth `[0x65B954]` and the device's alpha test — the state its **depth
//! masks** run on (`RE/notes/layouts.md` "The depth mask, to the instruction", `thrash.md` §8).
//!
//! Every enqueuer of the kit takes `[0x65B954]` as the quad's z (0x4D9286, 0x4D96BB, 0x4D9B36,
//! 0x4DA374 …), and the 2D pass draws with depth writes on and `LESSEQUAL` (0x4D30BE / 0x4D30C9)
//! over a depth buffer 0x4DA890 clears to 1.0 once a frame, setting the kit's depth to 0.99998.
//! A mask is 0x4DAA10 (the depth stepped nearer, twice), the stencil's draws, 0x4DAAC0 (flush,
//! `GREATEREQUAL`, the depth 0.001 back), the masked draws, 0x4DAB00 (flush, the compare back, the
//! depth to the stepped value). **Nothing puts the depth back up within a frame**: 0x4DAB00 and
//! 0x4DA9D0 both restore `[0x65B988]`, which 0x4DAA10 / 0x4DA9B0 set to the stepped value — so
//! each mask of a frame sits nearer than every one before it, and its masked draws (at its own
//! depth + 0.001, `GREATEREQUAL`) land only on its own stencil: an earlier stencil is farther, the
//! clear and the frame's depth farther still. That is the one property the port's draw needs, and
//! [`mask_begin`] keeps it by carrying the depth across the frame's widgets, in draw order.
//!
//! **The alpha test's reference** (Thrash 0x24, `dx7z.dll` 0x60003F9C) is not the kit's but the
//! device's, and the front end sets it in two places only (`thrash.md` §6d.4's sweep of every
//! immediate `setstate`): `R3DCarLOD::draw` sets **10** (0x43C86A…0x43C876) for every car and every
//! room — each of the 98 layouts names a `room=`, and a room model is a car model drawn by that class
//! every frame (`frontend-3d.md` §9), after the frame's 2D (the view's list, 0x4D3190) — and a list
//! box's drop-down switches the test **off** for its stencil (0x4C6683, value 0) and leaves **1**
//! (0x4C6696) for the rest of that frame. 0x4DA890's own 0 / 1 (0x4DA910…0x4DA923) is on the arm without a depth buffer, which `dx7z` never
//! takes. So a frame's 2D starts at 10 — the room of the frame before set it — and a list box turns
//! it to 1 for everything after it: [`begin_frame`] and [`set_alpha_test`]. After a race the state is
//! the steady 10 (the race's last car draw is the same `R3DCarLOD::draw`, `thrash.md` §6d.4 —
//! CONFIRMED 2026-09-30). **The first frame after the device is made, observed on the
//! original (Wine, `Drivers/dx7z.dll`, gdb on `_THRASH_setstate` 0x60003AC0, 2026-09-30 SV):** the
//! device does not start at 0 — dx7z's own init (0x6000A6BF…0x6000A6D7) reads its `ALPHATEST`
//! config key (default 0x10) and calls `setstate(0x24, 16)`, so the movies' 2D draws run at 16;
//! the capability probe (0x44E8B3) writes 0x80 and puts the old 16 back (0x44E8C7). The first
//! setter after that in the trace is the view flush's own 10 (0x466673, then 160 and 10 again,
//! 0x4666BC / 0x466709) and `R3DCarLOD::draw`'s (0x43C87C), all **before** the first hit of
//! 0x4DA890 — so the first front-end frame's 2D already sees 10, which is what [`begin_frame`]
//! takes. The row is closed: 10 is right; the device's own start is 16, not the 0 this note used to say
//! (`dx7z.dll` 0x6000A51C is another state's code).

use nfs_renderer::{Sprite, SpriteCompare, SpriteDepth};
use std::cell::Cell;

/// The front end's 2D depth for a frame, `[0x65B954]` = `0x3F7FFF00` (0.99998), set by 0x4DA890
/// (0x4DA892) and 0x4D2FCB before the screens draw.
pub const FE_FRAME_DEPTH: f32 = f32::from_bits(0x3F7F_FF00);

/// The reference `R3DCarLOD::draw` leaves (0x43C876), which every front-end frame's 2D sees.
pub const ROOM_ALPHA_REF: u8 = 10;

thread_local! {
    /// `[0x65B954]`
    static DEPTH: Cell<f32> = const { Cell::new(FE_FRAME_DEPTH) };
    /// Thrash 0x24 as the device holds it
    static ALPHA: Cell<Option<u8>> = const { Cell::new(Some(ROOM_ALPHA_REF)) };
}

/// One step of the kit's depth toward the eye, the rule 0x4DA950 and 0x4DAA10 share: 0.05 above
/// 0.75 (`[0x5B260C]`, `[0x5B26A0]`), 0.01 above 0.25 (`[0x5B6A58]` a double, `[0x5B27A8]`), else
/// 0.001 (`[0x5B27B4]`); `test ah, 0x41` / `jne` takes the smaller step on "not greater". The
/// constants are `float`s but the arithmetic is the x87's, kept in the register (53-bit
/// precision, the MSVC default — here `f64`).
fn depth_step(d: f64) -> f64 {
    if d > 0.75f32 as f64 {
        d - 0.05f32 as f64
    } else if d > 0.25 {
        d - 0.01f32 as f64
    } else {
        d - 0.001f32 as f64
    }
}

/// A mask's two depths from the kit's depth `d` (0x4DAA10, 0x4DAAC0): the depth the stencil
/// writes, and the depth the masked draws take under `GREATEREQUAL`. 0x4DAA10 steps **twice** —
/// 0x4DAA10…0x4DAA52 compute one step in `st0` without storing it, then 0x4DAA58…0x4DAAB9 test and
/// step that value again and store it to `[0x65B954]` and `[0x65B988]` — so from 0.99998 the mask
/// is 0.89998. 0x4DAAC0 adds 0.001 to the stored `float` (0x4DAAE6…0x4DAAF9).
pub fn mask_depths(d: f32) -> (f32, f32) {
    let mask = depth_step(depth_step(d as f64)) as f32;
    let inside = (mask as f64 + 0.001f32 as f64) as f32;
    (mask, inside)
}

/// 0x4DA890 at the start of a frame's 2D: the depth back to 0.99998, and the alpha test as the
/// previous frame's room left it.
pub fn begin_frame() {
    DEPTH.with(|d| d.set(FE_FRAME_DEPTH));
    ALPHA.with(|a| a.set(Some(ROOM_ALPHA_REF)));
}

/// Thrash 0x24 as the device holds it now.
pub fn alpha_test() -> Option<u8> {
    ALPHA.with(|a| a.get())
}

/// A front-end draw that sets Thrash 0x24 (the list box's 0x4C6683 / 0x4C6696).
pub fn set_alpha_test(v: Option<u8>) {
    ALPHA.with(|a| a.set(v));
}

/// **0x4DA950**, the single step (0x4C3E53 in `NFS5.ColorBox`'s editor, 0x4FF3FE / 0x4FF4F0 /
/// 0x4FF606 in the end-of-era show): the depth one step nearer, kept (0x4DA9B0 / 0x4DA9D0 store and
/// restore that same value). No mask, no compare: it only moves what the next mask steps from.
pub fn step_once() {
    DEPTH.with(|d| d.set(depth_step(d.get() as f64) as f32));
}

/// One depth mask's two draw states.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mask {
    /// the stencil's draws: the stepped depth, `LESSEQUAL` (which every stored depth passes), depth
    /// written wherever the alpha test keeps a pixel
    pub stencil: SpriteDepth,
    /// the masked draws: 0.001 back, `GREATEREQUAL` — only over this mask's stencil
    pub inside: SpriteDepth,
}

/// **0x4DAA10** (and the depth 0x4DAAC0 then draws at): step the kit's depth for a mask and keep
/// it, with the alpha test the device holds now for both halves.
pub fn mask_begin() -> Mask {
    let (stencil, inside) = mask_depths(DEPTH.with(|d| d.get()));
    DEPTH.with(|d| d.set(stencil));
    let a = alpha_test();
    Mask {
        stencil: SpriteDepth { z: stencil, compare: SpriteCompare::LessEqual, alpha_test: a },
        inside: SpriteDepth { z: inside, compare: SpriteCompare::GreaterEqual, alpha_test: a },
    }
}

/// Give `sprites` the draw state `d`.
pub fn apply(sprites: &mut [Sprite], d: SpriteDepth) {
    for s in sprites {
        s.depth = Some(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mask_steps_the_depth_twice_then_back() {
        let (mask, inside) = mask_depths(FE_FRAME_DEPTH);
        assert!((mask - 0.89998).abs() < 1e-5, "{mask}");
        assert!((inside - 0.90098).abs() < 1e-5, "{inside}");
        // the masked draws land on the stencil (`GREATEREQUAL`) and on nothing at the frame's depth
        // or the clear's
        assert!(inside >= mask && inside < FE_FRAME_DEPTH && inside < 1.0);
        // the lower ranges: 0.01 at or under 0.75, 0.001 at or under 0.25
        assert_eq!(mask_depths(0.7).0, ((0.7f32 as f64 - 0.01f32 as f64) - 0.01f32 as f64) as f32);
        assert_eq!(mask_depths(0.2).0, ((0.2f32 as f64 - 0.001f32 as f64) - 0.001f32 as f64) as f32);
    }

    #[test]
    fn every_mask_of_a_frame_is_nearer_than_the_one_before() {
        begin_frame();
        let mut last = mask_begin();
        assert_eq!(last.stencil.alpha_test, Some(ROOM_ALPHA_REF));
        for n in 0..100 {
            if n % 7 == 0 {
                step_once();
            }
            let m = mask_begin();
            // the new mask's draws pass over its own stencil and fail over the one before
            assert!(m.inside.z >= m.stencil.z && m.inside.z < last.stencil.z, "{n}: {m:?} after {last:?}");
            last = m;
        }
        begin_frame();
        assert_eq!(mask_begin().stencil.z, mask_depths(FE_FRAME_DEPTH).0);
    }
}
