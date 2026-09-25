//! The audio car slots — the eight-entry table `0x655D60` (stride 0x38) that decides which cars
//! other than the listener's own are heard at all (`RE/notes/sfx.md` §6, `engine-audio.md` §4.8).
//!
//! **0x4A7DC0**(first slot, count, screen), called from the audio update 0x4A8120 with the count
//! `{4, 2, 1}[AUDIOQUALITY]` (0x4A815A…0x4A8172; quality 3 turns the whole update off):
//!
//! 1. A list of `count` candidates starts as `{none, 300 m}` (0x4A7DE5…0x4A7DF8). Every car of the
//!    car list but the listener's own (`[0x655F50 + 0x50·screen]`) and those with `+0x7F` clear is
//!    measured to the listener `0x655E50` in 16.16 metres (0x4A7E56…0x4A7E82) and, when nearer than
//!    an entry, inserted before the first entry it is nearer than (strictly — 0x4A7EC6 `jl`),
//!    pushing the last one out (0x4A7EB4…0x4A7EF7). So the list is the nearest `count` cars inside
//!    300 m, in order of distance, earlier cars first among equals.
//! 2. A slot whose car is still a candidate keeps it and takes it off the list; any other occupied
//!    slot is marked free-to-reuse (−1) (0x4A7F1C…0x4A7F54).
//! 3. Each candidate left takes the first slot that is empty or marked (0x4A7F7C…0x4A802B) — its
//!    voices stopped first when it held another car (0x4A7FC8…0x4A8005).
//! 4. A slot still marked is emptied, its voices stopped (0x4A807D…0x4A80F7).
//!
//! A slot is sticky: a car keeps the slot it has for as long as it stays among the nearest.

/// The metres a candidate must be nearer than — the list's initial `0x12C0000` (0x4A7DEE).
pub const RANGE: f32 = 300.0;

/// The slot count per `AUDIOQUALITY` index (0 high … 2 low; 3 plays none) — the table built at
/// 0x4A815A…0x4A816A.
pub fn slot_count(audio_quality: u8) -> usize {
    match audio_quality {
        0 => 4,
        1 => 2,
        2 => 1,
        _ => 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot<K> {
    Empty,
    Marked(K),
    Held(K),
}

/// The slot table of one screen.
#[derive(Debug, Clone)]
pub struct AudioSlots<K> {
    slots: Vec<Slot<K>>,
}

impl<K: Copy + Eq> Default for AudioSlots<K> {
    fn default() -> Self {
        Self { slots: vec![Slot::Empty; 8] }
    }
}

impl<K: Copy + Eq> AudioSlots<K> {
    /// One pass of 0x4A7DC0 over `cars` — each car's key and its distance to the listener in
    /// metres, in the car list's order, the listener's own car and the inactive ones left out.
    /// Returns the slots whose car changed or went, with the car they had: the caller stops those
    /// voices, as 0x4A9890 does.
    pub fn update(&mut self, count: usize, cars: impl IntoIterator<Item = (K, f32)>) -> Vec<(usize, K)> {
        let count = count.min(self.slots.len());
        // 1. the nearest `count` inside RANGE, in 16.16 as the original compares them
        let mut list: Vec<(Option<K>, i32)> = vec![(None, (RANGE * 65536.0) as i32); count];
        for (key, d) in cars {
            let d = (d.max(0.0) * 65536.0) as i32;
            if let Some(at) = list.iter().position(|e| d < e.1) {
                list.insert(at, (Some(key), d));
                list.pop();
            }
        }
        let mut released = Vec::new();
        // 2. keep the slots whose car is still listed; mark the rest
        for s in self.slots.iter_mut().take(count) {
            if let Slot::Held(k) = *s {
                match list.iter_mut().find(|e| e.0 == Some(k)) {
                    Some(e) => e.0 = None,
                    None => *s = Slot::Marked(k),
                }
            }
        }
        // 3. the new ones into the first empty or marked slot
        for (key, _) in list {
            let Some(key) = key else { continue };
            if let Some(i) = self.slots.iter().take(count).position(|s| !matches!(s, Slot::Held(_))) {
                if let Slot::Marked(old) = self.slots[i] {
                    released.push((i, old));
                }
                self.slots[i] = Slot::Held(key);
            }
        }
        // 4. what is still marked goes (the loop, like the others, runs over `count` slots only)
        for (i, s) in self.slots.iter_mut().enumerate().take(count) {
            if let Slot::Marked(old) = *s {
                released.push((i, old));
                *s = Slot::Empty;
            }
        }
        released
    }

    /// The slot `key` holds, if any.
    pub fn slot_of(&self, key: K) -> Option<usize> {
        self.slots.iter().position(|s| *s == Slot::Held(key))
    }

    /// The cars that hold a slot, with it.
    pub fn held(&self) -> impl Iterator<Item = (usize, K)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| match s {
            Slot::Held(k) => Some((i, *k)),
            _ => None,
        })
    }

    /// Empty every slot (a new race), returning what they held.
    pub fn clear(&mut self) -> Vec<(usize, K)> {
        let out = self.held().collect();
        self.slots.iter_mut().for_each(|s| *s = Slot::Empty);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_within_range_and_sticky() {
        let mut t = AudioSlots::<u32>::default();
        // five cars, four slots: the far one (and the one past 300 m) are out
        let r = t.update(4, [(1, 50.0), (2, 10.0), (3, 400.0), (4, 20.0), (5, 30.0), (6, 60.0)]);
        assert!(r.is_empty());
        // taken nearest first: 2, 4, 5, 1
        assert_eq!(t.held().collect::<Vec<_>>(), [(0, 2), (1, 4), (2, 5), (3, 1)]);
        // car 4 drops out, car 6 comes in: 6 takes 4's slot, the others stay put
        let r = t.update(4, [(1, 50.0), (2, 10.0), (4, 500.0), (5, 30.0), (6, 40.0)]);
        assert_eq!(r, [(1, 4)]);
        assert_eq!(t.slot_of(6), Some(1));
        assert_eq!(t.slot_of(1), Some(3));
        // everyone gone: every slot is released
        let r = t.update(4, []);
        assert_eq!(r.len(), 4);
        assert_eq!(t.held().count(), 0);
    }

    #[test]
    fn ties_keep_the_earlier_car() {
        let mut t = AudioSlots::<u32>::default();
        t.update(1, [(7, 20.0), (8, 20.0)]);
        assert_eq!(t.slot_of(7), Some(0));
        assert_eq!(slot_count(0), 4);
        assert_eq!(slot_count(3), 0);
    }
}
