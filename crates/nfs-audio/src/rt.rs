//! The real-time plumbing of the mixer: what lets the game thread talk to the device callback
//! without either ever waiting on a lock the other holds (PERF F5, `RE/notes/PERF.md`).
//!
//! * [`channel`] — a bounded single-producer / single-consumer ring of commands. The game thread
//!   pushes, the callback pops; neither blocks, neither allocates (the slots are made once).
//! * [`SampleRing`] — the same for the music feeder's decoded `i16` samples.
//!
//! The ring is the only `unsafe` in the audio crate: a slot is written by the producer before it
//! publishes the tail (`Release`) and read by the consumer after it has seen that tail
//! (`Acquire`), and the producer reuses a slot only after the consumer has published the head
//! past it, so a slot is never touched by both at once.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicI16, AtomicUsize, Ordering};

struct Ring<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    /// the next slot to pop (written by the consumer only)
    head: AtomicUsize,
    /// the next slot to fill (written by the producer only)
    tail: AtomicUsize,
}

// SAFETY: see the module doc — a slot has one owner at a time, handed over by the head / tail
// atomics; `T` itself only ever moves between the two threads.
unsafe impl<T: Send> Send for Ring<T> {}
unsafe impl<T: Send> Sync for Ring<T> {}

impl<T> Drop for Ring<T> {
    fn drop(&mut self) {
        let (mut h, t) = (*self.head.get_mut(), *self.tail.get_mut());
        while h != t {
            // SAFETY: the slots in `head..tail` hold values pushed and not yet popped
            unsafe { (*self.slots[h & self.mask].get()).assume_init_drop() };
            h = h.wrapping_add(1);
        }
    }
}

pub struct Producer<T>(Arc<Ring<T>>);
pub struct Consumer<T>(Arc<Ring<T>>);

/// A ring of at least `capacity` slots (rounded up to a power of two).
pub fn channel<T>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let n = capacity.max(2).next_power_of_two();
    let slots = (0..n).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect::<Vec<_>>().into_boxed_slice();
    let r = Arc::new(Ring { slots, mask: n - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) });
    (Producer(r.clone()), Consumer(r))
}

impl<T> Producer<T> {
    /// Push `v`; `Err(v)` when the ring is full.
    pub fn push(&mut self, v: T) -> Result<(), T> {
        let r = &*self.0;
        let t = r.tail.load(Ordering::Relaxed);
        if t.wrapping_sub(r.head.load(Ordering::Acquire)) > r.mask {
            return Err(v);
        }
        // SAFETY: the slot at `t` is free (the consumer's head is past it) and only the producer,
        // which this `&mut` is, writes it
        unsafe { (*r.slots[t & r.mask].get()).write(v) };
        r.tail.store(t.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl<T> Consumer<T> {
    pub fn pop(&mut self) -> Option<T> {
        let r = &*self.0;
        let h = r.head.load(Ordering::Relaxed);
        if h == r.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: the slot at `h` was filled before the tail passed it, and only the consumer,
        // which this `&mut` is, takes it out
        let v = unsafe { (*r.slots[h & r.mask].get()).assume_init_read() };
        r.head.store(h.wrapping_add(1), Ordering::Release);
        Some(v)
    }
}

/// A single-producer / single-consumer ring of `i16` samples (the music stream's decoded PCM):
/// `push` from the feeder thread, `pop_frame` from the mixer callback. Safe code only — each
/// sample is an atomic.
pub struct SampleRing {
    buf: Box<[AtomicI16]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl SampleRing {
    pub fn new(capacity: usize) -> Self {
        let n = capacity.max(2).next_power_of_two();
        Self { buf: (0..n).map(|_| AtomicI16::new(0)).collect(), mask: n - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    /// Append what fits; returns the samples taken (the feeder throttles itself well below the
    /// capacity, so a short write is a bug in the caller, not a normal case).
    pub fn push(&self, samples: &[i16]) -> usize {
        let t = self.tail.load(Ordering::Relaxed);
        let free = self.buf.len() - t.wrapping_sub(self.head.load(Ordering::Acquire));
        let n = samples.len().min(free);
        for (k, s) in samples[..n].iter().enumerate() {
            self.buf[t.wrapping_add(k) & self.mask].store(*s, Ordering::Relaxed);
        }
        self.tail.store(t.wrapping_add(n), Ordering::Release);
        n
    }

    /// The samples queued.
    pub fn len(&self) -> usize {
        self.tail.load(Ordering::Acquire).wrapping_sub(self.head.load(Ordering::Acquire))
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The next frame of `cc` samples (1 or 2; the second is the first when mono), or `None` when
    /// fewer than `cc` are queued. Consumer side only.
    pub fn pop_frame(&self, cc: usize) -> Option<[f32; 2]> {
        let h = self.head.load(Ordering::Relaxed);
        if self.tail.load(Ordering::Acquire).wrapping_sub(h) < cc {
            return None;
        }
        let l = self.buf[h & self.mask].load(Ordering::Relaxed) as f32;
        let r = if cc > 1 { self.buf[h.wrapping_add(1) & self.mask].load(Ordering::Relaxed) as f32 } else { l };
        self.head.store(h.wrapping_add(cc), Ordering::Release);
        Some([l, r])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_order_and_refuses_when_full() {
        let (mut p, mut c) = channel::<Box<u32>>(4);
        for i in 0..4 {
            assert!(p.push(Box::new(i)).is_ok());
        }
        assert_eq!(*p.push(Box::new(9)).unwrap_err(), 9);
        assert_eq!(*c.pop().unwrap(), 0);
        assert!(p.push(Box::new(4)).is_ok());
        let got: Vec<u32> = std::iter::from_fn(|| c.pop().map(|b| *b)).collect();
        assert_eq!(got, [1, 2, 3, 4]);
        assert!(c.pop().is_none());
    }

    #[test]
    fn the_ring_hands_values_across_threads() {
        let (mut p, mut c) = channel::<u64>(64);
        let t = std::thread::spawn(move || {
            let mut i = 0u64;
            while i < 100_000 {
                if p.push(i).is_ok() {
                    i += 1;
                }
            }
        });
        let mut next = 0u64;
        while next < 100_000 {
            if let Some(v) = c.pop() {
                assert_eq!(v, next);
                next += 1;
            }
        }
        t.join().unwrap();
    }

    #[test]
    fn the_sample_ring_pops_whole_frames() {
        let r = SampleRing::new(8);
        assert_eq!(r.push(&[1, 2, 3]), 3);
        assert_eq!(r.pop_frame(2), Some([1.0, 2.0]));
        assert_eq!(r.pop_frame(2), None);
        assert_eq!(r.len(), 1);
        assert_eq!(r.pop_frame(1), Some([3.0, 3.0]));
        assert!(r.is_empty());
    }
}
