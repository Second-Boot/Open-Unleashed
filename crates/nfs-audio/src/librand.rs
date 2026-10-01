//! The sound library's own random generator **0x598FF0** and the draws a patch start makes from
//! it (0x5907B0 / 0x590420). The generator is the library's alone: its six words
//! `[0x5E347C…0x5E3490]` have no writer but itself (a byte scan of the six addresses finds only
//! 0x598FF0), so the sequence starts from the image's values at process start and every bank-patch
//! start of the game — one-shots, loops, engine layers, ambience, speech, the front end's — draws
//! from the one stream in the order the starts happen. The streams (music, the movies' sound,
//! 0x5612A0) never draw.

use nfs_formats::bnk::PatchRecord;

/// **0x598FF0**: an add-with-carry chain over five words plus a counter. With `s = [0x5E347C,
/// 0x5E3480, 0x5E3484, 0x5E3488, 0x5E348C, 0x5E3490]`:
///
/// ```text
/// t4 = s4 + s5            c = carry (t4 < s5 || t4 < s4)
/// t3 = t4 + (s3 + c)      c = t3 < s3 (unsigned)
/// t2 = t3 + (s2 + c)      c = t2 < s2
/// t1 = t2 + (s1 + c)      c = t1 < s1
/// t0 = s0 + t1 + c        s = [t0, t1, t2, t3, t4, s5 + 1]
/// ```
///
/// and when the counter wraps to 0 the words from `s4` down are incremented while each wraps
/// (0x59906C…0x5990AB), the last one, `s0`, also in the answer. The answer is the new `s0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibRand {
    s: [u32; 6],
}

/// The six words as the image holds them (`.data`, 0x5E347C…0x5E3490).
pub const SEED: [u32; 6] = [0xF22D_0E56, 0x8831_26E9, 0xC624_DD2F, 0x0702_C49C, 0x9E35_3F7D, 0x6FDF_3B64];

impl Default for LibRand {
    fn default() -> Self {
        LibRand { s: SEED }
    }
}

impl LibRand {
    /// 0x598FF0.
    pub fn next(&mut self) -> u32 {
        let [s0, s1, s2, s3, s4, s5] = self.s;
        let t4 = s4.wrapping_add(s5);
        let c = (t4 < s5 || t4 < s4) as u32;
        let t3 = t4.wrapping_add(s3.wrapping_add(c));
        let c = (t3 < s3) as u32;
        let t2 = t3.wrapping_add(s2.wrapping_add(c));
        let c = (t2 < s2) as u32;
        let t1 = t2.wrapping_add(s1.wrapping_add(c));
        let c = (t1 < s1) as u32;
        let mut t0 = s0.wrapping_add(t1.wrapping_add(c));
        let n5 = s5.wrapping_add(1);
        let mut w = [t0, t1, t2, t3, t4, n5];
        if n5 == 0 {
            // `inc` down the words while each wraps (0x59906E…0x5990AB)
            for k in (0..5).rev() {
                w[k] = w[k].wrapping_add(1);
                if w[k] != 0 {
                    break;
                }
            }
            t0 = w[0];
        }
        self.s = w;
        t0
    }

    /// **0x5990C0**: a spread of `x` — `x` clamped to 0…0x10000 (signed), then
    /// `((r & 0x7FFF) − 0x4000) · x >> 14` (`imul`, `sar`): −x…x·(0x3FFF/0x4000). The draw is made
    /// whatever `x` is; a negative `x` answers 0 after it.
    pub fn spread(&mut self, x: i32) -> i32 {
        let x = x.clamp(0, 0x10000);
        let r = self.next();
        (((r & 0x7FFF) as i32 - 0x4000) * x) >> 14
    }
}

/// What a started record takes from its draws — the voice fields 0x590420 writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartDraws {
    /// `voice+0x3E`: the patch pan `code 0x0C + rand(code 0x0D)`, clamped 0…127 (0x590470…0x590487).
    pub pan: u8,
    /// `voice+0x3C`: `clamp(code 0x0E + rand(code 0x0F), 0, 127) · sel / 127` (0x5905A5…0x5905F4;
    /// `0x81020409` and `sar 6` with the sign added back: a divide by 127 toward zero).
    pub level: u8,
    /// Cents added to the voice's pitch `voice+0x68`: `rand(code 0x24)` of the first record plus
    /// `rand(code 0x11)` (0x59052E, 0x590549).
    pub cents: i32,
    /// `voice+0x4A`, the volume step table's start (code 0x1F), 0 without it.
    pub volume_phase: u8,
    /// `voice+0x4B`, the pitch step table's start (code 0x23), 0 without it.
    pub pitch_phase: u8,
}

/// The request's byte `+5`: 0x561960's 0x3C — no caller of the game writes it.
pub const REQUEST_NOTE: i8 = 0x3C;

/// The first record's code 0x24, when set, drawn once per patch start (0x590862…0x590878) —
/// before any range is tested, so even a start that plays nothing draws it — and added to every
/// record's pitch word (0x59052E).
pub fn patch_draw(rng: &mut LibRand, records: &[PatchRecord]) -> i16 {
    match records.first() {
        Some(r) if r.patch_pitch_spread != 0 => rng.spread(r.patch_pitch_spread as i32) as i16,
        _ => 0,
    }
}

/// 0x590420's first draw, **before** the allocation (0x59045E): the pan spread (code 0x0D),
/// `code 0x0C + rand`, clamped 0…127 (0x590470…0x590487).
pub fn record_pan(rng: &mut LibRand, r: &PatchRecord) -> u8 {
    let mut pan = r.pan as i32;
    if r.pan_spread != 0 {
        pan += rng.spread(r.pan_spread as i32);
    }
    pan.clamp(0, 0x7F) as u8
}

/// 0x590420's draws once the voice is allocated: the pitch spread (code 0x11, 0x590541), the
/// volume spread (code 0x0F, 0x5905B4), the volume table's start (code 0x1F, 0x59066D) and the
/// pitch table's (code 0x23, 0x59068A); `patch_cents` is [`patch_draw`]'s, `pan` [`record_pan`]'s.
pub fn record_draws(rng: &mut LibRand, r: &PatchRecord, sel: i8, patch_cents: i16, pan: u8) -> StartDraws {
    // the word `voice+0x68` gets the patch's draw (0x59052E) and then its own (0x590549)
    let mut cents = patch_cents;
    if r.pitch_spread != 0 {
        cents = cents.wrapping_add(rng.spread(r.pitch_spread as i32) as i16);
    }
    let mut vol = r.volume as i32;
    if r.volume_spread != 0 {
        vol += rng.spread(r.volume_spread as i32);
    }
    let vol = vol.clamp(0, 0x7F);
    let level = (vol * sel as i32 / 127) as u8;
    let volume_phase = if r.volume_table_phase != 0 {
        // `movsx ecx` then `div ecx`: an unsigned divide by the sign-extended byte
        (rng.next() % (r.volume_table_phase as i32 as u32)) as u8
    } else {
        0
    };
    let pitch_phase = if r.pitch_table_phase != 0 {
        let r0 = rng.next();
        // `div` by 0 faults in the original; no shipped patch carries code 0x23 without 0x21
        if r.pitch_table_len == 0 { 0 } else { (r0 % r.pitch_table_len as u32) as u8 }
    } else {
        0
    };
    StartDraws { pan, level, cents: cents as i32, volume_phase, pitch_phase }
}

/// A bank patch's start, **0x5907B0**, for its records and the request's byte `+6` (`sel`): the
/// draws, made in the original's order, with `alloc(i)` the voice allocator's answer for record
/// `i` (0x5898F0 makes no draw of its own):
///
/// * [`patch_draw`] first;
/// * then, per record whose ranges hold `sel` and the note ([`PatchRecord::matches`]), 0x590420:
///   [`record_pan`], the allocation, [`record_draws`]. A refused allocation ends the start
///   (0x5908F1 → 0x5909A2): no later record is visited, and the records already started are
///   stopped again (0x5909A6…0x5909CA, 0x565D60 on each handle) — the start answers the error.
///
/// Answers each started record's [`StartDraws`] in record order, and whether an allocation was
/// refused (the started ones are then to be stopped).
pub fn patch_start(rng: &mut LibRand, records: &[PatchRecord], sel: i8, mut alloc: impl FnMut(usize) -> bool) -> (Vec<(usize, StartDraws)>, bool) {
    let mut out = Vec::new();
    let patch_cents = patch_draw(rng, records);
    for (i, r) in records.iter().enumerate() {
        if !r.matches(sel, REQUEST_NOTE) {
            continue;
        }
        let pan = record_pan(rng, r);
        if !alloc(i) {
            return (out, true);
        }
        out.push((i, record_draws(rng, r, sel, patch_cents, pan)));
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> PatchRecord {
        PatchRecord::from_codes(&[])
    }

    #[test]
    fn generator_step_by_hand() {
        // the first call from the image's words, worked through 0x598FF0 by hand
        let mut g = LibRand::default();
        let [s0, s1, s2, s3, s4, s5] = SEED;
        let t4 = s4.wrapping_add(s5); // 0x0E147AE1, carried
        assert_eq!(t4, 0x0E14_7AE1);
        let t3 = t4.wrapping_add(s3 + 1); // no carry out (t3 > s3)
        let t2 = t3.wrapping_add(s2);
        let c2 = (t2 < s2) as u32;
        let t1 = t2.wrapping_add(s1.wrapping_add(c2));
        let c1 = (t1 < s1) as u32;
        let t0 = s0.wrapping_add(t1.wrapping_add(c1));
        assert_eq!(g.next(), t0);
        assert_eq!(g.s, [t0, t1, t2, t3, t4, s5 + 1]);
    }

    #[test]
    fn counter_wrap_carries_down() {
        let mut g = LibRand { s: [5, 0, 0, 0, 0xFFFF_FFFF, 0xFFFF_FFFF] };
        // t4 = 0xFFFFFFFE with carry; t3 = t4 + 0 + 1 = 0xFFFFFFFF (no carry out: not < s3 = 0)
        let r = g.next();
        // the counter wraps to 0, so s4 goes up by one — 0xFFFFFFFF, no further wrap
        assert_eq!(g.s[5], 0);
        assert_eq!(g.s[4], 0xFFFF_FFFF);
        assert_eq!(r, g.s[0]);
    }

    #[test]
    fn spread_range_and_negative() {
        let mut g = LibRand::default();
        for _ in 0..1000 {
            let v = g.spread(22);
            assert!((-22..22).contains(&v), "{v}");
        }
        let before = g.clone();
        assert_eq!(g.spread(-5), 0);
        assert_ne!(g, before, "a negative spread still draws");
        // 0x10000 caps: ((r & 0x7FFF) − 0x4000) << 2
        let mut a = LibRand::default();
        let mut b = LibRand::default();
        let r = b.next();
        assert_eq!(a.spread(0x20000), ((r & 0x7FFF) as i32 - 0x4000) << 2);
    }

    #[test]
    fn draw_order_and_gates() {
        // record 0: pan spread and pitch spread, patch pitch spread 200; record 1 only for sel ≤ 78
        let r0 = PatchRecord { pan_spread: 6, pitch_spread: 40, patch_pitch_spread: 200, ..rec() };
        let r1 = PatchRecord { sel: (0, 78), volume_spread: 10, ..rec() };
        let mut g = LibRand::default();
        let (d, refused) = patch_start(&mut g, &[r0, r1], 0x7F, |_| true);
        assert!(!refused);
        // one record started; three draws in the order 0x24, 0x0D, 0x11
        let mut h = LibRand::default();
        let c24 = h.spread(200);
        let pan = (0x40 + h.spread(6)).clamp(0, 127) as u8;
        let c11 = h.spread(40);
        assert_eq!(d, vec![(0, StartDraws { pan, level: 0x7F, cents: c24 + c11, volume_phase: 0, pitch_phase: 0 })]);
        assert_eq!(g, h);
        // a refused allocation: 0x24 and the first record's pan only, then the patch stops
        let mut g = LibRand::default();
        assert_eq!(patch_start(&mut g, &[r0, r1], 50, |_| false), (vec![], true));
        let mut h = LibRand::default();
        h.spread(200);
        h.spread(6);
        assert_eq!(g, h);
        // both records for a level of 50 (r0 without a range): the second refused after the
        // first started — the start reports the refusal, the caller stops the first again
        let r0 = PatchRecord { pan_spread: 6, ..rec() };
        let mut g = LibRand::default();
        let (d, refused) = patch_start(&mut g, &[r0, r1], 50, |i| i == 0);
        assert!(refused && d.len() == 1 && d[0].0 == 0);
    }

    #[test]
    fn level_scales_by_sel() {
        let r = PatchRecord { volume: 99, ..rec() };
        let mut g = LibRand::default();
        let (d, _) = patch_start(&mut g, &[r], 64, |_| true);
        // 99 · 64 / 127 toward zero
        assert_eq!(d[0].1.level, 49);
        assert_eq!(g, LibRand::default(), "no spread, no draw");
    }

    #[test]
    fn table_phases() {
        let r = PatchRecord { volume_table_phase: 1, pitch_table_len: 100, pitch_table_phase: 1, ..rec() };
        let mut g = LibRand::default();
        let (d, _) = patch_start(&mut g, &[r], 0x7F, |_| true);
        let mut h = LibRand::default();
        #[allow(clippy::modulo_one)] // the draw is taken `% 1` like the original's call site
        let a = h.next() % 1;
        let b = h.next() % 100;
        assert_eq!((d[0].1.volume_phase, d[0].1.pitch_phase), (a as u8, b as u8));
    }
}
