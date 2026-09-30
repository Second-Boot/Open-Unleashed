//! The career's parts shop (`parts.lay`, `ID_BUYPARTSCBL`) and advanced repair
//! (`advancedrepair.lay`, `IDS_ADVREPAIRCBL`): the player's parts inventory, the two baskets and
//! what their buttons do. `RE/notes/career.md` §11.2c / §11.3a, read 2026-09-25.
//!
//! The original keeps three vectors of part pointers:
//!
//! * **`[0x69A0EC]`**, the inventory — records copied by value from `nfs5.prt` (0x5183C0), each
//!   with its own damage `part+0x164`. [`Career::spares`] here.
//! * `Parts.ShoppingList`'s `[this+0x24]` — 12-byte entries `{part*, u8 buy, car*}` (0x5160E0).
//!   [`ShopItem`] here; the `car*` is always 0 from the shop (0x515580 only ever writes 0 into its
//!   second out-parameter, 0x5155C1), so it is not kept.
//! * `Parts.RepairShoppingList`'s `[this+0x24]` — 8-byte entries `{part*, u8 replace}` (0x5172B0).
//!   [`RepairItem`] here, naming the installed part by its id (a car carries at most one part of
//!   an id: installing replaces the same category and type name, 0x518510).
//!
//! The original's basket entries are pointers into that inventory's records (0x5155DD / 0x5155FC),
//! and a record moves between vectors by its pointer (0x518510 erases the pointer from `[0x69A0EC]`
//! and pushes the same record on the car, 0x518638…0x518708). So a sell entry keeps its part when
//! the spare is installed from `carsetup.lay`: it still counts in `TOTAL` and its row (0x516500 reads
//! the record) but the purchase finds nothing in the inventory (0x5182D7…0x5182E5, `return`) and
//! pays nothing. Here [`ShopItem::Sell`] is a position and [`Career::install_spare`]'s caller turns
//! the entries of the moved spare into [`ShopItem::Moved`] (and shifts the later positions down by
//! the erased one) via [`Career::shop_spare_installed`] — the same identity the pointers give.

use crate::career::{Career, CareerError, CareerTables, OwnedCar, car_damage_pct, is_body_part, part_aged_price, part_less, part_repair_cost, part_resale};
use nfs_formats::career::Part;
use serde::{Deserialize, Serialize};

/// One part in the inventory `[0x69A0EC]`: which `nfs5.prt` record it is a copy of and the damage
/// `part+0x164` it carries — the two things the save keeps of any part (`savegame.md` §4.7).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SparePart {
    pub id: i16,
    pub state: f32,
}

/// A `Parts.ShoppingList` entry: a table part to buy (`[+4] = 1`) or a spare to sell (`[+4] = 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShopItem {
    /// an `nfs5.prt` id — the first section of `Parts.AllParts` hands out table records (0x5155DD)
    Buy(i16),
    /// an index into [`Career::spares`] — the second section hands out inventory parts (0x5155FC)
    Sell(usize),
    /// a `Sell` entry whose spare has since been installed (`id`, the record's damage as `f32`
    /// bits): the pointer's record lives on, on the car — priced by `TOTAL`, never found by 0x5182C0
    Moved(i16, u32),
}

/// A `Parts.RepairShoppingList` entry: one installed part and whether it is to be **replaced**
/// (`replacepart`, `[+4] = 1`) or **fixed** (`fixpart`, `[+4] = 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairItem {
    pub part: i16,
    pub replace: bool,
}

/// A `Parts.InstallList` row — `carsetup.lay`'s list, 8-byte entries `{part*, u8 installed}`
/// (0x5166E0): a part on the car (`[+4] = 1`) or a spare that fits it (`[+4] = 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallRow {
    /// an installed part's id
    Fitted(i16),
    /// an index into [`Career::spares`]
    Spare(usize),
}

/// The money clamp of 0x5012E0: the **amount** is held to ±999 999 999 (`0x3B9AC9FF`,
/// `0xC4653601`) before it is added to `[0x691D90]`.
const MONEY_STEP: i64 = 999_999_999;

/// **0x518900** / **0x518A80** — what replacing a part costs: its aged price less what the part
/// in the car is worth (`0x518220`'s resale, [`part_resale`]), never below 0.
///
/// ```text
/// aged   = price + price·Σ PART_AGE_PCT[0..age] / 100          ; 0x51890F…0x51897E
/// resale = (100 − clamp(_ftol(2·state + 20), 0, 100))·aged / 100 ; 0x518960…0x5189BA
/// cost   = aged − resale, or 0 when that is ≤ 0                  ; 0x5189D1…0x5189DD
/// ```
///
/// CONFIRMED. An undamaged part costs a fifth of its aged price to replace.
pub fn part_replace_cost(p: &Part, year: u16, state: f32) -> i64 {
    (part_aged_price(p, year) - part_resale(p, year, state)).max(0)
}

fn state_of(owned: &OwnedCar, p: &Part) -> f32 {
    owned.part_state.iter().find(|(id, _)| *id == p.id).map(|(_, s)| *s).unwrap_or(p.state as f32)
}

fn set_state(owned: &mut OwnedCar, id: i16, state: f32) {
    match owned.part_state.iter_mut().find(|(pid, _)| *pid == id) {
        Some((_, s)) => *s = state,
        None => owned.part_state.push((id, state)),
    }
}

impl Career {
    /// 0x5012E0: add `delta`, held to ±999 999 999 first. The sum itself is never clamped.
    pub(crate) fn add_money(&mut self, delta: i64) {
        self.money += delta.clamp(-MONEY_STEP, MONEY_STEP);
    }

    /// The inventory as `Parts.AllParts`'s second section lists it — **0x518B80**, which rebuilds
    /// the vector `[0x69A0E8]` from `[0x69A0EC]` leaving out every body part (category 10, type
    /// `"Body"`, 0x518BE9…0x518C23). Indices into [`Career::spares`], in inventory order.
    pub fn spare_view(&self, tables: &CareerTables) -> Vec<usize> {
        (0..self.spares.len())
            .filter(|&i| tables.parts.iter().find(|p| p.id == self.spares[i].id).is_none_or(|p| !is_body_part(p)))
            .collect()
    }

    /// Spare `idx`'s table record and its damage.
    pub fn spare_part<'a>(&self, tables: &'a CareerTables, idx: usize) -> Option<(&'a Part, f32)> {
        let s = self.spares.get(idx)?;
        Some((tables.parts.iter().find(|p| p.id == s.id)?, s.state))
    }

    /// **0x518220** — what the shop pays for a part with damage `state`: the inventory rows show it
    /// (0x515959) and a sale credits it (0x5182C0).
    pub fn part_sell_price(&self, p: &Part, state: f32) -> i64 {
        part_resale(p, self.year, state)
    }

    /// **0x5183C0(part, year, 0x5012E0)** — buy one part into the inventory: a copy of the table
    /// record (0x5183DF, `rep movsd` of 0x5A dwords, so its damage is the table's) pushed onto
    /// `[0x69A0EC]`, then the money callback with `−aged` (0x5184F5…0x5184FA). **No money check** —
    /// the purchase checks the basket's total first (0x516350).
    pub fn buy_spare(&mut self, tables: &CareerTables, id: i16) -> Result<(), CareerError> {
        let p = tables.parts.iter().find(|p| p.id == id).ok_or(CareerError::UnknownPart { id })?;
        let price = part_aged_price(p, self.year);
        self.spares.push(SparePart { id, state: p.state as f32 });
        self.add_money(-price);
        Ok(())
    }

    /// **0x5182C0(part, year, 0x5012E0)** — sell one spare: find it in `[0x69A0EC]` (0x5182D7),
    /// credit `0x518220` (0x518377), free it and close the gap (0x51837E…0x5183AD). A part that is
    /// not in the inventory is left alone. Returns whether it was sold.
    pub fn sell_spare(&mut self, tables: &CareerTables, idx: usize) -> bool {
        let Some((p, state)) = self.spare_part(tables, idx) else { return false };
        let credit = self.part_sell_price(p, state);
        self.add_money(credit);
        self.spares.remove(idx);
        true
    }

    /// **0x518510(part, car)** — `INSTALLPART` (0x4F1C90): put spare `spare` on garage car `car`.
    ///
    /// ```text
    /// 0x518541  for i in 0..n (n = the installed count on entry)
    /// 0x518550    installed[i] has the part's category (+0x26) and type name (+0x28, strcmp)
    /// 0x5185B4      -> push it onto the inventory, erase it from the car (0x4F0940), stop
    /// 0x518613  find the part in the inventory; not there -> return
    /// 0x518638  push it onto the car's installed list, erase it from the inventory
    /// ```
    ///
    /// So the part it replaces goes to the inventory **first**, and goes even when the new part
    /// then turns out not to be there. The replaced part keeps its damage; so does the new one.
    pub fn install_spare(&mut self, tables: &CareerTables, car: usize, spare: usize) -> bool {
        let Some(new) = self.spares.get(spare).and_then(|s| tables.parts.iter().find(|p| p.id == s.id)) else { return false };
        let Some(owned) = self.garage.get(car) else { return false };
        let old = owned
            .parts
            .iter()
            .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
            .find(|p| p.kind == new.kind && p.kind_name == new.kind_name)
            .map(|p| (p.id, state_of(owned, p)));
        if let Some((id, state)) = old {
            self.spares.push(SparePart { id, state });
            let owned = &mut self.garage[car];
            owned.parts.retain(|pid| *pid != id);
            owned.part_state.retain(|(pid, _)| *pid != id);
        }
        let s = self.spares.remove(spare);
        let owned = &mut self.garage[car];
        owned.parts.push(s.id);
        set_state(owned, s.id, s.state);
        owned.damage_pct = car_damage_pct(tables, owned);
        true
    }

    /// The basket after spare `spare` left the inventory for a car (the caller's
    /// [`Career::install_spare`] returned `true`, `moved` being that spare read beforehand): the
    /// entries naming it keep their record ([`ShopItem::Moved`]) and the later positions shift down
    /// by the erased one — what a basket of pointers does by itself.
    pub fn shop_spare_installed(basket: &mut [ShopItem], spare: usize, moved: SparePart) {
        for item in basket.iter_mut() {
            if let ShopItem::Sell(j) = *item {
                if j == spare {
                    *item = ShopItem::Moved(moved.id, moved.state.to_bits());
                } else if j > spare {
                    *item = ShopItem::Sell(j - 1);
                }
            }
        }
    }

    /// **0x518740(part, car)** — `UNINSTALLPART` (0x4F1C40): if the part is on the car
    /// (`car+0x758`), push it onto the inventory and erase it from the car (0x51876F…0x518869).
    pub fn uninstall_part(&mut self, tables: &CareerTables, car: usize, part: i16) {
        let Some(owned) = self.garage.get_mut(car) else { return };
        let Some(pos) = owned.parts.iter().position(|id| *id == part) else { return };
        let state = tables.parts.iter().find(|p| p.id == part).map(|p| state_of(owned, p)).unwrap_or(0.0);
        owned.parts.remove(pos);
        owned.part_state.retain(|(id, _)| *id != part);
        owned.damage_pct = car_damage_pct(tables, owned);
        self.spares.push(SparePart { id: part, state });
    }

    /// **0x517F70(vec, car)** — the spares that fit a car: for each spare in inventory order
    /// (0x517FC8), each id of the car's compatible list (`FECar+0x332` / `+0x334`) it matches
    /// (0x518003), unless it is a body part (0x518014…0x51804E).
    pub fn fitting_spares(&self, tables: &CareerTables, car: &nfs_formats::fedata::CarCatalogEntry) -> Vec<usize> {
        let mut out = Vec::new();
        for (i, s) in self.spares.iter().enumerate() {
            for id in &car.compatible_parts {
                if *id == s.id && tables.parts.iter().find(|p| p.id == s.id).is_some_and(|p| !is_body_part(p)) {
                    out.push(i);
                }
            }
        }
        out
    }

    /// **`Parts.InstallList`**, rebuilt by 0x5166E0: the car's installed parts but its body
    /// (0x516751…0x516801, `[+4] = 1`), then the spares that fit it (0x51682F → 0x517F70,
    /// `[+4] = 0`), then an exchange sort by 0x49C350 alone (0x5168E5…0x516928). The refresh
    /// starts by dropping the current row (slot 0x40, 0x5166EF).
    pub fn install_list(&self, tables: &CareerTables, car: usize) -> Vec<InstallRow> {
        let Some(owned) = self.garage.get(car) else { return Vec::new() };
        let Some(record) = tables.car(owned.car_id) else { return Vec::new() };
        let mut v: Vec<(InstallRow, &Part)> = owned
            .parts
            .iter()
            .filter_map(|id| tables.parts.iter().find(|p| p.id == *id))
            .filter(|p| !is_body_part(p))
            .map(|p| (InstallRow::Fitted(p.id), p))
            .collect();
        for i in self.fitting_spares(tables, record) {
            if let Some((p, _)) = self.spare_part(tables, i) {
                v.push((InstallRow::Spare(i), p));
            }
        }
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                if part_less(v[j].1, v[i].1) {
                    v.swap(i, j);
                }
            }
        }
        v.into_iter().map(|(r, _)| r).collect()
    }

    /// The part a shopping-list entry names, and its damage.
    pub fn shop_item_part<'a>(&self, tables: &'a CareerTables, item: ShopItem) -> Option<(&'a Part, f32)> {
        match item {
            ShopItem::Buy(id) => tables.parts.iter().find(|p| p.id == id).map(|p| (p, p.state as f32)),
            ShopItem::Sell(i) => self.spare_part(tables, i),
            ShopItem::Moved(id, bits) => tables.parts.iter().find(|p| p.id == id).map(|p| (p, f32::from_bits(bits))),
        }
    }

    /// What one entry does to the money — the figure its row prints (0x516017…0x516047): a part
    /// to buy **`−0x5181C0`**, its aged price negated (`neg eax`, 0x516032), a part to sell
    /// `+0x518220`.
    pub fn shop_item_value(&self, tables: &CareerTables, item: ShopItem) -> i64 {
        let Some((p, state)) = self.shop_item_part(tables, item) else { return 0 };
        match item {
            ShopItem::Buy(_) => -part_aged_price(p, self.year),
            ShopItem::Sell(_) | ShopItem::Moved(..) => self.part_sell_price(p, state),
        }
    }

    /// **`Parts.ShoppingList(TOTAL)`**, 0x516500: the sum of the entries' values — minus the aged
    /// price of every part to buy (0x516578), plus the resale of every part to sell (0x51658D).
    pub fn shop_total(&self, tables: &CareerTables, basket: &[ShopItem]) -> i64 {
        basket.iter().map(|i| self.shop_item_value(tables, *i)).sum()
    }

    /// **0x5160E0(part, buy, car)** — `BUYPARTCALLBACK` / `SELLPARTCALLBACK` add an entry. There is
    /// **no** duplicate check; the entry is pushed and the whole list re-sorted by an exchange
    /// sort (0x5161FB…0x51627E) that swaps `v[i]`, `v[j]` (`i < j`) when
    ///
    /// ```text
    /// 0x49C350(v[j].part, v[i].part)                  ; v[j] sorts first, or
    /// v[j].buy && !v[i].buy                           ; 0x516232…0x51623E, when it does not
    /// ```
    ///
    /// — the second call to 0x49C350 at 0x516229 asks the same question as the first.
    pub fn shop_add(&self, tables: &CareerTables, basket: &mut Vec<ShopItem>, item: ShopItem) {
        basket.push(item);
        let part = |i: ShopItem| self.shop_item_part(tables, i).map(|(p, _)| p);
        for i in 0..basket.len() {
            for j in i + 1..basket.len() {
                let (a, b) = (basket[i], basket[j]);
                let before = match (part(b), part(a)) {
                    (Some(pb), Some(pa)) => part_less(pb, pa),
                    _ => false,
                };
                let buy = |x: ShopItem| matches!(x, ShopItem::Buy(_));
                if before || (buy(b) && !buy(a)) {
                    basket.swap(i, j);
                }
            }
        }
    }

    /// **0x516350** — `PURCHASECALLBACK`. `TOTAL` negated against the money (0x516362): short, and
    /// it raises the one-button dialog **810** (0x516380, `FEStrings` 810, "not enough credits to
    /// complete this transaction") and changes nothing. Otherwise each entry in order: a part to
    /// buy goes through 0x5183C0 ([`Career::buy_spare`]), a part to sell through 0x518740 first
    /// when it names a car (never, from the shop) and then 0x5182C0 ([`Career::sell_spare`]); and
    /// the list is emptied (0x516412…0x516465).
    ///
    /// A buy appends to the inventory and a sale erases one spare, so the result is the inventory
    /// less the sold spares with the bought parts after it — which is what doing the sales last
    /// gives here, without the sell positions moving under the loop.
    pub fn shop_purchase(&mut self, tables: &CareerTables, basket: &mut Vec<ShopItem>) -> Result<(), CareerError> {
        let need = -self.shop_total(tables, basket);
        if self.money < need {
            return Err(CareerError::NoMoney { need, have: self.money });
        }
        let mut sold: Vec<usize> = Vec::new();
        for item in basket.iter() {
            match *item {
                ShopItem::Buy(id) => {
                    let _ = self.buy_spare(tables, id);
                }
                // the record is on a car, not in `[0x69A0EC]` (0x5182D7): nothing to sell
                ShopItem::Moved(..) => {}
                ShopItem::Sell(i) => {
                    // 0x5182D7: a part no longer in the inventory is not found and pays nothing
                    if i < self.spares.len() && !sold.contains(&i) {
                        if let Some((p, state)) = self.spare_part(tables, i) {
                            let credit = self.part_sell_price(p, state);
                            self.add_money(credit);
                        }
                        sold.push(i);
                    }
                }
            }
        }
        sold.sort_unstable();
        for i in sold.into_iter().rev() {
            self.spares.remove(i);
        }
        basket.clear();
        Ok(())
    }

    /// What one repair entry costs — the figure its row prints and `TOTAL` sums (0x5171C5 /
    /// 0x5171D8, 0x5175F8 / 0x517608): **0x518900** ([`part_replace_cost`]) for a part to replace,
    /// **0x518880** ([`part_repair_cost`]) for one to fix, both with the part's damage as it is on
    /// the car now.
    pub fn repair_item_cost(&self, tables: &CareerTables, car: usize, item: RepairItem) -> i64 {
        let Some(owned) = self.garage.get(car) else { return 0 };
        let Some(p) = tables.parts.iter().find(|p| p.id == item.part) else { return 0 };
        let state = state_of(owned, p);
        if item.replace { part_replace_cost(p, self.year, state) } else { part_repair_cost(p, self.year, state) }
    }

    /// **`Parts.RepairShoppingList(TOTAL)`**, 0x517580: the entries' costs summed.
    pub fn repair_total(&self, tables: &CareerTables, car: usize, basket: &[RepairItem]) -> i64 {
        basket.iter().map(|i| self.repair_item_cost(tables, car, *i)).sum()
    }

    /// **0x5172B0(part, replace)** — `fixpart` (0x519160, `replace` 0) and `replacepart`
    /// (0x519190, 1) add the part `Parts.OwnedDamageRepairList` is on (0x516F50,
    /// `installed[current]`). A part whose damage is exactly **0.0** is refused (0x5172BF `fcomp`
    /// against 0x5B23F8, `jne` past on equal); anything else is appended — no duplicate check,
    /// no sort.
    pub fn repair_add(&self, tables: &CareerTables, car: usize, basket: &mut Vec<RepairItem>, part: i16, replace: bool) {
        let Some(owned) = self.garage.get(car) else { return };
        let Some(p) = tables.parts.iter().find(|p| p.id == part) else { return };
        if state_of(owned, p) == 0.0 {
            return;
        }
        basket.push(RepairItem { part, replace });
    }

    /// **0x517630** — `acceptlist` (0x5191F0). `TOTAL` against the money (0x517645, `jge` goes on):
    /// short, one-button **810** (0x517663) and nothing done. Otherwise, with the car
    /// `OwnedCars.Car` is on (0x4F5A20) and its body part (0x4F0400, the first installed part of
    /// category 10 named `"Body"`), each entry in order:
    ///
    /// ```text
    /// 0x5176FC  entry is the body part -> zero the car's ten zone damages FECar+0x6A8…+0x6CF
    /// 0x517728  replace: 0x518A80 — money −max(aged − resale, 0), part+0x164 = 0
    /// 0x517738  fix:     0x5189F0 — money −aged·_ftol(2·state)/100, part+0x164 = 0
    /// ```
    ///
    /// and the list is emptied (0x517756…0x5177A9). Each charge reads the damage as it is at that
    /// moment, so a part listed twice pays the second time at its new damage — nothing to fix,
    /// a fifth of the aged price to replace. Returns what was spent.
    pub fn repair_accept(&mut self, tables: &CareerTables, car: usize, basket: &mut Vec<RepairItem>) -> Result<i64, CareerError> {
        let total = self.repair_total(tables, car, basket);
        if self.money < total {
            return Err(CareerError::NoMoney { need: total, have: self.money });
        }
        let Some(owned) = self.garage.get(car) else { return Err(CareerError::UnknownCar { id: 0 }) };
        let body = owned.parts.iter().filter_map(|id| tables.parts.iter().find(|p| p.id == *id)).find(|p| is_body_part(p)).map(|p| p.id);
        let mut spent = 0;
        for item in basket.iter() {
            if Some(item.part) == body {
                self.garage[car].damage = [0.0; 10];
            }
            let cost = self.repair_item_cost(tables, car, *item);
            self.add_money(-cost);
            spent += cost;
            set_state(&mut self.garage[car], item.part, 0.0);
        }
        let owned = &mut self.garage[car];
        owned.damage_pct = car_damage_pct(tables, owned);
        basket.clear();
        Ok(spent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nfs_formats::fedata::CarCatalogEntry;
    use nfs_formats::locale::Strings;

    fn part(id: i16, kind: u16, kind_name: &str, year: u16, price: u32) -> Part {
        Part {
            id,
            name_string_id: 5000 + id as u32,
            family: "356".into(),
            unknown_24: 0,
            kind,
            kind_name: kind_name.into(),
            year,
            zone_mask: [1; 10],
            mass_delta: 0.0,
            top_speed_pct: 0.0,
            price,
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

    fn tables(parts: Vec<Part>) -> CareerTables {
        CareerTables { raw: Default::default(), tracks: vec![], tournaments: vec![], parts, personalities: vec![], missions: vec![], strings: Strings::default(), catalog: vec![], package_count: 0 }
    }

    fn shop() -> (CareerTables, Career) {
        let t = tables(vec![
            part(1, 1, "Engine", 1950, 1000),
            part(2, 2, "Brakes", 1950, 500),
            part(3, 1, "Engine", 1950, 800),
            part(4, 10, "Body", 1950, 3000),
            part(5, 9, "Tires", 1950, 200),
        ]);
        let mut c = Career::default();
        c.money = 10_000;
        c.garage.push(OwnedCar::new(76, 0, 0));
        (t, c)
    }

    /// 0x5183C0 / 0x5182C0: buying charges the aged price and copies the record in with the
    /// table's damage; selling pays 0x518220 and takes that spare out.
    #[test]
    fn buying_and_selling_a_spare() {
        let (t, mut c) = shop();
        c.year = 1952; // two years: PART_AGE_PCT[0] + [1] = 3 %
        c.buy_spare(&t, 1).unwrap();
        assert_eq!(c.money, 10_000 - 1030);
        assert_eq!(c.spares, vec![SparePart { id: 1, state: 0.0 }]);
        // an undamaged part fetches 80 % of the aged price
        assert!(c.sell_spare(&t, 0));
        assert_eq!(c.money, 10_000 - 1030 + 824);
        assert!(c.spares.is_empty());
        assert!(!c.sell_spare(&t, 0), "nothing left to sell");
        // the buy has no money check of its own
        c.money = 0;
        c.buy_spare(&t, 2).unwrap();
        assert_eq!(c.money, -515);
    }

    /// 0x518B80: the inventory the shop lists leaves the body out.
    #[test]
    fn the_listed_inventory_leaves_the_body_out() {
        let (t, mut c) = shop();
        c.spares = vec![SparePart { id: 4, state: 0.0 }, SparePart { id: 2, state: 5.0 }, SparePart { id: 4, state: 0.0 }, SparePart { id: 5, state: 0.0 }];
        assert_eq!(c.spare_view(&t), vec![1, 3]);
    }

    /// 0x5160E0 / 0x516500: the basket keeps 0x49C350's order with buys ahead of sells, and its
    /// total is resale in, aged price out.
    #[test]
    fn the_basket_sorts_and_totals() {
        let (t, mut c) = shop();
        c.spares = vec![SparePart { id: 2, state: 10.0 }];
        let mut b = Vec::new();
        c.shop_add(&t, &mut b, ShopItem::Buy(1));
        c.shop_add(&t, &mut b, ShopItem::Sell(0));
        c.shop_add(&t, &mut b, ShopItem::Buy(3));
        c.shop_add(&t, &mut b, ShopItem::Buy(5));
        // category 1 (800 before 1000), then brakes, then tyres
        assert_eq!(b, vec![ShopItem::Buy(3), ShopItem::Buy(1), ShopItem::Buy(5), ShopItem::Sell(0)]);
        // the sale: c = 2·10 + 20 = 40 → 60 % of 500; the buys: 800 + 1000 + 200
        assert_eq!(c.shop_total(&t, &b), 300 - 2000);
        // a same-part pair: the buy goes first
        let mut b2 = Vec::new();
        c.spares = vec![SparePart { id: 2, state: 0.0 }];
        c.shop_add(&t, &mut b2, ShopItem::Sell(0));
        c.shop_add(&t, &mut b2, ShopItem::Buy(2));
        assert_eq!(b2, vec![ShopItem::Buy(2), ShopItem::Sell(0)]);
        assert_eq!(c.shop_total(&t, &b2), -500 + 400);
    }

    /// 0x516350: short of the total, nothing happens; otherwise the buys land in the inventory,
    /// the sales leave it, and the basket empties.
    #[test]
    fn the_purchase_checks_the_total_then_does_everything() {
        let (t, mut c) = shop();
        c.spares = vec![SparePart { id: 5, state: 0.0 }, SparePart { id: 2, state: 0.0 }];
        let mut b = vec![ShopItem::Buy(1), ShopItem::Sell(1), ShopItem::Sell(1)];
        // TOTAL counts the duplicate sale twice: the need is 1000 − 400 − 400
        c.money = 199;
        assert!(matches!(c.shop_purchase(&t, &mut b), Err(CareerError::NoMoney { need: 200, .. })));
        assert_eq!((c.money, b.len(), c.spares.len()), (199, 3, 2));
        c.money = 200; // exactly the need goes through (`jge`)
        c.shop_purchase(&t, &mut b).unwrap();
        // …but the spare is sold once: 200 − 1000 + 400
        assert_eq!(c.money, -400);
        assert!(b.is_empty());
        assert_eq!(c.spares, vec![SparePart { id: 5, state: 0.0 }, SparePart { id: 1, state: 0.0 }]);
    }

    /// The basket holds pointers (0x5155FC): a spare installed from `carsetup.lay` keeps its entry
    /// — still priced by `TOTAL`, never found by 0x5182C0 — and the entries behind it stay on
    /// their own parts.
    #[test]
    fn an_installed_spare_leaves_its_basket_entry_on_the_record() {
        let (t, mut c) = shop();
        c.garage[0].parts = vec![2, 1];
        c.spares = vec![SparePart { id: 5, state: 0.0 }, SparePart { id: 2, state: 10.0 }, SparePart { id: 3, state: 0.0 }];
        let mut b = Vec::new();
        c.shop_add(&t, &mut b, ShopItem::Sell(0));
        c.shop_add(&t, &mut b, ShopItem::Sell(1));
        c.shop_add(&t, &mut b, ShopItem::Sell(2));
        let total = c.shop_total(&t, &b);
        let moved = c.spares[1];
        assert!(c.install_spare(&t, 0, 1));
        Career::shop_spare_installed(&mut b, 1, moved);
        assert_eq!(b.iter().filter(|i| matches!(i, ShopItem::Moved(2, _))).count(), 1);
        // the total is unchanged: the record lives on, on the car (the replaced part joined the
        // inventory behind, so nothing before it moved)
        assert_eq!(c.shop_total(&t, &b), total);
        c.shop_purchase(&t, &mut b).unwrap();
        // the two spares still in the inventory (ids 5 and 3) were sold, the moved one was not
        assert!(!c.spares.iter().any(|s| s.id == 5 || s.id == 3), "{:?}", c.spares);
    }

    /// 0x518510 / 0x518740: installing swaps the same category and type name into the inventory,
    /// damage and all; uninstalling moves a part back.
    #[test]
    fn install_and_uninstall_move_parts_through_the_inventory() {
        let (t, mut c) = shop();
        c.garage[0].parts = vec![2, 1];
        c.garage[0].part_state = vec![(1, 12.0)];
        c.spares = vec![SparePart { id: 5, state: 0.0 }, SparePart { id: 3, state: 3.0 }];
        c.install_spare(&t, 0, 1);
        assert_eq!(c.garage[0].parts, vec![2, 3]);
        assert_eq!(c.garage[0].part_state, vec![(3, 3.0)]);
        assert_eq!(c.spares, vec![SparePart { id: 5, state: 0.0 }, SparePart { id: 1, state: 12.0 }]);
        // nothing of the tyre's category is on the car: it just goes on
        c.install_spare(&t, 0, 0);
        assert_eq!(c.garage[0].parts, vec![2, 3, 5]);
        assert_eq!(c.spares, vec![SparePart { id: 1, state: 12.0 }]);
        c.uninstall_part(&t, 0, 3);
        assert_eq!(c.garage[0].parts, vec![2, 5]);
        assert_eq!(c.spares, vec![SparePart { id: 1, state: 12.0 }, SparePart { id: 3, state: 3.0 }]);
        c.uninstall_part(&t, 0, 3);
        assert_eq!(c.spares.len(), 2, "a part not on the car is left alone");
    }

    /// 0x5166E0 / 0x517F70: the car's parts but the body, then the spares that fit it, in part
    /// order.
    #[test]
    fn the_install_list_is_the_car_and_the_spares_that_fit() {
        let (mut t, mut c) = shop();
        let mut rec = vec![0u8; nfs_formats::fedata::CAR_RECORD_SIZE];
        rec[0..2].copy_from_slice(&76u16.to_le_bytes());
        let ids: [i16; 3] = [3, 5, 4];
        rec[0x32E..0x330].copy_from_slice(&(ids.len() as i16).to_le_bytes());
        for (k, id) in ids.iter().enumerate() {
            rec[0x330 + 2 * k..0x332 + 2 * k].copy_from_slice(&id.to_le_bytes());
        }
        t.catalog = nfs_formats::fedata::parse_car_catalog(&rec).unwrap();
        c.garage[0].parts = vec![4, 2, 1];
        // the brakes spare does not fit, the body never lists
        c.spares = vec![SparePart { id: 2, state: 0.0 }, SparePart { id: 5, state: 0.0 }, SparePart { id: 3, state: 0.0 }, SparePart { id: 4, state: 0.0 }];
        assert_eq!(c.fitting_spares(&t, &t.catalog[0]), vec![1, 2]);
        assert_eq!(c.install_list(&t, 0), vec![InstallRow::Spare(2), InstallRow::Fitted(1), InstallRow::Fitted(2), InstallRow::Spare(1)]);
    }

    /// 0x518880 / 0x518900: fixing costs aged · ⌊2·state⌋ / 100, replacing the aged price less the
    /// part's resale.
    #[test]
    fn fix_and_replace_costs() {
        let p = part(1, 1, "Engine", 1950, 1000);
        assert_eq!(part_repair_cost(&p, 1950, 12.5), 250);
        assert_eq!(part_replace_cost(&p, 1950, 0.0), 200);
        assert_eq!(part_replace_cost(&p, 1950, 10.0), 400);
        assert_eq!(part_replace_cost(&p, 1950, 50.0), 1000);
    }

    /// 0x5172B0 / 0x517630: an undamaged part is refused; accepting checks the total, charges each
    /// entry at the damage it has then, clears the damage and, for the body, the zones.
    #[test]
    fn the_repair_basket() {
        let (t, mut c) = shop();
        c.garage[0].parts = vec![1, 2, 4];
        c.garage[0].part_state = vec![(1, 10.0), (2, 0.0), (4, 5.0)];
        c.garage[0].damage = [30.0; 10];
        let mut b = Vec::new();
        c.repair_add(&t, 0, &mut b, 2, false);
        assert!(b.is_empty(), "no damage, no entry");
        c.repair_add(&t, 0, &mut b, 1, false);
        c.repair_add(&t, 0, &mut b, 1, true);
        c.repair_add(&t, 0, &mut b, 4, true);
        // fix 1000·20/100, replace 1000 − 600, replace the body 3000 − (100 − 30)·3000/100
        assert_eq!(c.repair_total(&t, 0, &b), 200 + 400 + 900);
        c.money = 1499;
        assert!(matches!(c.repair_accept(&t, 0, &mut b), Err(CareerError::NoMoney { .. })));
        assert_eq!(b.len(), 3);
        c.money = 1500;
        // the second entry for part 1 finds it fixed and costs an undamaged part's 20 %
        assert_eq!(c.repair_accept(&t, 0, &mut b).unwrap(), 200 + 200 + 900);
        assert_eq!(c.money, 200);
        assert!(b.is_empty());
        assert_eq!(c.garage[0].damage, [0.0; 10]);
        assert!(c.garage[0].part_state.iter().all(|(_, s)| *s == 0.0));
    }

    /// 0x517D10: the car's compatible ids, no newer than the year, no body, in 0x49C350's order.
    #[test]
    fn the_shop_lists_what_fits_in_part_order() {
        let (mut t, mut c) = shop();
        t.parts.push(part(6, 1, "Engine", 1960, 100));
        // a zeroed nfs5.car record with only its compatible-parts list (+0x32E count, +0x330 ids)
        let mut rec = vec![0u8; nfs_formats::fedata::CAR_RECORD_SIZE];
        let ids: [i16; 7] = [5, 4, 1, 6, 3, 2, 99];
        rec[0x32E..0x330].copy_from_slice(&(ids.len() as i16).to_le_bytes());
        for (k, id) in ids.iter().enumerate() {
            rec[0x330 + 2 * k..0x332 + 2 * k].copy_from_slice(&id.to_le_bytes());
        }
        let car: CarCatalogEntry = nfs_formats::fedata::parse_car_catalog(&rec).unwrap().remove(0);
        assert_eq!(car.compatible_parts, ids);
        c.year = 1955;
        let ids: Vec<i16> = c.parts_for(&t, &car).iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![3, 1, 2, 5]);
        c.year = 1960;
        let ids: Vec<i16> = c.parts_for(&t, &car).iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![6, 3, 1, 2, 5]);
    }
}
