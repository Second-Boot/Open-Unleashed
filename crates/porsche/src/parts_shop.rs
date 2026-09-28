//! The front end's side of the parts shop (`parts.lay`, `ID_BUYPARTSCBL`) and the advanced repair
//! screen (`advancedrepair.lay`, `IDS_ADVREPAIRCBL`): the two baskets and the current rows of the
//! four lists, the rows those lists show, which buttons are live, and the ten callbacks. The rules
//! behind them are `nfs_game::shop`; `RE/notes/controls.md` §4.160 has the screens.
//!
//! Each screen class registers five callbacks through 0x50D760, each a thunk through its vtable:
//!
//! | callback | thunk | `parts.lay` (vtable 0x5BA694) | `advancedrepair.lay` (vtable 0x5BA5A0) |
//! |---|---|---|---|
//! | `BUYPARTCALLBACK` / `fixpart` | 0x4F7680 → +0xDC | 0x519550 | 0x519160 |
//! | `SELLPARTCALLBACK` / `replacepart` | 0x4F7690 → +0xE0 | 0x5195A0 | 0x519190 |
//! | `CLEARLISTCALLBACK` / `clearlist` | 0x4F76A0 → +0xE4 | 0x519600 | 0x5191C0 |
//! | `REMOVEITEMCALLBACK` / `removeitem` | 0x4F76B0 → +0xE8 | 0x519610 | 0x5191D0 |
//! | `PURCHASECALLBACK` / `acceptlist` | 0x510F00 → +0xEC | 0x519650 | 0x5191F0 |
//!
//! `carsetup.lay` (`ID_CARSETUP`, screen constructor 0x4F18D0) adds the two that move parts
//! between the car and the inventory: `UNINSTALLPART` 0x4F1C40 and `INSTALLPART` 0x4F1C90, both
//! on `Parts.InstallList`.

use crate::career_ui::CareerUi;
use nfs_formats::career::{Part, part_category_string};
use nfs_formats::locale::Strings;
use nfs_game::layout::{LayoutScreen, ListCell, ListRow};
use nfs_game::shop::{InstallRow, RepairItem, ShopItem};

/// The ten callback names the two shop screens register (0x519384…0x5193C8,
/// 0x519027…0x51906B) and `carsetup.lay`'s two (0x4F19B3, 0x4F19C4).
pub const CALLBACKS: [&str; 12] = [
    "BUYPARTCALLBACK",
    "SELLPARTCALLBACK",
    "CLEARLISTCALLBACK",
    "REMOVEITEMCALLBACK",
    "PURCHASECALLBACK",
    "fixpart",
    "replacepart",
    "clearlist",
    "removeitem",
    "acceptlist",
    "INSTALLPART",
    "UNINSTALLPART",
];

/// What a row of `Parts.AllParts` is, as 0x5152E0 decodes it — a section header (kind 0), a part
/// of the first section to buy (kind 1, the table record) or a spare of the second (kind 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllPartsRow {
    Header,
    /// a category heading above the next part — the original draws it inside that part's row
    /// (0x516080 answers 13 + 13 for a row that opens a category), so it picks that part
    Heading,
    Buy(i16),
    Sell(usize),
}

/// What a callback asks of the screen once it has run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// done; the lists and buttons want rebinding
    Done,
    /// the money is short: one-button dialog `FEStrings` **810** (0x516380, 0x517663)
    NoMoney,
}

/// The state the two screens' accessors keep between frames.
#[derive(Debug, Default)]
pub struct PartsShop {
    /// `Parts.ShoppingList`'s entries (`[this+0x24]`)
    pub basket: Vec<ShopItem>,
    /// `[0x69A128]` — the car the basket was filled for. The screen's constructor empties the
    /// basket when `OwnedCars.Car` is on another one (0x519357…0x51937A).
    pub basket_car: Option<usize>,
    /// `Parts.ShoppingList`'s current row, a port row index
    pub basket_sel: Option<usize>,
    /// `Parts.RepairShoppingList`'s entries
    pub repair: Vec<RepairItem>,
    /// `[0x69A110]`, the same for the repair basket (0x518FFA…0x51901D)
    pub repair_car: Option<usize>,
    /// `Parts.OwnedDamageRepairList`'s current row
    pub owned_sel: Option<usize>,
    /// `Parts.RepairShoppingList`'s current row
    pub repair_sel: Option<usize>,
    /// `Parts.InstallList`'s current row (`carsetup.lay`)
    pub install_sel: Option<usize>,
}

/// The garage car `OwnedCars.Car` is on.
fn car_of(c: &CareerUi) -> Option<usize> {
    (!c.career.garage.is_empty()).then(|| c.car_sel.min(c.career.garage.len() - 1))
}

/// Rows grouped the way every parts list draws them: a category heading over a row whose category
/// **string** (0x518D60) differs from the row above's (0x516FBA…0x516FCC, 0x5170D9…0x5170F8) — so
/// categories 6 and 7, which share one, share a heading. The right cell is the caller's; the left
/// one is `"%s %s"` (format string `0x5da738`), corrected 2026-09-29:
///
/// Disassembling the four row draws that carry that format string —
/// `Parts.AllParts` 0x515620 (`0x515b1b…0x515b2b`), `Parts.ShoppingList` 0x515F50
/// (`0x515ffe…0x516012`), `Parts.InstallList` 0x516960 (`0x516a0a…0x516a27`) and
/// `Parts.RepairShoppingList` 0x5170B0 (`0x517182…0x5171a2`) — each pushes the same two pointers in
/// the same order: the localised name (`0x4E06D0(record+4)`, the string id) pushed **first** (so it
/// is the **second** vararg after the `"%s %s"` format, cdecl pushes right-to-left) and
/// `record+8` — a 28-byte fixed field, pushed **second** (the **first** vararg) — right before the
/// call. `record+8` is this table's `family` (`nfs_formats::career::Part::family`, `nfs5.prt +8`,
/// a car-family code like `"356"`, never empty — confirmed by dumping the shipped `nfs5.prt`,
/// 683/683 records). So the row is **`family` + `" "` + the localised name**, e.g.
/// `"356 Стандартный Маховик"` — not the name followed by anything. The port's own
/// `crates/nfs-game/src/career.rs::part_family` already reads this exact field for matching; this
/// is its first use for *display*.
///
/// `Parts.OwnedDamageRepairList`/`Parts.UsedDamageList`'s row draw, 0x516D70, is different: its
/// disassembly carries no `"%s %s"` at all and no call to `0x4E06D0` either — it draws the name
/// plain (through a vtable-dispatched getter and `0x4DA350`, not `0x4DA530`'s format path), no
/// family prefix. That is `owned_rows` below, which does not call this with `family_prefix`.
///
/// `record+0x28` — `Part::kind_name` (`nfs5.prt +0x28`, the internal English label:
/// `"Flywheel"`/`"Carbs"`/`"Air Intake"`/`"Exhaust"`/`"Gear Box"`/…) — is a different field
/// entirely and **never appears in any of these five row draws**; it is `career.md`'s own
/// `+0x28` "type name" the install/uninstall/replace logic (`0x4F873F`, `0x518510`, `0x4F8A1E`)
/// `strcmp`s two *installed* parts' records by, never formatted into a string. The previous
/// reading here (`format!("{name} {}", p.kind_name)`) had the right shape — a second field
/// appended to the localised name — but the wrong field and the wrong side.
///
/// Returns the rows and, per row, the index of the item it belongs to (a heading belongs to the
/// part under it).
pub fn grouped<'a>(items: impl IntoIterator<Item = (&'a Part, ListCell)>, strings: &Strings, family_prefix: bool) -> (Vec<ListRow>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut owner = Vec::new();
    let mut group = None;
    for (k, (p, right)) in items.into_iter().enumerate() {
        let cat = part_category_string(p.kind);
        if k == 0 || group != cat {
            group = cat;
            rows.push(ListRow::Header(cat.and_then(|id| strings.get(id)).unwrap_or("").to_string(), false));
            owner.push(k);
        }
        let name = strings.get(p.name_string_id).unwrap_or("").to_string();
        let text = if family_prefix { format!("{} {name}", p.family) } else { name };
        rows.push(ListRow::Columns(vec![ListCell { fade: 0xFF, anchor: String::new(), dx: 3.0, text }, right]));
        owner.push(k);
    }
    (rows, owner)
}

/// `"%d"` right-aligned (align 2) at a named widget's x plus `dx` — `-0.0` for the widget's x
/// itself (0x516051, 0x5171EE), which a `ListCell` reads as right-aligned by its sign.
fn at(anchor: &str, dx: f32, v: i64) -> ListCell {
    ListCell { fade: 0xFF, anchor: anchor.into(), dx, text: format!("{v}") }
}

impl PartsShop {
    /// The two screens' constructors: a basket filled for another car is emptied.
    pub fn sync_car(&mut self, c: &CareerUi) {
        let car = car_of(c);
        if self.basket_car != car {
            self.basket.clear();
            self.basket_sel = None;
            self.basket_car = car;
            // `Parts.InstallList`'s refresh on another car drops its current row (0x5166EF)
            self.install_sel = None;
        }
        if self.repair_car != car {
            self.repair.clear();
            self.repair_sel = None;
            self.owned_sel = None;
            self.repair_car = car;
        }
    }

    /// `Parts.AllParts` — **2550** "PARTS AVAILABLE FOR %s" over the parts that fit the car
    /// (0x517D10, each at its aged price 0x5181C0, 0x51590A) and **2551** "PARTS INVENTORY" over
    /// the spares (0x518B80, each at 0x518220, 0x515959). A section's rows show only while its
    /// flag is set (0x5153A0). The price is four pixels left of `ID_PRICEPOS`.
    pub fn all_parts_rows(c: &CareerUi, strings: &Strings, open: [bool; 2]) -> Option<(Vec<ListRow>, Vec<AllPartsRow>)> {
        let idx = car_of(c)?;
        let car = c.tables.car(c.career.garage[idx].car_id)?;
        let title = car.display_name.to_string();
        let mut rows = vec![ListRow::Header(strings.get(2550).unwrap_or_default().replacen("%s", &title, 1), open[0])];
        let mut kinds = vec![AllPartsRow::Header];
        if open[0] {
            let parts = c.career.parts_for(&c.tables, car);
            let (r, owner) = grouped(parts.iter().map(|p| (*p, at("ID_PRICEPOS", -4.0, c.career.part_buy_price(p)))), strings, true);
            kinds.extend(r.iter().zip(&owner).map(|(row, k)| if matches!(row, ListRow::Header(..)) { AllPartsRow::Heading } else { AllPartsRow::Buy(parts[*k].id) }));
            rows.extend(r);
        }
        rows.push(ListRow::Header(strings.get(2551).unwrap_or_default().to_string(), open[1]));
        kinds.push(AllPartsRow::Header);
        if open[1] {
            let view = c.career.spare_view(&c.tables);
            let items: Vec<(usize, &Part, f32)> = view.iter().filter_map(|&i| c.career.spare_part(&c.tables, i).map(|(p, s)| (i, p, s))).collect();
            let (r, owner) = grouped(items.iter().map(|(_, p, s)| (*p, at("ID_PRICEPOS", -4.0, c.career.part_sell_price(p, *s)))), strings, true);
            kinds.extend(r.iter().zip(&owner).map(|(row, k)| if matches!(row, ListRow::Header(..)) { AllPartsRow::Heading } else { AllPartsRow::Sell(items[*k].0) }));
            rows.extend(r);
        }
        Some((rows, kinds))
    }

    /// The part a `Parts.AllParts` row stands for — **0x515580(&buy, &car)**: nothing for a section
    /// header (0x51560D), the part otherwise; a category heading answers for the part under it.
    pub fn all_parts_pick(kinds: &[AllPartsRow], row: usize) -> Option<ShopItem> {
        kinds.get(row..)?.iter().find_map(|k| match k {
            AllPartsRow::Header => Some(None),
            AllPartsRow::Heading => None,
            AllPartsRow::Buy(id) => Some(Some(ShopItem::Buy(*id))),
            AllPartsRow::Sell(i) => Some(Some(ShopItem::Sell(*i))),
        })?
    }

    /// `Parts.ShoppingList` — each entry's part with its value at `ID_PRICEPOS` itself: a buy
    /// **negative**, its aged price negated (0x516032), a sale its resale (0x51603F).
    pub fn basket_rows(&self, c: &CareerUi, strings: &Strings) -> (Vec<ListRow>, Vec<usize>) {
        let items: Vec<(&Part, i64)> = self.basket.iter().filter_map(|i| Some((c.career.shop_item_part(&c.tables, *i)?.0, c.career.shop_item_value(&c.tables, *i)))).collect();
        grouped(items.iter().map(|(p, v)| (*p, at("ID_PRICEPOS", -0.0, *v))), strings, true)
    }

    /// `Parts.RepairShoppingList` — each entry's part with its cost at `ID_COSTPOS` (0x5171C5 /
    /// 0x5171D8: replace 0x518900, fix 0x518880).
    pub fn repair_rows(&self, c: &CareerUi, strings: &Strings) -> (Vec<ListRow>, Vec<usize>) {
        let Some(car) = car_of(c) else { return (Vec::new(), Vec::new()) };
        let items: Vec<(&Part, i64)> = self
            .repair
            .iter()
            .filter_map(|i| Some((c.tables.parts.iter().find(|p| p.id == i.part)?, c.career.repair_item_cost(&c.tables, car, *i))))
            .collect();
        grouped(items.iter().map(|(p, v)| (*p, at("ID_COSTPOS", -0.0, *v))), strings, true)
    }

    /// `Parts.OwnedDamageRepairList` — every installed part (0x516C60 counts `car+0x758`) with
    /// its damage as `"%d %"` at `ID_DAMAGEPOS + 3` (0x516EF6). Its row draw, 0x516D70, is the one
    /// of the five that carries no `"%s %s"` at all (`grouped()`'s doc comment above) — no family
    /// prefix here, unlike the other four.
    pub fn owned_rows(c: &CareerUi, strings: &Strings) -> (Vec<ListRow>, Vec<i16>) {
        let Some(car) = car_of(c) else { return (Vec::new(), Vec::new()) };
        let parts = c.career.installed_parts(&c.tables, car);
        let (rows, owner) = grouped(
            parts.iter().map(|p| (*p, ListCell { fade: 0xFF, anchor: "ID_DAMAGEPOS".into(), dx: 3.0, text: format!("{} %", c.career.part_damage(car, p) as i64) })),
            strings,
            false,
        );
        (rows, owner.iter().map(|k| parts[*k].id).collect())
    }

    /// `Parts.InstallList` — each row's part and, at `ID_INSTALLYPOS + 3`, `FEStrings` **2552**
    /// "INSTALLED" for a part on the car (0x516A85) or **2565** "UNINSTALLED" for a spare
    /// (0x516A49). "UNINSTALLED" is drawn in the row colour with its alpha scaled by 0x80 / 255
    /// (0x516A3E…0x516A44 → 0x4D87F0), "INSTALLED" in the row colour as it is (0x516A85…).
    pub fn install_rows(c: &CareerUi, strings: &Strings) -> (Vec<ListRow>, Vec<InstallRow>) {
        let Some(car) = car_of(c) else { return (Vec::new(), Vec::new()) };
        let list = c.career.install_list(&c.tables, car);
        let items: Vec<(&Part, u32)> = list
            .iter()
            .filter_map(|r| match r {
                InstallRow::Fitted(id) => c.tables.parts.iter().find(|p| p.id == *id).map(|p| (p, 2552)),
                InstallRow::Spare(i) => c.career.spare_part(&c.tables, *i).map(|(p, _)| (p, 2565)),
            })
            .collect();
        let (rows, owner) = grouped(
            items.iter().map(|(p, id)| (*p, ListCell { fade: if *id == 2565 { 0x80 } else { 0xFF }, anchor: "ID_INSTALLYPOS".into(), dx: 3.0, text: strings.get(*id).unwrap_or_default().to_string() })),
            strings,
            true,
        );
        (rows, owner.iter().map(|k| list[*k]).collect())
    }

    /// What `Parts.InstallList`'s current row is on — its slot 27, 0x516BD0, `{part, installed}`.
    pub fn install_pick(&self, c: &CareerUi, strings: &Strings) -> Option<InstallRow> {
        let (_, rows) = Self::install_rows(c, strings);
        self.install_sel.and_then(|r| rows.get(r).copied())
    }

    /// The list a click landed on, by its accessor, moves that list's current row (the list box's
    /// slot 9 → the accessor's slot 11). `true` when the click was one of these three lists'.
    pub fn click(&mut self, accessor: &str, row: usize) -> bool {
        match accessor {
            "Parts.ShoppingList" => self.basket_sel = Some(row),
            "Parts.OwnedDamageRepairList" => self.owned_sel = Some(row),
            "Parts.RepairShoppingList" => self.repair_sel = Some(row),
            "Parts.InstallList" => self.install_sel = Some(row),
            _ => return false,
        }
        true
    }

    /// Which basket entry a current row is on, if any.
    fn entry(sel: Option<usize>, owner: &[usize]) -> Option<usize> {
        sel.and_then(|r| owner.get(r).copied())
    }

    /// The ten callbacks. `parts_sel` is `Parts.AllParts`'s current row, `open` its two section
    /// flags; `None` when `cb` is not one of these screens'.
    pub fn callback(&mut self, c: &mut CareerUi, strings: &Strings, open: [bool; 2], parts_sel: &mut Option<usize>, cb: &str) -> Option<Outcome> {
        self.sync_car(c);
        let car = car_of(c);
        match cb {
            // 0x519550 / 0x5195A0: with a current row on `Parts.AllParts` (slot 0x48), the part it
            // is on (0x515580) goes into the basket as a buy (0x5160E0(part, 1, 0)) or a sale
            // (0x5160E0(part, 0, 0)), and `Parts.AllParts` loses its current row (slot 0x40,
            // 0x4BC610). The tick only enables each button for its own section (0x519442,
            // 0x51947A), which is what stops a buy of a spare or a sale of a table part.
            "BUYPARTCALLBACK" | "SELLPARTCALLBACK" => {
                let (_, kinds) = Self::all_parts_rows(c, strings, open)?;
                let pick = parts_sel.and_then(|r| Self::all_parts_pick(&kinds, r));
                let buy = cb == "BUYPARTCALLBACK";
                match pick {
                    Some(item @ ShopItem::Buy(_)) if buy => c.career.shop_add(&c.tables, &mut self.basket, item),
                    Some(item @ ShopItem::Sell(_)) if !buy => c.career.shop_add(&c.tables, &mut self.basket, item),
                    _ => return Some(Outcome::Done),
                }
                *parts_sel = None;
            }
            // 0x519600 → 0x5162F0: every entry freed, then slot 0x40
            "CLEARLISTCALLBACK" => {
                self.basket.clear();
                self.basket_sel = None;
            }
            // 0x519610: with a current row, 0x516290(slot 0x28) frees that entry, then slot 0x40
            "REMOVEITEMCALLBACK" => {
                let (_, owner) = self.basket_rows(c, strings);
                if let Some(k) = Self::entry(self.basket_sel, &owner).filter(|k| *k < self.basket.len()) {
                    self.basket.remove(k);
                    self.basket_sel = None;
                }
            }
            // 0x519650 → 0x516350
            "PURCHASECALLBACK" => {
                if c.career.shop_purchase(&c.tables, &mut self.basket).is_err() {
                    return Some(Outcome::NoMoney);
                }
                self.basket_sel = None;
            }
            // 0x519160 / 0x519190: the part `Parts.OwnedDamageRepairList` is on (0x516F50) into
            // the repair basket (0x5172B0 with 0 / 1), then that list loses its current row
            "fixpart" | "replacepart" => {
                let car = car?;
                let (_, ids) = Self::owned_rows(c, strings);
                if let Some(id) = self.owned_sel.and_then(|r| ids.get(r).copied()) {
                    let before = self.repair.len();
                    c.career.repair_add(&c.tables, car, &mut self.repair, id, cb == "replacepart");
                    if self.repair.len() != before {
                        // 0x51730F: the basket's own slot 0x40 once the entry is in
                        self.repair_sel = None;
                    }
                }
                self.owned_sel = None;
            }
            // 0x5191C0 → 0x5174A0: slot 0x40, then every entry freed
            "clearlist" => {
                self.repair.clear();
                self.repair_sel = None;
            }
            // 0x5191D0 → 0x517440: the entry at the current row (slot 0x28) freed, then slot 0x40
            "removeitem" => {
                let (_, owner) = self.repair_rows(c, strings);
                if let Some(k) = Self::entry(self.repair_sel, &owner).filter(|k| *k < self.repair.len()) {
                    self.repair.remove(k);
                }
                self.repair_sel = None;
            }
            // 0x5191F0 → 0x517630
            "acceptlist" => {
                let car = car?;
                if c.career.repair_accept(&c.tables, car, &mut self.repair).is_err() {
                    return Some(Outcome::NoMoney);
                }
                self.repair_sel = None;
            }
            // 0x4F1C90: the part `Parts.InstallList` is on (slot 27) through 0x518510, then the
            // list's refresh 0x5166E0, which drops the current row. The screen's tick hides the
            // button on an installed part (0x4F1B29), so only a spare gets here.
            "INSTALLPART" => {
                let car = car?;
                if let Some(InstallRow::Spare(i)) = self.install_pick(c, strings) {
                    c.career.install_spare(&c.tables, car, i);
                }
                self.install_sel = None;
            }
            // 0x4F1C40: the same through 0x518740, which does nothing to a part not on the car
            "UNINSTALLPART" => {
                let car = car?;
                if let Some(InstallRow::Fitted(id)) = self.install_pick(c, strings) {
                    c.career.uninstall_part(&c.tables, car, id);
                }
                self.install_sel = None;
            }
            _ => return None,
        }
        Some(Outcome::Done)
    }

    /// The two screens' ticks — which buttons are live (`0x50D880(name, enable)`).
    ///
    /// `parts.lay`, 0x519410: `part = 0x515580(&buy, 0)` on `Parts.AllParts`;
    /// `ID_PARTSDESC` part ≠ 0; `ID_BUYPARTBUTTON` and `ID_PARTDESCRIPBUTTON` part ≠ 0, `buy` and a
    /// current row; `ID_SELLPARTBUTTON` part ≠ 0 and not `buy`; `ID_CLEARLIST` and
    /// `ID_PURCHASEPART` the basket not empty (slot 9 `> 0`); `ID_REMOVEFROMLIST` the basket has a
    /// current row (slot 0x48).
    ///
    /// `advancedrepair.lay`, 0x5190B0: `ID_FIXPART` and `ID_REPLACEPART`
    /// `Parts.OwnedDamageRepairList` has a current row; `ID_CLEARLIST` and `ID_ACCEPT` the repair
    /// basket not empty; `ID_REMOVEITEM` it has a current row.
    pub fn bind_buttons(&self, c: &CareerUi, strings: &Strings, open: [bool; 2], parts_sel: Option<usize>, l: &mut LayoutScreen) {
        let pick = Self::all_parts_rows(c, strings, open).and_then(|(_, kinds)| parts_sel.and_then(|r| Self::all_parts_pick(&kinds, r)));
        let buy = matches!(pick, Some(ShopItem::Buy(_)));
        let sell = matches!(pick, Some(ShopItem::Sell(_)));
        let names = [
            "ID_BUYPARTBUTTON",
            "ID_PARTDESCRIPBUTTON",
            "ID_SELLPARTBUTTON",
            "ID_PURCHASEPART",
            "ID_REMOVEFROMLIST",
            "ID_FIXPART",
            "ID_REPLACEPART",
            "ID_ACCEPT",
            "ID_REMOVEITEM",
            "ID_CLEARLIST",
        ];
        // `ID_CLEARLIST` is on both screens, each with its own basket
        let on_repair = !l.widgets_named(&["ID_ACCEPT"]).is_empty();
        let clear = if on_repair { !self.repair.is_empty() } else { !self.basket.is_empty() };
        let live = [
            buy,
            buy,
            sell,
            !self.basket.is_empty(),
            self.basket_sel.is_some(),
            self.owned_sel.is_some(),
            self.owned_sel.is_some(),
            !self.repair.is_empty(),
            self.repair_sel.is_some(),
            clear,
        ];
        for (index, which) in l.widgets_named(&names) {
            l.set_disabled_flag(index, !live[which]);
        }
    }
}

/// **0x4F0220(car)**'s test: the parts of the car's list `+0x75C` for which its installed list
/// `+0x758` holds no part with the same type name (`+0x28`, `strcmp`) **and** category (`+0x26`),
/// in `+0x75C`'s order (0x4F028E…0x4F0317 marks each one matched, 0x4F0348 walks the unmarked).
///
/// `+0x75C` is the car's **stock** list — not a "removed" list: the FE car constructor 0x4EEBA0
/// ends in **0x4EE9A0**, which fills it with copies of the record's stock parts (`FECar+0x3C0`
/// count / `+0x3C2` ids = `nfs5.car +0x3BC` / `+0x3BE`, each through 0x49C470) and copies every
/// one onto `+0x758`, sorted by 0x49C350. Nothing else writes it but the save's loader.
/// `controls.md` §4.169.6.
pub fn missing_parts<'a>(stock: &[&'a Part], installed: &[&Part]) -> Vec<&'a Part> {
    stock.iter().copied().filter(|r| !installed.iter().any(|p| p.kind_name == r.kind_name && p.kind == r.kind)).collect()
}

/// 0x4F031D…0x4F039F: `"%s  "` over `FEStrings` 650, then each missing part's name
/// (`FEStrings[part+4]`) — the first through `"%s %s"`, the rest through `"%s, %s"`.
pub fn missing_parts_text(head: &str, names: &[&str]) -> String {
    let mut t = format!("{head}  ");
    for (k, n) in names.iter().enumerate() {
        t = if k == 0 { format!("{t} {n}") } else { format!("{t}, {n}") };
    }
    t
}

/// Garage car `car`'s stock list `+0x75C` and installed list `+0x758`, as table records — the two
/// lists 0x4F0220 compares. Since 2026-09-25 the career carries both (`OwnedCar::stock`, filled
/// by 0x4EE9A0 in the constructor; `OwnedCar::parts`), so a stock part uninstalled (0x518740) or
/// replaced (0x518510) and its replacement uninstalled leaves its slot empty and 650 fires.
/// *(Until then this filled every open stock slot with the stock part, and 650 could not arise.)*
pub fn stock_and_installed<'a>(c: &'a CareerUi, car: usize) -> (Vec<&'a Part>, Vec<&'a Part>) {
    let Some(owned) = c.career.garage.get(car) else { return (Vec::new(), Vec::new()) };
    let stock: Vec<&Part> = owned.stock.iter().filter_map(|(id, _)| c.tables.parts.iter().find(|p| p.id == *id)).collect();
    (stock, c.career.installed_parts(&c.tables, car))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(id: i16, kind: u16, kind_name: &str) -> Part {
        Part {
            id,
            name_string_id: 5000 + id as u32,
            family: "356".into(),
            unknown_24: 0,
            kind,
            kind_name: kind_name.into(),
            year: 1950,
            zone_mask: [0; 10],
            mass_delta: 0.0,
            top_speed_pct: 0.0,
            price: 100,
            condition: 0.0,
            geometry_swaps: vec![],
            value_c0: 0.0,
            boost: 0.0,
            grip_class: 0,
            forward_gears: 0,
            gearbox_param: 0,
            gear_ratios: [0.0; 8],
            torque_curve: [0.0; 21],
            state: 0,
        }
    }

    #[test]
    fn a_stock_slot_is_filled_by_the_same_type_name_and_category_only() {
        let (springs, brakes, engine) = (part(1, 4, "Springs"), part(2, 2, "Brakes"), part(3, 1, "Engine"));
        let (sport_springs, engine_other_kind) = (part(11, 4, "Springs"), part(13, 7, "Engine"));
        let stock = [&springs, &brakes, &engine];
        // a replacement of the same slot fills it; the same name in another category does not
        let got = missing_parts(&stock, &[&sport_springs, &engine_other_kind]);
        assert_eq!(got.iter().map(|p| p.id).collect::<Vec<_>>(), vec![2, 3]);
        assert!(missing_parts(&stock, &[&springs, &brakes, &engine]).is_empty());
    }

    /// 0x4EE9A0 fits the stock parts, 0x518740 / 0x518510 move them to the inventory, and
    /// 0x4F0220 then finds the empty slot — dialog 650 can arise.
    #[test]
    fn removing_a_stock_part_leaves_its_slot_empty_for_650() {
        use nfs_game::career::{Career, CareerTables, OwnedCar};
        let parts = vec![part(1, 4, "Springs"), part(2, 2, "Brakes"), part(11, 4, "Springs")];
        let mut rec = vec![0u8; nfs_formats::fedata::CAR_RECORD_SIZE];
        rec[0..2].copy_from_slice(&76u16.to_le_bytes());
        rec[0x3BC..0x3BE].copy_from_slice(&2i16.to_le_bytes());
        rec[0x3BE..0x3C0].copy_from_slice(&1i16.to_le_bytes());
        rec[0x3C0..0x3C2].copy_from_slice(&2i16.to_le_bytes());
        let catalog = nfs_formats::fedata::parse_car_catalog(&rec).unwrap();
        let t = CareerTables { raw: Default::default(), tracks: vec![], tournaments: vec![], parts, personalities: vec![], missions: vec![], strings: Default::default(), catalog, package_count: 0 };
        let mut c = Career::default();
        c.garage.push(OwnedCar::construct(&t, 76, 0, 0));
        let find = |id: &i16| t.parts.iter().find(|p| p.id == *id).unwrap();
        let missing = |c: &Career| -> Vec<i16> {
            let stock: Vec<&Part> = c.garage[0].stock.iter().map(|(id, _)| find(id)).collect();
            let installed: Vec<&Part> = c.garage[0].parts.iter().map(find).collect();
            missing_parts(&stock, &installed).iter().map(|p| p.id).collect()
        };
        assert!(missing(&c).is_empty(), "a new car has every stock slot filled");
        // uninstalling the stock brakes empties their slot
        c.uninstall_part(&t, 0, 2);
        assert_eq!(missing(&c), vec![2]);
        c.install_spare(&t, 0, 0);
        assert!(missing(&c).is_empty());
        // sport springs replace the stock ones, which go to the inventory: the slot stays filled…
        c.spares.push(nfs_game::shop::SparePart { id: 11, state: 0.0 });
        c.install_spare(&t, 0, 0);
        assert_eq!(c.spares, vec![nfs_game::shop::SparePart { id: 1, state: 0.0 }]);
        assert!(missing(&c).is_empty());
        // …until the replacement comes off too
        c.uninstall_part(&t, 0, 11);
        assert_eq!(missing(&c), vec![1]);
    }

    #[test]
    fn the_650_text_lists_the_names_after_two_spaces_then_commas() {
        assert_eq!(missing_parts_text("MISSING:", &["Brakes", "Engine"]), "MISSING:   Brakes, Engine");
        assert_eq!(missing_parts_text("MISSING:", &["Brakes"]), "MISSING:   Brakes");
    }

    #[test]
    fn a_heading_picks_the_part_under_it_and_a_header_picks_nothing() {
        let kinds = [AllPartsRow::Header, AllPartsRow::Heading, AllPartsRow::Buy(7), AllPartsRow::Buy(8), AllPartsRow::Header, AllPartsRow::Heading, AllPartsRow::Sell(2)];
        assert_eq!(PartsShop::all_parts_pick(&kinds, 0), None);
        assert_eq!(PartsShop::all_parts_pick(&kinds, 1), Some(ShopItem::Buy(7)));
        assert_eq!(PartsShop::all_parts_pick(&kinds, 3), Some(ShopItem::Buy(8)));
        assert_eq!(PartsShop::all_parts_pick(&kinds, 4), None);
        assert_eq!(PartsShop::all_parts_pick(&kinds, 5), Some(ShopItem::Sell(2)));
        assert_eq!(PartsShop::all_parts_pick(&kinds, 9), None);
    }
}
