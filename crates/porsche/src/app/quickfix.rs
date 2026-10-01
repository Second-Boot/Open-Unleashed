//! The parts-shop QUICKFIX boxes: which `nfs5.prt` categories each box covers.

/// The `nfs5.prt` categories one `QUICKFIX.<name>` box's cost covers. The screen's constructor
/// caches the ten accessors by name at 0x50EEC0…0x50F03E into `[this+0x284]`…`[this+0x2A8]`
/// (ENGINE, BRAKES, TRANSMISSION, SUSPENSION, SHOCKS, **BODYWORK**, SWAYBARS, TIRES,
/// **COSMETIC**, OVERALL), and 0x50F110 adds slot k's getter: 0x4EFBC0 (category 1), 0x4EFC20
/// (2), 0x4EFC80 (3), 0x4EFCE0 (4), 0x4EFD40 (5), **0x4EFDA0 (6 and 7)**, 0x4EFE60 (8),
/// 0x4EFEC0 (9), **0x4EFF20 (10)** and 0x4EFF80 (1…10). `RE/notes/controls.md` §4.96.
/// *(Corrected 2026-09-26: BODYWORK and COSMETIC were swapped — BODYWORK is the dams, spoilers,
/// doors and hood, COSMETIC the body, rims and roof.)*
pub(crate) fn quickfix_kinds(name: &str) -> Option<&'static [u16]> {
    Some(match name {
        "ENGINE" => &[1],
        "BRAKES" => &[2],
        "TRANSMISSION" => &[3],
        "SUSPENSION" => &[4],
        "SHOCKS" => &[5],
        "BODYWORK" => &[6, 7],
        "SWAYBARS" => &[8],
        "TIRES" => &[9],
        "COSMETIC" => &[10],
        "OVERALL" => &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        _ => return None,
    })
}

/// The ten `QUICKFIX.*` names `repair.lay`'s check boxes carry, in the order the accessors are
/// registered at 0x4F252A…0x4F258B. `RE/notes/controls.md` §4.96.
pub(crate) const QUICKFIX_NAMES: [&str; 10] = [
    "ENGINE", "BRAKES", "TRANSMISSION", "SUSPENSION", "SHOCKS", "BODYWORK", "SWAYBARS", "TIRES", "COSMETIC", "OVERALL",
];

#[cfg(test)]
mod quickfix_tests {
    use super::{QUICKFIX_NAMES, quickfix_kinds};

    /// 0x50F14A…0x50F22B: slot k of +0x284…+0x2A8 adds 0x4EFBC0, 0x4EFC20, 0x4EFC80, 0x4EFCE0,
    /// 0x4EFD40, 0x4EFDA0, 0x4EFE60, 0x4EFEC0, 0x4EFF20, 0x4EFF80 — their `cmp word [part+0x26]`s.
    #[test]
    fn each_box_costs_its_slots_categories() {
        let want: [&[u16]; 10] = [&[1], &[2], &[3], &[4], &[5], &[6, 7], &[8], &[9], &[10], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]];
        for (name, kinds) in QUICKFIX_NAMES.iter().zip(want) {
            assert_eq!(quickfix_kinds(name), Some(kinds), "{name}");
        }
    }
}
