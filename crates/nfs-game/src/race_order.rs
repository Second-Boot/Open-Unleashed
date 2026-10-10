//! `0x5E8B88[]` — the race-order array (read 2026-09-26, `0x417710` to the instruction).
//!
//! `Car_register` 0x40F189 fills the array with every real car in registration order — the human,
//! the opponents, the cops, the traffic (`Race_setCars` 0x4ECF00) — and `0x417800`, the even-tick
//! race-order pass, ends by calling `0x417710` (0x4179BF), which bubble-sorts it to completion
//! (0x41771A…0x417760: passes until one makes no swap) ascending by `car+0xAF8 − (float)car+0xB20`
//! (0x41772D…0x417745: `fild [0xB20]; fsubr [0xAF8]` for both entries, `fcompp`), swapping only
//! when the later entry's key is **strictly** less (`test ah, 1; je` skips `>=`). Then every car
//! is told its place, `car+0x528 = index` (0x417768…0x417781). A stable sort that starts from the
//! previous sort's order: equal keys keep whatever order the last change of lead left them in,
//! which at the start is the registration order.
//!
//! The consumers read the array during the tick pair that follows, i.e. the order the previous
//! even tick's progress values gave: the racing AI's "two entries after my own" (0x40C2CC), the
//! lane pass's walk outward from `car+0x528` (0x401FA2…0x401FD3), the purgatory dtor's lead pick
//! (0x40E00F). (history: `game-history.md` §7.1)
//!
//! The second array `0x5E8BAC[]` (the same sort on `car+0xAF8` alone, with the swap records
//! `+0x550` / `+0x554`) is [`crate::traffic::TrafficSet::overtakes`].

/// `0x5E8B88[]` and every car's `car+0x528`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RaceOrder {
    /// the array: registration indices, ascending by the last sort's keys
    order: Vec<usize>,
    /// `car+0x528` per registration index
    place: Vec<usize>,
}

impl RaceOrder {
    /// The array as `Car_register` leaves it: `n` cars in registration order.
    pub fn new(n: usize) -> Self {
        Self { order: (0..n).collect(), place: (0..n).collect() }
    }

    /// `0x417710`'s first sort on this tick's keys, one per registration index (`car+0xAF8 −
    /// car+0xB20`; a car out of the world keeps the 0 its progress holds). An empty array — what
    /// the race start leaves, `Cars_resetAll` 0x4110A0 putting both arrays back in registration
    /// order on every pass of the session loop — or a different car count starts in registration
    /// order.
    pub fn resort(&mut self, keys: &[f32]) {
        let n = keys.len();
        if self.order.len() != n {
            *self = Self::new(n);
        }
        // 0x41771A…0x417760: `ebx = n − 1` compares per pass, `edi` = "swapped", repeat while set
        loop {
            let mut swapped = false;
            for j in 0..n.saturating_sub(1) {
                let (a, b) = (self.order[j], self.order[j + 1]);
                if keys[b] < keys[a] {
                    self.order.swap(j, j + 1);
                    swapped = true;
                }
            }
            if !swapped {
                break;
            }
        }
        // 0x417768…0x417781: `car+0x528 = index`
        for (i, &c) in self.order.iter().enumerate() {
            self.place[c] = i;
        }
    }

    /// The array, registration indices in order.
    pub fn order(&self) -> &[usize] {
        &self.order
    }

    /// `car+0x528` of registration index `car` (its own index before the first sort).
    pub fn place(&self, car: usize) -> usize {
        self.place.get(car).copied().unwrap_or(car)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sort_is_stable_and_keeps_the_previous_order_for_ties() {
        let mut o = RaceOrder::new(4);
        // registration order stands for equal keys
        o.resort(&[0.5, 0.5, 0.5, 0.5]);
        assert_eq!(o.order(), &[0, 1, 2, 3]);
        // car 3 takes the lead (the smallest key sorts first: the array is ascending)
        o.resort(&[0.5, 0.5, 0.5, 0.1]);
        assert_eq!(o.order(), &[3, 0, 1, 2]);
        assert_eq!((o.place(3), o.place(0), o.place(2)), (0, 1, 3));
        // keys equal again: the order the last change of lead left, not the registration order
        o.resort(&[0.5, 0.5, 0.5, 0.5]);
        assert_eq!(o.order(), &[3, 0, 1, 2]);
        // a car passing another with a strictly greater key moves exactly past it
        o.resort(&[0.5, 0.6, 0.5, 0.5]);
        assert_eq!(o.order(), &[3, 0, 2, 1]);
    }

    #[test]
    fn a_new_car_count_starts_the_array_afresh() {
        let mut o = RaceOrder::new(2);
        o.resort(&[0.9, 0.1]);
        assert_eq!(o.order(), &[1, 0]);
        o.resort(&[0.9, 0.1, 0.5]);
        assert_eq!(o.order(), &[1, 2, 0]);
        assert_eq!(o.place(0), 2);
    }
}
