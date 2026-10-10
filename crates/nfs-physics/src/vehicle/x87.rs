//! The x87 idioms the force step mirrors: the bare `fistp`, and the two clamp ladders that are not
//! `f32::clamp` (inverted bounds must not panic; NaN follows the compare chain).

/// A bare x87 `fistp`: round to nearest, **ties to even** — which is neither Rust's `f32::round`
/// (ties away from zero) nor `as i32` (truncate). `RE/CONVENTIONS.md` has the rule and the list of
/// the 141 sites in the executable that use it instead of `_ftol`.
pub(super) fn fistp(v: f32) -> i32 {
    v.round_ties_even() as i32
}

/// `Tyre` 0x4A238E…0x4A23CA, the over-limit clamp of the demand `d` (`[esp+0x24]`) to the raw
/// limit `L` (`wheel+0xAC`), written to `wheel+0xA8`: `fcomp [esi+0xAC]; test ah,0x41; jne` —
/// above `L` it takes `L`; otherwise `fld L; fchs; fld d; fcomp st(1); test ah,1; je` — below
/// `−L` (or unordered) it takes `−L`; else `d` stays. Two one-sided compares, never a panic: a
/// negative `L` (no floor since the 2026-09-26 audit) gives `L` for `d > L` and `−L` otherwise.
#[allow(clippy::neg_cmp_op_on_partial_ord)] // `!(d >= -lim)` is the x87 compare chain: true for NaN
pub(super) fn tyre_demand_clamp(d: f32, lim: f32) -> f32 {
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
pub(super) fn sign_clamp(x: f32, c: f32) -> f32 {
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
    use super::fistp;

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
