//! The VC6 STL's `std::sort`, instruction for instruction, for the places where the original's
//! order of **equal keys** is visible (it is not stable above sixteen elements).
//!
//! Read from the 16-byte-record instance under the damage-decal applier 0x447C50 (`RE/notes/
//! car-model.md` §11, 2026-09-27); every instance in the image has the same shape:
//!
//! * the caller: `depth = 2·⌊log2 n⌋` (0x447E3C…0x447E52), then `_Introsort_loop` 0x4483D0 and
//!   `_Final_insertion_sort` 0x4485A0;
//! * `_Introsort_loop` 0x4483D0: while more than 16 elements (`and 0xFFFFFFF0; cmp 0x100`,
//!   0x4483E5 / 0x4484F1) — at depth 0 `partial_sort(first, last, last)`, i.e. `make_heap`
//!   0x448880 and the `pop_heap` loop over `_Adjust_heap` 0x4489E0 (0x448509…0x448594); else
//!   the pivot is the **median of three** of `first`, `first + n/2`, `last − 1` (0x448407…0x448436),
//!   the **unguarded partition** (0x448453…0x4484D4: `while *i < p: ++i; --j; while p < *j: --j;
//!   if !(i < j) break; swap; ++i`), the right part recursed (0x4484E2), the left looped;
//! * `_Final_insertion_sort` 0x4485A0: above 16, `_Insertion_sort` over the first 16
//!   (0x4485C2…0x4486BF: an element below `*first` moves the whole run up, else a linear insert)
//!   and an unguarded linear insert for every later one (0x4486C9…0x448732); 16 or fewer, the
//!   insertion sort alone (0x44873C…0x44882F).
//!
//! `less` is the instance's `operator<`; every comparison in the original is `a < b` (never `<=`),
//! and this keeps each one in the same place, so equal keys land where the original puts them.

/// Sort `v` exactly as the VC6 STL's `std::sort(first, last, less)` does.
pub fn vc6_sort<T: Clone, F: Fn(&T, &T) -> bool>(v: &mut [T], less: F) {
    let n = v.len();
    if n <= 1 {
        return;
    }
    // `for (k = 0; n != 1; n >>= 1) ++k;` (0x447E45…0x447E50)
    let mut k = 0usize;
    let mut m = n;
    while m != 1 {
        m >>= 1;
        k += 1;
    }
    introsort_loop(v, 0, n, 2 * k, &less);
    final_insertion_sort(v, &less);
}

const THRESHOLD: usize = 16;

fn introsort_loop<T: Clone, F: Fn(&T, &T) -> bool>(v: &mut [T], first: usize, mut last: usize, mut depth: usize, less: &F) {
    while last - first > THRESHOLD {
        if depth == 0 {
            partial_sort_all(&mut v[first..last], less);
            return;
        }
        depth -= 1;
        let mid = first + (last - first) / 2;
        let pivot = median(&v[first], &v[mid], &v[last - 1], less).clone();
        let cut = unguarded_partition(v, first, last, &pivot, less);
        introsort_loop(v, cut, last, depth, less);
        last = cut;
    }
}

/// `__median(a, b, c)` (0x44841A…0x448436).
fn median<'a, T, F: Fn(&T, &T) -> bool>(a: &'a T, b: &'a T, c: &'a T, less: &F) -> &'a T {
    if less(a, b) {
        if less(b, c) {
            b
        } else if less(a, c) {
            c
        } else {
            a
        }
    } else if less(a, c) {
        a
    } else if less(b, c) {
        c
    } else {
        b
    }
}

fn unguarded_partition<T, F: Fn(&T, &T) -> bool>(v: &mut [T], mut i: usize, mut j: usize, pivot: &T, less: &F) -> usize {
    loop {
        while less(&v[i], pivot) {
            i += 1;
        }
        j -= 1;
        while less(pivot, &v[j]) {
            j -= 1;
        }
        if i >= j {
            return i;
        }
        v.swap(i, j);
        i += 1;
    }
}

/// `partial_sort(first, last, last)`: `make_heap` 0x448880, then `pop_heap` down to one element.
fn partial_sort_all<T: Clone, F: Fn(&T, &T) -> bool>(v: &mut [T], less: &F) {
    let len = v.len();
    if len >= 2 {
        let mut parent = (len - 2) / 2;
        loop {
            let value = v[parent].clone();
            adjust_heap(v, parent, len, value, less);
            if parent == 0 {
                break;
            }
            parent -= 1;
        }
    }
    let mut last = len;
    while last > 1 {
        last -= 1;
        let value = v[last].clone();
        v[last] = v[0].clone();
        adjust_heap(v, 0, last, value, less);
    }
}

/// `_Adjust_heap` 0x4489E0 with its `_Push_heap` tail.
fn adjust_heap<T: Clone, F: Fn(&T, &T) -> bool>(v: &mut [T], mut hole: usize, len: usize, value: T, less: &F) {
    let top = hole;
    let mut second = 2 * hole + 2;
    while second < len {
        if less(&v[second], &v[second - 1]) {
            second -= 1;
        }
        v[hole] = v[second].clone();
        hole = second;
        second = 2 * second + 2;
    }
    if second == len {
        v[hole] = v[second - 1].clone();
        hole = second - 1;
    }
    while hole > top {
        let parent = (hole - 1) / 2;
        if !less(&v[parent], &value) {
            break;
        }
        v[hole] = v[parent].clone();
        hole = parent;
    }
    v[hole] = value;
}

fn final_insertion_sort<T: Clone, F: Fn(&T, &T) -> bool>(v: &mut [T], less: &F) {
    if v.len() > THRESHOLD {
        insertion_sort(&mut v[..THRESHOLD], less);
        for i in THRESHOLD..v.len() {
            let value = v[i].clone();
            linear_insert(v, i, value, less);
        }
    } else {
        insertion_sort(v, less);
    }
}

fn insertion_sort<T: Clone, F: Fn(&T, &T) -> bool>(v: &mut [T], less: &F) {
    for i in 1..v.len() {
        let value = v[i].clone();
        if less(&value, &v[0]) {
            v[..=i].rotate_right(1);
        } else {
            linear_insert(v, i, value, less);
        }
    }
}

/// `_Unguarded_linear_insert`: move up while `value < *(next − 1)`.
fn linear_insert<T: Clone, F: Fn(&T, &T) -> bool>(v: &mut [T], mut i: usize, value: T, less: &F) {
    while less(&value, &v[i - 1]) {
        v[i] = v[i - 1].clone();
        i -= 1;
    }
    v[i] = value;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_and_keeps_small_runs_stable() {
        // 16 or fewer: the insertion sort alone, which is stable
        let mut v: Vec<(u32, usize)> = [3, 1, 3, 0, 1, 3].iter().enumerate().map(|(i, &k)| (k, i)).collect();
        vc6_sort(&mut v, |a, b| a.0 < b.0);
        assert_eq!(v, vec![(0, 3), (1, 1), (1, 4), (3, 0), (3, 2), (3, 5)]);
    }

    #[test]
    fn large_inputs_sort_by_key() {
        let mut seed = 12345u32;
        for n in [17usize, 35, 46, 200, 1000] {
            let mut v: Vec<(u32, usize)> = (0..n)
                .map(|i| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    ((seed >> 16) % 5, i)
                })
                .collect();
            vc6_sort(&mut v, |a, b| a.0 < b.0);
            assert!(v.windows(2).all(|w| w[0].0 <= w[1].0), "n = {n}");
            let mut idx: Vec<usize> = v.iter().map(|e| e.1).collect();
            idx.sort_unstable();
            assert_eq!(idx, (0..n).collect::<Vec<_>>());
        }
    }

    #[test]
    fn two_keys_above_sixteen_are_not_stable() {
        // 17 records on two pages, alternating: the partition reorders equal keys
        let mut v: Vec<(u32, usize)> = (0..17).map(|i| ((i % 2) as u32, i)).collect();
        vc6_sort(&mut v, |a, b| a.0 < b.0);
        assert!(v.windows(2).all(|w| w[0].0 <= w[1].0));
        let zeros: Vec<usize> = v.iter().filter(|e| e.0 == 0).map(|e| e.1).collect();
        assert_ne!(zeros, (0..17).step_by(2).collect::<Vec<_>>());
    }
}
