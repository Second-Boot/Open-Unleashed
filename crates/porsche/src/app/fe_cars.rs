//! The front end's car data: comparison, fields, damage, installed parts and the car logos.

use crate::*;

impl App {
    /// The two cars the car-compare screen shows, left then right.
    ///
    /// `carcompare.lay` names the left column `CarCompare1` and the right `GenericCar2.Car`. The
    /// left one comes out of the global `[0x699FE0]`, whose only writer is the plain setter at
    /// 0x50E6A0 — and that setter has **no direct caller**: it is reached through the front end's
    /// accessor table, which is built at run time, so what fills it cannot be read statically.
    /// The screen is entered from `dealership`, `QuickRace`, `knockout` and `quickknockout`
    /// (a bare `NFS5.GOTOBUTTON` in each), every one of which has exactly one car selected, so
    /// the port uses that selection. **MEDIUM** — the naming and the right-hand column are read
    /// from the binary; which car the opening screen hands over is inferred from where the button
    /// is.
    fn compare_cars(&self) -> Option<(&nfs_formats::fedata::CarCatalogEntry, &nfs_formats::fedata::CarCatalogEntry)> {
        let m = self.fe.menu.as_ref()?;
        // opened from the dealership, the left car is `DealerCar.Car`'s — in a career its own list
        let left = if self.fe.compare_dealer { self.dealer_car().or(m.cars.get(m.car_idx))? } else { m.cars.get(m.car_idx)? };
        Some((left, m.car2()?))
    }

    /// One bar or number of the car-compare screen. The source name is `<n>CarCompareCar<m>`:
    /// `FEGouraudIndicatorBar`'s accessor (0x4CA790) takes the **leading digit** as the rating
    /// index (0x4CA838 subtracts `'0'` and rejects anything outside 0..9), then compares the rest
    /// against `CarCompareCar1` — which resolves to `[0x699FE0]`, the left car — or
    /// `CarCompareCar2`, which falls through to the `GenericCar2.Car` accessor. The five ratings
    /// are the `i16`s at +0x324 … +0x32C of the catalogue record.
    pub(crate) fn compare_rating(&self, source: &str) -> Option<i32> {
        let (d, rest) = source.split_at_checked(1)?;
        let index = d.parse::<usize>().ok().filter(|i| *i < 5)?;
        let (car1, car2) = self.compare_cars()?;
        // 0x50E8BA tests only for `CarCompareCar1`; **anything else** falls through to
        // `GenericCar2.Car` (0x50E8F1), so the second column is the default, not a second name
        let car = if rest == "CarCompareCar1" { car1 } else { car2 };
        Some(car.compare[index] as i32)
    }

    /// One field of a car the player owns, for the accessors that ask a car for a number rather
    /// than its name. `None` means this port has nothing to answer with, and the widget stays blank
    /// rather than showing something else — which is what it used to do, printing the car's name
    /// where a percentage belongs.
    pub(crate) fn car_field(&self, c: &CareerUi, owned: &nfs_game::career::OwnedCar, modifier: &str) -> Option<String> {
        Some(match modifier {
            // 0x4F633E and 0x4F62E1, the same shape again: `"%s"` of 0x4DBF80's commas, no sign
            "CURRENTVALUE" => with_thousands(c.career.value_of(&c.tables, owned)?),
            "PURCHASEPRICE" => with_thousands(owned.purchase_price as i64),
            // and two more of the same shape, on the used-car screens: 0x4F61D0 reads
            // **`car+0x68C`**, the asking price while the car is listed, and 0x4F6281
            // **`car+0x698`**, the mileage. Both go through 0x4DBF80 and `"%s"`. With no car at
            // all 0x4F61C6 answers the empty buffer at [0x5E8E50], which a watch draws as nothing
            // (§4.10) — but with a car and no listing the field is simply 0, so it prints `0`.
            "ASKINGPRICE" => with_thousands(owned.asking_price.unwrap_or(0) as i64),
            "MILEAGE" => with_thousands(owned.mileage as i64),
            // what the repair would cost: the aged price of each installed part times its damage
            // doubled, over a hundred (`part_repair_cost`, 0x518880, at the chronicle year
            // 0x5032C0), summed over the installed parts `car+0x758` of one category — 0x4EFBC0 for
            // the engine and its neighbours (the arms 0x4F679F…0x4F6AA7) — and printed `"%s"` of
            // 0x4DBF80's thousands commas. `BODYWORKDAMAGECOST` is **0x4EFDA0: categories 6 and
            // 7**, two loops summed (0x4EFDCA, 0x4EFE14), like the `QUICKFIX` box; `OVERALL` is
            // 0x4EFF80, the ten categories' loops (2, 3, 4, 5, 6, 0x4EF5E0(7), 8, 9, 10,
            // 0x4EF5E0(1)). *(Until 2026-09-27 BODYWORK summed category 6 alone, and the sums ran
            // over the parts with a saved damage rather than the installed list.)*
            m if m.ends_with("DAMAGECOST") => {
                let kinds: &[u16] = match m {
                    "OVERALLDAMAGECOST" => &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
                    "BODYWORKDAMAGECOST" => &[6, 7],
                    _ => {
                        let k = PART_DAMAGE_CATEGORY.iter().find(|(n, _)| n.trim_end_matches("DAMAGE") == m.trim_end_matches("DAMAGECOST"))?;
                        std::slice::from_ref(&k.1)
                    }
                };
                let year = c.career.year;
                let total: i64 = Self::installed_states(c, owned)
                    .filter(|(p, _)| kinds.contains(&p.kind))
                    .map(|(p, state)| nfs_game::career::part_repair_cost(p, year, state))
                    .sum();
                with_thousands(total)
            }
            // the ten damage modifiers. Slot 5's arms all read the same way — 0x4F63A3 calls the
            // category's mean, 0x4F63A8 `fsubr [0x5B242C]` turns it into `100 − mean`, and
            // 0x4F63B4 prints it with **`"%2.0f"`**: no per-cent sign, and a leading space under
            // ten. `RE/notes/controls.md` §4.134. The CRT's `%.0f` rounds **half away from zero**:
            // `_fptostr` rounds the digit string up when the first dropped digit is `>= '5'`
            // (0x5ABD3D `cmp byte [ecx], 0x35; jl`), where Rust's `{:.0}` rounds a tie to even
            // (66.5 → "66", the original "67"). CONFIRMED 2026-09-30 that `f64::round` is
            // the CRT here: `_fltout` 0x5ABD70 hands `_fptostr` 0x5ABCF9 **17** significant digits
            // (`push 0x11`, 0x5ABD90), and for a value of magnitude below ~1000 (this one is in
            // 0…100) two doubles are further apart (≥ 1.4e-14 at 66) than that expansion's step
            // (1e-15), so no double below a tie can round up to it. Checked by running the CRT's
            // digit rule against the exact value over 6 007 001 doubles (every half-integer 0.5…1000.5
            // and ±3000 ulps around it): 0 differences. *(Until 2026-09-27 the port printed ties to
            // even.)*
            m if m == "OVERALLDAMAGE" || PART_DAMAGE_CATEGORY.iter().any(|(n, _)| *n == m) => {
                format!("{:>2.0}", self.car_damage(c, owned, m)?.round())
            }
            _ => return None,
        })
    }

    /// The **integer** an accessor's slot 6 hands back, where this port has one. The front end's
    /// only bars that ask for one are the damage family's (`FEOwnedCarAccessor`'s slot 6 is
    /// 0x4F6B50); everything else has no slot-6 override and falls back to the text.
    pub(crate) fn accessor_number(&self, src: &str) -> Option<i32> {
        let (base, modifier) = match src.split_once('(') {
            Some((b, m)) => (b.trim(), m.trim_end_matches(')').trim()),
            None => (src, ""),
        };
        if modifier != "OVERALLDAMAGE" && !PART_DAMAGE_CATEGORY.iter().any(|(n, _)| *n == modifier) {
            return None;
        }
        let c = self.career.as_ref()?;
        let car = match base {
            // slot 6 (0x4F6B50) on an empty list: 0x4F6B64 `test eax, 0xFFFFFFFC; je` goes
            // straight to 0x4F6EC2, `xor eax, eax` — every damage bar reads **0**, not nothing
            "UsedCar.Car" if self.used_car().is_none() => return Some(0),
            "UsedCar.Car" => self.used_car()?,
            "OwnedCars.Car" | "DealerCar.Car" | "GenericCar.Car" | "CARSETTINGS.CAR" => {
                c.career.garage.get(c.car_sel.min(c.career.garage.len().saturating_sub(1)))?
            }
            _ => return None,
        };
        Some(self.car_damage(c, car, modifier)? as i32)
    }

    /// The installed parts `car+0x758` of an owned car with the damage `part+0x164` each carries —
    /// the saved state, or the table's own for a part the car has never raced.
    fn installed_states<'a>(c: &'a CareerUi, owned: &'a nfs_game::career::OwnedCar) -> impl Iterator<Item = (&'a nfs_formats::career::Part, f32)> + 'a {
        owned.parts.iter().filter_map(move |id| {
            let p = c.tables.parts.iter().find(|p| p.id == *id)?;
            let state = owned.part_state.iter().find(|(pid, _)| *pid == *id).map(|(_, s)| *s).unwrap_or(p.state as f32);
            Some((p, state))
        })
    }

    /// `100 − damage` for one of `FEOwnedCarAccessor`'s ten damage modifiers — the condition the
    /// screens draw, despite the names. It is the number both
    /// slot 5 (the text, `"%2.0f"`) and slot 6 (the integer a bar draws, `_ftol` at 0x4F6BDD, so
    /// **truncated**) are built from.
    ///
    /// One category's damage is 0x4EF580 (1 → 0x4EF640, 2 → 0x4EF690, …): the sum of `part+0x164`
    /// over the **installed** parts `car+0x758` whose `part+0x26` is that category, divided by the
    /// count **only when the count is above one** (0x4EF5C2 `cmp esi, 1; jle`), and 0.0 when the
    /// category holds none. The categories are 1 engine, 2 brakes, 3 transmission, 4 suspension,
    /// 5 shocks, 6 and 7 bodywork, 8 sway bars, 9 tyres, 10 cosmetic. `RE/notes/controls.md` §4.134.
    /// Nothing is stored on the way: the getters hand the x87 value back in `st0` and the arm
    /// runs `fsubr [0x5B242C]` (100.0) on it, so the whole figure is carried as an `f64` (MSVC's
    /// 53-bit precision control, as [`nfs_game::career::car_damage_pct`]). `OVERALLDAMAGE` is
    /// 0x4EF960 itself (0x4F6739), which is `car_damage_pct`. *(Until 2026-09-27 the port summed
    /// over the parts with a saved damage — a part never raced was left out of the count — in
    /// `f32`, and computed its own overall figure in another order with `/ 9.0`.)*
    fn car_damage(&self, c: &CareerUi, owned: &nfs_game::career::OwnedCar, modifier: &str) -> Option<f64> {
        let mean = |cat: u16| {
            let mut sum = 0.0f64;
            let mut n = 0u32;
            for (_, state) in Self::installed_states(c, owned).filter(|(p, _)| p.kind == cat) {
                sum += state as f64;
                n += 1;
            }
            if n > 1 { sum / n as f64 } else { sum }
        };
        let v = match modifier {
            // 0x4EF7D0 sums category **6 and category 7** and halves them (0x4EF857 `faddp`,
            // 0x4EF85B `fmul [0x5B23F0]` = 0.5) — bodywork is two categories, not one
            "BODYWORKDAMAGE" => (mean(6) + mean(7)) * 0.5,
            "OVERALLDAMAGE" => nfs_game::career::car_damage_pct(&c.tables, owned),
            m => mean(PART_DAMAGE_CATEGORY.iter().find(|(n, _)| *n == m)?.1),
        };
        Some(100.0 - v)
    }

    /// **`PrizeCars.Car`** (class 0x502500): every method reads `0x4F4360(0x5048D0(0x504800()))`
    /// — the `+0x10C` prize car of 0x504990's record for the highest completed era, the
    /// table's first record when none is complete (0x504907) — e.g. 0x502550 its model name
    /// `+0x7F`, 0x502570 its `+0x1C0`. The finale race's car 0x4EB923 is the same lookup.
    /// *(Until 2026-09-27 the port answered the quick race's car list here, so `VictoryRoom`
    /// turned the menu's car, not the era's trophy car.)*
    pub(crate) fn prize_car(&self) -> Option<&nfs_formats::fedata::CarCatalogEntry> {
        let c = self.career.as_ref()?;
        let t = c.tables.finale_record(c.career.completed_era(&c.tables))?;
        c.tables.car(t.prize_car)
    }

    /// The catalogue record an `NFS5.CarLogo`'s source names. The original resolves the source in
    /// the order at 0x4C14E0: `othersource` `FactoryDriver` → the `FactoryDriver.Car` accessor,
    /// `EndOfEra` → `PrizeCars.Car`, `FactoryDriverPrize` → the factory-drive prize car, then the
    /// widget's own `accessor`, then `CarCompare1` → the compare screen's left car. `FactoryDriverPrize` and
    /// `CarCompare1` are code-side objects this port does not keep, so those widgets stay
    /// unrendered; `Chron` names a car logo indirectly, through the chronicle pack — see [`App::chronicle_logo`].
    pub(crate) fn logo_car(
        &self,
        accessor: Option<&str>,
        othersource: Option<&str>,
    ) -> Option<&nfs_formats::fedata::CarCatalogEntry> {
        let name = match othersource {
            Some("FactoryDriver") => "FactoryDriver.Car",
            Some("EndOfEra") => "PrizeCars.Car",
            // `0x51B050` reads the current factory record's `+0x10`, the prize car of the stage the
            // player has just finished, and 0x4F4360 looks it up in the master list.
            //
            // `FactoryVictory.lay`'s `NFS5.3DCAR` asks for the same thing under an accessor name —
            // `FDPrizeCars.Car`, which 0x4F2053 registers and 0x51A3E0 builds as a car accessor of
            // the factory-driver module's own. Its list method is a bare `ret` (0x514990), so what
            // fills it was not traced; pointing it at the prize car of the stage just finished is
            // an inference from the screen it is on and the name it carries. **MEDIUM.**
            Some("FactoryDriverPrize") => {
                let c = self.career.as_ref()?;
                let m = c.career.promotion_due(&c.tables).or_else(|| {
                    c.career
                        .current_mission(&c.tables)
                        .map(|i| &c.tables.missions[i])
                })?;
                return c.tables.car(m.prize_car as u16);
            }
            // `0x50E6B0` hands back `[0x699FE0]`, the accessor the button that opened the compare
            // screen set through 0x50E6A0: the callbacks `CARCOMPARE.*` / `SHOWCASE.*` (registered
            // in a run of near-identical blocks read from raw disassembly at 0x50C257…0x50C6xx —
            // corrected 2026-09-29: `SHOWCASE.DEALERCAR` is **not** the flat function at 0x50DB20
            // — that address registers as `SHOWCASE.GENERICCAR` — it is a per-screen vtable-slot-54
            // (`+0xD8`) thunk at **0x50E5C0**; `CARCOMPARE.MERGEDCAR` is **0x50DC40**, not 0x50DCC0
            // (`CHEATALLCARS`)). The shipped layouts use only `CARCOMPARE.MERGEDCAR` (QuickRace,
            // knockout, quickknockout) and `SHOWCASE.DEALERCAR` (dealership) — `zMergedList.Car`
            // and `DealerCar.Car`, both the front end's selected car here — so the logo is the
            // compare column's own left car. (Until 2026-09-25 the garage car stood in.) Also the
            // source `Showcase.Text`/`NFS5.ShowcasePicture` read — see `showcase_text`'s own doc
            // comment (fixed 2026-09-29, pass 10 CONFIRMED PORT DEFECT).
            Some("CarCompare1") => return self.compare_cars().map(|(a, _)| a),
            // handled by `bind_car_logos`, which loads a whole picture out of `FEData/Chron`
            Some("Chron") => return None,
            _ => match accessor {
                Some("FDPrizeCars.Car") => return self.logo_car(None, Some("FactoryDriverPrize")),
                a => a?,
            },
        };
        let base = name.split_once('(').map(|(b, _)| b.trim()).unwrap_or(name);
        match base {
            // the car the current factory-driver mission is run in (`nfs5.fac` `+0x…` car id, the
            // same one `career_ui`'s mission line names)
            "FactoryDriver.Car" => {
                let c = self.career.as_ref()?;
                let i = c.career.current_mission(&c.tables)?;
                c.tables.car(c.tables.missions[i].car_id as u16)
            }
            "OwnedCars.Car" => {
                let c = self.career.as_ref()?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                c.tables.car(c.career.garage.get(idx)?.car_id)
            }
            // the used-car lot's own selection — the car its logo, its 3D view and its bars are on
            "UsedCar.Car" => {
                let c = self.career.as_ref()?;
                c.tables.car(self.used_car()?.car_id)
            }
            // in a career the dealership has its own list (`fn dealer_cars`)
            "DealerCar.Car" if self.career.is_some() => self.dealer_car(),
            // `VictoryRoom`'s car: `PrizeCars.Car`'s methods (0x502550, 0x502570, …) all read
            // `0x4F4360(0x5048D0(0x504800()))`, the prize car `+0x10C` of the completed era's
            // finale record (`App::prize_car`)
            "PrizeCars.Car" if self.career.is_some() => self.prize_car(),
            // **`zMergedList.Car` while the merged index is in the garage half** — `car_garage`
            // (0x4F4DF0, above): the same record `OwnedCars.Car` answers.
            "zMergedList.Car" if self.fe.menu.as_ref().is_some_and(|m| m.car_garage) => {
                let c = self.career.as_ref()?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                c.tables.car(c.career.garage.get(idx)?.car_id)
            }
            // the same list the port's other car accessors answer from
            "zMergedList.Car" | "OpponentCar.Car" | "PrizeCars.Car"
            | "DealerCar.Car" | "GenericCar.Car" | "CARSETTINGS.CAR" => {
                let m = self.fe.menu.as_ref()?;
                m.cars.get(m.car_idx)
            }
            // the compare screen's two columns: `CarCompare1` is the car it was entered with and
            // `GenericCar2.Car` the one its combo box cycles (`fn compare_cars`)
            "CarCompare1" | "CarCompare.Data" => self.compare_cars().map(|(a, _)| a),
            "GenericCar2.Car" => self.compare_cars().map(|(_, b)| b),
            _ => None,
        }
    }
}

/// The repair screens' per-category condition accessors and the `nfs5.prt` category (`part+0x26`)
/// each one measures.
///
/// Every one is a getter of the same shape — 0x4EF640 is `ENGINEDAMAGE`'s: walk the car's
/// installed-parts vector at `car+0x758`, add `part+0x164` (the damage that part carries) for each
/// part whose `part+0x26` matches, and divide by how many matched when that is more than one. The
/// accessor then shows **`100 - that`** (`fsubr [0x5B242C]`, which is 100.0, formatted `"%2.0f"`
/// at 0x4F63B4) — a condition, not a damage. CONFIRMED, each getter read:
/// 1 → 0x4EF640, 2 → 0x4EF690, 3 → 0x4EF6E0, 4 → 0x4EF730, 5 → 0x4EF780, 6 → 0x4EF7D0,
/// 8 → 0x4EF870, 9 → 0x4EF8C0, 10 → 0x4EF910.
///
/// `OVERALLDAMAGE` is not here: 0x4EF960 combines the per-category means itself (it calls the
/// parameterised mean 0x4EF580 for 2, 3 and 7 and inlines the others), and the port answers it
/// from the car's own accumulated figure instead.
pub(crate) const PART_DAMAGE_CATEGORY: &[(&str, u16)] = &[
    ("ENGINEDAMAGE", 1),
    ("BRAKESDAMAGE", 2),
    ("TRANSMISSIONDAMAGE", 3),
    ("SUSPENSIONDAMAGE", 4),
    ("SHOCKSDAMAGE", 5),
    ("BODYWORKDAMAGE", 6),
    ("SWAYBARSDAMAGE", 8),
    ("TIRESDAMAGE", 9),
    ("COSMETICDAMAGE", 10),
];
