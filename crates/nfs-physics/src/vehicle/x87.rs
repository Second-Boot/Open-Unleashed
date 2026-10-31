//! The x87 idioms the force step mirrors: the bare `fistp`, and the two clamp ladders that are not
//! `f32::clamp` (inverted bounds must not panic; NaN follows the compare chain).

/// A bare x87 `fistp`: round to nearest, **ties to even** — which is neither Rust's `f32::round`
/// (ties away from zero) nor `as i32` (truncate). `RE/CONVENTIONS.md` has the rule and the list of
/// the 141 sites in the executable that use it instead of `_ftol`.
pub fn fistp(v: f32) -> i32 {
    v.round_ties_even() as i32
}

/// The dot product `0x532910`: `fld/fmul` three times and `faddp` twice in the 80-bit register, one
/// rounding to `f32` at the caller's `fstp`. The products of two `f32` are exact in `f64`; the sum is
/// rounded once to `f32` (differs from the 64-bit mantissa only in a double-rounding case).
pub fn dot_x87(a: glam::Vec3, b: glam::Vec3) -> f32 {
    dot_x87_wide(a, b) as f32
}

/// [`dot_x87`] before the `fstp`: the register value (the race runs at control word 0x27F, 53-bit mantissa), for the
/// callers that keep it in the register (`Susp_update` 0x49A488 `fst [car+0xD6C]`, the value stays on the stack).
pub fn dot_x87_wide(a: glam::Vec3, b: glam::Vec3) -> f64 {
    f64::from(a.x) * f64::from(b.x) + f64::from(a.y) * f64::from(b.y) + f64::from(a.z) * f64::from(b.z)
}

/// `0x5328D0` normalise: `0x532B20` takes `sqrt(x·x + y·y + z·z)` in the register, `1 / length` stays there unrounded
/// (0x5328DB `fdivr [1.0]`) and each component is `component × reciprocal`, rounded once at its `fstp`
/// (0x5328E8…0x5328FC). Control word 0x27F: `f64`. A zero vector stays zero (the callers test for it first).
pub fn normalize_x87(v: glam::Vec3) -> glam::Vec3 {
    let (x, y, z) = (f64::from(v.x), f64::from(v.y), f64::from(v.z));
    let len = (x * x + y * y + z * z).sqrt();
    if len == 0.0 {
        return glam::Vec3::ZERO;
    }
    let inv = 1.0 / len;
    glam::Vec3::new((x * inv) as f32, (y * inv) as f32, (z * inv) as f32)
}

/// `0x532880` → `0x56E8F5` cross product: every component `a.y·b.z − a.z·b.y` is formed in the register (products of two
/// `f32` are exact in `f64`) and rounded once at the `fstp` (0x56E92F…0x56E934).
pub fn cross_x87(a: glam::Vec3, b: glam::Vec3) -> glam::Vec3 {
    let d = |x: f32| f64::from(x);
    glam::Vec3::new(
        (d(a.y) * d(b.z) - d(a.z) * d(b.y)) as f32,
        (d(a.z) * d(b.x) - d(a.x) * d(b.z)) as f32,
        (d(a.x) * d(b.y) - d(a.y) * d(b.x)) as f32,
    )
}

/// The image's `2π` for the helpers 0x5327E0 / 0x5327F0 / 0x532800: the **double** at `[0x5BB2C0]` = 6.28318452835083
/// (not `TAU`; it is 1.2e-7 relative below 2π), `fmul qword [0x5BB2C0]` and then `fsin` / `fcos` on the 80-bit register.
pub const TWO_PI_IMAGE: f64 = 6.28318452835083;

/// `0x532800`: `(sin, cos)` of `turns · [0x5BB2C0]`, both stored to `f32` (0x532818, 0x532826). Used by the rotation builders
/// 0x532600 / 0x5325A0 / 0x532660 (the 64 Hz orientation step 0x49B8E0).
pub fn sin_cos_turns_f32(turns: f32) -> (f32, f32) {
    let (s, c) = nfs_math::portable::sin_cos(f64::from(turns) * TWO_PI_IMAGE);
    (s as f32, c as f32)
}

/// The tyre's two rotations of a force / slip pair by an angle in turns (0x4A2150…0x4A2193, 0x4A2717…0x4A2774): `sin` is stored
/// to `f32` (0x4A215E `fstp`), `cos` stays unrounded in the register, and `x' = cos·x − sin·z`, `z' = cos·z + sin·x`
/// are one register chain each with a single `fstp`.
pub fn rotate_turns(turns: f32, x: f32, z: f32) -> (f32, f32) {
    let (s, c) = nfs_math::portable::sin_cos(f64::from(turns) * TWO_PI_IMAGE);
    let s = f64::from(s as f32);
    ((c * f64::from(x) - s * f64::from(z)) as f32, (c * f64::from(z) + s * f64::from(x)) as f32)
}

/// [`sign_clamp`] on a register value (the roll target stays unrounded, 0x49A695…0x49A6D8).
pub fn sign_clamp_wide(x: f64, c: f32) -> f64 {
    let c = f64::from(c);
    if x > 0.0 {
        if x <= c { x } else { c }
    } else if x > -c {
        x
    } else {
        -c
    }
}

/// `Tyre` 0x4A238E…0x4A23CA, the over-limit clamp of the demand `d` (`[esp+0x24]`) to the raw
/// limit `L` (`wheel+0xAC`), written to `wheel+0xA8`: `fcomp [esi+0xAC]; test ah,0x41; jne` —
/// above `L` it takes `L`; otherwise `fld L; fchs; fld d; fcomp st(1); test ah,1; je` — below
/// `−L` (or unordered) it takes `−L`; else `d` stays. Two one-sided compares, never a panic: a
/// negative `L` (no floor since the 2026-09-26 audit) gives `L` for `d > L` and `−L` otherwise.
#[allow(clippy::neg_cmp_op_on_partial_ord)] // `!(d >= -lim)` is the x87 compare chain: true for NaN
pub fn tyre_demand_clamp(d: f32, lim: f32) -> f32 {
    if d > lim {
        lim
    } else if !(d >= -lim) {
        -lim
    } else {
        d
    }
}

/// `Susp_update`'s sign-first clamps of the pitch (0x49A5A5…0x49A5EC, `specs[0x134]`) and the roll
/// (0x49A695…0x49A6D8, `specs[0x138]`): `fcom 0; test ah,0x41; jne` splits on `x > 0`. Above zero
/// `test ah,0x41; jne` keeps `x <= c`, else `c`; at or below zero (or unordered) `fchs` builds `−c`
/// and `test ah,0x41; je` keeps `x > −c`, else `−c`. For a negative `c` that is `c` for every
/// positive `x` and `−c` for every other — not `f32::clamp`, which panics on inverted bounds.
pub fn sign_clamp(x: f32, c: f32) -> f32 {
    if x > 0.0 {
        if x <= c { x } else { c }
    } else if x > -c {
        x
    } else {
        -c
    }
}

#[cfg(test)]
mod tests {
    use super::{cross_x87, dot_x87, fistp, normalize_x87, rotate_turns, TWO_PI_IMAGE};
    use glam::Vec3;

    /// The register keeps what an `f32` chain loses: `1e8 + 1 − 1e8` is 1 in the 53-bit register, 0 in `f32` steps
    /// (0x532910 `fmul` ×3, `faddp` ×2, one rounding at the `fstp`).
    #[test]
    fn the_dot_product_rounds_once() {
        let (a, b) = (Vec3::new(1.0e8, 1.0, -1.0e8), Vec3::ONE);
        assert_eq!(dot_x87(a, b), 1.0);
        assert_eq!(a.x * b.x + a.y * b.y + a.z * b.z, 0.0);
    }

    /// 0x56E8F5: each component is one rounding of `a.y·b.z − a.z·b.y` (the products are exact in `f64`).
    #[test]
    fn the_cross_product_rounds_once() {
        let c = cross_x87(Vec3::new(0.0, 1.0e8, 0.0), Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(c, Vec3::new(1.0e8, 0.0, 0.0));
        let c = cross_x87(Vec3::new(0.0, 16777217.0, 1.0), Vec3::new(0.0, 1.0, 1.0));
        assert_eq!(c.x, 16777215.0); // 2^24 (the f32 of the first factor) · 1 − 1 · 1
        assert_eq!(normalize_x87(Vec3::ZERO), Vec3::ZERO);
        let n = normalize_x87(Vec3::new(3.0, 0.0, 4.0));
        assert_eq!(n, Vec3::new(0.6, 0.0, 0.8));
    }

    /// 0x5327E0 multiplies by the image's own double (`[0x5BB2C0]`), which is not `2π`: a quarter turn gives a cosine of
    /// about +1.95e-7 (the image's constant is 1.2e-7 relative below `2π`) rather than a true quarter turn's zero.
    #[test]
    fn the_turn_helpers_use_the_images_two_pi() {
        assert_eq!(TWO_PI_IMAGE.to_bits(), 6.28318452835083f64.to_bits());
        let (x, z) = rotate_turns(0.25, 1.0, 0.0);
        assert!((x - 1.947e-7).abs() < 2e-9, "{x}");
        assert!((z - 1.0).abs() < 1e-6);
    }

    /// The hand-written ties-to-even this file had before `round_ties_even`.
    fn fistp_reference(v: f32) -> i32 {
        let r = v.round();
        if (v - v.trunc()).abs() == 0.5 && (r as i32) % 2 != 0 { r as i32 - v.signum() as i32 } else { r as i32 }
    }

    fn same(bits: u32) {
        let v = f32::from_bits(bits);
        assert_eq!(fistp(v), fistp_reference(v), "{v} ({bits:#010x})");
    }

    /// Every half-integer up to 2^24 in both signs (the ties), the values either side of each, and
    /// the specials.
    #[test]
    fn round_ties_even_equals_the_hand_written_ties_to_even_on_ties_and_specials() {
        for k in 0..=(1u32 << 25) {
            let v = k as f32 * 0.5;
            for x in [v, -v, v.next_up(), (-v).next_down(), v.next_down(), (-v).next_up()] {
                same(x.to_bits());
            }
        }
        for x in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -0.0, f32::MAX, f32::MIN, f32::MIN_POSITIVE, 2147483520.0, -2147483648.0, 2147483648.0] {
            same(x.to_bits());
        }
    }

    /// A sparse sweep of every exponent and sign (all of them: `cargo test -- --ignored`).
    #[test]
    fn round_ties_even_equals_the_hand_written_ties_to_even_on_a_sweep_of_bit_patterns() {
        let mut b = 0u32;
        while b < u32::MAX - 4099 {
            same(b);
            b += 4099;
        }
    }

    #[test]
    #[ignore = "all 2^32 bit patterns, about two minutes in a debug build"]
    fn round_ties_even_equals_the_hand_written_ties_to_even_on_every_f32() {
        for b in 0..=u32::MAX {
            same(b);
        }
    }
}
