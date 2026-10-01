//! Allocation-free lower-casing for the front end's per-frame table lookups (performance work
//! A8, 2026-10-01): every `.lay` / `.ctr` key lookup lower-cases its key (`Element::get`,
//! `Part::value`, `Control::part`, …), which was a `String` (and a `format!`) per call, thousands
//! of times a frame. For an ASCII key — every key of the shipped data — the answer is the same
//! bytes `str::to_lowercase` gives, built in a stack buffer; anything else takes the old path.

/// `key.to_lowercase() + suffix`, handed to `f` (the suffix is appended as written).
pub fn with_lower<R>(key: &str, suffix: &str, f: impl FnOnce(&str) -> R) -> R {
    const N: usize = 128;
    let n = key.len() + suffix.len();
    if key.is_ascii() && n <= N {
        let mut buf = [0u8; N];
        for (d, s) in buf.iter_mut().zip(key.bytes()) {
            *d = s.to_ascii_lowercase();
        }
        buf[key.len()..n].copy_from_slice(suffix.as_bytes());
        // the bytes are ASCII from `key` (checked) plus the caller's `&str` suffix
        match std::str::from_utf8(&buf[..n]) {
            Ok(s) => f(s),
            Err(_) => f(&format!("{}{}", key.to_lowercase(), suffix)),
        }
    } else {
        f(&format!("{}{}", key.to_lowercase(), suffix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_to_lowercase_plus_suffix() {
        for k in ["fontPathName", "pos", "", "ÄBC", "X.Y"] {
            for s in ["", ".value", ".screenkey"] {
                assert_eq!(with_lower(k, s, str::to_string), format!("{}{}", k.to_lowercase(), s));
            }
        }
        let long = "A".repeat(200);
        assert_eq!(with_lower(&long, ".value", str::to_string), format!("{}.value", long.to_lowercase()));
    }
}
