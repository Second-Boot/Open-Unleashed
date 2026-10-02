//! The front end's paint-colour rows, selection and assignment.

use crate::*;

impl App {
    /// A colour list's rows (0x4B9390, see `combo_rows`): the colours as swatches, the custom
    /// row `FEStrings` 0x263, and the saved custom colours in use in slot order (0x5019F0).
    pub(crate) fn colour_rows(&self, cols: Vec<u32>, at: usize) -> (Vec<ListRow>, Option<usize>) {
        let rgba = colour_rgba;
        let n = cols.len();
        let mut rows: Vec<ListRow> = cols.into_iter().map(|c| ListRow::Swatch(rgba(c))).collect();
        rows.push(ListRow::CustomColour(self.fe.strings.get(0x263).unwrap_or_default().to_string()));
        let saved = self.career.as_ref().map(|c| c.career.custom_colours).unwrap_or_default();
        rows.extend(saved.iter().filter(|c| **c != 0).map(|c| ListRow::Swatch(rgba(*c))));
        (rows, (at < n).then_some(at))
    }

    /// A colour accessor's list colours — `colour(i)` below the count, each the stored dword
    /// byte-swapped (§4.171.1's table) — and the row the port keeps selected:
    ///
    /// * `DealerCar.*` — the dealership's car: its paint sets' colour 0, its interiors;
    /// * `OwnedCar.*` — the garage car's (`savegame.md` §4.7a): the paint sets for the exterior and
    ///   both stripes, the interior list for the interior;
    /// * anything else, and `OwnedCar.*` with no garage car — the catalogue car's paint sets.
    ///
    /// The third is the colour when it is not a row (`OwnedCar::custom`, `Menu::paint_custom`):
    /// a saved custom colour, which is what the accessor's `[+0x2C]` then holds.
    pub(crate) fn colour_state(&self, base: &str) -> Option<(Vec<u32>, usize, Option<u32>)> {
        if matches!(base, "DealerCar.Colour" | "DealerCar.IntColour")
            && let Some(e) = self.dealer_car()
        {
            return Some(if base == "DealerCar.Colour" {
                (e.paint_sets.iter().map(|p| p.packed[0]).collect(), self.fe.dealer.paint, None)
            } else {
                (e.interior_colours.iter().map(|c| rgb_word(*c)).collect(), self.fe.dealer.interior, None)
            });
        }
        if base.starts_with("OwnedCar.")
            && let Some((e, o)) = self.garage_car()
        {
            let set = |k: usize| e.paint_sets.iter().map(|p| p.packed[k]).collect::<Vec<u32>>();
            let k = owned_colour_slot(base);
            return Some(match base {
                "OwnedCar.ExtColour" => (set(0), o.paint_set as usize, o.custom[k]),
                "OwnedCar.IntColour" => (e.interior_colours.iter().map(|c| rgb_word(*c)).collect(), o.trim.interior as usize, o.custom[k]),
                "OwnedCar.S1Colour" => (set(1), o.trim.stripe1 as usize, o.custom[k]),
                _ => (set(2), o.trim.stripe2 as usize, o.custom[k]),
            });
        }
        let m = self.fe.menu.as_ref()?;
        let cols = m.cars.get(m.car_idx).map(|c| c.paint_sets.iter().map(|p| p.packed[0]).collect()).unwrap_or_default();
        Some((cols, m.paint_idx, m.paint_custom))
    }

    /// The saved custom colours in use, in slot order — 0x5019F0(`n`) is the `n`-th non-zero of
    /// `[0x691D98…0x691DB8]`, 0x5019D0 their count.
    fn saved_colours(&self) -> Vec<u32> {
        self.career.as_ref().map(|c| c.career.custom_colours).unwrap_or_default().into_iter().filter(|c| *c != 0).collect()
    }

    /// The list row a colour accessor is on (its slot 10, `[+0x18]`).
    ///
    /// **`OwnedCar.*`** (read 2026-09-30): the accessor's own index, [`Self::colour_idx`]. The
    /// original keeps it beside the colour `[+0x2C]` and the two are written apart: slot 11
    /// (0x4B92B0 → 0x4BC240, `[+0x18] = row`) sets both, while the paint-shop screen's update
    /// 0x511220 — when its car pointer `[this+0x2A4]` differs from `car.slot35()` (0x51123C) —
    /// copies the car's four colours into the accessors with 0x4B9580, which is `[+0x2C] = colour`
    /// and nothing else (0x511259…0x5112A8), and every update hands the accessors' colours back to
    /// the car (0x511480…0x51152C: 0x4B94C0 into the car's slots 37…40). So after a load — or after
    /// a change of car — the swatch is the car's colour and the row is whatever the accessor last
    /// selected (0 from the constructor): the drop-down's highlight and the arrows' step start from
    /// that row, not from the row the car's colour would be. The accessors are built once at
    /// 0x4F1EE0 (no caller: a table entry), so the row survives screens and loads. Slot 10 is
    /// 0x4BC4B0: `[+0x1D]` ? -1 : `[+0x18]` — a plain read, it recounts nothing; the recount is
    /// [`Self::colour_recount`].
    ///
    /// Any other colour accessor (`DealerCar.*`, `GenericCar.Colour`): the row derived from the
    /// port's state — its row, or for a saved custom colour the row `count + 1 + rank` (slot 27
    /// 0x4B9320's `count + 0x501A40(slot) + 1`, see [`Self::picker_tell_accessor`]).
    /// `GenericCar.Colour` (closed 2026-10-02 — settled by c9990e47: a changed car
    /// row sends the accessor to `slot 11(0)`): while the colour is a saved custom one, its row is
    /// the row slot 11 was given (`FeState::generic_colour_row`, 0x4BC240), so two equal saved colours
    /// keep their own rows. The dealership's two boxes have `allowcustom=0` (dealership.lay): only a
    /// colour's row is ever stored there, the derived row is the stored one. Not kept: the initial
    /// by-value start (the ctor's `[+0x18]` = 0 while the lit row follows JJ's capture).
    pub(crate) fn colour_row_of(&self, base: &str) -> Option<usize> {
        let (cols, at, custom) = self.colour_state(base)?;
        if base.starts_with("OwnedCar.") && self.garage_car().is_some() {
            return Some(self.colour_idx[owned_colour_slot(base)].get());
        }
        Some(match custom {
            // `GenericCar.Colour`: the row slot 11 was given for the saved colour
            // (`[+0x18]`, 0x4BC240), while it is still that colour; a row past the list is the
            // recount's 0 (0x4BC31C)
            Some(_) if self.generic_colour_has_row(base) => {
                let row = self.fe.generic_colour_row.map_or(0, |(_, r)| r);
                if row >= cols.len() + self.saved_colours().len() + 1 { 0 } else { row }
            }
            Some(c) => self.saved_colours().iter().position(|s| *s == c).map_or(0, |r| cols.len() + 1 + r),
            None => at,
        })
    }

    /// `GenericCar.Colour` holds a saved custom colour together with the row slot 11 gave it.
    pub(crate) fn generic_colour_has_row(&self, base: &str) -> bool {
        base == "GenericCar.Colour"
            && matches!((self.fe.generic_colour_row, self.fe.menu.as_ref().and_then(|m| m.paint_custom)), (Some((c, _)), Some(m)) if c == m)
    }

    /// `FEColourAccessor`'s slot 2, **0x4B9350** → 0x4BC310(`size`): the recount — `size` is
    /// `slot24(0xFF)` + (with `[+0x30]`) `0x5019D0() + 1`, stored in `[+0x14]`, and a row `[+0x18]`
    /// at or past it becomes 0 (0x4BC31C `jl`). Its callers (enumerated 2026-10-02 from the
    /// vtable: slot 2 is `call [x+8]`): the constructor's own tail (0x4B9268, row 0 anyway) and the
    /// head of every tick of a widget holding the accessor — `FEAccessorWatch::tick` 0x4BD230 (call
    /// 0x4BD24A), which `FeComboBox::tick` 0x4C5460 runs first (0x4C5469), and `refreshRows`
    /// 0x4C53A0 (0x4C53AC). Base slot 2 is `0x514990`, a bare `ret`. Nothing else asks: the arrows
    /// 0x4B95A0 and slot 11 read `[+0x18]` / `[+0x14]` as they are. The port runs it where it
    /// builds a colour box's rows (`combo_rows`, from `bind_lists`, the widgets' tick), for `OwnedCar.*`
    /// (the other accessors keep no row of their own, see [`Self::colour_row_of`]).
    pub(crate) fn colour_recount(&self, base: &str) {
        if !base.starts_with("OwnedCar.") || self.garage_car().is_none() {
            return;
        }
        let Some((cols, _, _)) = self.colour_state(base) else { return };
        let n = cols.len();
        let allow = self.fe.layout_preview.as_ref().is_none_or(|l| l.colour_allows_custom(base));
        let size = if allow { n + self.saved_colours().len() + 1 } else { n };
        let cell = &self.colour_idx[owned_colour_slot(base)];
        if cell.get() >= size {
            cell.set(0);
        }
    }

    /// `FEColourAccessor`'s slot 11, **0x4B92B0**(`row`), on the port's state: a row at or past
    /// the list's size (`slot9()`, the colours plus — with `[+0x30]` — the custom row and the
    /// saved colours, 0x4B9350) becomes 0 (0x4B92C2); a row below the count is that colour
    /// (`[+0x2C] = colour(row)`, 0x4B92DE); the custom row itself changes nothing (0x4B92ED
    /// `je`); a row past it takes the saved colour `0x5019F0(row − count − 1)` (0x4B930C).
    pub(crate) fn colour_select(&mut self, base: &str, row: usize) {
        let Some((cols, _, _)) = self.colour_state(base) else { return };
        let n = cols.len();
        let saved = self.saved_colours();
        let allow = self.fe.layout_preview.as_ref().is_none_or(|l| l.colour_allows_custom(base));
        let size = if allow { n + saved.len() + 1 } else { n };
        let row = if row >= size { 0 } else { row };
        if row == n {
            return;
        }
        // 0x4BC240 stores the row in `[+0x18]`
        if base.starts_with("OwnedCar.") && self.garage_car().is_some() {
            self.colour_idx[owned_colour_slot(base)].set(row);
        }
        let pick = if row < n { Ok(row) } else { Err(saved.get(row - n - 1).copied().unwrap_or(0)) };
        // 0x4BC240 stores the row in `[+0x18]` for `GenericCar.Colour` too: kept with the
        // saved colour it came with; a list row is its own row (`paint_idx`)
        if base == "GenericCar.Colour" {
            self.fe.generic_colour_row = pick.err().map(|c| (c, row));
        }
        self.colour_assign(base, pick);
    }

    /// What `colour_select` ends in: the accessor holds a list row (`Ok`) or a saved custom colour
    /// (`Err`, `[+0x2C]`) — and the restore a cancelled editor makes (0x4C4A08) hands back the
    /// same pair the accessor had.
    pub(crate) fn colour_assign(&mut self, base: &str, pick: Result<usize, u32>) {
        if base.starts_with("OwnedCar.")
            && let Some(c) = self.career.as_mut()
        {
            let sel = c.car_sel.min(c.career.garage.len().saturating_sub(1));
            if let Some(o) = c.career.garage.get_mut(sel) {
                let k = owned_colour_slot(base);
                o.custom[k] = pick.err();
                if let Ok(r) = pick {
                    let r = r as u8;
                    match k {
                        0 => o.paint_set = r,
                        1 => o.trim.interior = r,
                        2 => o.trim.stripe1 = r,
                        _ => o.trim.stripe2 = r,
                    }
                }
                return;
            }
        }
        if matches!(base, "DealerCar.Colour" | "DealerCar.IntColour") && self.dealer_car().is_some() {
            // `allowcustom=0` on both of the dealership's boxes, so only a row reaches here
            if let Ok(r) = pick {
                if base == "DealerCar.Colour" {
                    self.fe.dealer.paint = r;
                } else {
                    self.fe.dealer.interior = r;
                }
            }
            return;
        }
        if let Some(m) = self.fe.menu.as_mut() {
            m.paint_custom = pick.err();
            if let Ok(r) = pick {
                m.paint_idx = r;
            }
        }
    }

    /// A colour list's step, one row per unit of `delta` (forward is the caption-strip release,
    /// back only the port's own Left key — §4.171.8): the wrapping pair slot 21
    /// 0x4BC690 (at the last row go to row 0, else slot 23 = slot 19(+1)) and slot 20 0x4BC660
    /// (at row 0 go to the last row, else slot 22 = slot 19(−1)), whose slot 19 is
    /// `FEColourAccessor`'s own **0x4B95A0**: `slot11(cur + d)`, stepping **over** the custom row
    /// (`cur + d == count` → `+ d` again, 0x4B95B5). So the arrows walk the colours and then the
    /// saved colours, never opening the editor. The last row is the list's size less one, and
    /// with no saved colours that is the custom row itself, where slot 11 does nothing — so
    /// stepping back from row 0 then stays put, as in the original.
    pub(crate) fn colour_step(&mut self, base: &str, delta: i32) {
        for _ in 0..delta.unsigned_abs() {
            let Some((cols, _, _)) = self.colour_state(base) else { return };
            let n = cols.len();
            let allow = self.fe.layout_preview.as_ref().is_none_or(|l| l.colour_allows_custom(base));
            let size = if allow { n + self.saved_colours().len() + 1 } else { n };
            let cur = self.colour_row_of(base).unwrap_or(0);
            self.colour_select(base, colour_step_row(cur, n, size, delta > 0));
        }
    }

    /// A colour accessor's current colour, `[+0x2C]` (0x4B94C0) — what a closed `NFS5.ColorBox`
    /// paints as its swatch (§4.171.4), as `[r, g, b, a]` with `a` the dword's `b0`.
    pub(crate) fn colour_value(&self, name: &str) -> Option<[u8; 4]> {
        let base = name.split_once('(').map_or(name, |(b, _)| b.trim());
        if !COLOUR_ACCESSORS.contains(&base) {
            return None;
        }
        let (cols, at, custom) = self.colour_state(base)?;
        custom.or(cols.get(at).copied()).map(colour_rgba)
    }
}

/// The row a colour list's arrow asks slot 11 for (see `App::colour_step`): `cur` the row it is
/// on, `n` the colour count, `size` the list's rows. Forward, slot 21 0x4BC690: the last row wraps
/// to 0, else 0x4B95A0(+1). Back, slot 20 0x4BC660: row 0 wraps to the last, else 0x4B95A0(−1).
/// 0x4B95A0 adds `d` once more when it lands on the custom row `n` (0x4B95B5, an equality test
/// of `cur + d` against the count, so the sign does not matter). Slot 11 then maps a row at or
/// past `size` to 0 (0x4B92C0, a signed `jl` against `slot9()`).
fn colour_step_row(cur: usize, n: usize, size: usize, forward: bool) -> usize {
    let d: i64 = if forward { 1 } else { -1 };
    if forward && cur + 1 >= size {
        return 0;
    }
    if !forward && cur == 0 {
        return size.saturating_sub(1);
    }
    let mut r = cur as i64 + d;
    if r == n as i64 {
        r += d;
    }
    r.max(0) as usize
}

/// Which of `FECar+0x674…+0x680` an `OwnedCar.*Colour` accessor stands for — the order of the
/// setters 37…40 (0x4F6F90…0x4F7020) and of `OwnedCar::custom`.
fn owned_colour_slot(base: &str) -> usize {
    match base {
        "OwnedCar.ExtColour" => 0,
        "OwnedCar.IntColour" => 1,
        "OwnedCar.S1Colour" => 2,
        _ => 3,
    }
}

pub(crate) fn colour_rgba(c: u32) -> [u8; 4] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
}

/// **The colour `GenericCar.Colour`/`DealerCar.Colour` starts on — not row 0 of the car later
/// shown.** `FEColourAccessor`'s constructor (0x4B91F0, `controls.md` §4.171.1) sets `[+0x2C] =
/// colour_fn(0)` exactly once, when the accessor itself is built — before `Menu::cars`/`set_cars`
/// ever sorts or filters the merged list (`a7104509`/`dealer_flag_admitted`, a
/// report of 2026-09-30) — so `colour_fn(0)` reads whichever car `GenericCar.Car`'s
/// pointer starts on: `nfs5.car`'s own record **0** (`'50 356 1100 Cabriolet`, the raw file's
/// first record in file order, unsorted), its own paint set 0. `controls.md` §4.171.6: downstream
/// of the accessor there is only ever a **colour**, never an index — the original never resets
/// this to "row 0 of the new car" when the merged list's selection changes, it just keeps the one
/// dword; [`default_paint_row`] is the same "derive the row from the stored colour" shape
/// `colour_row_of` already carries for a saved custom colour. Confirmed against the live
/// original's own QuickRace `цвет` swatch (`RE/reference/2026-09-28/cmp_p10_QuickRace.jpg`,
/// sampled ≈(245,255,210) against record 0's own exact (243,255,204) — JPEG give, not a
/// mismatch): record 19 (`'59 356 A 1600 Super Convertible D`, the shipped default car after
/// `dealer_flag_admitted`) happens to carry that identical dword at its own paint set **2**, which
/// is why the reference capture shows that row lit for *this* car — a different default car would
/// light a different row, or none.
pub(crate) fn default_paint_colour(catalog: &[nfs_formats::fedata::CarCatalogEntry]) -> u32 {
    catalog.first().and_then(|c| c.paint_sets.first()).map(|p| p.packed[0]).unwrap_or(0xFFF3FFCC)
}

/// The row of `paint_sets` carrying [`default_paint_colour`]'s `target` dword, else row 0 — a car
/// with no matching colour of its own, the same fallback `colour_select`'s own `row ≥ size → 0`
/// (0x4B92C2) takes.
pub(crate) fn default_paint_row(target: u32, paint_sets: &[nfs_formats::fedata::PaintSet]) -> usize {
    paint_sets.iter().position(|p| p.packed[0] == target).unwrap_or(0)
}

fn rgb_word(c: [u8; 3]) -> u32 {
    0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32
}

/// The player car's lamp byte `car+0xD87` after `Car_applyInput`'s switch on the packet's command
/// code (0x49933E, jump table 0x499480): bit 0 | 1 the headlights, 2 the high beams, 3 (8) the
/// left and 4 (0x10) the right indicator.
///
/// ```text
/// 10 SIGNALLEFT   0x499365  right on: right off, and if left is on too, done (the hazards
///                           become the left signal); then left toggled
/// 11 SIGNALRIGHT  0x4993A1  the same the other way round
/// 12 SIREN        0x4993DA  both on: both off; else both on
/// 13 LIGHTSON     0x49941D  ^ 3
/// 15 HIGHBEAMS    0x499427  ^ 4
/// 7 / 8 / 9       0x49935A  the police calls, `car+0xD8C` (not a lamp); 14 nothing
/// ```
///
/// (The blink words `+0x1230` / `+0x1232` it also writes have no reader.)
/// An interior colour as the accessor's dword (0x4F0D20's byte swap): the port keeps only the
/// three colour bytes of the entry, and its `b0` alpha is 0xFF in every shipped one.
/// The seven `FEColourAccessor`s (0x4F1EE9…0x4F2200, §4.171.1).
pub(crate) const COLOUR_ACCESSORS: [&str; 7] =
    ["OwnedCar.ExtColour", "OwnedCar.IntColour", "OwnedCar.S1Colour", "OwnedCar.S2Colour", "GenericCar.Colour", "DealerCar.Colour", "DealerCar.IntColour"];

#[cfg(test)]
mod colour_step_tests {
    use super::colour_step_row;

    /// 0x4BC690 / 0x4BC660 over 0x4B95A0: five colours, the custom row 5, two saved colours 6 and
    /// 7 — the arrows walk past the custom row into the saved ones and wrap at both ends.
    #[test]
    fn the_arrows_skip_the_custom_row_and_reach_the_saved_colours() {
        let (n, size) = (5, 8);
        let walk: Vec<usize> = std::iter::successors(Some(0), |&c| Some(colour_step_row(c, n, size, true))).take(9).collect();
        assert_eq!(walk, [0, 1, 2, 3, 4, 6, 7, 0, 1]);
        let back: Vec<usize> = std::iter::successors(Some(0), |&c| Some(colour_step_row(c, n, size, false))).take(9).collect();
        assert_eq!(back, [0, 7, 6, 4, 3, 2, 1, 0, 7]);
        // no saved colours: back from row 0 asks for the custom row, where slot 11 does nothing
        assert_eq!(colour_step_row(0, 5, 6, false), 5);
        // …and forward from the last colour steps over it to 6, past the list, which slot 11
        // (0x4B92C2) turns into row 0
        assert_eq!(colour_step_row(4, 5, 6, true), 6);
        // `allowcustom=0`: the list is the colours alone
        assert_eq!(colour_step_row(0, 5, 5, false), 4);
        assert_eq!(colour_step_row(4, 5, 5, true), 0);
    }
}
