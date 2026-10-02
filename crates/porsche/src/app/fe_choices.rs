//! The front end's option accessors: combo pickers, choice lists, display settings and the HUD configuration.

use crate::*;

impl App {
    /// Everything a changed accessor can show through: a slider that changed the car changes the
    /// logo with it (0x4C1A6A: the widget reloads when the car's pair differs from the one it
    /// cached), the lists, the bars, the panels.
    pub(crate) fn bind_after_accessor(&mut self) {
        self.bind_car_logos();
        self.bind_indicator_bars();
        self.bind_chronicle_icons();
        self.bind_chronicle_group();
        self.bind_repair_bars();
        self.bind_usedcars_panels();
        self.bind_garage();
        self.bind_carsetup_gearratio();
        self.bind_laps_selector();
        self.bind_player_buttons();
        self.bind_parts_buttons();
        self.bind_mergedlist_panel();
        self.bind_showcase_pictures();
        self.bind_track_maps();
        self.bind_player_pictures();
        self.bind_lists();
        self.bind_checks();
    }

    /// A combo's accessor **slot 11**, `SetSelection(row)` — what the row commit (0x4C6D82) and
    /// the wrapping step (0x4BC690's `slot11(0)`) call. The base list's slot 11 is **0x4BC240**:
    /// the row clamped to `count − 1` and then to 0 (signed, `jl` / `jge` at 0x4BC269 / 0x4BC275),
    /// stored in `[+0x18]` with the "none selected" byte `[+0x1D]` cleared, and the accessor's
    /// change callback `[+0x10]` run. The colour accessors' slot 11 is 0x4B92B0 (`colour_select`);
    /// `zMergedList.Car`'s is **0x4F49F0**: below list A's count `A.slot11(row)`, else
    /// `B.slot11(row − A.count)`, then its own 0x4BC240 (0x4F0A30).
    ///
    /// The port's accessors keep their own indices, so the setter is reached as the difference
    /// from the current row through `set_accessor`, whose wrap or clamp lands exactly on `row`
    /// for any `row` inside the list. **`zMergedList.Car`'s list-A half (the garage cars),
    /// settled 2026-09-27**: a row below list A's count is `A->slot11(row)`, which is
    /// `FEOwnedCarAccessor`'s own **0x4F5BF0** — the exact function `OwnedCars.Car`'s own combo
    /// reaches, so `row` lands on the same owned car it would standalone (`garage_rows`'s own
    /// numbering, headings and all) — reached here through `set_accessor("OwnedCars.Car", …)`,
    /// the difference from that accessor's own current row. `car_garage` then records list A as
    /// current, matching 0x4F0A30's own `[+0x18] = row` on the merged accessor itself.
    /// *(Until 2026-09-27 a garage row closed the list and committed nothing.)*
    pub(crate) fn combo_select(&mut self, name: &str, row: usize) {
        let base = name.split_once('(').map_or(name, |(b, _)| b.trim());
        if COLOUR_ACCESSORS.contains(&base) {
            let base = base.to_string();
            self.colour_select(&base, row);
            return;
        }
        // the same rows the drop-down shows (`bind_lists`: a choice accessor first, then the
        // families `combo_rows` builds). *(Until 2026-09-29 only `combo_rows` was asked, which
        // knows no choice accessor — every `Graphics.*` / `Game.*` / … drop-down row released
        // onto nothing and the setting stayed put. User report: the graphics options could not
        // be changed with the mouse.)*
        // `Comms.IPList`'s drop-down (`PeerToPeerIP.lay`): the list accessor's selection, and the
        // input box beside it follows it (0x4C2310)
        if base == "Comms.IPList" {
            self.pick_ip_row(row);
            return;
        }
        let Some((rows, cur)) = self.choice_rows(name).or_else(|| self.combo_rows(name)) else { return };
        if rows.is_empty() {
            return;
        }
        // 0x4BC240's clamp
        let row = row.min(rows.len() - 1);
        if base == "zMergedList.Car" {
            if let Some(m) = self.fe.menu.as_mut() {
                m.merged_at_zero = false; // 0x4F49F0 records the picked index
            }
            let a = rows.len().saturating_sub(self.fe.menu.as_ref().map_or(0, |m| m.cars.len()));
            if row < a {
                let Some((grows, gat)) = self.combo_rows("OwnedCars.Car") else { return };
                let cur_a = gat.unwrap_or(0);
                self.set_accessor("OwnedCars.Car", selectable_index(&grows, row) - selectable_index(&grows, cur_a));
                if let Some(m) = self.fe.menu.as_mut() {
                    m.car_garage = true;
                }
                return;
            }
            // list B: the merged accessor's own index moves into the catalogue half too — B is
            // still flat here (`combo_rows`'s own `zMergedList.Car` arm doc), so `row`/`cur` need
            // no `selectable_index` conversion in this branch specifically
            if let Some(m) = self.fe.menu.as_mut() {
                m.car_garage = false;
            }
            let cur = cur.unwrap_or(0);
            self.set_accessor(name, row as i32 - cur as i32);
            return;
        }
        let cur = cur.unwrap_or(0);
        self.set_accessor(name, selectable_index(&rows, row) - selectable_index(&rows, cur));
    }

    /// The accessor's **slot 21**, **0x4BC690** — the only way the game reaches the wrapping
    /// pair (§4.171.8): `slot10() == slot9() − 1` → `slot11(0)`, else slot 23 0x4BC6D0 =
    /// `slot19(+1)` (the base 0x4BC640: `slot11(slot10() + 1)`; a colour accessor's 0x4B95A0
    /// steps over the custom row, `colour_step`). `slot10()` is −1 with nothing selected
    /// (0x4BC4B7), which steps to row 0. An accessor `combo_rows` does not list takes
    /// `set_accessor(+1)`, which is the same wrap for each of them (read 2026-09-30): the
    /// choice accessors (`Display.*`, `Graphics.*`, `Car.*`, `Audio.*`, `Comms.Collision`,
    /// `CDPlayer.Options`/`View`, `Tournament.EventType`: `rem_euclid` over their option list), the
    /// int ranges' own arm, `Comms.Positional` (the two-row list), `Comms.IPList` / the lobby
    /// lists (`rem_euclid` over the rows), `Comms.ModemList` / `PhoneList` (no device: empty, no
    /// step) and `CDPlayer.Title` (`cd_set_title`, wrapping, 0x512100).
    fn combo_step_forward(&mut self, name: &str) {
        let base = name.split_once('(').map_or(name, |(b, _)| b.trim());
        if COLOUR_ACCESSORS.contains(&base) {
            let base = base.to_string();
            self.colour_step(&base, 1);
            return;
        }
        let Some((rows, cur)) = self.combo_rows(name) else {
            self.set_accessor(name, 1);
            return;
        };
        if rows.is_empty() {
            return;
        }
        let next = match cur {
            Some(c) if c + 1 != rows.len() => c + 1,
            Some(_) => 0,
            None => 0,
        };
        self.combo_select(name, next);
    }

    /// A release in closed combo `i`'s caption strip with no `data.callback` / `destination`:
    /// slot 21 and sound 5 (0x4C6C2B…0x4C6C3D).
    pub(crate) fn combo_strip_release(&mut self, i: usize) {
        let Some(a) = self.fe.layout_preview.as_ref().and_then(|l| l.combo_sliders().into_iter().find(|w| w.index == i)).and_then(|w| w.accessor) else {
            return;
        };
        self.combo_step_forward(&a);
        self.fe_sound(5);
        self.bind_after_accessor();
    }

    /// The commit of row `row` of combo `i`'s open drop-down (the release path above it in
    /// `window_event` says which addresses): the latch for a colour box's custom row, else slot
    /// 11; sound 5 unless `data.disablesound`; the list closed unless the latch went up. `false`
    /// when the widget is no combo with an accessor.
    /// The colour accessor the open `NFS5.ColorBox` edits (`[this+0x220]`), without its modifier.
    pub(crate) fn picker_accessor(&self) -> Option<String> {
        let l = self.fe.layout_preview.as_ref()?;
        let i = l.open_picker()?;
        let acc = l.combo_sliders().into_iter().find(|w| w.index == i)?.accessor?;
        Some(acc.split_once('(').map_or(acc.as_str(), |(b, _)| b.trim()).to_string())
    }

    /// **Slot 27, 0x4B9320(`slot`)**, the tail of the editor's press and move (0x4C47E3…0x4C480E
    /// and 0x4C4900…0x4C493B: `0x5019C0(well, colour)` writes the well, then `accessor->slot27(well)`
    /// with `[this+0x284]`, the well being edited): the accessor takes the list row
    /// `count + 0x501A40(slot) + 1` through slot 11 (0x4B92B0) — `0x501A40` is the number of
    /// non-zero wells *before* `slot` — so the row of the well being dragged is the one selected
    /// and, through 0x4B92B0's saved-colour arm (0x4B930C), the accessor's colour follows the well.
    /// This is what lets the car change colour live while the editor is dragged, and what puts the
    /// selection on a saved colour once the editor has changed it.
    pub(crate) fn picker_tell_accessor(&mut self) {
        let Some(base) = self.picker_accessor() else { return };
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let (well, wells) = (l.picker().well.min(7), l.picker().wells);
        let Some((cols, _, _)) = self.colour_state(&base) else { return };
        let rank = wells[..well].iter().filter(|c| **c != 0).count();
        self.colour_select(&base, cols.len() + 1 + rank);
        self.bind_after_accessor();
    }

    pub(crate) fn combo_commit(&mut self, i: usize, row: usize) -> bool {
        let Some(acc) = self.fe.layout_preview.as_ref().and_then(|l| l.combo_sliders().into_iter().find(|w| w.index == i)).and_then(|w| w.accessor) else {
            return false;
        };
        let custom = self.combo_rows(&acc).and_then(|(rows, _)| rows.iter().position(|r| matches!(r, ListRow::CustomColour(_))));
        let latch = custom == Some(row);
        if latch {
            // the editor's first drawn frame reads the accessor's row into `[this+0x29C]`
            // (0x4C3CBD…0x4C3CC8), the same instant it copies the wells
            let base = acc.split_once('(').map_or(acc.as_str(), |(b, _)| b.trim());
            // `[this+0x29C]` is the accessor's row (slot 0x28 = `[+0x18]`), and cancel hands that
            // row back to slot 11 (0x4C4A08) — for an `OwnedCar.*` accessor the row it kept, not the
            // row of the car's colour
            self.fe.picker_prev = if (base.starts_with("OwnedCar.") && self.garage_car().is_some()) || self.generic_colour_has_row(base) {
                self.colour_row_of(base).map(Ok)
            } else {
                self.colour_state(base).map(|(_, at, custom)| custom.map_or(Ok(at), Err))
            };
            if let Some(l) = self.fe.layout_preview.as_mut() {
                l.raise_picker_latch();
                // the editor's draw copies all eight wells the first frame it is up, and cancel
                // puts that copy back (§4.66)
                l.picker_opened();
            }
        } else {
            self.combo_select(&acc, row);
        }
        if !self.fe.layout_preview.as_ref().is_some_and(|l| l.has_int_attr(i, "disablesound")) {
            self.fe_sound(5);
        }
        if latch {
            return true;
        }
        self.bind_after_accessor();
        self.fe.layout_open_combo = None;
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.set_open_combo(None);
        }
        true
    }

    pub(crate) fn combo_rows(&self, name: &str) -> Option<(Vec<ListRow>, Option<usize>)> {
        let (base, modifier) = match name.split_once('(') {
            Some((b, m)) => (b.trim(), m.trim_end_matches(')').trim()),
            None => (name, ""),
        };
        let m = self.fe.menu.as_ref()?;
        let num = |lo: i64, hi: i64, cur: i64| {
            let rows = (lo..=hi).map(|v| ListRow::Text(v.to_string())).collect();
            Some((rows, Some((cur - lo).max(0) as usize)))
        };
        match base {
            // **`zMergedList.Car` is two lists, not one.** `FEMergedCarListAccessor` (vtable
            // 0x5B773C) holds list A at `[this+0x28]` and list B at `[this+0x2C]`. Its rebuild
            // 0x4F4DB0 rebuilds both and then sets its own count to `A.count + B.count`
            // (0x4F4DD6 `add edi, eax` into 0x4BC310), and its row text 0x4F4A90 answers
            // `i < A.count ? A.row(i) : B.row(i − A.count)`. Its one construction, 0x4F2278,
            // passes the owning object's `[esi+0x14]` and `[esi+4]`, which the same registration
            // built as **`OwnedCars.Car`** (0x4F1ED2) and **`GenericCar.Car`** (0x4F20C5). Both
            // sub-lists' row text is the car record's name field — `owned+6` at 0x4F5C5F and
            // `record+2` at 0x4F300F, which is the same `display_name`. So the drop-down is the
            // player's garage first, then the whole catalogue. `RE/notes/controls.md` §4.133.
            //
            // **And its garage half is grouped — correction, its catalogue half likely is too.**
            // Slot 26 (0x4F4D40) routes the row height to `A->slot26(i)` below `A.count` and
            // `B->slot26(i − A.count)` above it, and slot 25 (0x4F4D20) is `A->slot25() +
            // B->slot25()` — the two lists' heights added. List A is `OwnedCars.Car`, whose rows
            // are 42 / 28 / 14 with era and class headings (§4.144). This paragraph used to say
            // list B ("stays flat over the catalogue") too, on the strength of nothing more than
            // B not having its own §4.144 entry yet — **not true**: B is `GenericCar.Car`, a bare
            // `FEGenericCarAccessor` (§4.137), and cluster 9's own pass (`catalog_rows`'s doc,
            // 0x4F39E0) found that class's *own* slot 26 is 40/27/14 with the identical
            // era-then-class heading, not the base's flat 14 — so `B->slot26` is almost certainly
            // grouped too, the same as the standalone `GenericCar.Car`/`DealerCar.Car`/
            // `GenericCar2.Car` arms below now are (`catalog_rows`). **Not implemented here**:
            // grouping this merged list's own B half needs its own row/index bookkeeping (the `a`
            // boundary below assumes a flat `m.cars.len()`-sized B, and `combo_select`'s
            // `zMergedList.Car` catalogue-half branch assumes the same) that this pass did not
            // take on — left an honest, flat B half rather than a half-guessed grouping. §4.148.
            "zMergedList.Car" => {
                let short = modifier == "SHORT";
                let mut rows: Vec<ListRow> = Vec::new();
                self.garage_rows(&mut rows, short, usize::MAX);
                let a = rows.len();
                rows.extend(m.cars.iter().map(|c| {
                    ListRow::Text(if short { c.family.to_string() } else { c.display_name.to_string() })
                }));
                // **the current row, either half.** `slot10()`'s own field is `car_garage`
                // (0x4F4DF0, above): below `A.count` it is the garage's own row — `garage_rows`
                // over the same `select` finds it — otherwise `A.count + car_idx` into the
                // catalogue (list B).
                let cur = if m.car_garage {
                    self.garage_rows(&mut Vec::new(), short, self.career.as_ref().map_or(usize::MAX, |c| c.car_sel))
                } else {
                    Some(a + m.car_idx)
                };
                Some((rows, cur))
            }
            // the car tables — `(SHORT)` is the family name, the plain form the display name,
            // exactly as the caption resolves them
            // `usedcars.lay`'s `ID_CHOOSEUSEDBOX` is a combo on `UsedCar.Car(SHORT)` — the market,
            // one row per generated car, not the catalogue
            "UsedCar.Car" => {
                let c = self.career.as_ref()?;
                let rows = self
                    .used_cars()
                    .into_iter()
                    .filter_map(|o| c.tables.car(o.car_id))
                    .map(|e| ListRow::Text(if modifier == "SHORT" { e.family.to_string() } else { e.display_name.to_string() }))
                    .collect::<Vec<_>>();
                let cur = self.fe.usedcar_sel.min(rows.len().saturating_sub(1));
                Some((rows, Some(cur)))
            }
            // **`OpponentCar.Car` is not a car list.** `FEOpponentCarAccessor`'s row text
            // (0x4F5720) is a jump table over five `FEStrings` ids — 640 `same car`, 642
            // `same era`, 643 `same class`, 644 `same era/class`, 641 `random car`, in that row
            // order — and its own `slot25` answers **70**, five rows of fourteen. The value the
            // widget carries is the row index, which is what `[accessor+0x33C4]` holds and what
            // `nfs_game::opponents::OpponentChoice::from_index` reads (`modes.md` §1.1a).
            "OpponentCar.Car" => {
                let rows = nfs_game::opponents::OpponentChoice::ROWS
                    .iter()
                    .map(|c| ListRow::Text(self.fe.strings.get(c.string_id()).unwrap_or_default().to_string()))
                    .collect();
                Some((rows, Some((m.opp_car as usize).min(4))))
            }
            // **The garage picker is the garage, and it is grouped.** `garage.lay`'s
            // `ID_CHOOSECARBOX` and `paintshop.lay`'s `ID_COMBOBOX` are `OwnedCars.Car(SHORT)`,
            // and `FEOwnedCarAccessor`'s row heights (slot 26, **0x4F60F0**) are **42** for the
            // first row and for any row whose **era** differs from the one above it, **28** when
            // only its **class** differs, and **14** otherwise. The painter 0x4F5D48 draws the
            // heading block for exactly those changes — two rules 13 apart with
            // `FEStrings[the id]` between them — and the ids are the FE car object's `+0x1B4` and
            // `+0x1B0`, which are the record's `+0x1B0` **era** and `+0x1AC` **class** (the
            // object holds the record at +4, `savegame.md` §4.7a). §4.144.
            "OwnedCars.Car" => {
                let c = self.career.as_ref()?;
                let mut rows: Vec<ListRow> = Vec::new();
                let at = self.garage_rows(&mut rows, modifier == "SHORT", c.car_sel.min(c.career.garage.len().saturating_sub(1)));
                Some((rows, at))
            }
            // **The catalogue lists are grouped too**, by `FEGenericCarAccessor`'s own slot 26
            // (`catalog_rows`'s doc, 0x4F39E0) — `DealerCar.Car` (the career dealership) and
            // `GenericCar.Car` (the quick-race/showroom catalogue, `m.cars`) are both already
            // `era_class_year_sort`-ordered, so the same boundary test the height function makes
            // is always a real one.
            "DealerCar.Car" if let Some(v) = self.dealer_cars() => {
                let short = modifier == "SHORT";
                let at = self.fe.dealer.car.min(v.len().saturating_sub(1));
                Some(self.catalog_rows(v.into_iter(), short, Some(at)))
            }
            // one row, the era's prize car (`App::prize_car`)
            "PrizeCars.Car" if let Some(car) = self.prize_car() => {
                Some((vec![ListRow::Text(if modifier == "SHORT" { car.family.to_string() } else { car.display_name.to_string() })], Some(0)))
            }
            "GenericCar.Car" => Some(self.catalog_rows(m.cars.iter(), modifier == "SHORT", Some(m.car_idx))),
            "PrizeCars.Car" | "CARSETTINGS.CAR" => {
                let rows = m
                    .cars
                    .iter()
                    .map(|c| {
                        ListRow::Text(if modifier == "SHORT" { c.family.to_string() } else { c.display_name.to_string() })
                    })
                    .collect();
                Some((rows, Some(m.car_idx)))
            }
            // `GenericCar2.Car`'s own list `cars2` — `[0x6919F4]`, separate from `cars` above
            // (`Menu::cars2`'s field doc) — same class, same grouping as `GenericCar.Car`.
            "GenericCar2.Car" => Some(self.catalog_rows(m.cars2.iter(), modifier == "SHORT", Some(m.car2_idx))),
            // `LoopedTrack.Track` is its own accessor over the circuits `[0x691D08]`, and
            // `Knockout.Track` forwards to the selected row's `Track<n>.Track` (0x4B8580)
            "LoopedTrack.Track" => {
                let l = m.looped_tracks();
                let cur = if m.looped_idx < l.len() { m.looped_idx } else { 0 };
                Some((l.iter().map(|t| ListRow::Text(self.track_full_name(t))).collect(), Some(cur)))
            }
            "Knockout.Track" => Some((m.tracks.iter().map(|t| ListRow::Text(self.track_full_name(t))).collect(), Some(m.ko_row(m.ko_sel).track_idx))),
            "Track1.Track" | "Track.Track" => {
                Some((m.tracks.iter().map(|t| ListRow::Text(self.track_full_name(t))).collect(), Some(m.track_idx)))
            }
            "Record.Track" => self.record_track_rows(),
            // **A colour list is swatches, not numbers.** `FEColourAccessor` (vtable 0x5B4E60,
            // built by 0x4B91F0 with a count and a colour callback at `[this+0x24]` / `[this+0x28]`)
            // draws each row through 0x4B9390: rows below the count are the car's colours as
            // 20-wide swatches, the row **at** the count is `FEStrings` 0x263 (`custom...`), and
            // the rows past it are the profile's saved custom colours in use (0x5019F0, the n-th
            // non-zero of `[0x691D98]`). The list is `count + 0x5019D0() + 1` rows when the
            // accessor's `[+0x30]` is set (0x4B9232, 0x4B9353) and `count` rows when not — the
            // widget writes it from `data.allowcustom` (0x4B9590), so the layout drops the tail
            // for a box with `allowcustom=0`. The callbacks, per accessor (0x4F1EE9…0x4F2200):
            //
            // | accessor | count | colour(i) |
            // |---|---|---|
            // | `OwnedCar.ExtColour` | 0x4F0E20: `record+0x3E6` | 0x4F0C60: `record+0x3EC + 16i` — set colour 0 |
            // | `OwnedCar.S1Colour` | 0x4F0E20 | 0x4F0DA0: `record+0x3F0 + 16i` — colour 1 |
            // | `OwnedCar.S2Colour` | 0x4F0E20 | 0x4F0E60: `record+0x3F4 + 16i` — colour 2 |
            // | `OwnedCar.IntColour` | 0x4F0CE0: `record+0x528` | 0x4F0D20: `record+0x530 + 16i` |
            // | `GenericCar.Colour`, `DealerCar.Colour` | car slot 40 (0x4F3280) | slot 42: set colour 0 |
            // | `DealerCar.IntColour` | slot 41 (0x4F3380) | slot 43: the interior |
            //
            // each colour the dword byte-swapped (`b0<<24 | b1<<16 | b2<<8 | b3`), `b0` the alpha
            // — 0xFF in every entry of the shipped `nfs5.car`. The selection is the row itself
            // (0x4B92B0 stores `colour(row)` for a row below the count).
            "OwnedCar.ExtColour" | "OwnedCar.IntColour" | "OwnedCar.S1Colour" | "OwnedCar.S2Colour"
            | "GenericCar.Colour" | "DealerCar.Colour" | "DealerCar.IntColour" => {
                let (cols, _, _) = self.colour_state(base)?;
                // this build is the colour box's tick (`bind_lists`), whose head runs the
                // accessor's slot 2 — the recount 0x4B9350 (0x4C5460 → 0x4BD230) — before
                // the row is read
                self.colour_recount(base);
                let (rows, _) = self.colour_rows(cols, 0);
                Some((rows, self.colour_row_of(base)))
            }
            // the direction's rows are `DIRECTION_OPTIONS`' four (`FEDirectionAccessor`'s fixed
            // table), the selection `REVERSE + 2·MIRROR`; the traffic's `TRAFFIC_OPTIONS`' four.
            // The `Knockout.*` ones are the selected row's (row 0 is `Track1`, `Menu::ko_row`).
            "Track1.Direction" | "Knockout.Direction" => {
                let row = if base == "Knockout.Direction" { m.ko_row(m.ko_sel) } else { m.ko_row(0) };
                let rows = DIRECTION_OPTIONS.iter().map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string())).collect();
                Some((rows, Some(row.direction())))
            }
            "Knockout.Traffic" => {
                let t = m.ko_row(m.ko_sel).traffic;
                let rows = TRAFFIC_OPTIONS.iter().map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string())).collect();
                Some((rows, Some((t as usize).min(3))))
            }
            "Knockout.Laps" => num(1, LAPS_MAX as i64, m.ko_row(m.ko_sel).laps as i64),
            // `Game.Difficutly` — the file's own spelling — rows are `DIFFICULTY_OPTIONS`' strings
            "Game.Difficutly" => {
                let rows = DIFFICULTY_OPTIONS.iter().map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string())).collect();
                Some((rows, Some((m.difficulty as usize).min(2))))
            }
            // `Tournament.Tournament` — class 0x5B88F0, whose `[this+0x24]` is **0x5053C0**, the
            // function that picks the tournament vector, and whose row text (slot 12, 0x506560) is
            // `FEStrings[rec+0x24]`, the port's `title_string_id`. 0x5053C0's own choice is the
            // jump table at 0x505450 over `Tournament.EventType`'s index: **0** falls to 0x505404,
            // which switches on the era id `[0x691E50]` — 0x209, 0x20A, 0x20B are 521, 522, 523,
            // the three eras — and **1**, **2**, **3** take the classic, golden and modern club
            // lists at 0x505447, 0x505441 and 0x50543B. §4.105.
            "Tournament.Tournament" => {
                let c = self.career.as_ref()?;
                let list = c.event_type_list(self.choice_index("Tournament.EventType").map_or(0, |(i, _)| i));
                let cur = c.current().and_then(|t| list.iter().position(|x| x.id == t.id));
                let rows = list
                    .iter()
                    .map(|t| ListRow::Text(self.fe.strings.get(if modifier == "SHORT" { t.name_string_id } else { t.title_string_id }).unwrap_or_default().to_string()))
                    .collect::<Vec<_>>();
                (!rows.is_empty()).then(|| (rows, cur.or(Some(0))))
            }
            // `Tournament.RemainingTracks` — class 0x5B7E80, `[this+0x24] = 0x505170`, and its row
            // text (slot 12, 0x4FA560) is `FEStrings[rec+4]` of the i-th record in that vector.
            // `+4` is a `.trk` record's own name id (the port's `Track::name_string_id`), which is
            // what makes these rows the remaining events' **track names**. 0x4FA575 wraps an index
            // at or past the count back to 0.
            "Tournament.RemainingTracks" => {
                let c = self.career.as_ref()?;
                let t = c.current()?;
                let rows: Vec<ListRow> = c
                    .career
                    .undone_events(t)
                    .into_iter()
                    .filter_map(|k| t.events.get(k))
                    .map(|e| ListRow::Text(c.tables.track_name(e.track_id).to_string()))
                    .collect();
                let at = self.fe.remaining_sel.min(rows.len().saturating_sub(1));
                (!rows.is_empty()).then(|| (rows, Some(at)))
            }
            // `Controller.AllControllers` and `Controller.AllPads` — two empty string lists
            // (0x4B8990 at 0x51E4AE and 0x51E4E1) that 0x511809 fills from the input layer's own
            // enumeration: the count `0x51F050()`, each device's name `0x51F060(i)` appended to
            // the controllers' list, and the same name appended to the pads' list only when
            // `0x51F080(i)` says so. §4.103.
            "Controller.AllControllers" | "Controller.AllPads" => {
                let pads_only = base == "Controller.AllPads";
                let rows: Vec<ListRow> = self
                    .gamepads
                    .devices()
                    .into_iter()
                    .filter(|(_, is_pad)| !pads_only || *is_pad)
                    .map(|(n, _)| ListRow::Text(n.to_string()))
                    .collect();
                let sel = if pads_only { self.fe.pads_sel } else { self.fe.controllers_sel }.min(rows.len().saturating_sub(1));
                (!rows.is_empty()).then(|| (rows, Some(sel)))
            }
            // `Graphics.Resolutions` — real modes now ([`App::resolution_list`]), formatted the
            // same way 0x4BC710 does, `"%d x %d x %d"` (0x5D7574). §4.104.
            "Graphics.Resolutions" => {
                let modes = self.resolution_list();
                let rows = modes.iter().map(|(w, h, d)| ListRow::Text(format!("{w} x {h} x {d}"))).collect();
                let sel = self.fe.resolution_sel.min(modes.len().saturating_sub(1));
                (!modes.is_empty()).then(|| (rows, Some(sel)))
            }
            // `Game.Damage` is the front end's one **boolean** accessor a layout names: class
            // 0x5B4C64, two rows, the row text from 0x4B8770 — index 1 is `FEStrings` 0x2C0 = 704
            // "on" and anything else 0x2C1 = 705 "off". §4.102.
            "Game.Damage" => {
                let rows = BOOL_OPTIONS.iter().map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string())).collect();
                Some((rows, Some(usize::from(m.damage))))
            }
            // and the four counted settings, at the ranges their own steppers clamp to
            "Game.NumOpponents" => num(0, 7, m.opponents as i64),
            "Game.NumKOOpponents" => num(1, 7, m.ko_opponents as i64),
            "Track1.Laps" => num(1, LAPS_MAX as i64, m.laps as i64),
            "Track1.Traffic" => num(0, 3, m.traffic as i64),
            _ => None,
        }
    }

    pub(crate) fn choice_rows(&self, name: &str) -> Option<(Vec<ListRow>, Option<usize>)> {
        let base = name.split_once('(').map(|(b, _)| b).unwrap_or(name);
        let (index, options) = self.choice_index(base)?;
        let rows = options
            .iter()
            .map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string()))
            .collect();
        Some((rows, Some(index)))
    }

    /// `Graphics.Resolutions`' rows (COVERAGE §14.6.3): the original's
    /// **`0x4BC710`** formats one row per Direct3D device mode as `"%d x %d x %d"` (`0x5D7574`)
    /// from `0x44E720(i, &out)`'s width/height/depth over the modes `0x44E780(i)` passes — a real
    /// enumeration this port cannot reproduce (there is no Direct3D device, and `wgpu`'s surface
    /// takes whatever size the window is, not a fixed device mode list). `accessors.rs:296` and
    /// `main.rs`'s `choice_rows` used to hardcode one fake row, `"640 x 480 x 16"`, so the
    /// dropdown could never offer a real choice.
    ///
    /// **PORT-DEVIATION D11 (PORT-DEVIATIONS.md)** [0x4BC710, 0x44E720, 0x44E780]: this lists the distinct sizes of the **current monitor's** own video
    /// modes (`winit::monitor::MonitorHandle::video_modes`, the modes the OS itself reports as
    /// running the display at) — the modes this port can actually ask the window to become, not
    /// the original's Direct3D device-mode list, which depended on the graphics driver and has no
    /// equivalent here. The bit depth is the monitor mode's own (`VideoMode::bit_depth`); this
    /// port always renders in a modern `wgpu` surface format regardless of it, so the depth column
    /// is cosmetic (matching the original's own format), not a real render setting. With no window
    /// yet (should not happen once one is created at start-up) or no monitor reachable (a
    /// sandboxed / headless test run), it falls back to one row at the window's own current inner
    /// size, `"{w} x {h} x 32"` — the same "no hardware device: one mode" shape 0x44E72A's arm
    /// takes, PORT-DEVIATION D11 on the depth (0x44E73D) (the original's own fallback is `0x10`, 16-bit, a
    /// DirectDraw-era default this port has no equivalent reason to pick).
    pub(crate) fn resolution_list(&self) -> Vec<(u32, u32, u32)> {
        let Some(window) = self.window.as_ref() else { return vec![(640, 480, 32)] };
        let modes: std::collections::BTreeSet<(u32, u32, u32)> = window
            .current_monitor()
            .into_iter()
            .flat_map(|m| m.video_modes())
            .map(|m| {
                let s = m.size();
                (s.width, s.height, m.bit_depth() as u32)
            })
            .collect();
        if modes.is_empty() {
            let s = window.inner_size();
            vec![(s.width, s.height, 32)]
        } else {
            modes.into_iter().collect()
        }
    }

    /// `Graphics.Resolutions`' pick: it is remembered here and **not** applied to the window. The
    /// original's front end always runs in 640x480x16 (`FEApplication::run` 0x4D3420 sets that mode
    /// first, 0x4D3440); the pick reaches the option globals `[0x657A48/4C/50]` (option ids
    /// 0x3E/0x3F/0x40, written by the accessor's slot 1 0x4BC850 into the launch's option stream)
    /// and the device mode is switched to them only when a race is set up (`SetMode` 0x467FC0 at
    /// 0x41562E / 0x4B6CB9) — [`App::enter_race_mode`]; the front end's own mode comes back at the
    /// next 0x4D3440 ([`App::leave_race_mode`]).
    pub(crate) fn apply_resolution(&mut self) {
        let Some((w, h, _)) = self.resolution_list().get(self.fe.resolution_sel).copied() else { return };
        let saved = self.fe.resolution_race.and_then(|(_, s)| s);
        self.fe.resolution_race = Some(((w, h), saved));
    }

    /// `SetMode(0x657A48, 0x657A4C, 0x657A50, 2)` at a race's set-up (0x41562E, 0x4B6CB9): the
    /// window takes the picked size (a mode never picked keeps the current one — 0x467FF5 falls
    /// back to the current mode when the width is 0).
    pub(crate) fn enter_race_mode(&mut self) {
        let Some(((w, h), saved)) = self.fe.resolution_race.as_mut() else { return };
        let Some(window) = self.window.as_ref() else { return };
        if saved.is_none() {
            *saved = Some(window.inner_size());
        }
        let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(*w, *h));
    }

    /// The front end's own mode back (0x4D3440, 640x480x16 at every front-end entry): the window
    /// returns to the size it had when the race took the picked mode.
    pub(crate) fn leave_race_mode(&mut self) {
        let Some((_, saved)) = self.fe.resolution_race.as_mut() else { return };
        if let (Some(size), Some(window)) = (saved.take(), self.window.as_ref()) {
            let _ = window.request_inner_size(size);
        }
    }

    pub(crate) fn choice_index(&self, name: &str) -> Option<(usize, &'static [u32])> {
        let (_, key, options) = CHOICE_ACCESSORS.iter().find(|(n, _, _)| *n == name)?;
        // `Tournament.EventType`'s count is the era's (0x506AA0) and its table the club lists
        // alone once the Evolution is over (0x4FD1A0) — `CareerUi::event_type_rows`
        let options: &'static [u32] = match self.career.as_ref() {
            Some(c) if name == "Tournament.EventType" => c.event_type_rows(),
            _ => options,
        };
        // diagnostic: `NFS_CHOICE=Graphics.CarChrome=2,Display.RearView=1` overrides a choice for
        // a `--screenshot` run, where no front end sets it
        if let Ok(over) = nfs_game::config::cfg().nfs_choice.var()
            && let Some(v) = over.split(',').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == name).and_then(|(_, v)| v.trim().parse::<usize>().ok())
        {
            return Some((v.min(options.len() - 1), options));
        }
        // `OwnedCar.PackageNum` shows the garage car's `FECar+0x684` (slot 33, 0x4F6F50)
        if name == "OwnedCar.PackageNum"
            && let Some((_, o)) = self.garage_car()
        {
            return Some(((o.trim.package as usize).min(options.len() - 1), options));
        }
        // the fourteen Display options are the profile's HUD record (`hud_cfg`), not an accessor value
        let cfg = Self::hud_cfg2_slot(key).and_then(|slot| self.hud_cfg_get(slot));
        let v = match cfg.or_else(|| self.choices.get(Self::choice_slot(name, key)).copied()) {
            Some(v) => v,
            // a Display option `fe.txt` does not carry keeps its `nfs5-2.hud` Config 1 default
            // (hud.md §2) — only `gear` is missing from the shipped file, and its default is 1.
            //
            // Four `Graphics.*` accessors are missing from the shipped `fe.txt` too, and unlike
            // `gear` a bare `0` is provably wrong for all four (RE/notes/REFERENCE-2026-09-28.md
            // "Pass 13" §3, live on the original with a from-scratch profile): `LensFlare`
            // ("блики") and `SkyRendering` ("небо") never read `off`/index 0 on either a
            // genuinely fresh profile or a long-played one, and `CarChrome`/`CarDetail`
            // ("освещение авто"/"детали авто") never read their own list's index 0 either.
            // `native_TEST.sav` — a real original save, captured by `CREATEPLAYERDONE` alone
            // with no later Options visit, so its accessor block is untouched by any later
            // in-game edit — settles the exact index for all four: `Graphics.LensFlare=1`
            // (`on`), `Graphics.SkyRendering=2` (`1504` "high"), `Graphics.CarChrome=2` and
            // `Graphics.CarDetail=2` (`1504` "high" in the file's own 3-item list — but the RUN-TIME list is
            // `nfs5.dll`'s five items 0x5B6D5C, where index 2 is `401` «обычно», which is what the Wine
            // fresh profile's screen shows, `RE/notes/nfs5-dll.md` §2.1).
            // `DRIVER`'s long-played profile (pass 12/13) independently agrees on `SkyRendering`
            // and `CarChrome` (`высоко` both); its own `CarDetail` read «обычно» — the same index 2
            // under the five-item list (it was read as index 1 against the three-item one).
            //
            // Three more `Graphics.*` accessors — `TextureDetail`, `SpecialFX`, `ViewDistance`
            // ("детализация текстур"/"окружающая среда"/"дистанция обзора") — are **not** in
            // this list: the shipped `fe.txt` already carries `texturedetail=3` /
            // `specialfx=3` / `adjustableviewdistance=3`, which `self.registry.u32` below
            // already reads ahead of any hardcoded fallback, and those three already match the
            // real original's own readings for a capable machine (`очень высоко`/`очень высоко`
            // for the last two, on both `DRIVER` and `native_TEST.sav`; `TextureDetail`'s own
            // in-binary hardware probe — `0x44DFB0` calling the tier classifier `0x44E370`,
            // which grades texture memory `[0x619790]`, total physical RAM (`call 0x53C3D0`),
            // a CPU-speed benchmark `[0x5CE908]` and the colour depth `[0x61978C]` into a
            // 0..3 tier written straight into the `TextureDetail` record slot `[0x657D88]` —
            // saturates at tier 3 for any texture memory ≥ 14 MB / RAM ≥ 85 MB / CPU benchmark
            // ≥ 450 units / colour depth > 16, thresholds any real machine the port targets
            // clears trivially, so tier 3 — `1504` "high", `fe.txt`'s shipped `texturedetail=3`
            // — is the correct outcome of that same probe on capable hardware, not a guess).
            // No exact hardware-probe address was found for `LensFlare`/`SkyRendering`/
            // `CarChrome`/`CarDetail` themselves (searched every literal-address writer of
            // their record slots, `[0x657D60]`/`[0x657D68]`/`[0x657D7C]`/`[0x657D9C]`; only the
            // accessors' own UI-driven commit callbacks write them, `0x4E5710`/`0x4E56D0`) — the
            // `SkyRendering`/`CarChrome`/`CarDetail` values above are the best-evidenced real
            // numbers from genuine save data (APPROXIMATION for the exact tier, not for the
            // "never off/never lowest" direction, which is CONFIRMED).
            //
            // **Where the four-tier probe results land (PROBE 2026-10-02, read on nfs5.exe, the
            // file's 0x44E370…0x44E670 are `jmp`s into the runtime DLL under Wine, so the rules are
            // SRVC §2's gdb-read ones):** the options container's constructor 0x4E5760 builds the
            // accessors through 0x4BB680 (CarChrome 0x4E57DF, CarDetail 0x4E5816, both with the list
            // 0x5B6D94 = 1502 / 1503 / 1504 = IDS_LOW «низко» / IDS_OP_MEDIUM «обычно» / IDS_HIGH «высоко»,
            // festrings.h) and ends (0x4E5B14…0x4E5B8E) by calling SetSelection (vtable +0x2C = 0x4BB6E0 →
            // 0x4BC240, clamped to count-1, then the commit callback 0x4E5710 / 0x4E56D0 writes
            // `[0x657D60]` / `[0x657D68]`) with the probe's tier: CarChrome ← 0x44E5F0 (0x4E5B51),
            // CarDetail ← the same 0x44E5F0 (0x4E5B62), sky ← 0x44E410, texture ← 0x44E370, SpecialFX ←
            // 0x44E480, ViewDistance ← 0x44E670, WideScreen ← 0, TripleBuffer ← 1. 0x44E5F0 reads
            // texture memory, RAM and the CPU benchmark only — NOT the colour depth — so the 16-bit
            // «D < 32 → 2» rule of 0x44DFB0 (which maps `[0x657D88]` → `[0x619770]` for textures only,
            // called from 0x4B67BD) does not touch car detail. gdb on the Wine run saw 0x44E5F0 return 2
            // for both accessors. Hence a modern machine (T ≥ 6 MB, R ≥ 64 MB, C ≥ 302) starts with
            // CarChrome = CarDetail = 2 («высоко»): the port's 2 / 2 is right for the 32-bit case.
            // **Settled (NFS5DLL, 2026-10-02):** the Wine fresh profile's Graphics screen read car detail
            // «обычно» because the run-time list is `nfs5.dll`'s 0x5B6D5C (1506 / 1502 / 401 / 1504 / 1507),
            // not the file's 0x5B6D94: the constructor's 2 is that list's index 2 = 401 «обычно»; nothing
            // lowers it. CarChrome keeps the three-item list (the dll leaves 0x4E57DF alone).
            //
            // **Read from the running original's memory (2026-10-01 SV, Wine, dx7z, gdb, an empty
            // `savedata`, the first front-end frame, before any profile):** `[0x657D60]` CarChrome = 2,
            // `[0x657D68]` CarDetail = 2, `[0x657D7C]` LensFlare = 0, `[0x657D9C]` SkyRendering = 0 (and
            // `[0x657D88]` TextureDetail = 3); the same four after `DRIVER`'s profile was loaded; 0 / 0 / 0 / 0
            // in a race launched with `fe = 0` (the stream zeroes the block, 0x4B4DB6). So the two detail
            // words are 2 = the port's 2 from the start, while the lens flare and the sky are 0 in the
            // globals where the port's accessor default is 1 / 2: the accessors' own state (what the
            // Graphics screen shows, `DRIVER` pass 12/13: lens on, sky high) is not the same thing as these
            // globals until something commits it. *(2026-10-02, what commits it is the race
            // launch — 0x4ED5D0 → 0x4ED653 → 0x4B7F10 dumps every accessor with a registry id into
            // the option stream through slot 1 = 0x4BC3D0, and 0x4B4DA0 / 0x4B4B80 store it into the
            // cleared block, so a race started from the front end has `[0x657D7C]` = this accessor's
            // selection; `RE/notes/lighting.md` §7. `lens_flare.rs` reads it at race load.)* The Graphics screen of a truly fresh profile could not be
            // reached (mouse buttons never reached the game on the server), so the defaults stay the port's.
            // the race HUD's option block `cfg2` is the profile's `XXhud2XX.hud` — a copy of the
            // shipped `nfs5-2.hud`'s record for the layout — and it, not `fe.txt`, is where a
            // profile that never changed a Display option has them (HM, [`hud_cfg2_default`])
            None => {
                // a `fe = 0` launch (`App::fe0`) has no dump, the block is cleared and holds only the keys
                // `fe.txt` / the command line name (`modes.md` §1.2, the SV run's `skyrendering = 2` line): the four
                // accessors the shipped file has no key for read 0 there — the dump's selection exists only when the
                // front end launched the race. `NFS_CHOICE` still overrides (above).
                let hw_default = match name {
                    _ if self.fe0 => 0,
                    "Graphics.LensFlare" => 1,
                    "Graphics.SkyRendering" | "Graphics.CarChrome" | "Graphics.CarDetail" => 2,
                    _ if *key == "gear" => 1,
                    _ => 0,
                };
                self.registry.u32(key, hw_default) as usize
            }
        };
        Some((v.min(options.len() - 1), options))
    }

    /// The slot of a Display option in a `nfs5-2.hud` record's option run (cfg2 `+0x30…+0x64`,
    /// `hud.md` §2: SPEEDOMETER, SPEEDUNITS, HUDTACHOMETER, DAMAGEDISPLAY, MAP, MAPSCALE,
    /// OPPONENTLIST, SPLITTIMES, DISPLAYCOLORS, DRIVERSNAPSHOT, TRANSLUCENCY, REARVIEWMIRRROR,
    /// RECORDTIME, GEAR). The `fe.txt` keys of the thirteen the Display screen shows.
    pub(crate) fn hud_cfg2_slot(key: &str) -> Option<usize> {
        [
            "speedometer",
            "speedunits",
            "tachometer",
            "damagedisplay",
            "map",
            "mapscale",
            "opponentlist",
            "splittimes",
            "displaycolors",
            "driversnapshot",
            "translucency",
            "rearviewmirrror",
            "recordtime",
            "gear",
        ]
        .iter()
        .position(|k| *k == key)
    }

    /// The race HUD's option tables for the current profile ([`hud_cfg`]), loaded the first time they
    /// are needed and again whenever the profile changes: `<Name>1.hud` / `<Name>2.hud` of the save
    /// directory, else the shipped `FEData/Data/nfs5-1.hud` / `nfs5-2.hud` (the `XXhud?XX.hud` of
    /// 0x41F7A0).
    pub(crate) fn hud_cfg_ensure(&self) {
        let who = self.career.as_ref().map(|c| (c.save_dir.clone(), c.profile.name.clone()));
        let mut cache = self.hud_cfg.borrow_mut();
        if cache.loaded && cache.who == who {
            return;
        }
        let shipped = self.paths.fe_data().join("Data");
        let loaded = hud_cfg::load(who.as_ref().map(|(d, n)| (d.as_path(), n.as_str())), &shipped);
        cache.who = who;
        cache.loaded = true;
        (cache.cfg, cache.from_profile) = match loaded {
            Some((c, p)) => (Some(c), p),
            None => (None, false),
        };
    }

    /// The record the HUD options are read from and written to: `Display.ScreenLayout`'s "Config N" at
    /// the race's resolution (0x41F7A0). The resolution is the one the Graphics screen picked
    /// ([`App::apply_resolution`], the launch's `[0x657A48/4C]`), 640×480 before any pick — the
    /// shipped file's own records.
    fn hud_cfg_key(&self) -> (usize, u32, u32) {
        let layout = self.choices.get("screenlayout").copied().unwrap_or(0).min(3);
        let (w, h) = self.fe.resolution_race.map_or((640, 480), |(wh, _)| wh);
        (layout, w, h)
    }

    /// Display option `slot` (cfg2 `+0x30 + 4·slot`) as the Display screen's accessor holds it: the
    /// value changed since `INUSE` filled it (0x4CB650), else the current record's — the layout's own
    /// 640×480 record's while the (layout, resolution) record has not been made (0x41EBF0 copies it).
    fn hud_cfg_get(&self, slot: usize) -> Option<usize> {
        self.hud_cfg_ensure();
        let (layout, w, h) = self.hud_cfg_key();
        let cache = self.hud_cfg.borrow();
        cache.cfg.as_ref()?.accessor_value(layout, w, h, slot).map(|v| v as usize)
    }

    /// **`NFS5.HUD`'s pieces** (OPTS, the draw 0x4CB970, read 2026-10-03; decompiled and checked
    /// against the disassembly): over the `disp` schematic the draw lays one `hud.fsh` piece for
    /// each option that shows an element, at the element's stored position — the getter's `f32`
    /// pair times 240 / 180 (0x5B6030 / 0x5B6034) from the schematic's origin — and at the piece's
    /// own size times `640 / W`, `480 / H` (0x4CBBEA…0x4CBC13; `W` / `H` are the `Graphics.Resolutions`
    /// row's, `0x4BAAF0` / `0x4BC840` — the first `%d` of 0x4BC710's `"%d x %d x %d"` is the
    /// `+0x38` array that 0x4BAAF0 indexes, so it is the width; `hud.md` had them the other way).
    /// The order is the draw's: Damage (1 → `dam3` entry 3, 2 → `dam1` entry 1 — both at 0x420D00,
    /// cfg1 `+0x1A0`; 3 → `dam2` entry 2 at 0x420D40, `+0x1B0`), Map (1, 2 → `lapG` entry 4,
    /// 3 → `Rada` entry 5, at the track map 0x420DC0), OppList ≠ 0 → `pos` 6 (0x420D60), RearView ≠ 0
    /// → `mir` 7 (0x420DE0), SplitTimes ≠ 0 → `mtme` 10 (0x420CE0, cfg2 `+0x0C`), Gear ≠ 0 → `gear`
    /// 11 (0x420CA0), Tach (1 → digital `spee` 12 at 0x420D20, 2 → `tach` 14 at 0x420D80), Speedometer
    /// (1 → `spee` 12 at 0x420CC0, 2 → `spee` 13 at 0x420DA0). `Units`, `MapScale`, `DispCols`,
    /// `Translucency` and `RecordTime` choose no piece (0x4CB970 reads none of them). The positions are
    /// the record's (the one 0x41E170 made current for the layout and resolution, [`Self::hud_cfg_key`]);
    /// none (no `.hud` file at all) → no pieces.
    pub(crate) fn hud_preview_pieces(&self) -> (Vec<(usize, f32, f32)>, f32, f32) {
        self.hud_cfg_ensure();
        let (layout, w, h) = self.hud_cfg_key();
        // the options first: `choice_index` borrows the cache itself
        let opt = |name: &str| self.choice_index(name).map_or(0, |(i, _)| i);
        let (damage, map, opp, rear, split, gear, tach, speedo) = (
            opt("Display.Damage"),
            opt("Display.Map"),
            opt("Display.OppList"),
            opt("Display.RearView"),
            opt("Display.SplitTimes"),
            opt("Display.Gear"),
            opt("Display.Tach"),
            opt("Display.Speedometer"),
        );
        let cache = self.hud_cfg.borrow();
        let Some(pos) = cache.cfg.as_ref().and_then(|c| c.find(layout, w, h).map(|i| c.stored_positions(i))) else {
            return (Vec::new(), 1.0, 1.0);
        };
        // indices into `stored_positions`
        const GEAR: usize = 0;
        const SPEED: usize = 1;
        const TACH_DIGITAL: usize = 3;
        const TACH: usize = 4;
        const SPEEDOMETER: usize = 5;
        const POSITION: usize = 6;
        const TRACK_MAP: usize = 7;
        const REARVIEW: usize = 9;
        const MULTI_TIME: usize = 10;
        const TEXT_DAMAGE: usize = 11;
        const GRAPHIC_DAMAGE: usize = 12;
        let mut out = Vec::new();
        let mut put = |entry: usize, at: usize| out.push((entry, pos[at].0 * 240.0, pos[at].1 * 180.0));
        match damage {
            1 => put(3, TEXT_DAMAGE),
            2 => put(1, TEXT_DAMAGE),
            3 => put(2, GRAPHIC_DAMAGE),
            _ => {}
        }
        match map {
            1 | 2 => put(4, TRACK_MAP),
            3 => put(5, TRACK_MAP),
            _ => {}
        }
        if opp != 0 {
            put(6, POSITION);
        }
        if rear != 0 {
            put(7, REARVIEW);
        }
        if split != 0 {
            put(10, MULTI_TIME);
        }
        if gear != 0 {
            put(11, GEAR);
        }
        match tach {
            1 => put(12, TACH_DIGITAL),
            2 => put(14, TACH),
            _ => {}
        }
        match speedo {
            1 => put(12, SPEED),
            2 => put(13, SPEEDOMETER),
            _ => {}
        }
        (out, 640.0 / w as f32, 480.0 / h as f32)
    }

    /// A Display accessor's setter: the accessor holds the value (`staged`) and nothing is written
    /// (N02, read 2026-10-02). The record and both files are written by 0x4CB860 → 0x41EA50 at
    /// exactly two moments: the HUD widget's `NOTINUSE` ([`Self::hud_cfg_apply`], slot 22 0x4CB820)
    /// and a change of the layout combo ([`Self::hud_cfg_layout_changed`], 0x4CB9A0). A change that
    /// arrives with the widget not yet told `INUSE` arms it, so it is never lost.
    pub(crate) fn hud_cfg_set(&mut self, slot: usize, v: usize) {
        self.hud_cfg_ensure();
        let mut cache = self.hud_cfg.borrow_mut();
        let Some(cfg) = cache.cfg.as_mut() else { return };
        cfg.armed = true;
        cfg.staged[slot] = Some(v as u32);
    }

    /// The HUD widget's `INUSE` (slot 21 0x4CB650, the screen shown or uncovered): flag armed, the
    /// accessors refilled from the current record — what they held is dropped.
    ///
    /// `INUSE` also runs **HUD_init** (0x4CB6E4 → 0x41E170, the first of a session: `[0x60C978]` 0):
    /// 0x41F7A0 searches the tables for the (layout, resolution) record and, when there is none,
    /// 0x41EBF0 appends the copy and **writes both files** at once (the two 0x59E130 calls at the tail of 0x41EBF0) — so
    /// the first Options visit at a resolution the profile has no record for creates
    /// `<Name>1.hud` / `<Name>2.hud` even when nothing is changed (N02a). At 640×480 the shipped
    /// tables (the `XXhud?XX` fallback) hold all four layouts' records: nothing is created.
    pub(crate) fn hud_cfg_in_use(&mut self) {
        self.hud_cfg_ensure();
        let (layout, w, h) = self.hud_cfg_key();
        let mut cache = self.hud_cfg.borrow_mut();
        let who = cache.who.clone();
        let Some(cfg) = cache.cfg.as_mut() else { return };
        cfg.in_use();
        if cfg.count() == 0 || cfg.find(layout, w, h).is_some() {
            return;
        }
        cfg.record(layout, w, h);
        cache.from_profile = true;
        if let Some((dir, name)) = who
            && let Some(cfg) = cache.cfg.as_ref()
            && let Err(e) = hud_cfg::save(cfg, &dir, &name)
        {
            tracing::warn!(error = %e, "could not write the profile's HUD option files");
        }
    }

    /// 0x4CB860 on the current record, then 0x41EA50: both files written. `disarm` is the widget's
    /// `NOTINUSE` (0x4CB820 tests `[this+0x218]` and clears it); the layout change (0x4CB9A0) stores
    /// whatever the screen shows and stays armed.
    fn hud_cfg_store(&mut self, disarm: bool) {
        self.hud_cfg_ensure();
        let (layout, w, h) = self.hud_cfg_key();
        let mut cache = self.hud_cfg.borrow_mut();
        let who = cache.who.clone();
        let Some(cfg) = cache.cfg.as_mut() else { return };
        if disarm {
            if !cfg.armed {
                return;
            }
            cfg.armed = false;
        }
        cfg.commit(layout, w, h);
        cache.from_profile = true;
        if let Some((dir, name)) = who
            && let Some(cfg) = cache.cfg.as_ref()
            && let Err(e) = hud_cfg::save(cfg, &dir, &name)
        {
            tracing::warn!(error = %e, "could not write the profile's HUD option files");
        }
    }

    /// Slot 22 0x4CB820 — the Options screen (which holds `display.lay`'s `NFS5.HUD`) is destroyed
    /// or its layer closed (`NOTINUSE`, the broadcast of 0x4D2570's swap and 0x4D5610): the options
    /// are stored when the widget was told `INUSE`.
    pub(crate) fn hud_cfg_apply(&mut self) {
        self.hud_cfg_store(true);
    }

    /// 0x4CB9A0: the layout combo's value differs from the one the widget last drew (`[this+0x21C]`):
    /// the options are stored on the record of the layout that was current — call this **before**
    /// `Display.ScreenLayout` takes its new value — and the accessors then read the new layout's.
    pub(crate) fn hud_cfg_layout_changed(&mut self) {
        self.hud_cfg_store(false);
    }

    /// **Port-only migration:** a profile whose `.sav` carries Display values but which has no
    /// `<Name>2.hud` yet (saved by a build before that change) keeps them — they seed the current record in
    /// memory, no file written. The original has no such path: its accessor values from the `.sav` are
    /// overwritten by cfg2 whenever the Display screen opens (0x4CB6F4…), so cfg2 is the master.
    pub(crate) fn hud_cfg_seed(&mut self, slot: usize, v: usize) {
        self.hud_cfg_ensure();
        let (layout, w, h) = self.hud_cfg_key();
        let mut cache = self.hud_cfg.borrow_mut();
        if let Some(cfg) = cache.cfg.as_mut() {
            let i = cfg.record(layout, w, h);
            cfg.set_opt(i, slot, v as u32);
        }
    }

    /// Where this port keeps a choice accessor's index. The original keeps it in the accessor
    /// object itself, `[this+0x18]` (0x4BC460 / 0x4BC480); the `fe.txt` key is only where the
    /// *saved* ones are written back. Several accessors have **no** `fe.txt` key — `Car.Roof`,
    /// `Comms.Collision`, `OwnedCar.PackageNum`, `CDPlayer.View`, `Tournament.EventType` — so
    /// those are kept under their own name instead of all sharing the empty one.
    pub(crate) fn choice_slot<'a>(name: &'a str, key: &'a str) -> &'a str {
        if key.is_empty() { name } else { key }
    }
}

/// The front end's **multiple-choice** accessors: the name a layout writes, the `fe.txt` key that
/// holds the chosen index, and the `FEStrings` ids of the choices in order.
///
/// All three columns are read from the binary. The registration call is
/// `0x4BB680(name, slot, options, 0, 0, 0)` — for example 0x4E4FC6 pushes `0x5C`,
/// `0x5B6D08` and `"Display.Speedometer"` — where `options` is a zero-terminated list of
/// `FEStrings` ids (0x4BB6B7 counts it) and `slot` indexes the registry table the front end builds
/// at 0x4B1000…0x4B18xx as 12-byte records `{ int slot, void *storage, const char *key }`
/// (0x4B1504 is `{0x5C, 0x657DBC, "SPEEDOMETER"}`). Joining the two on the slot gives this table,
/// and the twelve `Display.*` rows line up with `fe.txt`'s own key names exactly. CONFIRMED.
///
/// The rows whose slot is not a registry slot keep their choice in screen state:
/// `Track<n>.Laps`, `Track<n>.Traffic`, `Track<n>.Direction`, `OwnedCar.PackageNum`,
/// `Car.Transmission`, `Game.Difficutly` (the original's own spelling), `Comms.Positional` and
/// `Comms.Collision`. `CDPlayer.View` is one of them and is listed here with an **empty key**,
/// because a list accessor's index starts at 0 (0x4BC1F0 writes `[this+0x18] = 0`) — so the
/// original opens on `FEStrings` 1851, "track time remaining", and the slider is not blank.
///
/// Read again 2026-09-19 by walking every `0x4BB680` call: the registry record is
/// `{ int slot, void *storage, const char *key }`, so a key's slot is the dword **three** before
/// it, not the one after — and joining the two that way puts every row above on its own `fe.txt`
/// name. The only row that was missing is `Display.RecordTime`, slot 0x68 = `RECORDTIME`.
/// How often a held scrollbar arrow steps — `0x529220(this, 0x96, 1)` at 0x4D0A97, 150 ms.
pub(crate) const SCROLL_REPEAT_MS: u32 = 0x96;

/// The two rows of the front end's boolean accessor, in index order — 0x4B8770 answers `FEStrings`
/// **705** "off" for every index but 1, and **704** "on" for 1.
pub(crate) const BOOL_OPTIONS: [u32; 2] = [705, 704];

/// `Comms.Positional`'s two rows, in index order — the option list 0x5B9674 that 0x50BB03 hands
/// 0x4BB680: `FEStrings` 281 "input exchange" and 282 "positional". 0x50BB1C starts it on **1**.
pub(crate) const POSITIONAL_OPTIONS: [u32; 2] = [281, 282];

/// `Game.Difficutly`'s three rows, in index order — the option list **0x5D9F08** = `0x1C2, 0x1C3,
/// 0x1C4, 0` that 0x4E560A hands 0x4BB680 with the name: `FEStrings` 450 "beginner", 451
/// "advanced", 452 "expert", the same three `Track1.Track(DIFFICULTY)` indexes with `0x1C2 + n`.
/// (0x4E561B then calls the setter, slot 11, with **1** — `Menu::difficulty`.)
pub(crate) const DIFFICULTY_OPTIONS: [u32; 3] = [450, 451, 452];

pub(crate) const CHOICE_ACCESSORS: &[(&str, &str, &[u32])] = &[
    ("Audio.Mode", "audio3d", &[1500, 1501]),
    ("Audio.Quality", "audioquality", &[1504, 1503, 1502, 1500]),
    // the three the settings screens carry that this port had no answer for. Each is built by
    // **0x4BB680**, the choice accessor, with a NUL-terminated list of `FEStrings` ids:
    // `Car.ABS` at 0x4F25F1 and `Car.Traction` at 0x4F262E both take the list at 0x5DAA20 /
    // 0x5DAA0C — **1500, 1501**, "off" and "on" — and `Car.Roof` at 0x4F266B takes 0x5DA9F0,
    // **1509, 1510**, "up" and "down". The `fe.txt` this installation ships carries `car[0].abs`
    // and `car[0].traction`; it has no key for the roof, so that one starts on the slot's own
    // zero, as `CDPlayer.View` does.
    //
    // Each one's sixth argument is a predicate — 0x4F1DA0, 0x4F1DF0 and 0x4F1E40 — that looks the
    // current car up through `CARSETTINGS.CAR` and, when the byte at `car+0x2E2` (ABS), `car+0x2E3`
    // (traction) or `car+0x1BF` (roof) is clear, forces the value to 0 and locks it. The roof's is
    // applied where the race reads the value (`.CABRIODOWN`, the player's car upload); the other
    // two, and the lock, are not.
    // Three more the same walk turned up, each a `0x4BB680` call whose list this port had no row
    // for: `Car.Transmission` at 0x4F22B8 takes 0x5DAB5C, **560, 561** — "manual" and "automatic"
    // — and `fe.txt` names it `car[0].transmission`; `OwnedCar.PackageNum` at 0x4F1FD1 takes
    // 0x5B7644, **620…627**, "none" and "package 1" through "package 7"; `Comms.Collision` at
    // 0x50BB47 takes 0x5B95F4, **1500, 1501**. Neither of the last two has a `fe.txt` key, so both
    // open on index 0, which is where 0x4BC1F0 starts a list accessor. (`Comms.Collision`'s call
    // passes a **2** in the argument the others leave 0; what 0x4BC1F0 does with that slot was not
    // traced, so it is not read as a default here.)
    ("Car.ABS", "car[0].abs", &[1500, 1501]),
    ("Car.Roof", "", &[1509, 1510]),
    ("Car.Traction", "car[0].traction", &[1500, 1501]),
    ("Car.Transmission", "car[0].transmission", &[560, 561]),
    ("Comms.Collision", "", &[1500, 1501]),
    ("OwnedCar.PackageNum", "", &[620, 621, 622, 623, 624, 625, 626, 627]),
    // `Tournament.EventType` is the **two-list** choice accessor 0x4BB6F0: 0x4FD13E builds it
    // through 0x506A70 with `[this+0x24] = 0x5B81A4` — `FEStrings` 470, 471, 472, 473 — and
    // `[this+0x28] = 0x5B81B8` — 470, 476, 477, 478, the short forms. The drop-down's rows and
    // the combo's caption both come from slot 12 (0x4BB750, `[this+0x24]`), reached through
    // 0x4BC350 at 0x4BC38B and through 0x4BC4E0, so the long list is the one listed here; the
    // short one answers `(SHORT)`. It has no `fe.txt` key. §4.105.
    ("Tournament.EventType", "", &[470, 471, 472, 473]),
    ("CDPlayer.Options", "randomplaylist", &[1856, 1855]),
    ("CDPlayer.View", "", &[1851, 1850]),
    ("Display.Damage", "damagedisplay", &[1500, 1805, 1806, 1807]),
    ("Display.DispCols", "displaycolors", &[1821, 1822, 1823]),
    ("Display.Gear", "gear", &[1500, 1501]),
    ("Display.Map", "map", &[1500, 1808, 1809, 1830]),
    ("Display.MapScale", "mapscale", &[1810, 1811, 1812, 1813, 1814]),
    ("Display.OppList", "opponentlist", &[1500, 1832, 1833]),
    ("Display.RearView", "rearviewmirrror", &[1500, 1501]),
    ("Display.RecordTime", "recordtime", &[1500, 1501]),
    ("Display.ScreenLayout", "screenlayout", &[1824, 1825, 1826, 1827]),
    ("Display.SpeedUnits", "speedunits", &[1803, 1804]),
    ("Display.Speedometer", "speedometer", &[1500, 1801, 1802]),
    ("Display.SplitTimes", "splittimes", &[1500, 1816, 1817, 1819]),
    ("Display.Tach", "tachometer", &[1500, 1801, 1802]),
    ("Display.Translucency", "translucency", &[1501, 1500]),
    ("Graphics.CarChrome", "carchrome", &[1502, 1503, 1504]),
    // the RUN-TIME list, not the file's: `nfs5.dll` (the Verok patch's runtime patcher) rewrites the push operand
    // 0x4E5817 from 0x5B6D94 (1502 / 1503 / 1504) to 0x5B6D5C (dll 0x10001320, `RE/notes/nfs5-dll.md` §2.1) — the
    // same five items as `TextureDetail`'s; index 2 is 401 «обычно», the tier the constructor's probe stores (0x4E5B62)
    ("Graphics.CarDetail", "cardetail", &[1506, 1502, 401, 1504, 1507]),
    ("Graphics.DetailedShadows", "detailedshadows", &[1500, 1501]),
    ("Graphics.Headlights", "headlights", &[1508, 1505]),
    ("Graphics.LensFlare", "lensflare", &[1500, 1501]),
    ("Graphics.Reflections", "reflections", &[1502, 1503, 1504]),
    ("Graphics.ShadowMapping", "shadowmapping", &[1500, 1501]),
    ("Graphics.SkyRendering", "skyrendering", &[1500, 1511, 1504]),
    ("Graphics.SpecialFX", "specialfx", &[1502, 1503, 1504, 1507]),
    ("Graphics.TextureDetail", "texturedetail", &[1506, 1502, 401, 1504, 1507]),
    ("Graphics.TexturedDamage", "textureddamage", &[1500, 1501]),
    ("Graphics.TripleBuffer", "triplebuffer", &[1500, 1501]),
    ("Graphics.ViewDistance", "adjustableviewdistance", &[1502, 1503, 1504, 1507]),
    ("Graphics.WideScreen", "widescreen", &[1500, 1501]),
];

/// The front end's **range** accessors: a name, its `fe.txt` key, and the default, minimum and
/// maximum the original registers for it.
///
/// The registration is `0x4BA310(name, slot, default, min, max, step, callback)` — 0x4E4937 pushes
/// `"Audio.MusicLevel"`, slot `0x37` (`MUSICLEVEL`), `0x7F`, `0`, `0x7F` — and the ctor builds the
/// accessor over `max - min + 1` entries (0x4BA329 computes exactly that, then hands it to the
/// same base 0x4BC1F0 the option lists use). The slots are the registry table of
/// [`CHOICE_ACCESSORS`]. CONFIRMED: the five audio defaults are the 127 `fe.txt` ships, and the
/// gamma default is its 100.
///
/// `Chronicle.Year` is registered the same way, with 1948, 1948 and 1996 — which is where the
/// chronicle's range comes from.
pub(crate) const RANGE_ACCESSORS: &[(&str, &str, i32, i32, i32)] = &[
    ("Audio.MusicLevel", "musiclevel", 127, 0, 127),
    ("Audio.SfxLevel", "sfxlevel", 127, 0, 127),
    ("Audio.EngineLevel", "enginelevel", 127, 0, 127),
    ("Audio.SpeechLevel", "speechlevel", 127, 0, 127),
    ("Audio.AmbientLevel", "ambientlevel", 127, 0, 127),
    ("FF.TotalVolume", "fftotalvolume0", 50, 0, 100),
    ("FF.RoadVolume", "ffroadvolume0", 50, 0, 100),
    ("FF.EngineVolume", "ffenginevolume0", 50, 0, 100),
    ("FF.CollisionVolume", "ffcollisionvolume0", 50, 0, 100),
    ("FF.RoadGripVolume", "ffroadgripvolume0", 50, 0, 100),
    ("Graphics.GammaCorrection", "gammacorrection", 100, 0, 400),
];

/// `Track<n>.Traffic`'s option list, from the registration at 0x4BB680: `FEStrings` 403, 402, 401
/// and 400 — "none", "light", "medium", "heavy". CONFIRMED.
pub(crate) const TRAFFIC_OPTIONS: [u32; 4] = [403, 402, 401, 400];

/// `Track<n>.Direction`'s option list: `FEStrings` 410…413 — "forward", "backward",
/// "forward/mirrored", "backward/mirrored". CONFIRMED. The port has no mirrored tracks, so it
/// uses the first two.
pub(crate) const DIRECTION_OPTIONS: [u32; 4] = [410, 411, 412, 413];

/// `Track<n>.Laps` is a list accessor over `FEStrings` 420…427 — the eight rows "1".."8" of the
/// table at **0x5B9650**, registered at 0x4F9A76 — and the launch reads its index and adds one
/// (0x4FC1BC, 0x4FC320). So the lap count a front-end race can ask for is **1…8**, not any larger
/// number; the list accessor's own stepper (0x4BC240) clamps at the ends. CONFIRMED.
pub(crate) const LAPS_MAX: u32 = 8;
