//! The name-box cheat codes — **0x4D8680**, which `CREATEPLAYERDONE` (0x510804) asks before it
//! makes a profile (`RE/notes/controls.md` §4.147).
//!
//! The typed name is copied, zero-padded to nine bytes when shorter (0x4D86AA…0x4D86C8), and its
//! first eight bytes are **bit-transposed** — byte `j` of the result takes bit `j` of each byte `i`
//! as its bit `i` (0x4D86E6…0x4D870F). The eight bytes are compared with the seven 12-byte records
//! at **0x5D9618** (eight code bytes and a dword id, 0x4D8718…0x4D8738). A match plays front-end
//! sound **0xC** (0x4D8745) and sets bit *id* of `[0x65B940]` (0x4D8771…0x4D8781), except id 4,
//! which calls 0x5012E0(−1) instead — no shipped record carries 4. `0x4D8790(n)` reads bit `n`.

/// The records at 0x5D9618, as the image holds them.
pub const TABLE: [([u8; 8], u32); 7] = [
    ([0x0E, 0x14, 0x01, 0x04, 0x10, 0x1E, 0x1F, 0x00], 0),
    ([0x53, 0xA1, 0x6F, 0x1C, 0xA2, 0xFE, 0xFF, 0x00], 1),
    ([0x15, 0xDA, 0xEF, 0x88, 0x72, 0xFF, 0xFF, 0x00], 3),
    ([0xDD, 0xAA, 0x88, 0x11, 0x33, 0x77, 0xFF, 0x00], 6),
    ([0x52, 0x61, 0x57, 0x88, 0xE4, 0xFF, 0x7F, 0x00], 8),
    ([0x52, 0xAD, 0x63, 0x9C, 0xDE, 0xFE, 0xFF, 0x00], 5),
    ([0x4F, 0x09, 0x42, 0x12, 0xC9, 0xBE, 0xDF, 0x00], 7),
];

/// The transposition 0x4D86E6…0x4D870F applies to the name's first eight bytes.
pub fn encode(name: &str) -> [u8; 8] {
    let mut input = [0u8; 8];
    for (d, s) in input.iter_mut().zip(name.bytes()) {
        *d = s;
    }
    let mut out = [0u8; 8];
    for (j, o) in out.iter_mut().enumerate() {
        for (i, b) in input.iter().enumerate() {
            *o |= ((b >> j) & 1) << i;
        }
    }
    out
}

/// The cheat id a typed name is, if it is one.
pub fn code_of(name: &str) -> Option<u32> {
    let e = encode(name);
    TABLE.iter().find(|(c, _)| *c == e).map(|(_, id)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seven_codes_decode_to_names() {
        // the transposition is its own inverse, so the table reads back as the names it hides
        let names: Vec<String> = TABLE.iter().map(|(c, _)| {
            let mut out = [0u8; 8];
            for (j, o) in out.iter_mut().enumerate() {
                for (i, b) in c.iter().enumerate() {
                    *o |= ((b >> j) & 1) << i;
                }
            }
            String::from_utf8_lossy(&out).trim_end_matches('\0').to_string()
        }).collect();
        assert_eq!(names, ["Dakar", "Gulliver", "evenstvn", "yraGyraG", "fetherw8", "Fuzzyfuz", "Smash Up"]);
        assert_eq!(code_of("Gulliver"), Some(1));
        assert_eq!(code_of("gulliver"), None, "case matters");
        assert_eq!(code_of("Hans"), None);
    }
}
