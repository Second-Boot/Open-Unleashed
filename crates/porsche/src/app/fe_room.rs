//! The front end's 3D rooms (garage, dealer, track rooms): loading, cars, paints and their draw items.

use crate::*;

impl App {
    /// The room on show — the room object `[fe+0xE8]`, which is the one the screen names in its
    /// `room=` (`RE/notes/frontend-3d.md` §1) except while the last screen's room is still going
    /// out.
    pub(crate) fn fe_room_of(&self) -> Option<&'static nfs_game::room::Room> {
        self.fe.room_obj.map(|(r, _)| r)
    }

    /// The room the **bottom layer's** screen names in its `room=` (its slot 0x8C).
    ///
    /// **Not the top screen's (read 2026-10-02).** Both places that compare or make the
    /// room take the screen from the **first** element of the layer vector `[fe+0xD8]`, the
    /// bottom layer — the tick 0x4D2570 at 0x4D26E7…0x4D26F8 (`[[[fe+0xD8]]]`: the layer record's
    /// pending screen `[+4]`, else its screen `[+0]`) and 0x4D1BD0 (run on every open, 0x4D23DA) at
    /// 0x4D1BDA…0x4D1BEB, which sends the room / background / callback list out (slot 0x7C) only
    /// when *that* screen names another one — while the input dispatch 0x4D2850 and the
    /// pop at 0x4D26A3 take the **last** element as the top. So a layer pushed over a screen
    /// (`offset > 0`: `settings` over `garage` / `QuickRace`, `repair`, `selectname`, …) never
    /// changes the room: the covered screen's room stays lit behind the overlay (the running
    /// original shows the garage's walls, floor and car behind `settings`' oval, COVERAGE 14.93),
    /// where the port used to swap in the top screen's `ID_NONE` (`FEEmpty3DRoom`, "just the car").
    pub(crate) fn fe_screen_room(&self) -> Option<&'static nfs_game::room::Room> {
        let l = self.fe_base_layout()?;
        nfs_game::room::room(l.layout.screen().and_then(|s| s.get("room")).unwrap_or(""))
    }

    /// The **bottom layer's** screen — the first element of the layer vector `[fe+0xD8]`
    /// (`[[[fe+0xD8]]]`, pending else current: 0x4D2570 0x4D26E7…0x4D26F8, 0x4D1BD0
    /// 0x4D1BDA…0x4D1BEB) — which names the front end's background `[fe+0xE4]` (slot 0x84), callback
    /// list `[fe+0xE0]` (slot 0x88), room `[fe+0xE8]` (slot 0x8C) and music (slot 0x94) objects.
    /// The port keeps the covered layers in `layout_under` (bottom first)
    /// and the top one in `layout_preview`, so with no layer under it the screen on show is the
    /// bottom one.
    pub(crate) fn fe_base_layout(&self) -> Option<&nfs_game::layout::LayoutScreen> {
        self.fe.layout_under.first().map(|(_, u)| u).or(self.fe.layout_preview.as_ref())
    }

    /// The room object's part of the front end's tick 0x4D2570: advance its transition (+0x80,
    /// 0x4D2738); once it is down (0x4E0E10) delete it (0x4D275D); and with none, make the one
    /// the screen on show names, open it and start it in (0x4D280E…0x4D283E) — on the same tick.
    pub(crate) fn fe_room_tick(&mut self, dt_ms: u32) {
        if let Some((_, t)) = self.fe.room_obj.as_mut() {
            *t = t.advance(dt_ms);
            if matches!(t, nfs_game::layout::Transition::Out { .. }) && t.done() {
                self.fe.room_obj = None;
            }
        }
        if self.fe.room_obj.is_none() {
            self.fe_room_create();
        }
    }

    /// 0x4E7870 with the screen's `room=`, then the room's open and its in (slot 0x78, 0x4E09A0).
    pub(crate) fn fe_room_create(&mut self) {
        self.fe.room_obj = self.fe_screen_room().map(|r| (r, nfs_game::layout::Transition::In { frames: nfs_game::layout::TRANS_DELAY_FRAMES, ms: 0 }));
    }

    /// Load the room's models and the car that stands in it, keeping whatever is already loaded.
    pub(crate) fn load_fe_room(&mut self) {
        let Some(room) = self.fe_room_of() else {
            self.fe.room.clear();
            return;
        };
        // the car the screen's own `NFS5.3DCAR` names — with `positiontype` 0 the room owns it
        // the cars the screen's `NFS5.3DCAR` widgets name. `positiontype` 0 hands the car to the
        // room; 1, 2 and 3 put one of their own at a fixed spot (`RE/notes/frontend-3d.md` §4). A
        // source the port cannot resolve falls back to the car the front end has selected, as the
        // accessor answers elsewhere.
        // the model **and its body style** — `nfs5.car +0x1C0`, which is what picks the `[styleN]`
        // section (`car-model.md`, `applyStyle` 0x437B00)
        let fallback = self.fe.menu.as_ref().and_then(|m| m.car()).map(|c| (c.model.clone(), c.variant));
        let mut widgets = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.car_3d())
            .unwrap_or_default();
        // **The covered layers' room-car widgets (read 2026-09-30).** Every layer's screen
        // ticks (0x4D2570's first loop calls `[screen]->slot 0x80` for each of `[fe+0xD8]`, no
        // cover test) and a `positiontype` 0 `NFS5.3DCAR`'s tick 0x4C0520 → 0x4BFFD0 → 0x4BFEE0
        // hands its car to **the room on show** (`[[0x65B928] + 0xE8]->slot41(car)`, 0x4BFEEC…
        // 0x4BFF0A, the base 0x4E7950 `[room+0x26C] = car`), whichever screen owns the widget: so a
        // screen with no such widget of its own that covers one — every shipped overlay: `settings`
        // over `garage` / `QuickRace` / `factorydriver`, `victorymessage` over `VictoryRoom`,
        // `testdriveoval` over `dealership`, all with `room=ID_NONE`, `FEEmpty3DRoom` "just the
        // car" (0x4E96E0) — shows the covered screen's car in its own room. Layers tick bottom
        // first, so the last widget to write wins: the topmost covered layer that has one.
        // (Not `slot 0xA0` as an earlier note said: 0x4D3040's loop reaches each widget's slot 26,
        // which for `FECar3DControl` (vtable 0x5B5878) is the base stub 0x5258A0 `ret 4`. The car of
        // a `positiontype` ≠ 0 widget is queued by slot 25 0x4C0A90 in 0x4D31CD's loop over the same
        // layers — only `carcompare` and `usedcardamage` have such widgets and nothing is ever
        // pushed over either, so covered views of that kind are not carried: APPROXIMATION, no
        // shipped case.)
        // (formerly approximation, MEDIUM — CLOSED 329adc1a, confirmed on the Wine original: the covered garage car shows behind the oval) was not read: that a covered screen's tick 0x80 reaches its
        // widgets' tick 0x4C0520 with no cover test, and that `FEEmpty3DRoom`'s slot 41 stores the car.
        // A dx7 capture of `settings` over `garage` decides it (a faint car behind the oval or none).
        if !widgets.iter().any(|w| w.position().is_none())
            && let Some(covered) = self
                .fe
                .layout_under
                .iter()
                .rev()
                .map(|(_, u)| u.car_3d().into_iter().filter(|w| w.position().is_none()).collect::<Vec<_>>())
                .find(|v| !v.is_empty())
        {
            widgets.extend(covered);
        }
        let mut wanted: Vec<(usize, String, i16, Vec<[i16; 3]>, Paint, Option<i8>)> = Vec::new();
        for w in &widgets {
            let model = self
                .logo_car(w.accessor.as_deref(), w.othersource.as_deref())
                .map(|c| (c.model.clone(), c.variant, self.fe_car_swaps(w.accessor.as_deref(), c), self.fe_car_paint(w.accessor.as_deref(), c), self.fe_car_decal(w.accessor.as_deref())))
                .or_else(|| fallback.clone().map(|(m, v)| (m, v, Vec::new(), self.paint.into(), None)));
            if let Some((model, variant, swaps, paint, decal)) = model {
                wanted.push((
                    if w.position_type == 0 {
                        usize::MAX
                    } else {
                        w.index
                    },
                    model,
                    variant,
                    swaps,
                    paint,
                    decal,
                ));
            }
        }
        // a real room with no `NFS5.3DCAR` on the screen still stands the selected car in it; an
        // empty room (`ID_NONE`) without one shows nothing
        // (not the track room: `FETrackSelect3DRoom` never touches the room's car `[+0x26C]`)
        if wanted.is_empty()
            && !room.models.is_empty()
            && room.id != "ID_TRACK"
            && let Some((model, variant)) = fallback
        {
            wanted.push((usize::MAX, model, variant, Vec::new(), self.paint.into(), None));
        }
        if self
            .fe.room
            .iter()
            .map(|(n, _)| n.as_str())
            .ne(room.models.iter().copied())
        {
            self.fe.room.clear();
            let dir = self.paths.fe_data().join("Models");
            for name in room.models {
                let Some(r) = self.renderer.as_mut() else {
                    return;
                };
                let loaded = nfs_assets::car::CarAssets::load_in(&dir, name)
                    .map(|mut a| {
                        a.front_end_room = true;
                        a
                    })
                    .map_err(anyhow::Error::from)
                    .and_then(|a| CarModel::upload(r, &a, &self.paths, 1, [0xC0, 0xC0, 0xC0]));
                match loaded {
                    Ok(m) => self.fe.room.push((name.to_string(), m)),
                    Err(e) => {
                        tracing::warn!(room = room.id, model = name, error = %e, "front-end room model unavailable")
                    }
                }
            }
            tracing::info!(
                room = room.id,
                models = self.fe.room.len(),
                "front-end room loaded"
            );
        }
        // **The key is the model file, the body style and the paint — not the file alone.** 87
        // playable cars share **21** model files: a family is one `.crp` and its cars differ by
        // the `[styleN]` section `nfs5.car +0x1C0` picks (`applyStyle` 0x437B00) — `356A` is one
        // file for twelve cars and twelve styles, `964` one for eight. Keyed on the file alone,
        // stepping through a family never reloaded, so the showroom kept the first car's body
        // and paint while the name under it changed. Found in play, 2026-09-21.
        let key: Vec<(usize, String, i16, Paint, Vec<[i16; 3]>, Option<i8>)> = wanted.iter().map(|(i, n, v, sw, p, dn)| (*i, n.clone(), *v, *p, sw.clone(), *dn)).collect();
        // `RCar::load` in the front end (`[0x65B928]` set) hands the specular layer `FEDU` / `FE  ` /
        // `FESH` (0x441AF1…0x441B3E → 0x440940); the `CarExtEnv` passes are built as in a race by
        // `Graphics.CarChrome` (0x44F020 → `[0x6197F8]`, `lighting.md` §5.3). The front end's
        // `R3DCar` slot 15 leaves `+0x190` clear (0x43AD8E), so the pass is not gated off, and binds
        // no sky map (0x43AD64…0x43AD87) — the items carry none.
        let chrome = self.choice_index(choice::GRAPHICS_CAR_CHROME).is_some_and(|(i, _)| i >= 1);
        let env_stale = self.fe.cars_env.is_none_or(|e| e.chrome_passes != chrome);
        if env_stale && let Some(r) = self.renderer.as_mut() {
            let specular = match self.fe.cars_env {
                Some(e) => e.specular,
                None => nfs_game::world::car_specular_maps(r, &self.paths, true),
            };
            self.fe.cars_env = Some(nfs_game::world::CarEnv { specular, chrome_passes: chrome, rs_bias: false });
        }
        let car_env = self.fe.cars_env.unwrap_or_default();
        if env_stale || self.fe.cars_key != key || self.fe.cars.len() != key.len() {
            // The dealership keeps the car it is leaving: its tick builds a second car `[+0x2F8]`
            // from the name and paint of the car shown until then, which it copies from the
            // accessor at the end of every tick (0x4E9624…0x4E967F), and draws it where the row
            // slides away from (`nfs_game::room::DealerRow`)
            self.fe.dealer_prev = if room.id == "ID_DEALER" {
                self.fe.cars.iter().position(|c| c.0 == usize::MAX).map(|k| self.fe.cars.swap_remove(k).4).or(self.fe.dealer_prev.take())
            } else {
                None
            };
            self.fe.cars.clear();
            self.fe.cars_key = key;
            for (slot, model, variant, swaps, paint, decal) in wanted {
                let Some(r) = self.renderer.as_mut() else {
                    return;
                };
                let loaded = nfs_assets::car::CarAssets::load(&self.paths, &model)
                    .map(|c| {
                        let mut c = c.with_style(variant).with_style_overlay(swaps);
                        // a garage car's race number, which the front end sets on the car info
                        // from `DecalNum` (0x4D1800, filtered to a style-10 "on" test elsewhere —
                        // `fe_car_decal`'s `>= 0`, matching `applyStyle`'s own `>= 0` gate,
                        // `car-model.md` §"What shows the number"). The licence, read 2026-09-27:
                        // every FE 3D car's own constructor (0x4D1160, the widget's call building
                        // the object at `[+0x2F8]`) unconditionally copies **the current profile's
                        // name** — `call 0x501270` (the same function the race launch's plate
                        // reads, `car-model.md` §5.3) — into car-info `+0x1C0` `.LICENSE` right
                        // after allocating it (0x4D11F8…0x4D1241, a `strcpy`-shaped `repne scasb` +
                        // `rep movsd/movsb` pair), before any per-car field is touched — so a
                        // garage (or dealer/showroom) car's plate always reads the player's own
                        // name, not the car's, and does so whatever `DecalNum` is (`.LICENSE` and
                        // `.RACENUMBER` are set by two unrelated calls). The port had tied the
                        // whole `PageText` — license included — to `decal` being `Some` (i.e.
                        // `DecalNum >= 0`), so a shipped car (`DecalNum` −1 as always) drew no
                        // plate text at all; `page_text` is now built whatever `decal` is, only
                        // its `race_number` following that filtered value (`-1` when it is `None`,
                        // same as a shipped car's own `DecalNum`). *(Until now: no licence text,
                        // and no race-number text either, on any shipped garage/dealer car.)*
                        c.page_text = Some(nfs_assets::car::PageText {
                            license: self.career.as_ref().map(|c| c.profile.name.clone()).unwrap_or_default(),
                            race_number: decal.unwrap_or(-1) as i32,
                        });
                        c
                    })
                    .map_err(anyhow::Error::from)
                    .and_then(|a| {
                        // the room car's half box, 0x445090's `car+0x3A8…+0x3B0` (a front-end
                        // car is a racer's kind: LOD 5), for the garage's shadow projector
                        if slot == usize::MAX {
                            let (lo, hi) = nfs_game::world::car_model_box(&a, 5);
                            self.fe.room_car_half = Some((hi - lo) * 0.5);
                        }
                        CarModel::upload_with_env(r, &a, &self.paths, 1, paint, car_env, false, None, None)
                    });
                match loaded {
                    Ok(m) => self.fe.cars.push((slot, model, variant, paint.exterior, m)),
                    Err(e) => tracing::warn!(model, error = %e, "front-end car model unavailable"),
                }
            }
        }
    }

    /// The 3D car widget's style overlay (0x4C0340…0x4C0486): the 120 dwords start at −1, then a
    /// car the accessor's slot 0x8C answers — a garage record — has its installed parts' swaps
    /// written in (`+0x758`, 0x4C0420…0x4C0486), and otherwise the catalogue record its slot 0x90
    /// answers has its **stock** parts' (`+0x3BC` / `+0x3BE`, 0x4C036A…0x4C0405, each through the
    /// part lookup 0x49C470).
    /// A garage car's four colours and package — what `FEOwnedCarAccessor`'s slots +0x74…+0x84
    /// answer from `FECar+0x674…+0x684`, the 3D car widget's colour pulls (0x4C014F…0x4C0299).
    pub(crate) fn paint_of(entry: &nfs_formats::fedata::CarCatalogEntry, o: &nfs_game::career::OwnedCar) -> Paint {
        let t = o.trim;
        Paint::owned(entry, o.paint_set as usize, t.interior as usize, t.stripe1 as usize, t.stripe2 as usize, t.package as u32).with_custom(o.custom)
    }

    /// The catalogue car in `GenericCar.Colour`'s saved custom colour, when one is chosen: slot 29
    /// answers the colour accessor's `[+0x2C]` (0x4F388F), slots 30…32 the record's interior 0 and
    /// paint set 0's stripes, 33 package 0 (§4.137).
    pub(crate) fn menu_custom_paint(m: &Menu, car: &nfs_formats::fedata::CarCatalogEntry) -> Option<Paint> {
        let c = m.paint_custom?;
        Some(Paint::from_catalogue(car, 0).with_custom([Some(c), None, None, None]))
    }

    /// The paint the 3D car widget puts on `car`: a garage or market car's own (`OwnedCars.Car`,
    /// `UsedCar.Car`), otherwise the catalogue accessor's — the chosen exterior, interior 0, set
    /// 0's stripes, package 0.
    /// The race number the 3D car widget hands 0x4D1800 — slot +0x88 of its accessor: a garage
    /// car's `DecalNum`; the catalogue's is −1 (0x502710), which shows nothing, so `None`.
    /// `UsedCar.Car` is an `FEOwnedCarAccessor` too (`savegame.md` §6.2a), so `usedcardamage.lay`'s
    /// 3D car shows the market car's own number — the one 0x4F8440 drew.
    pub(crate) fn fe_car_decal(&self, accessor: Option<&str>) -> Option<i8> {
        let base = accessor.map(|a| a.split_once('(').map_or(a, |(b, _)| b));
        match base {
            Some("OwnedCars.Car") => self.garage_car().map(|(_, o)| o.trim.decal),
            // `zMergedList.Car`'s garage half — the same owned record (`car_garage`, above)
            Some("zMergedList.Car") if self.fe.menu.as_ref().is_some_and(|m| m.car_garage) => {
                self.garage_car().map(|(_, o)| o.trim.decal)
            }
            Some("UsedCar.Car") => self.used_car().map(|o| o.trim.decal),
            _ => None,
        }
        .filter(|n| *n >= 0)
    }

    fn fe_car_paint(&self, accessor: Option<&str>, car: &nfs_formats::fedata::CarCatalogEntry) -> Paint {
        let base = accessor.map(|a| a.split_once('(').map_or(a, |(b, _)| b));
        if base == Some("OwnedCars.Car")
            && let Some((_, o)) = self.garage_car()
            && !car.paint_sets.is_empty()
        {
            return Self::paint_of(car, o);
        }
        // `zMergedList.Car`'s garage half wears the owned car's own paint too — 0x4F49F0 routes
        // the tail's car-object slots to list A while `car_garage` (0x4F4DF0) holds, and A's own
        // tail (`FEOwnedCarAccessor`'s slots 29…34, §4.145) reads the FE car's own `+0x674…+0x688`,
        // the same fields `paint_of` builds from.
        if base == Some("zMergedList.Car")
            && self.fe.menu.as_ref().is_some_and(|m| m.car_garage)
            && let Some((_, o)) = self.garage_car()
            && !car.paint_sets.is_empty()
        {
            return Self::paint_of(car, o);
        }
        // the used-car lot's car answers the same slots from the market car (`FEOwnedCarAccessor`,
        // `savegame.md` §6.2a): the paint set, stripes, interior and package 0x4F82C0 drew
        if base == Some("UsedCar.Car")
            && let Some(o) = self.used_car()
            && !car.paint_sets.is_empty()
        {
            return Self::paint_of(car, o);
        }
        // the dealership's car wears the rows chosen on it: slot 29 the paint set's exterior,
        // slot 30 the interior, 31 / 32 paint set 0's stripes, 33 package 0 (§4.137)
        if base == Some("DealerCar.Car")
            && self.dealer_cars().is_some()
            && !car.paint_sets.is_empty()
        {
            let d = self.fe.dealer;
            return Paint::owned(car, d.paint, d.interior, 0, 0, 0);
        }
        // the catalogue car's slot 29 is `GenericCar.Colour`'s current colour (0x4F388F) —
        // *always*, not only a saved custom one: `controls.md` §4.171.6 reads it to the
        // instruction — "so `GenericCar.Car` wears `GenericCar.Colour`'s `[+0x2C]` whatever it
        // is" — and that value is a *row* (`m.paint_idx`) whenever it is not a saved custom dword
        // (`m.paint_custom`). Before this fix the un-started preview fell all the way to
        // `self.paint`, which the front end only ever sets when a race actually **starts**
        // (`start_from_layout`'s `self.paint = car.paint_sets.get(m.paint_idx…)`) or a career/
        // replay loads one — so browsing QuickRace showed whichever colour a *previous* race left
        // in `self.paint` (a fresh `App`'s own struct default, red) instead of the selected row,
        // the same mismatch [`default_paint_row`]'s doc fixes for the `цвет` swatch itself.
        if matches!(base, Some("zMergedList.Car" | "GenericCar.Car")) {
            let m = self.fe.menu.as_ref();
            if let Some(p) = m.and_then(|m| Self::menu_custom_paint(m, car)) {
                return p;
            }
            if let Some(m) = m
                && !car.paint_sets.is_empty()
            {
                return Paint::from_catalogue(car, m.paint_idx % car.paint_sets.len());
            }
        }
        Paint::matching(car, self.paint)
    }

    fn fe_car_swaps(&self, accessor: Option<&str>, car: &nfs_formats::fedata::CarCatalogEntry) -> Vec<[i16; 3]> {
        let Some(cu) = self.career.as_ref() else { return Vec::new() };
        let base = accessor.map(|a| a.split_once('(').map_or(a, |(b, _)| b));
        let ids: Vec<i16> = match base {
            Some("OwnedCars.Car") => cu.career.garage.get(cu.car_sel).map(|o| o.parts.clone()).unwrap_or_default(),
            // `zMergedList.Car`'s garage half — the owned record's own installed parts, not the
            // catalogue's stock ones (`car_garage`, above)
            Some("zMergedList.Car") if self.fe.menu.as_ref().is_some_and(|m| m.car_garage) => {
                cu.career.garage.get(cu.car_sel).map(|o| o.parts.clone()).unwrap_or_default()
            }
            Some("UsedCar.Car") => self.used_car().map(|o| o.parts.clone()).unwrap_or_default(),
            _ => car.stock_parts.clone(),
        };
        // 0x4D1318…0x4D133E copy the array over `carinfo+0x32C` with index 10 forced to −1
        ids.iter()
            .filter_map(|id| cu.tables.parts.iter().find(|p| p.id == *id))
            .flat_map(|p| p.geometry_swaps.iter().copied())
            .filter(|s| !(s[0] == 0 && s[1] == 10))
            .collect()
    }

    /// The car a `NFS5.3DCAR` widget shows: the one loaded for its own slot, or — for the widget
    /// with no `positiontype` of its own — the room's car (`usize::MAX`).
    fn fe_car_slot_of(&self, widget: usize) -> Option<usize> {
        let l = self.fe.layout_preview.as_ref()?;
        let w = l.car_3d().into_iter().find(|w| w.index == widget)?;
        let slot = if w.position().is_some() { widget } else { usize::MAX };
        self.fe.cars.iter().position(|c| c.0 == slot)
    }

    /// A click's part events: the lids go to the car's own easers (0x439A00…0x439BD0), the sounds
    /// through 0x4AADB0.
    pub(crate) fn fe_car_part_events(&mut self, widget: usize, events: &[nfs_game::carparts::PartEvent]) {
        use nfs_game::carparts::PartEvent;
        for e in events {
            match *e {
                PartEvent::Hood(open) => {
                    if let Some(k) = self.fe_car_slot_of(widget) {
                        self.fe.cars[k].4.set_lid(true, open);
                    }
                }
                PartEvent::Trunk(open) => {
                    if let Some(k) = self.fe_car_slot_of(widget) {
                        self.fe.cars[k].4.set_lid(false, open);
                    }
                }
                PartEvent::Sfx(n) => self.fe_sound(n as usize),
                // the roof click's `Car.Roof` write, index 1 "down" / 0 "up" (0x4C0F9E / 0x4C0FA7)
                PartEvent::Roof(down) => {
                    self.choices.insert("Car.Roof".to_string(), usize::from(down));
                }
            }
        }
    }

    /// One frame of the parts: the step (0x4C0696…0x4C07CF), then each widget's doors onto its
    /// car's channels 6 and 7 and the lids' easers onto 8 and 9.
    fn fe_car_parts_step(&mut self) {
        // `NFS_FE_PARTS=1,3` clicks those zones of every 3D car at tick 0, once, so a still taken
        // with `NFS_FE_CLOCK` shows them open — the way `NFS_FE_DZ` stands in for a dragged bar
        if !self.fe.parts_forced
            && let Some(v) = nfs_game::config::cfg().nfs_fe_parts.as_str()
        {
            self.fe.parts_forced = true;
            let widgets: Vec<usize> = self.fe.layout_preview.as_ref().map(|l| l.car_3d().iter().map(|w| w.index).collect()).unwrap_or_default();
            for w in widgets {
                for z in v.split(',').filter_map(|z| z.trim().parse::<u32>().ok()) {
                    let ev = self.fe.layout_preview.as_mut().map(|l| l.car_click_zone(w, z, 0)).unwrap_or_default();
                    self.fe_car_part_events(w, &ev);
                    if let Some(k) = self.fe_car_slot_of(w) {
                        self.fe.cars[k].4.update_lids(1000);
                    }
                }
            }
        }
        // the widget's tick first looks at `Car.Roof` (0x4C05E7…0x4C063F) — and the roof zone is
        // there only when the car's roof animation has a length (0x438190 at 0x4C0D10)
        let roof_down = self.choice_index(choice::CAR_ROOF).is_some_and(|(i, _)| i == 1);
        let widgets: Vec<usize> = self.fe.layout_preview.as_ref().map(|l| l.car_3d().iter().map(|w| w.index).collect()).unwrap_or_default();
        for w in widgets {
            let has_roof = self.fe_car_slot_of(w).is_some_and(|k| self.fe.cars[k].4.anim_length(1) > 0);
            let ev = match self.fe.layout_preview.as_mut() {
                Some(l) => {
                    l.set_car_roof(w, has_roof);
                    l.sync_car_roof(w, roof_down)
                }
                None => Vec::new(),
            };
            self.fe_car_part_events(w, &ev);
        }
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        // the step's closing sounds, 0x31 / 0x33 / 0x35 (0x4C0778…0x4C07A6)
        let closing = l.step_car_parts();
        for e in closing {
            if let nfs_game::carparts::PartEvent::Sfx(n) = e {
                self.fe_sound(n as usize);
            }
        }
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        let widgets: Vec<usize> = l.car_3d().iter().map(|w| w.index).collect();
        // `data.damage` (0x4BFCEE → `[+0x1AD]`): every tick the widget hands the car its ten zone
        // damages — the owned car's `FECar+0x6A8` floats when the key is set and the accessor has
        // a car, 0 otherwise (0x4C0662…0x4C068B, through 0x4D1A00 into `car+0x4E4 + 4z`)
        let damages: Vec<(usize, [f32; 10])> = l.car_3d().iter().map(|w| (w.index, if w.damage { self.fe_car_damage(w.accessor.as_deref()) } else { [0.0; 10] })).collect();
        // `CARDETAIL` `[0x6197C4]` gates the crush 0x43E390 in the front end too
        let detail = self.choice_index(choice::GRAPHICS_CAR_DETAIL).map_or(0, |(i, _)| i as u8);
        for (w, d) in damages {
            if let Some(k) = self.fe_car_slot_of(w)
                && let Some(r) = self.renderer.as_mut()
            {
                self.fe.cars[k].4.update_damage(r, &d, false, detail);
            }
        }
        let dt = (self.frame_dt * 1000.0).round().clamp(0.0, 250.0) as u32;
        for w in widgets {
            let doors = self.fe.layout_preview.as_ref().map(|l| l.car_doors(w)).unwrap_or((0.0, 0.0));
            let roof = self.fe.layout_preview.as_ref().map(|l| l.car_roof_state(w)).unwrap_or(0);
            if let Some(k) = self.fe_car_slot_of(w) {
                let car = &mut self.fe.cars[k].4;
                car.set_channel(6, doors.0);
                car.set_channel(7, doors.1);
                // 0x4C07E8: after the step, the roof animation follows zone 0's state
                if let Some(r) = self.renderer.as_mut() {
                    car.set_roof(r, roof);
                }
                // 0x438751: channel 16 follows the roof in the front end
                car.follow_roof();
            }
        }
        for c in self.fe.cars.iter_mut() {
            c.4.update_lids(dt);
        }
    }

    /// The room's meshes and the car turning in it, plus the camera and lighting the room's own rig
    /// gives them (`RE/notes/frontend-3d.md` §2, §3).
    pub(crate) fn fe_room_items(&mut self) {
        let Some(room) = self.fe_room_of() else {
            return;
        };
        // the light block's fade (0x4E84DE): 0x4E0D90(1) on the room — the larger of the room's
        // own progress and the screen on show's (0x4D6300)
        let screen_p = self.fe.layout_preview.as_ref().map_or(0, |l| l.transition().progress());
        let room_p = self.fe.room_obj.map_or(0, |(_, t)| t.progress());
        let fade = screen_p.max(room_p) as f32 / nfs_game::layout::TRANS_FULL as f32;
        // **The room moves toward the camera when a door is zoomed into**, not with the screen:
        // `[room+0x2E8]` is written only by its slot 0xB8 (0x4E7800), which only the room car's
        // widget calls, each tick, with its zoom (0x4C05E1)
        let zoom = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.car_3d().iter().filter(|w| w.position().is_none()).map(|w| l.car_zoom(w.index).0).fold(0.0f32, f32::max))
            .unwrap_or(0.0);
        let p = room.zoomed_pos(zoom);
        let pos = Vec3::new(p[0], p[1], p[2]);
        // `YRot`, then `XRot`, then `ZRot`: 0x4D1600 builds the y matrix fresh (0x532600) and
        // 0x4D1620 / 0x4D1690 multiply the x and z ones on its right (0x532470), which on the
        // original's row vectors turns about y first. *(Until 2026-09-26 this composed them the
        // other way round; every shipped rig but `ID_NONE`'s −0.01 has `XRot` 0, and no shipped
        // `ID_NONE` screen shows the room's car, so no picture changed.)*
        let turn = |t: f32| {
            let rot = Quat::from_rotation_z(room.rot[2] * std::f32::consts::TAU)
                * Quat::from_rotation_x(room.rot[0] * std::f32::consts::TAU)
                * Quat::from_rotation_y((t + room.rot[1]) * std::f32::consts::TAU);
            Mat4::from_rotation_translation(rot, pos)
        };
        // the parts a click opened (`nfs_game::carparts`): the step, then the doors onto channels
        // 6/7 and the lids' easers onto 8/9 of the car each `NFS5.3DCAR` shows
        self.fe_car_parts_step();
        let spin = room.spin_turns(self.fe.ms);
        let mut items = Vec::new();
        let room_m = turn(room.models_turn.unwrap_or(spin));
        // **The light block** 0x4E84C0 fills for each model (`RE/notes/lighting.md` §6): the
        // room's own models (0x4EA07A, flag 0) take the rig's ambient and sun, the car
        // (0x4EA08A, flag 1) a fixed 0.3 / 0.7 grey from (0, −0.707, 0.707); both faded by
        // `1 − progress / 512`. The direction is carried into this port's frame the way the models
        // are, through `MODEL_TO_BODY`, so a surface meets the light at the angle it does there.
        let light_fade = 1.0 - fade;
        let sun_dir = nfs_game::world::MODEL_TO_BODY.transform_vector3(Vec3::from(room.sun_dir));
        let room_light = nfs_game::world::LightBlock::room(room.ambient, room.sun, sun_dir, light_fade, false);
        let mut car_light = nfs_game::world::LightBlock::room(room.ambient, room.sun, sun_dir, light_fade, true);
        car_light.direction = nfs_game::world::MODEL_TO_BODY.transform_vector3(car_light.direction);
        // **The dealership** is its own draw, `FEDealership3DRoom` slot 15 0x4E8C70 after its tick
        // 0x4E93B0 (`nfs_game::room::DealerRow`): a row of sections, one per car, which the camera
        // slides along as the car changes, drawn twice over on a machine that is not a low-end card
        let dealer = match room.id {
            "ID_DEALER" => self.fe_dealer_items(&spin, zoom, &room_light, &car_light),
            "ID_TRACK" => self.fe_track_items(&room_light),
            _ => None,
        };
        let dealer_drawn = dealer.is_some();
        if room.id != "ID_DEALER" {
            self.fe.dealer_row = None;
        }
        if room.id != "ID_TRACK" {
            self.fe.track_room = None;
        }
        // `FEVictory3DRoom`'s tick 0x4EB3E0: its movie rendered into the room model's page 5
        // (`nfs_game::room::VictoryMovie`, [`nfs_game::room::VICTORY_MOVIE`] whatever the era). The
        // room object owns it (freed by its destructor 0x4EB390), so it goes with the room.
        if room.id != "ID_VICTORY" {
            self.fe.victory_movie = None;
        } else if !self.fe.room.is_empty() {
            let now = self.fe.ms;
            if self.fe.victory_movie.is_none()
                && let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.fe_data().join("Movies"), nfs_game::room::VICTORY_MOVIE)
                && let Ok(data) = std::fs::read(&path)
                && let Ok(tables) = nfs_formats::mad::MadTables::load(&self.paths.root)
                && let Some(m) = nfs_game::room::VictoryMovie::open(tables, data)
            {
                tracing::debug!(w = m.width, h = m.height, "victory room movie opened");
                // the first frame, which 0x4E1EB0 decoded, is blitted on the first render
                if let Some(r) = self.renderer.as_mut() {
                    self.fe.room[0].1.blit_page(r, 5, &m.rgba(), m.width, m.height, 0, 0);
                }
                self.fe.victory_movie = Some((m, now));
            }
            if let Some((m, last)) = self.fe.victory_movie.as_mut() {
                let stepped = m.tick(now.saturating_sub(*last) as f32);
                *last = now;
                if stepped && let Some(r) = self.renderer.as_mut() {
                    self.fe.room[0].1.blit_page(r, 5, &m.rgba(), m.width, m.height, 0, 0);
                }
            }
        }
        if dealer.is_none() {
            // the room's own tick: the garage's and the factory's fans (`room_channels`)
            let now = (self.fe.ms * 128 / 1000) as u32;
            for (_, model) in self.fe.room.iter_mut() {
                for (n, v) in nfs_game::room::room_channels(room.id, now) {
                    model.set_channel_vec(n, v);
                }
            }
            // **The garage's car shadow** — `FEGarage3DRoom`'s draw 0x4E9CE0, before its base draw:
            // with a car (`[+0x26C]`, 0x4E9CFF) it clears the car's blob-shadow flag
            // `[R3DCar+0x700]` (0x4E9D15) and hands the room model's projector slot `+0x2DC` the
            // `FESH` map of the shared bundle `[0x613E04]` = `Render/common.fsh` (0x4E9E2D, 0x44D320
            // with 0x48534546: 64×64, black under an alpha of 0…187 — not `carcmn.fsh`'s `FESH`,
            // the specular map; 0x440910) and the planes it stores at
            // `[0x690A30…0x690A54]` (0x440920 with 0x690A18): columns 0 and 2 of
            //
            //   T(−car.x, 0, −car.z) · R_y(−360·YRot) · S(1/(2·car+0x3A8), 1, 1/(2·car+0x3B0)) · T(½, 0, ½)
            //
            // on row vectors (0x539B80, 0x539BD0 → 0x5326C0 about y, 0x539B30, 0x539AD0 = this ·
            // arg). The car stands at the room's own place (both through 0x4D1700, 0x4EA058 /
            // 0x4EA06A) and the room model is drawn `(−1, 1, −1)`-flipped and turned by the same
            // `YRot` (0x443490), so that is the point in the room model's flipped frame — the
            // port's body frame — scaled by the car's box: the texture under the car, turning with
            // it. Without a car the slot gets none (0x4E9E67). The model's materials whose method
            // carries `R3DShadowMapTextureLayer` receive it (`CarIntShd` in `garage.crp`).
            //
            // The place and `YRot` are the **previous frame's**: 0x4E9FF0, called after, is what
            // moves the car (0x4EA06A) and turns the room (0x4EA0CF). **MEASURED 2026-10-02 (SRV-B,
            // `RE/notes/REFERENCE-2026-10-02-SRVB.md` §4): no observable effect** — the room's first draw has no car (`[+0x26C]` 0, and
            // the shadow arm needs one), the car's place stays `(0, -1.1, 4.8)` with `[+0x2E8]` 0 in all
            // 3 426 frames watched, so "previous" = current but for `YRot` (~0.003 rad a frame). The note it
            // replaces: on the room's first draw the original's previous place is the constructor's
            // (0x4E9B30 / 0x4EA240: `(0, -1.1, 4.8)`, `YRot` 0 from 0x4E76C0), this port takes the
            // current one. Taking the constructor's turn instead was tried and reverted: a still
            // builds the room once, so that single draw is the picture and the shadow left the car.
            if room.id == "ID_GARAGE"
                && self.fe.garage_shadow_tex.is_none()
                && let Some(r) = self.renderer.as_mut()
                && let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh")
                && let Ok(data) = std::fs::read(&path)
                && let Ok(images) = nfs_assets::texture::decode_pack(&path, &data)
                && let Some(img) = images.iter().find(|t| t.name.trim() == "FESH")
            {
                self.fe.garage_shadow_tex = Some(r.upload_texture_mips(img.width, img.height, &img.mips));
            }
            let shadow = (room.id == "ID_GARAGE")
                .then(|| {
                    let half = self.fe.room_car_half?;
                    self.fe.cars.iter().find(|c| c.0 == usize::MAX)?;
                    let tex = self.fe.garage_shadow_tex?;
                    let inv = self.fe.garage_shadow_prev.unwrap_or(room_m).inverse();
                    let r0 = inv.row(0) / (2.0 * half.x);
                    let r2 = inv.row(2) / (2.0 * half.z);
                    Some((tex, ([r0.x, r0.y, r0.z, r0.w + 0.5], [r2.x, r2.y, r2.z, r2.w + 0.5])))
                })
                .flatten();
            self.fe.garage_shadow_prev = (room.id == "ID_GARAGE").then_some(room_m);
            if let (Some((tex, _)), Some(r)) = (shadow, self.renderer.as_mut()) {
                r.set_projector_texture(Some(tex));
            }
            for (_, model) in &self.fe.room {
                let mut its = model.draw_items_block(room_m, [0.0; 4], [0.0; 2], [0.0; 4], &nfs_game::world::FRONT_END_PASSES, &room_light);
                model.apply_projector(&mut its, shadow.map(|s| s.1));
                items.extend(its);
            }
        }
        let widgets = self
            .fe.layout_preview
            .as_ref()
            .map(|l| l.car_3d())
            .unwrap_or_default();
        // the room's own car goes in with the room; a widget with a `positiontype` of its own is
        // drawn into an offscreen view instead, so it lands in that widget's place in the screen's
        // drawing order rather than behind all of it
        let mut views: Vec<(usize, Vec<nfs_renderer::DrawItem>)> = Vec::new();
        for (slot, _, _, _, car) in &self.fe.cars {
            let widget = widgets.iter().find(|w| w.index == *slot);
            match widget.and_then(|w| w.position().map(|p| (p, w.alpha))) {
                Some((p, alpha)) => {
                    // the table at 0x4BFF28 is kept exactly as it is in the binary, and its `x` is
                    // **mirrored here**: `carcompare.lay` gives the widget at `pos=175` (the left
                    // half of the 640-wide screen) `positiontype=1`, which the table places at
                    // `x = +2.4`, and the one at `pos=350` `positiontype=2` at `x = -2.4`. So in
                    // the original's front-end camera space +x is toward screen *left*, and this
                    // port's camera has +x to the right. CONFIRMED by the two widgets' own boxes.
                    let m = Mat4::from_rotation_translation(
                        Quat::from_rotation_y(spin * std::f32::consts::TAU),
                        Vec3::new(-p[0], p[1], p[2]),
                    );
                    // **The widget lights its own car**, not the room: `FECar3DControl` slot 25
                    // **0x4C0A90**, which the frame calls on every widget of every layer's screen
                    // (0x4D31CF → the screen's slot 39 0x4E0CA0) after the car was queued and
                    // before the flush 0x4D31FE, fills the car's block only when the car is there
                    // (`[+0x1AC]`, 0x4C002F) and `positiontype` is not 0 (`[+0x1A4]`): slots 0–2
                    // `0.4 · a`, 4–6 `0.9 · a`, direction (0, −0.707, 0.707), `+0x74` white, with
                    // `a` the widget's transition alpha 0x4C8840 `× 1/255` (0x4C0AE2) — brighter
                    // than the room's 0.3 / 0.7 and faded by the widget, not by the room.
                    let a = alpha as f32 * f32::from_bits(0x3b80_8081);
                    let (amb, sun) = ([a * f32::from_bits(0x3ecc_cccd); 3], [a * f32::from_bits(0x3f66_6666); 3]);
                    let own_light = nfs_game::world::LightBlock {
                        colours: [amb, amb, amb, [0.0; 3], sun, sun, sun],
                        direction: nfs_game::world::MODEL_TO_BODY
                            .transform_vector3(Vec3::new(0.0, f32::from_bits(0xbf34_fdf4), f32::from_bits(0x3f34_fdf4))),
                        material: [1.0; 3],
                    };
                    views.push((*slot, car.draw_items_block(m, [0.0; 4], [0.0; 2], [0.0; 4], &nfs_game::world::FRONT_END_PASSES, &own_light)));
                }
                None if dealer_drawn => {}
                None => items.extend(car.draw_items_block(turn(spin), [0.0; 4], [0.0; 2], [0.0; 4], &nfs_game::world::FRONT_END_PASSES, &car_light)),
            }
        }
        if let Some(d) = dealer {
            items.extend(d);
        }
        self.scene.items = items;
        self.scene.headlight = None;
        self.scene.ambient = room.ambient.iter().sum::<f32>() / (3.0 * 255.0);
        self.scene.sun_direction =
            -Vec3::new(room.sun_dir[0], room.sun_dir[1], room.sun_dir[2]).normalize();
        // the camera sits at the origin looking down +z — the rooms are placed in front of it —
        // with the rig's own vertical angle
        self.camera.position = Vec3::ZERO;
        self.camera.target = Vec3::Z;
        self.camera.up = Vec3::Y;
        // `V.A.` (`+0x290`) is the FE view's **horizontal** half-angle: 0x4E84A0 →
        // 0x4D6240 stores it in the view `[0x65B928]+0xFC`'s `+0x130`, which 0x466F30 doubles into
        // 0x5391B0 (`m00 = cot h`, `m11 = cot h · A`); that view is built full-screen (0x4D3728:
        // viewport 0…1 × 0…1), so `A` is the mode's `W / H` under `nfs5.dll` (0x466FE7 / 0x466FF4)
        // and the vertical tangent `tan h / r` — `camera::fov_y_window`. Measured on the server's
        // dealership frame (static background, 1024 × 768): the exit sign and the ceiling lights
        // sit 1.33× farther from the centre than the port's old "vertical 2 × V.A." drew them.
        // The dll's cell replaces the literal 37.5 the dealer, factory and garage constructors
        // store (0x4E8B7F, 0x4E9900, 0x4E9BAA → dll 0x10003C30 / 0x10003C50 / 0x10003C70); the
        // victory's 32 and the track's 25 are not patched. `ID_NONE` sets none (its base 0x4E76C0
        // keeps the view's current angle, 0x4E7762) and the port's stand-in is the garage's.
        let h = if room.view_angle == nfs_game::camera::DEFAULT_HALF_ANGLE {
            nfs_game::camera::mode_half_angle()
        } else {
            room.view_angle
        };
        self.camera.fov_y_radians = nfs_game::camera::fov_y_window(h);
        self.camera.near = 0.05;
        self.camera.far = 500.0;
        if views.is_empty() {
            return;
        }
        let (w, h) = self
            .renderer
            .as_ref()
            .map(|r| r.size())
            .unwrap_or((1280, 800));
        let (camera, ambient, sun) = (self.camera, self.scene.ambient, self.scene.sun_direction);
        let mut rendered: Vec<(usize, nfs_renderer::TextureId)> = Vec::new();
        for (slot, items) in views {
            let reuse = self.fe.layout_preview.as_ref().and_then(|l| l.car_view(slot));
            let scene = Scene {
                items,
                ambient,
                sun_direction: sun,
                ..Scene::default()
            };
            let Some(r) = self.renderer.as_mut() else {
                return;
            };
            rendered.push((slot, r.render_offscreen(&scene, &camera, w, h, reuse, None, None)));
        }
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (slot, tex) in rendered {
                l.set_car_view(slot, Some(tex));
            }
        }
    }

    /// `DealerCar.Car`'s current row and row count — the accessor's slots 10 (current car index)
    /// and 9 (car count), which the dealership's tick (0x4E940C) and draw (0x4E8CCF) read. These
    /// are the accessor's own **list** count/index, not the combo drop-down's row count — since
    /// `catalog_rows` (`combo_rows`'s `DealerCar.Car` arm) now inserts era/class heading rows
    /// (slot 26, `catalog_rows`'s own doc), `combo_rows`'s `rows.len()`/`at` no longer equal
    /// slots 9/10 and must not be used for this; computed directly from `dealer_cars()` instead.
    fn dealer_index_count(&self) -> (i32, i32) {
        let Some(v) = self.dealer_cars() else { return (0, 1) };
        let count = v.len().max(1) as i32;
        (self.fe.dealer.car.min(v.len().saturating_sub(1)) as i32, count)
    }

    /// `FEDealership3DRoom`: the tick **0x4E93B0** and the draw **0x4E8C70**, in the order the
    /// original makes its draws — each one is flushed at once through the object list
    /// (0x467290 after every 0x4D15E0 / 0x4D15F0), so the list below is the drawing order.
    ///
    /// * **The sections.** `DealerRow::layout` places the car; the sections `cur − 1 … cur + 1`
    ///   (wider on the side a slide comes from) stand `XOff`, `YOff` apart, each pinned at half a
    ///   turn (0x4E8D24…0x4E8D7F) — `dealer2a` for the first car, `dealer2c` from the last on,
    ///   `dealer2b` between. Every section's **channel 1** is `(0, spin, 0)` (0x4E8DD4…0x4E8E4B):
    ///   its `Platform` and `Platformtop` are bound to channel 1, so the turntable turns under the
    ///   car. The channel's axis-angle build 0x5326C0 about y gives the matrix 0x532600 gives the
    ///   car's own `YRot`, `[c 0 −s; 0 1 0; s 0 c]`, so the platform turns exactly as the car does
    ///   and the port hands both the same value.
    /// * **The early pass** (0x4E90A7…0x4E9231), skipped only on a low-end card (0x4E35C0) — never
    ///   in this port: each section with `[R3DCar+0x2C8] = 1`, the car and the car being left with
    ///   3, through 0x4D15F0, whose `+0x471 = 1` makes the car draw 0x444E00 take a single call of
    ///   LOD `[+0x2C8] + 1` with mask **0x30** (0x444F2B). It lands under the ordinary pass at the
    ///   same places, so only what the ordinary pass does not cover shows — the section's additive
    ///   light casts, which both LODs carry, add up twice.
    /// * **The ordinary pass** (0x4E9236…0x4E93A1): the sections, the car at the row's `(X, Y, Z)`,
    ///   and while sliding the car being left at `(X + d·XOff, Y, Z + d·YOff)` with the current
    ///   car's door channels 6 and 7 copied onto it (0x4E930D…0x4E9380).
    ///
    /// The x of every place is mirrored into this port's camera, as the `positiontype` cars' is.
    fn fe_dealer_items(&mut self, spin: &f32, zoom: f32, room_light: &nfs_game::world::LightBlock, car_light: &nfs_game::world::LightBlock) -> Option<Vec<nfs_renderer::DrawItem>> {
        use nfs_game::room::DealerRow;
        use nfs_game::world::{CarPass, FRONT_END_PASSES};
        let room = self.fe_room_of()?;
        if self.fe.room.len() != 3 {
            return None;
        }
        let (index, count) = self.dealer_index_count();
        let now = (self.fe.ms * 128 / 1000) as u32;
        let row = self.fe.dealer_row.get_or_insert(DealerRow::new(index));
        let started = row.tick(now, index);
        let layout = row.layout(room.pos, zoom);
        let cur = row.cur;
        let sliding = row.sliding;
        let spin = *spin;
        // 0x4E9580…0x4E9596: the car being left takes the current car's roof state
        if started
            && let Some(roof) = self.fe.cars.iter().find(|c| c.0 == usize::MAX).map(|c| c.4.roof_state())
            && let (Some(prev), Some(r)) = (self.fe.dealer_prev.as_mut(), self.renderer.as_mut())
        {
            prev.set_roof(r, roof);
        }
        for (_, m) in self.fe.room.iter_mut() {
            m.set_channel_vec(1, [0.0, spin, 0.0]);
        }
        let place = |p: [f32; 3], t: f32| {
            let rot = Quat::from_rotation_y((t + room.rot[1]) * std::f32::consts::TAU) * Quat::from_rotation_x(room.rot[0] * std::f32::consts::TAU);
            Mat4::from_rotation_translation(rot, Vec3::new(-p[0], p[1], p[2]))
        };
        let sections = layout.sections(cur, count);
        let prev_at = layout.previous_car().filter(|_| sliding);
        let early_room = [CarPass { lod: 2, mask: 0x30, flag: false }];
        let early_car = [CarPass { lod: 4, mask: 0x30, flag: false }];
        let mut items = Vec::new();
        let car = self.fe.cars.iter().find(|c| c.0 == usize::MAX).map(|c| &c.4);
        for (passes_room, passes_car) in [(&early_room[..], &early_car[..]), (&FRONT_END_PASSES[..], &FRONT_END_PASSES[..])] {
            for (k, p) in &sections {
                let m = &self.fe.room[*k].1;
                items.extend(m.draw_items_block(place(*p, 0.5), [0.0; 4], [0.0; 2], [0.0; 4], passes_room, room_light));
            }
            if let Some(car) = car {
                items.extend(car.draw_items_block(place(layout.pos, spin), [0.0; 4], [0.0; 2], [0.0; 4], passes_car, car_light));
                if let (Some(at), Some(prev)) = (prev_at, self.fe.dealer_prev.as_mut()) {
                    if passes_car == &FRONT_END_PASSES[..] {
                        prev.set_channel_vec(6, car.channel_vec(6));
                        prev.set_channel_vec(7, car.channel_vec(7));
                    }
                    items.extend(prev.draw_items_block(place(at, spin), [0.0; 4], [0.0; 2], [0.0; 4], passes_car, car_light));
                }
            }
        }
        Some(items)
    }

    /// 0x4FC0B0's track and direction for the game type the front end holds — what the track
    /// room's tick asks (0x4EA9C4…0x4EAA04): the tournament's remaining event for 4 / 5 (its
    /// `evt+0x3A4` / `+0x3A5`), for 2 the running series' round `Track<raceIndex+1>` (0x4DDF00 set,
    /// 0x4FC20E) or else the knockout table's selected row (`Knockout.Track`) forward and
    /// unmirrored (0x4FC3CD), for 1 `LoopedTrack.Track` with `Track1.Direction`, for 0 / 3
    /// `Track1.Track` with `Track1.Direction`, for the test drives 6…8 `Track1.Track` forward
    /// (0x4FC691), and nothing above 11. *(Until 2026-09-26 the not-running knockout and the test
    /// drives took the row's / `Track1.Direction`'s direction.)* The mission arm (9, 0x4FC50F…0x4FC5A0): the current mission 0x51AE80 (the
    /// first without a pass, the table's first record with none), the `nfs5.trk` record whose id
    /// is its `+0x20` — the first record `[[0x691D00]]` when none is — reversed when `+0x28 == 2`,
    /// never mirrored. The finale arms (10 / 11, 0x4FC5BF…0x4FC690): the finale's event 0x504990
    /// (`0x504800()`), `evt+0x3A4` / `+0x3A5`, its track id 0x504930 looked up the same way.
    /// *(Until 2026-09-26 these took `Track1.Track`, an approximation — no track room shows during
    /// them, so nothing on screen changes.)*
    fn fe_room_track(&mut self) -> Option<(nfs_formats::career::TrackInfo, bool, bool)> {
        if matches!(self.fe.game_type, game_type::TOURNAMENT | game_type::TOURNAMENT_TEST_DRIVE)
            && let Some(c) = self.career.as_ref()
        {
            let t = c.current()?;
            let ev = t.events.get(c.career.remaining_event(t, self.fe.remaining_sel)?)?;
            return Some((c.tables.track(ev.track_id)?.clone(), ev.backwards, ev.direction_flag != 0));
        }
        if matches!(self.fe.game_type, game_type::MISSION | game_type::FINALE | game_type::FINALE_TEST_DRIVE)
            && let Some(c) = self.career.as_ref()
        {
            // a track id no record carries falls to the first record (0x4FC555 / 0x4FC66C)
            let by_id = |id: i16| c.tables.track(id).or_else(|| c.tables.tracks.first()).cloned();
            if self.fe.game_type == game_type::MISSION {
                let m = c.career.current_mission(&c.tables).or((!c.tables.missions.is_empty()).then_some(0)).and_then(|i| c.tables.missions.get(i))?;
                return Some((by_id(m.track_id)?, m.backwards, false));
            }
            let t = c.career.finale_tournament(&c.tables)?;
            let done = c.career.tournaments.get(&t.id).map_or(0, |s| s.events.len());
            let ev = t.events.get(done.min(t.events.len().saturating_sub(1)))?;
            return Some((by_id(ev.track_id)?, ev.backwards, ev.direction_flag != 0));
        }
        let m = self.fe.menu.as_ref()?;
        let (file, rev, mir) = match self.fe.game_type {
            // a running series: its round `Track<raceIndex+1>` with that row's direction
            // (0x4FC20E…0x4FC3C3); none running: `Knockout.Track`'s linked row (slot 0x78, then
            // 0x4FA690, 0x4FC3CD…0x4FC411) and nothing else — `rev` / `mir` keep the zeros
            // 0x4FC0E3 / 0x4FC0E6 wrote, so the wall shows the selected row forward and unmirrored
            game_type::KNOCKOUT => match self.knockout.as_ref().filter(|k| k.state.active) {
                Some(k) => {
                    let r = m.ko_row(k.state.race_index.max(0) as usize);
                    (m.tracks.get(r.track_idx)?.clone(), r.backwards, r.mirror)
                }
                None => (m.ko_track(m.ko_sel).to_string(), false, false),
            },
            game_type::QUICK_KNOCKOUT => (m.looped_track().to_string(), m.backwards, m.mirror),
            // 0 and 3 (0x4FC108 / 0x4FC148): `Track1.Track` with `Track1.Direction`
            0 | 3 => (m.track().to_string(), m.backwards, m.mirror),
            // the test drives 6…8 (0x4FC691): `Track1.Track` alone — forward, unmirrored
            6..=8 => (m.track().to_string(), false, false),
            // above 11 the jump table is skipped (`ja 0x4FC6C6`) and the answer is null
            _ => return None,
        };
        let paths = &self.paths;
        let table = self.fe.trk_table.get_or_insert_with(|| {
            std::fs::read(paths.fe_data().join("Data").join("nfs5.trk")).ok().and_then(|d| nfs_formats::career::parse_track_table(&d).ok()).unwrap_or_default()
        });
        Some((table.iter().find(|t| t.file.eq_ignore_ascii_case(&file))?.clone(), rev, mir))
    }

    /// `FETrackSelect3DRoom` — its tick **0x4EA9B0** and draw **0x4EA660** (`nfs_game::room::
    /// TrackRoom`): the floor `track` once, then `trackwall` four times at `YRot`, `+0.25`, `+0.5`
    /// and `+0.75`, each at the fly-in's place and `XRot` (0x4EA7D3…0x4EA94F), both lit by the room
    /// block (0x4E84C0 with 0, 0x4EA74E / 0x4EA75E). No car: the class never reads `[+0x26C]`.
    ///
    /// The pictures: each blit the tick asks for writes frame `k` of `TrackArt/<art>.fsh` (the
    /// `nfs5.trk` row's `+0x44`, 0x4EACD5…0x4EACFF) into the page at (0, 0) — flipped left to right
    /// around the blit on a mirrored track (0x563320) — and on frame 0 the finish flag (entry 6) on
    /// a reversed track or the start flag (entry 5) otherwise at `track_marker_at` (0x4EADA5…
    /// 0x4EAE34). Both go through the software blitter into the 565 page, so the flag's alpha is
    /// dropped (0x56D4C0 → 0x571630 converts it to format 0x7D, 8888, and the page's row writer
    /// keeps `r g b`) — MEDIUM, the converter's table was not followed to the end.
    ///
    /// The fly-in starts when the room's transition is settled (0x4E0E20, `[+0x12C] == 2`) — the
    /// room object's own (`fe_room_tick`). *(Until 2026-09-26 the port took the screen's, an
    /// approximation.)* The x of the place is mirrored into this port's camera,
    /// and so is the turn about y, as for the other rooms; `XRot` is applied after `YRot`, as
    /// 0x4D1620 multiplies it on the right of the matrix 0x4D1600 made (0x532470).
    fn fe_track_items(&mut self, room_light: &nfs_game::world::LightBlock) -> Option<Vec<nfs_renderer::DrawItem>> {
        use nfs_game::room::{track_marker_at, TrackRoom};
        use nfs_game::world::FRONT_END_PASSES;
        if self.fe.room.len() != 2 {
            return None;
        }
        let now = (self.fe.ms * 128 / 1000) as u32;
        // 0x4E0E20 on the room itself: its own transition at rest
        let settled = self.fe.room_obj.is_some_and(|(_, t)| matches!(t, nfs_game::layout::Transition::Idle));
        let track = self.fe_room_track();
        let back = self.fe.track_turn_back;
        let tr = self.fe.track_room.get_or_insert_with(|| TrackRoom::new(now));
        let (blits, turn) = match &track {
            Some((info, rev, mir)) => tr.tick(now, info.id as usize, *rev, *mir, back),
            None => (Vec::new(), 0.001),
        };
        tr.fly(now, settled);
        let (pos, xrot, yrot) = (tr.pos, tr.xrot, tr.yrot);
        if let Some((info, rev, mir)) = track.filter(|_| !blits.is_empty()) {
            if self.fe.track_art.as_ref().is_none_or(|(a, _)| !a.eq_ignore_ascii_case(&info.art)) {
                let dir = self.paths.fe_data().join("TrackArt");
                let want = format!("{}.fsh", info.art);
                let file = std::fs::read_dir(&dir)
                    .ok()
                    .and_then(|d| d.flatten().map(|e| e.path()).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case(&want))));
                let entries = file
                    .and_then(|p| std::fs::read(p).ok())
                    .and_then(|data| {
                        let f = nfs_formats::fsh::Fsh::parse(&data).ok()?;
                        Some((0..f.entries.len()).filter_map(|i| Some((f.decode(&data, i).ok()?, f.entries[i].blocks.first()?.misc))).collect::<Vec<_>>())
                    })
                    .unwrap_or_default();
                self.fe.track_art = Some((info.art.clone(), entries));
            }
            // 0x563320 around each blit on a mirrored track: the art flipped left to right
            let flip = |img: &nfs_formats::fsh::DecodedImage| -> Vec<u8> {
                let (w, h) = (img.width as usize, img.height as usize);
                let mut out = img.rgba.clone();
                if mir {
                    for y in 0..h {
                        for x in 0..w {
                            out[(y * w + x) * 4..][..4].copy_from_slice(&img.rgba[(y * w + (w - 1 - x)) * 4..][..4]);
                        }
                    }
                }
                out
            };
            let art = self.fe.track_art.as_ref().map(|(_, e)| e.as_slice()).unwrap_or(&[]);
            if let Some(r) = self.renderer.as_mut() {
                let wall = &mut self.fe.room[1].1;
                for b in &blits {
                    let Some((img, misc)) = art.get(b.frame) else { continue };
                    wall.blit_page(r, b.page, &flip(img), img.width, img.height, 0, 0);
                    if b.frame == 0
                        && let Some((mark, mm)) = art.get(if rev { 6 } else { 5 })
                    {
                        let (x, y) = track_marker_at(misc[0] as i16, img.width as u16, mm[0] as i16, mm[1] as i16, mark.width as u16, mir);
                        wall.blit_page(r, b.page, &flip(mark), mark.width, mark.height, x, y);
                    }
                }
            }
        }
        // the panels, channels 20…31 (0x4EAF75…0x4EB0A4)
        for n in 20..=31u8 {
            self.fe.room[1].1.set_channel_vec(n, [turn, 0.0, 0.0]);
        }
        use std::f32::consts::TAU;
        let place = |y: f32| Mat4::from_translation(Vec3::new(-pos[0], pos[1], pos[2])) * Mat4::from_rotation_x(xrot * TAU) * Mat4::from_rotation_y(-y * TAU);
        let mut items = self.fe.room[0].1.draw_items_block(place(yrot), [0.0; 4], [0.0; 2], [0.0; 4], &FRONT_END_PASSES, room_light);
        for k in 0..4 {
            items.extend(self.fe.room[1].1.draw_items_block(place(yrot + k as f32 * 0.25), [0.0; 4], [0.0; 2], [0.0; 4], &FRONT_END_PASSES, room_light));
        }
        Some(items)
    }

    /// The front end's own sprites: the previewed `.lay` screen over its background, the bottom bar
    /// and the port's key hint, or the text menu when no layout is loaded.
    pub(crate) fn fe_sprites(&self) -> Vec<Sprite> {
        match &self.fe.layout_preview {
            Some(l) => {
                let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(1.6);
                let controls = l.controls();
                // `state_for`'s `selected` parameter stands in for `[widget+0x10C] & 8`
                // (controls.md §4.109/§4.111), which the original sets only on the chronicle's
                // five model-group buttons (0x5077D0) — never from a keyboard cursor, since the
                // FE is pointer-only (§4.38). The port's own Up/Down stepping needs some index to
                // paint, but painting it before it has ever been used invents a highlight no
                // original screen shows at rest (2026-09-28: `selectname.lay`'s `создать игрока`
                // showed permanently selected/gold this way). `layout_nav_used` gates it to
                // "after the first Up/Down on this screen" — `layout_sel` itself still names
                // whichever control Enter would activate.
                let sel = controls
                    .get(self.fe.layout_sel.min(controls.len().saturating_sub(1)))
                    .map(|c| c.index)
                    .filter(|_| self.fe.layout_nav_used);
                let mut v = l.letterbox_bars(aspect);
                // 0x4DA890 before the frame's 2D: the kit's depth back to 0.99998 over a cleared
                // depth buffer, the alpha test as the last frame's room left it — every depth mask
                // of the frame (the background, the screens, the bar, a dialog) steps from here
                nfs_game::layout::fe_depth::begin_frame();
                // 0x4D2F30, the front end's draw: the one background `[fe+0xE4]` (the bottom
                // layer's `data.screen`, 0x4D30E7; see the draw below), the bottom bar's career era tabs (slot 41,
                // 0x4D30F2), then the screens bottom first — the covered ones under the one on
                // show. (the earlier premise below is WITHDRAWN (ed2b895d): 0x4D3040 reaches widget slot 26 = the stub 0x5258A0, nothing is drawn there; the covered car is the room-car feed above. Former text:) a covered screen's own 3D car
                // views are not rendered (the port renders the top screen's only). The original runs
                // **every** layer's screen's slot 0xA0 each frame (0x4D3040…0x4D3079, a loop over the
                // layer vector `[fe+0xD8]` with no cover test), so a car view under an overlay still
                // draws; the shipped overlay pairs with one are settings over QuickRace, VictoryMessage
                // over VictoryRoom and testdriveoval over dealership. Needs the covered screens'
                // widgets carried into `load_fe_room` / the view loop (their `fe.cars` slots collide
                // by widget index across layers) and an overlay flow to check it by eye.
                // 0x4C8760 / 0x4C8840: a widget whose screen is up takes the state of the first
                // layer's screen (0x4D63C0) and the larger progress of the two (0x4D6300) — the
                // bottom bar and the screens of the layers above the first
                let front = self.fe.layout_under.first().map_or(l, |(_, u)| u).transition();
                for (_, u) in self.fe.layout_under.iter().skip(1) {
                    u.take_front_transition(front);
                }
                if !self.fe.layout_under.is_empty() {
                    l.take_front_transition(front);
                }
                if let Some(b) = &self.fe.layout_bottom {
                    b.take_front_transition(front);
                }
                // the background object `[fe+0xE4]` is made from the **bottom** layer's `data.screen`
                // (0x4D2570 0x4D27B0…0x4D27D8 reads slot 0x84 of `[[[fe+0xD8]]]`; 0x4D1BD0 swaps it
                // only when that name differs), so an overlay keeps the covered screen's
                let base = self.fe.layout_under.first().map_or(l, |(_, u)| u);
                v.extend(base.background_sprites(aspect, &self.fe.strings, &|a| self.accessor_value(a)));
                v.extend(self.fe_career_tabs(aspect));
                // the first layer's screen (0x4D30F8…0x4D3114), then the bottom bar (slot 0x3C,
                // 0x4D3166, the bar made at 0x4D39D3), then the layers 1… (0x4D323E skips index
                // 0, 0x4D3250) — so a layer raised over the first covers the bar
                let screen = |v: &mut Vec<Sprite>, u: &nfs_game::layout::LayoutScreen, sel: Option<usize>| {
                    v.extend(u.sprites_own(aspect, &self.fe.strings, sel, &|a| self.accessor_value(a)));
                };
                match self.fe.layout_under.first() {
                    Some((_, u)) => screen(&mut v, u, None),
                    None => screen(&mut v, l, sel),
                }
                if let Some(b) = &self.fe.layout_bottom {
                    // the bar's slot 15 0x4D83B0: its oval flash (0x520B80), then its own parts
                    // (0x528760)
                    v.extend(b.bar_oval_sprites(aspect));
                    v.extend(
                        b.sprites(aspect, &self.fe.strings, None, &|a| self.accessor_value(a)),
                    );
                }
                if !self.fe.layout_under.is_empty() {
                    for (_, u) in self.fe.layout_under.iter().skip(1) {
                        screen(&mut v, u, None);
                    }
                    screen(&mut v, l, sel);
                }
                // a dialog over everything (§4.147)
                if let Some((d, _)) = &self.fe.dialog {
                    v.extend(l.dialog_sprites(d, aspect, &self.fe.strings, self.fe.mouse_down));
                }
                // the port's own key-hint line, a diagnostic the original has no counterpart
                // to: drawn only under `NFS_FE_HINTS` (decided 2026-09-27 — until
                // then it was always on, and the credits roll drew over it in the oval)
                if let Some(f) = self.fe.menu.as_ref().and_then(|m| m.font.as_ref()).filter(|_| nfs_game::config::cfg().nfs_fe_hints.is_set()) {
                    let name = self.fe.layout_name.clone().unwrap_or_default();
                    let hint = if l.can_play() {
                        "UP/DOWN SELECT   LEFT/RIGHT CHANGE   ENTER RACE   B BACK   ESC EXIT"
                    } else {
                        "UP/DOWN SELECT   LEFT/RIGHT CHANGE   ENTER FOLLOW   B BACK   ESC EXIT"
                    };
                    // above the bottom bar, not on it: the bar's four buttons occupy the bottom
                    // 24 of the 480 design rows (`bottombar.lay` section 7), and since the draw
                    // order was corrected (§4.67) they are actually drawn there. This line is the
                    // port's own key hint, not the original's.
                    v.extend(f.sprites(
                        &format!("{}   {hint}", name.to_string()),
                        [0.02, 0.915],
                        0.025,
                        [0.6, 0.6, 0.6, 0.9],
                    ));
                }
                v
            }
            None => {
                let mut v = self.fe.menu.as_ref().map(|m| m.sprites(self.career.as_ref())).unwrap_or_default();
                // no screen on show: 0x4D63C0 answers 2 (up) and 0x4D6300 0
                if let Some(b) = &self.fe.layout_bottom {
                    b.take_front_transition(nfs_game::layout::Transition::Idle);
                }
                // a dialog over the port's own text menu, painted by the bottom bar's screen —
                // the dialog belongs to no screen of its own (§4.147)
                if let (Some((d, _)), Some(b)) = (&self.fe.dialog, &self.fe.layout_bottom) {
                    let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(1.6);
                    v.extend(b.dialog_sprites(d, aspect, &self.fe.strings, self.fe.mouse_down));
                }
                v
            }
        }
    }

    /// The bottom bar's career era tabs, **0x4D83F0** (`RE/notes/career.md` §12): drawn only
    /// with a current screen `[[0x65B928]+0xE0]` (0x4D841B), `Game.GameType` 4 (0x4D8454) and the
    /// screen's slot 0xAC false (0x4D8461 — [`slot_ac`]). The era is `[0x691E50]` (0x5032D0) as
    /// 0…2, the year `[0x5DB764]` (0x5032C0); with no career loaded neither global was set
    /// by one, so no era is lit and no tab drawn. The front screen's progress is 0x4D6300's
    /// `0x4E0D90(0)` on the bottom layer's screen.
    fn fe_career_tabs(&self, aspect: f32) -> Vec<Sprite> {
        // slot 0xAC of `[[0x65B928]+0xE0]` (0x4D8461) — the callback list, the bottom layer's
        let (Some(l), Some(b)) = (self.fe_base_layout(), self.fe.layout_bottom.as_ref()) else { return Vec::new() };
        if self.fe.game_type != game_type::TOURNAMENT || slot_ac(&callback_list_of(l)) {
            return Vec::new();
        }
        let era = self.career.as_ref().map(|c| match c.career.era {
            nfs_formats::career::Era::Classic => 0,
            nfs_formats::career::Era::Golden => 1,
            nfs_formats::career::Era::Modern => 2,
        });
        let year = self.career.as_ref().map_or(0, |c| i32::from(c.career.year));
        let front = self.fe.layout_under.first().map_or(l, |(_, u)| u).transition().progress();
        b.career_tabs_sprites(aspect, &self.fe.strings, front, era, year)
    }

    /// The zone damages of the car a 3D widget's accessor holds (`FECar+0x6A8`, ten floats, the
    /// owned car's own) — the garage's selected car for `OwnedCars.Car`, the lot's for
    /// `UsedCar.Car`; nothing for a source without one.
    fn fe_car_damage(&self, accessor: Option<&str>) -> [f32; 10] {
        let Some(c) = self.career.as_ref() else { return [0.0; 10] };
        match accessor.map(|a| a.split('(').next().unwrap_or(a).trim()) {
            Some("OwnedCars.Car") => c.career.garage.get(c.car_sel).map(|o| o.damage).unwrap_or([0.0; 10]),
            Some("UsedCar.Car") => c.career.market.get(self.fe.usedcar_sel).map(|o| o.damage).unwrap_or([0.0; 10]),
            _ => [0.0; 10],
        }
    }
}
