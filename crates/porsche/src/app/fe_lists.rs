//! The front end's car and part lists: garage / dealer / used-car rows, the era-class ordering and the list helpers.

use crate::*;

impl App {
    /// `Showcase.Text(<n>)` — entry **`n / 2 - 1`** of the selected car's own
    /// `FEData/Locale/<key>.loc`, the file its catalogue record names at +0x4D
    /// (`nfs_formats::loc`). The showcase screens (`chassis.lay`, `general.lay`,
    /// `glancestats.lay`) are nothing but these.
    ///
    /// The halving is not a guess. Every car `.loc` holds 45 entries, while `chassis.lay` asks for
    /// `Showcase.Text` up to 90, and every index any screen uses is even. `general.lay` settles it
    /// by pairing each watch with a label: `FEStrings` 12702 "weight" sits beside
    /// `Showcase.Text(24)`, 12700 "length" beside (26), 12708 "width" beside (28), 12707 "height"
    /// beside (30) and 12706 "wheel base" beside (32) — and the '96 993 Carrera's own entries 11 to
    /// 15 are "1390Kg", "4260 mm", "1735 mm", "1315 mm", "2272 mm", in that order. Five rows, five
    /// matches. CONFIRMED.
    ///
    /// **Which car, fixed 2026-09-29 (pass 10 CONFIRMED PORT DEFECT).** `general.lay`'s and
    /// `chassis.lay`'s watches carry a bare `accessor=Showcase.Text(n)` — `FEShowcaseTextAccessor`
    /// (vtable `0x5B8D64`), a class of its own, not `zMergedList.Car`'s. Its string getter
    /// (`0x508E80`, called by `FEAccessorWatch::tick`'s modified-text path, `controls.md` §4.10)
    /// parses the index and hands it to **`0x508CB0`**: `mov eax,[0x699D58]; …; call 0x536DA0` —
    /// the index into whatever `.loc` file object sits at the **global** `[0x699D58]`, not a
    /// live per-frame car lookup. What loads that global is **`0x508B70`**, which every
    /// `SHOWCASE.*`-family click handler calls in the same breath as `0x50E6A0` (the `[0x699FE0]`
    /// setter `fn compare_cars`'s own `CarCompare1` already reads): it frees the previous `.loc`
    /// object, builds `<car+0xE3>.fsh` for the picture pack (falling back to `993tCo96.fsh` at
    /// `0x59DC30` if the car's own pack is missing on disk — the exact numbers this doc comment
    /// used to cite before the bug was found), builds `<car+0x4D>.loc` and loads *that* into
    /// `[0x699D58]`, then sets its language (`0x4B73F0`, `0x508C78`). The two shipped triggers —
    /// `dealership.lay`'s `показ`/`carcompare` buttons (`callback=SHOWCASE.DEALERCAR`) and
    /// `QuickRace.lay`/`knockout.lay`/`quickknockout.lay`'s (`callback=CARCOMPARE.MERGEDCAR`) —
    /// are the *same two* the port already tracks as `self.fe.compare_dealer` (`:17585`); the
    /// registration table at `0x50C257…` (walked with `RE/tools/fn.py`/a raw capstone scan, since
    /// `SHOWCASE.DEALERCAR` turned out to be a per-screen vtable-slot-54 thunk at **0x50E5C0**, not
    /// the flat `0x50DB20` this doc comment used to cite — that address is really registered under
    /// the name `SHOWCASE.GENERICCAR`, and `CARCOMPARE.MERGEDCAR` is **0x50DC40**, not `0x50DCC0`
    /// — `0x50DCC0` is `CHEATALLCARS`) confirms these are the only two names any shipped `.lay`
    /// invokes. So `Showcase.Text`'s own car is the same one `self.fe.compare_dealer` already
    /// picks, and the fix is to read it through the same routing `compare_cars`/`CarCompare1`
    /// already implements, not `zMergedList.Car` unconditionally.
    pub(crate) fn showcase_text(&self, index: usize) -> Option<String> {
        let index = (index / 2).checked_sub(1)?;
        let key = self.logo_car(Some("CarCompare1"), None)?.locale_key.clone();
        if key.is_empty() {
            return None;
        }
        let dir = self.paths.fe_data().join("Locale");
        let path = GamePaths::find_file(&dir, &format!("{key}.loc"))?;
        let d = std::fs::read(path).ok()?;
        // the language is 0x4B73F0's, set on the file at 0x508C78 (`App::language`); block 0 of
        // the car files is English in this install
        let lang = usize::from(self.language);
        nfs_formats::loc::LocFile::parse(&d, lang).ok()?.get(index).map(str::to_string)
    }

    /// A range accessor's value: what the player has changed this session, else `fe.txt`, else the
    /// default the original registers. Returns the value with its minimum and maximum.
    pub(crate) fn range_value(&self, name: &str) -> Option<(i32, i32, i32)> {
        let (_, key, default, lo, hi) = RANGE_ACCESSORS.iter().find(|(n, ..)| *n == name)?;
        // the five audio levels are live state the mixers read every tick, so they come from there
        let a = &self.snd.audio_settings;
        let v = match name {
            "Audio.MusicLevel" => a.music_level as i32,
            "Audio.SfxLevel" => a.sfx_level as i32,
            "Audio.EngineLevel" => a.engine_level as i32,
            "Audio.SpeechLevel" => a.speech_level as i32,
            "Audio.AmbientLevel" => a.ambient_level as i32,
            _ => match self.choices.get(*key) {
                Some(v) => *v as i32,
                None => self.registry.u32(key, *default as u32) as i32,
            },
        };
        Some((v.clamp(*lo, *hi), *lo, *hi))
    }

    /// The index a multiple-choice accessor is on: what the player has changed this session, else
    /// the `fe.txt` value, else the slot's own static initialiser — zero in the shipped image for
    /// every one of these (their storage in `.bss` at 0x657D5C…0x657DC4 is all zero) **except**
    /// four `Graphics.*` accessors `choice_index` gives their own non-zero fallback: the original
    /// never leaves `LensFlare`/`SkyRendering`/`CarChrome`/`CarDetail` at their list's own index 0
    /// on a real, freshly-created profile (`REFERENCE-2026-09-28.md` "Pass 13" §3, settled against
    /// a genuine original save, `choice_index`'s own doc comment has the addresses).
    /// The rows a combo slider's drop-down would list: every option of the choice accessor it
    /// edits, and the one it is on. An accessor that is not one of [`CHOICE_ACCESSORS`] — a
    /// numeric range, say — has no enumerable options here, and that combo simply does not open.
    /// The rows a combo drops down when its accessor is **not** one of the multiple-choice ones.
    ///
    /// `FeComboBox` does not keep a list of its own: 0x4C66E8's loop walks the accessor, asking it
    /// for each entry in turn, so the drop-down is exactly the accessor's own enumeration with the
    /// current index marked. For an **int range** that enumeration is `max - min + 1` entries
    /// whose index is the value less the minimum — 0x4BA329 builds precisely that many and
    /// 0x4BA34A / 0x4BA362 index them that way (§4.81) — and for a list accessor it is the list.
    ///
    /// So each family here is enumerated from the very source this port already answers the
    /// combo's **caption** from, which is what makes it a reading rather than a guess: the car
    /// combos from the car table the caption's `(*m.car()?)` indexes, the track combos from
    /// `m.tracks`, the paint combos from the selected car's paint sets, and the four counted
    /// settings from the clamps their own steppers carry. §4.91.
    /// Every car on the used-car lot — `UsedCar.Car`'s list. The vector at `[0x691B88]` holds both
    /// the cars the game generates (0x4F80C0) and the player's own listings (`0x4F91C0` appends
    /// them with `car+0x6A0 = 1`), but the accessor does not read it: its list thunk, registered
    /// at 0x4F221D, is **0x4F9010**, which rebuilds `[0x691B8C]` from the cars of `[0x691B88]`
    /// whose `+0x6A0` is **0** (0x4F905E…0x4F9068) — the generated cars alone, in list order. So
    /// the lot, its drop-down and `BUYCAR` (0x501F40 buys `UsedCar.Car`'s current car, 0x501F6B)
    /// never see a listing. That is [`Career::market`], whose order is the list's.
    /// *(Corrected 2026-09-26: the port chained the player's listings after the market.)*
    pub(crate) fn used_cars(&self) -> Vec<&nfs_game::career::OwnedCar> {
        let Some(c) = self.career.as_ref() else { return Vec::new() };
        c.career.market.iter().collect()
    }

    /// Race slot `slot`'s car-info `.LICENSE` (`+0x1C0`) and `.RACENUMBER` (`+0x510`), which the
    /// page builder writes into the plate and the race-number patch (0x444880, `car-model.md`
    /// §5.3). The front end's per-car race set-up 0x4EEDA0 streams its fourth argument — the
    /// driver's name — as both `.DRIVERNAME` 0x13B (0x4EEDEE) and `.LICENSE` 0x13C (0x4EEE7B, the
    /// same `ebp`), so a race car's plate reads its driver: the player's is the profile name
    /// (0x501270 `[0x691DC8]`, 0x4EBA2E; the driver name 0x5012A0 in a network session), an
    /// opponent's its own. `fe.txt`'s `car[n].license` (I8A4RE) is only what the record holds
    /// before that.
    ///
    /// **`.RACENUMBER` 0x24D, read 2026-09-27 (correcting the previous note, which missed it):
    /// it *is* streamed** — 0x4EF15D…0x4EF165 pushes `[ebx+0x688]` under key `0x24D`. `ebx` is
    /// the `FECar` 0x4EEDA0 was called on, and `+0x688` is `FECar::init` 0x4EEBA0's own **7th**
    /// stack argument (`[ebp+0x674…0x688]` ← its 2nd…7th stack arguments in that field order,
    /// 0x4EEBBF…0x4EEBE8). At its one traced construction site for a race opponent — 0x4F51B6,
    /// inside the `FEOpponentCarAccessor` refill 0x4F4FA0 the draw-order table above already
    /// names (`quick_opponents` calls it before the opponents' own 0x4EEDA0, "the package / decal
    /// pair") — that 7th argument is `esi`, the same **decal** draw as the table's `0x4F517B
    /// rand()%100 > 80 → 0x4F518D rand()%100 − 1`: **−1** four times in five, else `0…98`. So a
    /// procedurally-built opponent's plate is almost always blank (`0-1`) and occasionally shows a
    /// random one- or two-digit number, independent of `fe.txt`. The player's own `FECar` is never
    /// rebuilt through this constructor (it is the persistent garage car), so nothing here touches
    /// it; `fe.txt`'s `car[n].racenumber` — 0 in a slot without the key — is what reaches it,
    /// unchanged, matching this port.
    ///
    /// *(Corrected 2026-09-27: this said the decal draw itself was unported, guarding against
    /// disturbing `quick_opponents`' shared C-`rand` stream. Read again at the instruction
    /// (0x4F5153…0x4F519C): `nfs_game::opponents::fill` has drawn it since 1149de15, on the same
    /// `CRand` as the package roll and in the same order — package test/draw first (0x4F5153,
    /// 0x4F516B), decal test/draw second (0x4F517B, 0x4F518D) — so the stream, and the race
    /// goldens that pin it, were never missing this draw. What was actually missing was wiring
    /// the already-drawn `OpponentPick`/`CareerOpponent::decal` to this function for opponent
    /// slots; `decal` below is that wiring — `None` keeps the player's `fe.txt` path unchanged.)*
    /// *(Until 2026-09-25 the plate was `car[n].license` for every car.)*
    pub(crate) fn page_text(&self, slot: usize, driver: &str, decal: Option<i32>) -> nfs_assets::car::PageText {
        nfs_assets::car::PageText {
            license: driver.to_string(),
            race_number: decal.unwrap_or_else(|| self.registry.get(&format!("car[{slot}].racenumber")).and_then(|v| v.parse().ok()).unwrap_or(0)),
        }
    }

    /// The garage's selected car and its catalogue record — what every `OwnedCar.*` accessor reads.
    pub(crate) fn garage_car(&self) -> Option<(&nfs_formats::fedata::CarCatalogEntry, &nfs_game::career::OwnedCar)> {
        let c = self.career.as_ref()?;
        let o = c.career.garage.get(c.car_sel.min(c.career.garage.len().saturating_sub(1)))?;
        Some((c.tables.car(o.car_id)?, o))
    }

    /// `DealerCar.Car`'s list `[0x691A00]`, filled by 0x4F4000…0x4F4294: every catalogue record the
    /// dealer offers this year (`Career::dealer_lists`), then 0x4F0F50's exchange sort — for each
    /// `i`, every later `j` that 0x4F1D40 puts first swaps with it — on era `+0x1B0`, then class
    /// `+0x1AC`, then first year `+0x1F8`, each ascending. Outside a career, `None`.
    pub(crate) fn dealer_cars(&self) -> Option<Vec<&nfs_formats::fedata::CarCatalogEntry>> {
        let c = self.career.as_ref()?;
        let mut v: Vec<&nfs_formats::fedata::CarCatalogEntry> = c.tables.catalog.iter().filter(|e| c.career.dealer_lists(e)).collect();
        era_class_year_sort(&mut v, |e| era_class_year_key(e));
        Some(v)
    }

    /// The dealership's selected car, when a career has a dealer list.
    pub(crate) fn dealer_car(&self) -> Option<&nfs_formats::fedata::CarCatalogEntry> {
        let v = self.dealer_cars()?;
        v.get(self.fe.dealer.car.min(v.len().saturating_sub(1))).copied()
    }

    /// The car the used-car screens are on — `UsedCar.Car`'s current row.
    pub(crate) fn used_car(&self) -> Option<&nfs_game::career::OwnedCar> {
        let all = self.used_cars();
        all.get(self.fe.usedcar_sel.min(all.len().saturating_sub(1))).copied()
    }

    /// The catalogue records behind the player's garage, in save order — `OwnedCars.Car`'s own
    /// list. Its accessor (`FEOwnedCarAccessor`, vtable 0x5B7868) counts
    /// `(end − begin) / 4` of the vector its `[this+0x2C]` thunk hands back (0x4F5CBD) and reads
    /// row `i` as `begin[i] + 6` (0x4F5C5F) — the FE car object's `+4` record plus its `+2` name.
    fn garage_cars(&self) -> Vec<(usize, &nfs_formats::fedata::CarCatalogEntry)> {
        let Some(c) = self.career.as_ref() else { return Vec::new() };
        self.owned_list().into_iter().filter_map(|i| Some((i, c.tables.car(c.career.garage.get(i)?.car_id)?))).collect()
    }

    /// `OwnedCars.Car`'s list — **0x501AD0**'s `[0x691DFC]` (`Career::owned_list`): the garage,
    /// less the cars from a later model year than the chronicle's when `Game.GameType` is 4.
    pub(crate) fn owned_list(&self) -> Vec<usize> {
        self.career.as_ref().map(|c| c.career.owned_list(&c.tables, self.fe.game_type == 4)).unwrap_or_default()
    }

    /// Keep the garage selection on a car `OwnedCars.Car` lists. 0x4F5980 (slot 0x90) reads
    /// `list[index]`, `list[0]` when the index is at or past the count (0x4F59B1 `jb`), null on
    /// an empty list — the accessor's **own** index into the list (slot 0x28) is the selection
    /// every owned-car reader shares, so when 0x501AD0 rebuilds the list (the chronicle year or the
    /// game type changed) the index stays and names whatever now sits there. The port keeps a
    /// garage index (`car_sel`) and the list it was last read against (`fe.owned_seen`): on a
    /// changed list the old position of `car_sel` is looked up there and read from the new list
    /// (a position past its end → the first car, as 0x4F59B1). A selection the list does not hold
    /// and no earlier list placed goes to the first car.
    pub(crate) fn fe_owned_select(&mut self) {
        let list = self.owned_list();
        let seen = self.fe.owned_seen.replace(list.clone());
        let Some(c) = self.career.as_mut() else { return };
        let Some(first) = list.first().copied() else { return };
        if let Some(prev) = seen.filter(|p| *p != list)
            && let Some(at) = prev.iter().position(|i| *i == c.car_sel)
        {
            c.car_sel = list.get(at).copied().unwrap_or(first);
            return;
        }
        if !list.contains(&c.car_sel) {
            c.car_sel = first;
        }
    }

    /// The garage half of a car list, with `FEOwnedCarAccessor`'s headings — §4.144. Pushes a
    /// `GroupHeading` for the era **and** the class when the era changes, one for the class when
    /// only the class does, then the car's own row; returns the row index of `select`.
    pub(crate) fn garage_rows(&self, rows: &mut Vec<ListRow>, short: bool, select: usize) -> Option<usize> {
        let mut at = None;
        let mut prev: Option<(u32, u32)> = None;
        for (i, e) in self.garage_cars() {
            let key = (e.era_string_id, e.class_string_id);
            let heading = |id: u32| ListRow::GroupHeading(self.fe.strings.get(id).unwrap_or_default().to_string());
            match prev {
                None => {
                    rows.push(heading(key.0));
                    rows.push(heading(key.1));
                }
                Some(p) if p.0 != key.0 => {
                    rows.push(heading(key.0));
                    rows.push(heading(key.1));
                }
                Some(p) if p.1 != key.1 => rows.push(heading(key.1)),
                _ => {}
            }
            prev = Some(key);
            if i == select {
                at = Some(rows.len());
            }
            rows.push(ListRow::Text(if short { e.family.to_string() } else { e.display_name.to_string() }));
        }
        at
    }

    /// A catalogue car list's own headings — `FEGenericCarAccessor`'s **own** slot 26, **0x4F39E0**
    /// (the base class §4.137 calls "the full implementation"; `FEOwnedCarAccessor` above
    /// *overrides* it for the garage). Not read until this pass: the same era-then-class group
    /// heading as `garage_rows`, drawn inline by the row painter **0x4F3510** (0x4F3548's era
    /// compare, 0x4F3639's class compare, both against `record+0x1B0`/`+0x1AC` — the identical
    /// fields `era_class_year_key` already sorts these lists by, so the grouping the height
    /// function implies always lines up with a real boundary) — but the row height it returns is
    /// **40 / 27 / 14** (0x4F39F1/0x4F3A17/0x4F3A35: `0x28`/`0x1B`/`0xE`), not the garage's
    /// 42/28/14, because its heading is **13 px** (`0x5B7734` is the same 13 both classes use for
    /// the rule spacing, but this one costs one row of `PART_HEADER_H`, not `LIST_ROW_H`, per
    /// heading — 14+13+13=40, 14+13=27) — the same two-black-rule-plus-bevel shape `ListRow::Header`
    /// already draws (`layout/widgets.rs`'s plain, non-`parts_glass` arm), reused here rather than
    /// a second copy of it under `GroupHeading`'s 14 px shape. `GenericCar.Car`, `GenericCar2.Car`
    /// and `DealerCar.Car` (`career.md` §10.9, `combo_rows` below) are the three shipped lists this
    /// class serves unmodified, all three already `era_class_year_sort`-ordered so a boundary here
    /// is always genuine, never a false split from an unsorted run.
    pub(crate) fn catalog_rows<'a>(&self, cars: impl Iterator<Item = &'a nfs_formats::fedata::CarCatalogEntry>, short: bool, select: Option<usize>) -> (Vec<ListRow>, Option<usize>) {
        let mut rows = Vec::new();
        let mut at = None;
        let mut prev: Option<(u32, u32)> = None;
        for (i, e) in cars.enumerate() {
            let key = (e.era_string_id, e.class_string_id);
            let heading = |id: u32| ListRow::Header(self.fe.strings.get(id).unwrap_or_default().to_string(), false);
            match prev {
                None => {
                    rows.push(heading(key.0));
                    rows.push(heading(key.1));
                }
                Some(p) if p.0 != key.0 => {
                    rows.push(heading(key.0));
                    rows.push(heading(key.1));
                }
                Some(p) if p.1 != key.1 => rows.push(heading(key.1)),
                _ => {}
            }
            prev = Some(key);
            if Some(i) == select {
                at = Some(rows.len());
            }
            rows.push(ListRow::Text(if short { e.family.to_string() } else { e.display_name.to_string() }));
        }
        (rows, at)
    }
}

/// The dealership's current car, paint set and interior rows (`DealerCar.Car`, `.Colour`,
/// `.IntColour`). A change of car puts both colour rows back to 0 (0x4F30A4…0x4F3110).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DealerSel {
    pub(crate) car: usize,
    pub(crate) paint: usize,
    pub(crate) interior: usize,
}

impl DealerSel {
    /// `DealerCar.Car`'s slot 11, **0x4F3060**(`row`), on this state (`count` the list's size): a row
    /// past `count − 1` or below 0 becomes 0 (0x4F306D…0x4F3074), the car is set (0x4F0A30 → 0x4BC240),
    /// and when the row is **not** the current one and the accessor has been set before (`[+0x38]`,
    /// written 1 at 0x4F3110 by every earlier call, so in the port's flows always) both colour
    /// accessors take slot 11(0) (0x4F30B2…0x4F3108): paint and interior row 0. The same row again
    /// touches nothing (0x4F3094 clears `[+0x38]` first, so 0x4F30A9 skips the colours).
    pub(crate) fn slot11(&mut self, row: i64, count: usize) {
        let row = if row < 0 || row > count as i64 - 1 { 0 } else { row as usize };
        if row != self.car {
            *self = DealerSel { car: row, paint: 0, interior: 0 };
        }
    }
}

/// **0x4F0F50**'s comparator key: `(era_string_id, class_string_id, year)` at `+0x1B0`/`+0x1AC`/
/// `+0x1F8`, `year` the signed 16-bit word the comparator's own `cmp ax, word ptr […]` /
/// `setl` reads (0x4F1D75…0x4F1D86) — not the full record, so a tie on all three fields never
/// compares (0x4F1D8E `xor al, al`, matching [`era_class_year_sort`]'s `!(a<b) && !(b<a)` no-swap).
pub(crate) fn era_class_year_key(c: &nfs_formats::fedata::CarCatalogEntry) -> (u32, u32, i16) {
    (c.era_string_id, c.class_string_id, c.year as i16)
}

/// **The original's own exchange sort, byte for byte.** `0x4F0F50` (outer index `i` 0..n, inner
/// index `j` i+1..n) calls comparator **0x4F1D40** as `cmp(v[j], v[i])` and swaps `v[i]`/`v[j]`
/// the instant it answers true — not a min-tracking selection sort, and not stable: several `j`s
/// can each overwrite `v[i]` in turn, so a run of equal-keyed records lands in whatever order this
/// exact instruction-for-instruction walk produces, not their original file order (`Vec::sort*`,
/// stable or not, is not proven to reproduce it and is not used here). `dealer_cars` (`career.md`
/// §10.9 "Dealer / garage order", matched) and the `GenericCar.Car`/`GenericCar2.Car` catalogue
/// lists (`0x4F3E40`'s six lists, all six sorted with this same call at 0x4F42A1…0x4F42DC) all go
/// through this one routine. Generic over the element so the same walk serves both an owned
/// `Vec<CarCatalogEntry>` (the catalogue lists) and a `Vec<&CarCatalogEntry>` (`dealer_cars`).
pub(crate) fn era_class_year_sort<T>(v: &mut [T], key: impl Fn(&T) -> (u32, u32, i16)) {
    for i in 0..v.len() {
        for j in i + 1..v.len() {
            if key(&v[j]) < key(&v[i]) {
                v.swap(i, j);
            }
        }
    }
}

/// The accessor index `row` visually belongs to, in a `combo_rows` list that may carry
/// `Header`/`GroupHeading` rows (`garage_rows`, `catalog_rows`, §4.144) ahead of the row they
/// head — the original's own accessor never indexes those separately: one heading is *extra
/// height on the same car's row* (its row painter draws the heading, then the row, in the one
/// call `slot26`'s taller answer made room for — 0x4F5D48…0x4F5DB3, 0x4F3548…0x4F3655), not a
/// second selectable entry. So `combo_rows`'s own row numbering (needed for the on-screen hit
/// test and scroll math, which does treat every pushed `ListRow` as its own band) is not the
/// accessor's index, and `combo_select`'s `set_accessor(name, delta)` needs the *car* delta, not
/// the *row* delta, whenever headings are present — a click landing inside a heading band belongs
/// to the row right after it (the same visual block), so `row` is snapped forward to the next
/// non-heading row before it is counted. A no-op (returns `row` unchanged) on any list with no
/// headings, which is every list this project had before `catalog_rows`.
pub(crate) fn selectable_index(rows: &[ListRow], row: usize) -> i32 {
    if rows.is_empty() {
        return 0;
    }
    let is_heading = |r: &ListRow| matches!(r, ListRow::Header(..) | ListRow::GroupHeading(_));
    let snapped = (row..rows.len()).find(|&i| !is_heading(&rows[i])).unwrap_or(rows.len() - 1);
    rows[..=snapped].iter().filter(|r| !is_heading(r)).count() as i32 - 1
}

/// **`GenericCar.Car`'s own dealer-availability admit rule — the `[0x6919EC]` `+0x204` question
/// left open, resolved by a fresh disassembly of 0x4F4087…0x4F41B7 (`0x4F3E40`'s per-record kind-0
/// arm) and cross-checked against the shipped `nfs5.car` data and a live original capture.**
///
/// The arm taken is `Game.GameType == 4` (career/Evolution, `edi`/`[esp+0x18]` compared against the
/// same `4` the append step reuses as its pointer stride) vs. every other value:
///
/// * `GameType == 4` (0x4F4093…0x4F40A5): only `flag == 2` is excluded — `flag ∈ {0, 1, 3}` all
///   reach the append at 0x4F40A7. Matches `career.md` §10.5's dealer list, which is the *other*
///   `+0x204` reader and excludes `{2, 3}` — the two are deliberately different: the dealer also
///   drops the three "Factory Driver" duplicates (`flag == 3`: `FBoxster`/`F901`/`F996`, the shipped
///   file's own literal duplicate `display_name` of an existing racing car), but Evolution's own
///   catalogue keeps them (they *are* what a Factory Driver mission is set up to race).
/// * any other `GameType` (0x4F4166…0x4F41B7, QuickRace/Knockout/QuickKnockout and — unreached by any
///   shipped `.lay` in this port — Multiplayer): `flag == 0` or `flag == 2` admit unconditionally
///   (0x4F4170/0x4F4176); `flag == 1` — the *majority*, 69 of 87 kind-0 records — only when
///   `GameType == 3` (Multiplayer) or the `[0x691A04]` cheat byte is set (0x4F4182/0x4F4178); `flag
///   == 3` never (0x4F4187…0x4F418B, checked even after the cheat bypass). `[0x691A04]`'s only
///   *setter* in the whole image is a single `mov byte,1; ret` micro-function at **0x4F4440**, whose
///   one caller is **0x50DD70**, `modes.md`'s own `CHEATALLCARS` — "the three callbacks no shipped
///   `.lay` names" — so on any reachable QuickRace/Knockout/QuickKnockout screen the byte is always
///   0 and `flag == 1` records are always excluded. Not ported: no cheat-code entry point exists in
///   this port for `CHEATALLCARS` to hang off.
///
/// **Confirmed against the live original, not just the disassembly**: era/class/year-sorting the
/// `GameType == 0` (QuickRace) survivors of `nfs5.car` (flag ∈ {0, 2} only, 11 + 4 of 87 records)
/// lands row 0 on **`'59 356 A 1600 Super Convertible D`** — matching
/// `REFERENCE-2026-09-28.md`'s "Pass 10" `QuickRace(a)` capture of the *original*'s default car,
/// read there as `356 A Convertible`. `b5f1896e`'s earlier fix instead matched the *unfiltered*
/// kind-0 sort's row 0, `'50 356 1100 Cabriolet` (`flag == 1`, year 1950) — a different, wrong car
/// that superficially reads the same at a glance ("356", the earliest era) but is not what the
/// original's own screenshot names. This supersedes that reading; `b5f1896e`'s fix to the *sort
/// itself* (era/class/year, exchange, not `Vec::sort_by_key(year)`) and to `reset_track_rows`
/// resetting `car_idx` on every `SETTYPE*` both still stand, unchanged.
fn dealer_flag_admitted(flag: u16, game_type: u8) -> bool {
    if game_type == 4 {
        flag != 2
    } else {
        match flag {
            0 | 2 => true,
            1 => game_type == 3, // Multiplayer, or the un-ported CHEATALLCARS cheat — see above
            _ => false,
        }
    }
}

/// `GenericCar.Car`'s own list, `[0x6919EC]` — kind-0 records the current `Game.GameType` admits
/// ([`dealer_flag_admitted`]), 0x4F0F50-sorted. Shared by [`Menu::set_cars`] and the initial
/// `Menu` construction, which needs the filtered/sorted vector before a `Menu` exists yet (to
/// resolve `NFS_FE_CAR`'s starting index).
pub(crate) fn generic_car_list(catalog: &[nfs_formats::fedata::CarCatalogEntry], game_type: u8) -> Vec<nfs_formats::fedata::CarCatalogEntry> {
    let mut v: Vec<_> = catalog.iter().filter(|c| c.kind == 0 && dealer_flag_admitted(c.dealer_flag_204, game_type)).cloned().collect();
    era_class_year_sort(&mut v, era_class_year_key);
    v
}

/// **0x4F0F50/0x4F1D40 against the shipped `nfs5.car`**, pinning the exchange sort's exact,
/// non-stable output — not merely "row 0 matches" (`b5f1896e`'s own weaker check). Computed
/// independently in Python from the same file (era/class/year at `+0x1B0`/`+0x1AC`/`+0x1F8`, the
/// same instruction-for-instruction exchange walk) and cross-checked against `career.md` §10.9's
/// already-`matched` dealer-list comparator citation. Rows 10…14 (the big 1956 356 A cluster) are
/// the load-bearing ones: a plain `Vec::sort_by_key` on the same three-tuple key would leave that
/// cluster in file order, which is not what this walk produces.
#[cfg(test)]
mod catalogue_order_tests {
    use super::{era_class_year_key, era_class_year_sort};
    use nfs_assets::GamePaths;

    #[test]
    fn the_exchange_sort_matches_the_original_instruction_for_instruction() {
        let Ok(paths) = GamePaths::locate() else { return };
        let Ok(bytes) = std::fs::read(paths.fe_data().join("Data").join("nfs5.car")) else { return };
        let Ok(catalog) = nfs_formats::fedata::parse_car_catalog(&bytes) else { return };
        let mut cars: Vec<_> = catalog.into_iter().filter(|c| c.kind == 0).collect();
        era_class_year_sort(&mut cars, era_class_year_key);
        assert_eq!(cars.len(), 87, "the shipped file has 87 kind-0 racing records");
        let want = [
            "'50 356 1100 Cabriolet",
            "'50 356 1100 Coupé Ferdinand",
            "'51 356 1300 Cabriolet",
            "'51 356 1300 Coupé",
            "'52 356 1500 Cabriolet",
            "'52 356 1500 Coupé",
            "'53 356 1500 Super Cabriolet",
            "'53 356 1500 Super Coupé",
            "'54 356 1300 Super Cabriolet",
            "'54 356 1300 Super Coupé",
            "'56 356 A 1300 Super Cabriolet",
            "'56 356 A 1300 Super Coupé",
            "'56 356 A 1600 Cabriolet",
            "'56 356 A 1600 Coupé",
            "'56 356 A 1600 Super Cabriolet",
        ];
        let got: Vec<&str> = cars.iter().take(want.len()).map(|c| c.display_name.as_str()).collect();
        assert_eq!(got, want);
    }
}

/// `FEGenericCarAccessor`'s own slot 26 (0x4F39E0, `catalog_rows`'s doc) and the row-space vs.
/// car-index bug it would otherwise carry into `combo_select` (`selectable_index`'s doc).
#[cfg(test)]
mod catalog_heading_tests {
    use super::{selectable_index, ListRow};

    fn heading(s: &str) -> ListRow {
        ListRow::Header(s.to_string(), false)
    }
    fn text(s: &str) -> ListRow {
        ListRow::Text(s.to_string())
    }

    /// Two era/class groups: `[Head(A), Head(A), car0, car1, Head(B), Head(B), car2, car3, car4]`
    /// — the exact shape `catalog_rows` builds for two 40 px group starts.
    fn two_groups() -> Vec<ListRow> {
        vec![
            heading("era A"), heading("class A"), text("car0"), text("car1"),
            heading("era B"), heading("class B"), text("car2"), text("car3"), text("car4"),
        ]
    }

    #[test]
    fn a_flat_list_has_no_headings_to_skip() {
        let rows = vec![text("a"), text("b"), text("c")];
        for i in 0..rows.len() {
            assert_eq!(selectable_index(&rows, i), i as i32, "row {i} is its own car index with no headings");
        }
    }

    #[test]
    fn every_row_in_a_group_resolves_to_its_own_car() {
        let rows = two_groups();
        // car0's own row (index 2) and both headings ahead of it (0, 1) all belong to car 0 —
        // a click on the heading band is a click on the row right after it (§4.144's one visual
        // block, one accessor index).
        assert_eq!(selectable_index(&rows, 0), 0, "era A heading -> car0");
        assert_eq!(selectable_index(&rows, 1), 0, "class A heading -> car0");
        assert_eq!(selectable_index(&rows, 2), 0, "car0's own row");
        assert_eq!(selectable_index(&rows, 3), 1, "car1, no heading of its own");
        assert_eq!(selectable_index(&rows, 4), 2, "era B heading -> car2");
        assert_eq!(selectable_index(&rows, 5), 2, "class B heading -> car2");
        assert_eq!(selectable_index(&rows, 6), 2, "car2's own row");
        assert_eq!(selectable_index(&rows, 7), 3, "car3");
        assert_eq!(selectable_index(&rows, 8), 4, "car4");
    }

    /// The bug `selectable_index` exists to fix: without it, `combo_select`'s plain `row − cur`
    /// would feed `set_accessor` a delta of 5 (rows 7 − 2) for a car0 -> car3 click, three more
    /// than the real car-index delta of 3 (3 − 0) — wrapping a 5-car garage back onto car0 instead
    /// of landing on car3.
    #[test]
    fn the_row_delta_would_have_overshot_the_car_delta() {
        let rows = two_groups();
        let (from_row, to_row) = (2, 7); // car0's row -> car3's row
        let row_delta = to_row as i32 - from_row as i32;
        let car_delta = selectable_index(&rows, to_row) - selectable_index(&rows, from_row);
        assert_eq!(row_delta, 5);
        assert_eq!(car_delta, 3, "the real step is three cars, not five rows");
        assert_ne!(row_delta, car_delta);
    }
}

/// The `[0x6919EC]` `+0x204` dealer-availability admit rule ([`dealer_flag_admitted`]/
/// [`generic_car_list`]).
#[cfg(test)]
mod dealer_flag_tests {
    use super::{dealer_flag_admitted, generic_car_list};
    use nfs_assets::GamePaths;

    /// The two arms read off 0x4F4087…0x4F41B7, flag by flag: career (`GameType == 4`) admits
    /// everything but the bonus cars (flag 2); QuickRace/Knockout/QuickKnockout (any other
    /// `GameType` the port reaches) admit only flag 0/2 — flag 1 (the majority) needs Multiplayer
    /// (`GameType == 3`, unreached by any shipped `.lay` here) and flag 3 (the Factory Driver
    /// duplicates) never passes this arm at all.
    #[test]
    fn the_two_arms_by_flag() {
        assert_eq!([0, 1, 2, 3].map(|f| dealer_flag_admitted(f, 4)), [true, true, false, true], "career");
        assert_eq!([0, 1, 2, 3].map(|f| dealer_flag_admitted(f, 0)), [true, false, true, false], "quick race");
        assert_eq!([0, 1, 2, 3].map(|f| dealer_flag_admitted(f, 1)), [true, false, true, false], "quick knockout");
        assert_eq!([0, 1, 2, 3].map(|f| dealer_flag_admitted(f, 2)), [true, false, true, false], "knockout");
        assert_eq!([0, 1, 2, 3].map(|f| dealer_flag_admitted(f, 3)), [true, true, true, false], "multiplayer");
    }

    /// Against the shipped `nfs5.car`: QuickRace's filtered, sorted catalogue's row 0 is `'59 356 A
    /// 1600 Super Convertible D` — matching `REFERENCE-2026-09-28.md`'s "Pass 10" `QuickRace(a)`
    /// capture of the *original*'s own default car (read there as `356 A Convertible`), not
    /// `b5f1896e`'s unfiltered `'50 356 1100 Cabriolet`. Also pins the much smaller row count (15
    /// of 87 kind-0 records: 11 flag-0 + 4 flag-2) — the over-inclusion the "many duplicates" user
    /// report was seeing in the drop-down.
    #[test]
    fn quickrace_catalogue_against_the_shipped_file() {
        let Ok(paths) = GamePaths::locate() else { return };
        let Ok(bytes) = std::fs::read(paths.fe_data().join("Data").join("nfs5.car")) else { return };
        let Ok(catalog) = nfs_formats::fedata::parse_car_catalog(&bytes) else { return };
        let cars = generic_car_list(&catalog, 0);
        assert_eq!(cars.len(), 15);
        assert_eq!(cars[0].display_name, "'59 356 A 1600 Super Convertible D");
        // the three shipped display-name duplicates (a flag-0/1 racing car and its flag-3 Factory
        // Driver twin) never both show through this arm — at most one of each pair survives
        for name in ["'73 911 Carrera RS 2.7 Coupe", "'97 Boxster", "'00 911 Turbo (996)"] {
            assert!(cars.iter().filter(|c| c.display_name == name).count() <= 1, "{name}");
        }
    }
}

#[cfg(test)]
mod dealer_sel_tests {
    use super::DealerSel;

    /// 0x4F3060: a new row puts both colour rows back to 0; the same row keeps them; a row out of
    /// range is 0; the list's head (0x4F3E40) is slot 11(0).
    #[test]
    fn slot_11_resets_the_colours_only_on_a_new_car() {
        let mut d = DealerSel { car: 3, paint: 2, interior: 1 };
        d.slot11(3, 10);
        assert_eq!((d.car, d.paint, d.interior), (3, 2, 1));
        d.slot11(4, 10);
        assert_eq!((d.car, d.paint, d.interior), (4, 0, 0));
        d.paint = 5;
        d.slot11(10, 10);
        assert_eq!((d.car, d.paint), (0, 0));
        d.car = 2;
        d.paint = 1;
        d.slot11(-1, 10);
        assert_eq!((d.car, d.paint), (0, 0));
        d.paint = 1;
        d.slot11(0, 0);
        assert_eq!((d.car, d.paint), (0, 1), "already on row 0: nothing moves");
    }
}
