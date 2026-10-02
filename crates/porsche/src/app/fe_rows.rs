//! The front end's list rows (`list_rows`, the parts rows) and the widget binders for them.

use crate::*;

impl App {
    /// The rows an `NFS5.ListBox`'s accessor hands out, and the row it has selected. The original
    /// asks the accessor for its count (vtable +0x24), its current index (+0x28) and each row's
    /// string (+0x30), and the base accessor draws that string as the whole row
    /// (`RE/notes/controls.md` §4). Only the lists this port keeps the contents of are answered;
    /// the ones whose accessor draws columns of its own (`Track.Records`, the `Parts.*` family) and
    /// the ones the port has no state for (`CDPlayer.*`, `Comms.*`) stay empty rather than invented.
    pub(crate) fn list_rows(&self, accessor: &str) -> Option<(Vec<ListRow>, Option<usize>)> {
        let base = accessor
            .split_once('(')
            .map(|(b, _)| b.trim())
            .unwrap_or(accessor);
        match base {
            // the four network layouts, filled from the `'d'` answers a `'w'` probe collects
            // `Comms.ServerList`'s row draw is 0x4B9780 (`network.md` §9): the row number
            // right-aligned 4 left of `ID_SEVERPOS` with the name 4 right of it, the players
            // (`[+0x48]`) at `ID_PLAYERPOS` and the ping (`min(ping, 9999)`, blank when negative)
            // at `ID_PINGPOS`; `ID_GAMEPOS`'s `[+0x3C] >> 16` is not something a `'d'` carries.
            "Comms.ServerList" => {
                let l = self.net.lobby.as_ref()?;
                let cell = |anchor: &str, dx: f32, text: String| ListCell { align: None, anchor: anchor.into(), dx, text, fade: 0xFF };
                let rows = l
                    .rows()
                    .into_iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let mut cells = vec![cell("ID_SEVERPOS", -4.0, format!("{}", i + 1)), cell("ID_SEVERPOS", 4.0, r.name), cell("ID_PLAYERPOS", 0.0, format!("{}", r.players))];
                        if let Some(p) = r.ping {
                            cells.push(cell("ID_PINGPOS", 0.0, format!("{p}")));
                        }
                        ListRow::Columns(cells)
                    })
                    .collect();
                Some((rows, Some(l.sel)))
            }
            // `Comms.GamesList` (`udpip` / `local`) — **the row draw is 0x4BA030** (`network.md`
            // §11.5), the columns by the layout's tags through 0x4D63E0 (`(tag, align, alpha, fmt,
            // value)` answers the tag's x, and the text goes at x − 4 right-aligned (align 2) or x
            // + 4 left-aligned (align 0)):
            //
            // | tag | text | source |
            // |---|---|---|
            // | `ID_GAMEPOS` | the row number `%d` right, then the name `%s` left | row + 1 (0x4BA11E); `[+0x4C]` (0x4BA14C) |
            // | `ID_PLAYERSPOS` | `%d` right, **`Comms.GameMode` = 1 only** (`local`) | `[+0x48]`, the room's player count (0x4BA1B7) |
            // | `ID_TRACKPOS` | with mode ≠ 1 (`udpip`) the player count `%d` right (0x4BA20C); with mode 1 the ping `%d` right when 0x48C6E0 is ≥ 0 (0x4BA1F5) | |
            // | `ID_TRACKPOS` | the track `%s` left | `FEStrings[desc word +0x26 + 0x28]` (0x4BA22D…0x4BA251) |
            // | `ID_MODEPOS` | `%s` left | `FEStrings[0x5B9674[desc byte +0x21]]` = 281 / 282 (0x4BA26A…0x4BA292) |
            // | `ID_VERPOS` | `%s` left | `[+0x8C]`, what follows the `~` of the name (0x4BA2AE) |
            //
            // The entry's fields are the `'d'`'s descriptor (`nfs_net::comm::desc`); a text id the
            // table lacks reads "Not Found" (0x4E073D → 0x5D9B38).
            "Comms.GamesList" => {
                let l = self.net.lobby.as_ref()?;
                let local = self.fe.comms_game_mode == 1;
                let text = |id: u32| self.fe.strings.get(id).map_or_else(|| "Not Found".to_string(), str::to_string);
                let cell = |anchor: &str, dx: f32, text: String| ListCell { align: None, anchor: anchor.into(), dx, text, fade: 0xFF };
                let rows = l
                    .rows()
                    .into_iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let mut cells = vec![cell("ID_GAMEPOS", -4.0, format!("{}", i + 1)), cell("ID_GAMEPOS", 4.0, r.name)];
                        if local {
                            cells.push(cell("ID_PLAYERSPOS", -4.0, format!("{}", r.players)));
                            if let Some(p) = r.ping {
                                cells.push(cell("ID_TRACKPOS", -4.0, format!("{p}")));
                            }
                        } else {
                            cells.push(cell("ID_TRACKPOS", -4.0, format!("{}", r.players)));
                        }
                        cells.push(cell("ID_TRACKPOS", 4.0, text(u32::from(r.track_id) + 0x28)));
                        // the table 0x5B9674 has two ids and then a 0 (0x4E06D7: id 0 is ""); a byte
                        // past that reads whatever follows in the image, which the port does not have
                        let mode = POSITIONAL_OPTIONS.get(usize::from(r.mode)).map_or_else(String::new, |id| text(*id));
                        cells.push(cell("ID_MODEPOS", 4.0, mode));
                        cells.push(cell("ID_VERPOS", 4.0, r.version));
                        ListRow::Columns(cells)
                    })
                    .collect();
                Some((rows, Some(l.sel)))
            }
            "Comms.IPList" => {
                let l = self.net.lobby.as_ref()?;
                Some((
                    l.addresses().into_iter().map(ListRow::Text).collect(),
                    Some(l.ip_sel),
                ))
            }
            "Comms.LobbyPlayerList" => {
                // 0x4BAD40 (the refill) lists `net+0x260` — the players of the connected NfsNet, only
                // while `net+8` is set and `+0xBF` clear (0x4BAD8A…0x4BADA1) — and nothing else feeds
                // it: no session, no rows (`network.md` §11.14). 0x4BAEF0 draws each row: the number
                // right at `ID_SMALLPLAYERNAMETHINGPOS − 4`, the name left at `+4`, the ping (when
                // ≥ 0, at most 9999) right at `ID_SMALLPINGTHINGPOS − 4`
                let cell = |anchor: &str, dx: f32, text: String| ListCell { anchor: anchor.into(), dx, text, fade: 0xFF, align: None };
                let rows = self
                    .net
                    .session
                    .as_ref()
                    .filter(|n| n.link_up())
                    .map(|n| {
                        n.room
                            .players
                            .iter()
                            .enumerate()
                            .map(|(i, p)| {
                                let name: String = p.name.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
                                let mut cells = vec![cell("ID_SMALLPLAYERNAMETHINGPOS", -4.0, format!("{}", i + 1)), cell("ID_SMALLPLAYERNAMETHINGPOS", 4.0, name)];
                                if let Some(ping) = n.ping_shown(p.id) {
                                    cells.push(cell("ID_SMALLPINGTHINGPOS", -4.0, format!("{ping}")));
                                }
                                ListRow::Columns(cells)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Some((rows, None))
            }
            // 0x4BBBD0: the room's players and "empty" (FE 287) to eight — eight "empty" rows
            // with no room (0x4BBDE8)
            "Comms.PlayerList" => self.net_player_rows(),
            "Comms.Positional" => {
                // with no session yet the answer is the accessor's own starting index, 1
                let on = match self.net.session.as_ref() {
                    Some(n) => n.positional,
                    None => self.net.lobby.as_ref().map_or(true, |l| l.positional),
                };
                let rows = POSITIONAL_OPTIONS
                    .iter()
                    .map(|id| ListRow::Text(self.fe.strings.get(*id).unwrap_or_default().to_string()))
                    .collect();
                Some((rows, Some(usize::from(on))))
            }
            // no modem in this port
            "Comms.ModemList" | "Comms.PhoneList" => Some((Vec::new(), None)),
            "LoopedTrack.Track" => {
                let m = self.fe.menu.as_ref()?;
                let l = m.looped_tracks();
                let cur = if m.looped_idx < l.len() { m.looped_idx } else { 0 };
                Some((l.iter().map(|t| ListRow::Text(self.track_full_name(t))).collect(), Some(cur)))
            }
            "Track1.Track" | "Knockout.Track" | "Track.Track" => {
                let m = self.fe.menu.as_ref()?;
                Some((
                    m.tracks
                        .iter()
                        .map(|t| ListRow::Text(self.track_full_name(t)))
                        .collect(),
                    Some(if base == "Knockout.Track" { m.ko_row(m.ko_sel).track_idx } else { m.track_idx }),
                ))
            }
            // `Track.Records` draws its row in four columns, each at the x of another widget of the
            // same screen (0x4FB2A0): the position number 10 px left of `ID_RECORDPLAYERPOS`, the
            // driver 4 px right of it, the car 4 px right of `ID_RECORDCARPOS` and the time 4 px
            // right of `ID_RECORDTIMEPOS`. The rows are the `nfs5.trk` slots of the selected track.
            "Record.Track" => self.record_track_rows(),
            // 0x4FB2A0 reads the records of `Game.GameType == 1 ? LoopedTrack.Track :
            // Track1.Track` (0x4FB2CB…0x4FB304) — no knockout arm, unlike `Record.Track`
            "Track.Records" => {
                let t = self
                    .fe.menu
                    .as_ref()
                    .map(|m| if self.fe.game_type == 1 { m.looped_track() } else { m.track() }.to_string())
                    .unwrap_or_else(|| self.track.name.clone());
                let rows = self
                    .records
                    .track(&t)?
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        ListRow::Columns(vec![
                            ListCell { align: None,
                                fade: 0xFF,
                                anchor: "ID_RECORDPLAYERPOS".into(),
                                dx: -10.0,
                                text: format!("{}", i + 1),
                            },
                            ListCell { align: None,
                                fade: 0xFF,
                                anchor: "ID_RECORDPLAYERPOS".into(),
                                dx: 4.0,
                                text: r.driver.clone(),
                            },
                            ListCell { align: None,
                                fade: 0xFF,
                                anchor: "ID_RECORDCARPOS".into(),
                                dx: 4.0,
                                text: r.car.clone(),
                            },
                            ListCell { align: None,
                                fade: 0xFF,
                                anchor: "ID_RECORDTIMEPOS".into(),
                                dx: 4.0,
                                text: format_ticks(r.time as u64),
                            },
                        ])
                    })
                    .collect();
                Some((rows, None))
            }
            // the CD player (`cdplayer.lay`). `CDPlayer.AvailableList` is the whole song table
            // `[0x65B628]` — 0x4ADE20 builds it from the streams matching `game*`, each record's
            // `+0x24` its stream index — row `i` its title `rec+0xC` (0x4BEBB0), all of them, the
            // ones already in the playlist too. `CDPlayer.SelectedList` is the playlist
            // `[0x65B62C]`, row `i` the title of table record `playlist[i]` (0x4BEC30).
            "CDPlayer.AvailableList" | "CDPlayer.SelectedList" => {
                let names = self.cd_names();
                let table = Self::cd_table(&names);
                let list = self.cd_playlist();
                let title = |n: &String| ListRow::Text(nfs_audio::music::split_title(n).0.to_string());
                let (rows, sel): (Vec<ListRow>, Option<usize>) = match base {
                    "CDPlayer.SelectedList" => (list.iter().filter_map(|i| table.get(*i)).map(|(_, n)| title(n)).collect(), self.fe.cd_selected.filter(|r| *r < list.len())),
                    _ => (table.iter().map(|(_, n)| title(n)).collect(), self.fe.cd_available.filter(|r| *r < table.len())),
                };
                Some((rows, sel))
            }
            "UserList.UserList" => {
                let profiles = nfs_game::save::list_profiles(&nfs_game::save::save_dir());
                let current = self.user_row(&profiles);
                Some((
                    profiles
                        .into_iter()
                        .map(|p| ListRow::Text(p.name))
                        .collect(),
                    current,
                ))
            }
            "Replay.List" => {
                let names = self.replay_names();
                let sel = (!names.is_empty()).then(|| self.fe.replay_sel.min(names.len() - 1));
                Some((names.into_iter().map(ListRow::Text).collect(), sel))
            }
            // the parts screens. Each row is the part's name and its price right-aligned four
            // pixels left of the widget named `ID_PRICEPOS`, and the first row of each category
            // carries the category's own heading (0x515F50 with `drawPartsHeading` 0x518DE0).
            // `Parts.AllParts` is what fits the selected car and the player's inventory,
            // `Parts.InstallList` is what is on the car, and `Parts.ShoppingList` is the shop's
            // basket (its `(TOTAL)` watch the basket's sum). `Parts.UsedDamageList` /
            // `Parts.OwnedDamageRepairList` are the installed parts again with their damage
            // percentage, and `Parts.RepairShoppingList` the advanced repair screen's basket of
            // parts to fix or replace (`parts_shop`, §4.160).
            // The five `Parts.*` lists differ only in which parts they hold and what the right-hand
            // column says (`controls.md` §…, row draws 0x515620 / 0x515F50 / 0x516960 / 0x516D70 /
            // 0x5170B0). The two damage lists hold **every installed part** — 0x516C60 counts
            // `feCar+0x758` outright — and print the part's own damage as `"%d %"` at
            // `ID_DAMAGEPOS + 3` (0x516EF6).
            "Parts.AllParts"
            | "Parts.InstallList"
            | "Parts.ShoppingList"
            | "Parts.UsedDamageList"
            | "Parts.OwnedDamageRepairList"
            | "Parts.RepairShoppingList" => {
                let c = self.career.as_ref()?;
                let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                let car = c.tables.car(c.career.garage.get(idx)?.car_id)?;
                let damage_list = matches!(
                    base,
                    "Parts.UsedDamageList"
                        | "Parts.OwnedDamageRepairList"
                        | "Parts.RepairShoppingList"
                );
                // `Parts.AllParts` is not a flat list: 0x5152E0 decodes a row as **0** a section
                // header, **1** a part of section one or **2** a part of section two, and
                // 0x5153A0 counts the list as **2** plus whichever section is open. Both flags
                // start **false** (0x515190, 0x515193), so the list opens as two header rows, and
                // clicking one flips it (0x51548B / 0x5154B2, a `sete`).
                //
                // The headers' text is 0x5157A1 / 0x5157C2: `FEStrings` **2550**,
                // "PARTS AVAILABLE FOR %s", with the car, and **2551**, "PARTS INVENTORY", with
                // nothing; the second section is the player's inventory (0x518B80). The shop's
                // own lists are `parts_shop`'s (§4.160).
                match base {
                    "Parts.AllParts" => {
                        let (rows, _) = parts_shop::PartsShop::all_parts_rows(c, &self.fe.strings, self.fe.parts_open)?;
                        return Some((rows, None));
                    }
                    "Parts.ShoppingList" => return Some((self.fe.shop.basket_rows(c, &self.fe.strings).0, self.fe.shop.basket_sel)),
                    "Parts.RepairShoppingList" => return Some((self.fe.shop.repair_rows(c, &self.fe.strings).0, self.fe.shop.repair_sel)),
                    "Parts.OwnedDamageRepairList" => return Some((parts_shop::PartsShop::owned_rows(c, &self.fe.strings).0, self.fe.shop.owned_sel)),
                    "Parts.InstallList" => return Some((parts_shop::PartsShop::install_rows(c, &self.fe.strings).0, self.fe.shop.install_sel)),
                    _ => {}
                }
                let parts: Vec<&nfs_formats::career::Part> = match base {
                    "Parts.UsedDamageList" => c.career.installed_parts(&c.tables, idx),
                    _ => c.career.parts_for(&c.tables, car),
                };
                Some((Self::parts_rows(c, idx, parts, &self.fe.strings, damage_list), None))
            }
            _ => None,
        }
    }

    /// Which row `Parts.AllParts`'s **second** section header sits on: 1 while the first section
    /// is closed, and `1 + <the first section's rows>` while it is open (0x5152E0's walk).
    pub(crate) fn parts_header_b(&self) -> usize {
        if !self.fe.parts_open[0] {
            return 1;
        }
        let Some(c) = self.career.as_ref() else { return 1 };
        parts_shop::PartsShop::all_parts_rows(c, &self.fe.strings, self.fe.parts_open)
            .and_then(|(_, kinds)| kinds.iter().skip(1).position(|k| *k == parts_shop::AllPartsRow::Header))
            .map_or(1, |p| p + 1)
    }

    /// One `Parts.*` row per part, grouped under the category headings every one of the five
    /// lists carries (`partCategoryString` 0x518D60, `drawPartsHeading` 0x518DE0). The right cell
    /// is the price, or the damage for the two damage lists. **The only live caller of this
    /// function passes `damage_list=true`** (`Parts.UsedDamageList`, the one `base` this file's
    /// match above does not already return early for) — and `Parts.UsedDamageList` shares its row
    /// draw, 0x516D70, with `Parts.OwnedDamageRepairList` (`controls.md`'s `NFS5.ListBox` note,
    /// the `Parts.*` row-draw table: both list `0x516C20`/`0x516D70`/`0x516F80`), which
    /// `grouped()`'s doc comment traces as the one row draw with **no** `"%s %s"`/family prefix at
    /// all. So this always passes `family_prefix = !damage_list` — `false` on `UsedDamageList`'s
    /// live path, matching that.
    fn parts_rows(
        c: &career_ui::CareerUi,
        idx: usize,
        parts: Vec<&nfs_formats::career::Part>,
        fe_strings: &nfs_formats::locale::Strings,
        damage_list: bool,
    ) -> Vec<ListRow> {
        // the headings go by category **string**, so 6 and 7 share one (`parts_shop::grouped`)
        let cells = parts.into_iter().map(|p| {
            let right = if damage_list {
                ListCell { align: None,
                    fade: 0xFF,
                    anchor: "ID_DAMAGEPOS".into(),
                    dx: 3.0,
                    text: format!("{} %", c.career.part_damage(idx, p) as i64),
                }
            } else {
                // 0x51590A: a part to buy is listed at `0x5181C0`, its aged price
                ListCell { align: None, fade: 0xFF, anchor: "ID_PRICEPOS".into(), dx: -4.0, text: format!("{}", c.career.part_buy_price(p)) }
            };
            (p, right)
        });
        parts_shop::grouped(cells, fe_strings, !damage_list).0
    }

    /// Fill every `NFS5.ListBox` on the previewed screen from its accessor.
    /// Put the ten `QUICKFIX.*` ticks on the screen's check boxes. `FECheckBox::refresh`
    /// 0x4C3730 asks the `boolAccessor` and picks `onIndex` or `offIndex` by the answer; this is
    /// that, with the answer coming from `repair_ticks`. §4.96.
    pub(crate) fn bind_checks(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let wanted: Vec<(usize, Option<bool>)> = l
            .check_boxes()
            .into_iter()
            .map(|(i, a)| {
                let on = a.strip_prefix("QUICKFIX.").map(|n| self.fe.repair_ticks.contains(n));
                (i, on)
            })
            .collect();
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (i, on) in wanted {
                l.set_check(i, on);
            }
        }
    }

    pub(crate) fn bind_lists(&mut self) {
        self.sync_track_lists();
        self.fe_owned_select();
        let Some(l) = self.fe.layout_preview.as_ref() else {
            return;
        };
        let wanted: Vec<(usize, Option<(Vec<ListRow>, Option<usize>)>)> = l
            .list_boxes()
            .into_iter()
            .map(|w| {
                (
                    w.index,
                    w.accessor.as_deref().and_then(|a| self.list_rows(a)),
                )
            })
            // a combo slider's drop-down is drawn by the very same code (§4.43), so it takes the
            // same binding — the options of the choice accessor it edits, and which one it is on
            .chain(l.combo_sliders().into_iter().map(|w| {
                (
                    w.index,
                    // …and when it is neither a choice nor one of the families `combo_rows`
                    // enumerates, the accessor is still an accessor: 0x4C66E8's loop asks it for
                    // its count and its rows exactly as a list box's does (§4.91), so the combo
                    // falls through to the same `list_rows` a list box would get. That is what
                    // puts the `Comms.*` lists into the combo on `netgamesetup.lay`.
                    w.accessor.as_deref().and_then(|a| self.choice_rows(a).or_else(|| self.combo_rows(a)).or_else(|| self.list_rows(a))),
                )
            }))
            .collect();
        // a colour box's closed face is its accessor's current colour (§4.171.4)
        let swatches: Vec<(usize, Option<[u8; 4]>)> =
            l.combo_sliders().into_iter().map(|w| (w.index, w.accessor.as_deref().and_then(|a| self.colour_value(a)))).collect();
        // the chat boxes take the chat's lines (`netplay::NetPlay::chat_rows`), and one being on
        // screen is what sets the room's chat callback (0x4C2940 / 0x4C2A80)
        let chat_boxes = l.chat_boxes();
        let chat = self.net.session.as_mut().map(|n| {
            n.chat_box(!chat_boxes.is_empty());
            let fe = |id: u32| self.fe.strings.get(id).unwrap_or_default().to_string();
            n.chat_rows(&fe(287), &fe(580), &fe(581))
        });
        let Some(l) = self.fe.layout_preview.as_mut() else {
            return;
        };
        for (index, rows) in wanted {
            match rows {
                Some((rows, current)) => l.set_list_rows(index, rows, current),
                None => l.set_list_rows(index, Vec::new(), None),
            }
        }
        for (index, colour) in swatches {
            l.set_swatch(index, colour);
        }
        for index in chat_boxes {
            l.set_chat_lines(index, chat.as_deref().unwrap_or_default());
        }
    }

    /// Point every `NFS5.CarLogo` on the previewed screen at the car its source names. The pack and
    /// the entry pair are the car record's own +0x147 and +0x1F6 (`RE/notes/layouts.md` §4).
    pub(crate) fn bind_car_logos(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else {
            return;
        };
        let wanted: Vec<(usize, Option<(String, i16)>)> = l
            .car_logos()
            .into_iter()
            .map(|w| {
                // `othersource=Chron` is a chronicle picture, not a car logo
                if w.othersource.as_deref() == Some("Chron") {
                    return (w.index, None);
                }
                let car = self.logo_car(w.accessor.as_deref(), w.othersource.as_deref());
                (w.index, car.map(|c| (c.logo.clone(), c.logo_pair)))
            })
            .collect();
        // the `NFS5.ChroniclePicture` widgets of `chroniclepicture.lay` and `chroniclemedia.lay`
        let chron: Vec<usize> = l.numbered_widgets("NFS5.ChroniclePicture").into_iter().map(|(i, _)| i).collect();
        // a `NFS5.CarLogo` with `othersource=Chron` is **still a car logo** — 0x4C1747 takes entry
        // 1 of the chronicle pack (its `ci` icon) and reads the string its `0x6F` attachment
        // carries, which is a two-digit logo pair followed by the logo pack's name
        // (`MA601a.fsh` says `04finallogos2`). It draws nothing when that string is shorter than
        // three characters (0x4C1763).
        let chron_logo: Vec<usize> = l.car_logos().into_iter().filter(|w| w.othersource.as_deref() == Some("Chron")).map(|w| w.index).collect();
        let picture = (!chron.is_empty() || !chron_logo.is_empty())
            .then(|| self.chronicle_picture())
            .flatten();
        let chron_pair = picture.as_deref().filter(|_| !chron_logo.is_empty()).and_then(|f| self.chronicle_logo(f));
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else {
            return;
        };
        for (index, car) in wanted {
            match car {
                Some((pack, pair)) => l.set_car_logo(r, &self.paths, index, &pack, pair),
                None => l.set_car_logo(r, &self.paths, index, "", -1),
            }
        }
        for index in chron {
            l.set_chronicle_picture(r, &self.paths, index, picture.as_deref().unwrap_or(""));
        }
        for index in chron_logo {
            match &chron_pair {
                Some((pack, pair)) => l.set_car_logo(r, &self.paths, index, pack, *pair),
                None => l.set_car_logo(r, &self.paths, index, "", -1),
            }
        }
    }

    /// The car-picking screens show **one of two** widgets at one place: the garage button or the
    /// colour box. 0x50CC85:
    ///
    /// ```text
    /// 0x50CC8C  owned = 0x4F4DF0([this+0x280])                       ; the merged list accessor
    /// 0x50CCA5  0x50D910("ID_MERGEDLISTCOLOR",    owned,  0)         ; hide the paint box
    /// 0x50CCB9  0x50D910("ID_MERGEDLISTCARSETUP", !owned, 0)         ; hide the garage button
    /// ```
    ///
    /// and 0x4F4DF0 is three instructions of meaning:
    ///
    /// ```text
    /// 0x4F4DF9  n   = [acc+0x28]->slot9()   ; the sub-accessor's row count
    /// 0x4F4E02  cur = acc->slot10()         ; the merged list's own index
    /// 0x4F4E08  setl                        ; cur < n
    /// ```
    ///
    /// `FEMergedCarListAccessor` puts the player's **owned** cars first, and 0x4EB88F proves what
    /// the test means: when it is true 0x4EB89C takes an existing record through `[vtable+0x8C]`,
    /// and when it is false 0x4EB8B4 **allocates a fresh 0x760-byte car**. So the two widgets are
    /// "this is a car you own — take it to the garage" against "this is one you do not — here is
    /// its paint". Five screens carry the pair, at the same `pos` in each: `QuickRace`,
    /// `creategame`, `joingame`, `knockout` and `quickknockout`. §4.115.
    ///
    /// *(Correction, 2026-09-27: the port's merged list now keeps this same index — `car_garage`
    /// — so `owned` below is 0x4F4DF0's own `cur < n` directly, not the id-match this used to
    /// stand in with; the two can differ when the player owns a car of the same model as the
    /// catalogue row they currently have picked.)*
    pub(crate) fn bind_mergedlist_panel(&mut self) {
        self.sync_merged_zero();
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let names = ["ID_MERGEDLISTCARSETUP", "ID_MERGEDLISTCOLOR"];
        let want = l.widgets_named(&names);
        if want.is_empty() {
            return;
        }
        let owned = self.fe.menu.as_ref().is_some_and(|m| m.car_garage);
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                l.set_hidden_flag(index, if which == 0 { !owned } else { owned });
            }
        }
    }

    /// **`zMergedList.Car` at its initial index 0** (`Menu::merged_at_zero`): 0x4F4DF0's `cur < n`
    /// with `cur = 0` is true exactly when list A (`OwnedCars.Car`'s list, 0x4F4DF9 `slot9`) is not
    /// empty, so with a garage the merged list is on the owned car (`гараж...`, no colour row) and
    /// without one on catalogue row 0. Re-evaluated every frame like the original's test (the
    /// profile load, the game type's list rebuild and the chronicle year change A's count).
    pub(crate) fn sync_merged_zero(&mut self) {
        if !self.fe.menu.as_ref().is_some_and(|m| m.merged_at_zero) {
            return;
        }
        let owned = !self.owned_list().is_empty();
        if let Some(m) = self.fe.menu.as_mut() {
            m.car_garage = owned;
        }
    }

    /// `parts.lay`, `advancedrepair.lay` or `carsetup.lay` is up in a career — the screens
    /// `parts_shop` serves.
    pub(crate) fn on_parts_shop(&self) -> bool {
        self.career.is_some() && self.fe.layout_name.as_deref().is_some_and(|n| ["parts", "advancedrepair", "carsetup"].iter().any(|s| n.eq_ignore_ascii_case(s)))
    }

    /// The two shop screens' ticks (0x519410, 0x5190B0) — which of their buttons are live — and
    /// their constructors' basket reset for another car (0x519357, 0x518FFA). §4.160.
    fn bind_parts_shop(&mut self) {
        if !self.on_parts_shop() {
            return;
        }
        if let (Some(cu), Some(l)) = (self.career.as_ref(), self.fe.layout_preview.as_mut()) {
            self.fe.shop.sync_car(cu);
            self.fe.shop.bind_buttons(cu, &self.fe.strings, self.fe.parts_open, self.fe.parts_sel, l);
        }
    }

    /// `ID_BUYPARTSCBL`'s and `IDS_ADVREPAIRCBL`'s callbacks (0x519250, 0x518F00): `true` when
    /// `cb` was one of theirs. A short purchase raises the one-button 810. §4.160.
    pub(crate) fn parts_shop_callback(&mut self, cb: &str) -> bool {
        let Some(cu) = self.career.as_mut() else { return false };
        let mut sel = self.fe.parts_sel;
        let Some(out) = self.fe.shop.callback(cu, &self.fe.strings, self.fe.parts_open, &mut sel, cb) else { return false };
        self.fe.parts_sel = sel;
        if out == parts_shop::Outcome::NoMoney {
            self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 810, [0, 0], DialogFor::Notice);
        }
        self.bind_lists();
        self.bind_parts_buttons();
        true
    }

    /// `parts.lay`'s install and uninstall buttons are **one of two**, and both go away when no
    /// part is picked. The screen's refresh 0x4F1AC0:
    ///
    /// ```text
    /// 0x4F1ADE  [this+0x288]->slot27(&picked, &installed)     ; two out-parameters
    /// 0x4F1AED  0x50D880("ID_PARTSDESC", picked != 0, 0)      ; enable the description
    /// 0x4F1AFD  picked == 0 -> 0x4F1B47
    ///           set: 0x50D880("ID_UNINSTALLBUTTON", 1)        ; enable it
    /// 0x4F1B29       0x50D910("ID_INSTALLBUTTON",   installed)  ; hide when it is fitted
    /// 0x4F1B40       0x50D910("ID_UNINSTALLBUTTON", !installed) ; hide when it is not
    /// 0x4F1B47  clear: 0x50D910("ID_INSTALLBUTTON", 1)        ; hide install
    /// 0x4F1B6C         0x50D910("ID_UNINSTALLBUTTON", 0)      ; show uninstall
    /// 0x4F1B7B         0x50D880("ID_UNINSTALLBUTTON", 0)      ; and disable it
    /// ```
    ///
    /// so the three states are: **nothing picked** — description disabled, install hidden,
    /// uninstall shown but dead; **a part that is not fitted** — install shown, uninstall hidden;
    /// **a part that is fitted** — install hidden, uninstall shown and live. §4.114.
    ///
    /// `[this+0x288]` is **`Parts.InstallList`** (the constructor 0x4F18D0 binds it at 0x4F198B),
    /// whose slot 27 is 0x516BD0 — the part its current row is on and the row's own `[+4]`
    /// "installed" byte. *(Corrected 2026-09-25: this read `Parts.AllParts`, which `carsetup.lay`
    /// does not carry, so nothing was ever picked here.)* §4.160.
    pub(crate) fn bind_parts_buttons(&mut self) {
        self.bind_parts_shop();
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let names = ["ID_PARTSDESC", "ID_INSTALLBUTTON", "ID_UNINSTALLBUTTON"];
        let want = l.widgets_named(&names);
        if want.is_empty() {
            return;
        }
        let picked = self.parts_picked();
        // (hidden, enabled) for the three, in the order of `names`
        let state: [(bool, Option<bool>); 3] = match picked {
            None => [(false, Some(false)), (true, None), (false, Some(false))],
            Some(false) => [(false, Some(true)), (false, None), (true, Some(true))],
            Some(true) => [(false, Some(true)), (true, None), (false, Some(true))],
        };
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                let (hide, enable) = state[which];
                l.set_hidden_flag(index, hide);
                if let Some(en) = enable {
                    l.set_disabled_flag(index, !en);
                }
            }
        }
    }

    /// Whether the part `Parts.InstallList` is on is **fitted** — `None` when nothing is picked
    /// (0x516BD0 answers a null part without a current row, 0x516BE5).
    fn parts_picked(&self) -> Option<bool> {
        let c = self.career.as_ref()?;
        Some(matches!(self.fe.shop.install_pick(c, &self.fe.strings)?, nfs_game::shop::InstallRow::Fitted(_)))
    }

    /// `usedcars.lay` shows **one of two panels**, and disables its three buttons when the market
    /// is empty. `FEUsedCarsScreen`'s refresh, 0x4F7384 onwards:
    ///
    /// ```text
    /// 0x4F7394  n = the used-car list accessor's count (slot 9)
    /// 0x4F7399  n != 0 -> 0x4F73E3
    ///           n == 0: hide ID_SHOWDAMAGEBUTTON, hide ID_SHOWPARTSBUTTON,
    ///                   0x50D880 on ID_SHOWPARTSBUTTON and ID_TESTDRIVEUSEDBUTTON,
    ///                   then `jmp 0x4F74BA` — disable ID_BUYUSEDBUTTON
    /// 0x4F73E3  showparts = [this+0x290]
    ///           set: hide ID_SHOWPARTSBUTTON, hide ID_USEDCARDAMAGEPANEL,
    ///                show ID_PARTSPANEL, show ID_PARTSBOX, show ID_SHOWDAMAGEBUTTON,
    ///                then `jmp 0x4F749F` — enable ID_SHOWDAMAGEBUTTON
    /// 0x4F7448  clear: show ID_SHOWPARTSBUTTON, show ID_USEDCARDAMAGEPANEL,
    ///                hide ID_PARTSPANEL, hide ID_PARTSBOX, hide ID_SHOWDAMAGEBUTTON,
    ///                enable ID_SHOWPARTSBUTTON
    /// 0x4F74A6  and both arms end: enable ID_TESTDRIVEUSEDBUTTON, enable ID_BUYUSEDBUTTON
    /// ```
    ///
    /// `[this+0x290]` is flipped by the two callbacks the constructor registers at 0x4F71C0 and
    /// 0x4F71D1 — `SHOWPARTS` (0x4F76A0) and `SHOWDAMAGE` (0x4F76B0) — which are the `callback`
    /// lines on `ID_SHOWPARTSBUTTON` and `ID_SHOWDAMAGEBUTTON`. The constructor itself
    /// (0x4F71E2…0x4F726D) leaves the screen on the **damage** panel with `ID_Used3DCar` and
    /// `ID_GREYRECT` hidden. §4.113.
    pub(crate) fn bind_usedcars_panels(&mut self) {
        const SHOW: &str = "ID_SHOWPARTSBUTTON";
        const DAMAGE: &str = "ID_SHOWDAMAGEBUTTON";
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let names = [SHOW, DAMAGE, "ID_USEDCARDAMAGEPANEL", "ID_PARTSPANEL", "ID_PARTSBOX", "ID_Used3DCar", "ID_GREYRECT", "ID_TESTDRIVEUSEDBUTTON", "ID_BUYUSEDBUTTON"];
        let want = l.widgets_named(&names);
        if want.is_empty() {
            return;
        }
        let n = self.used_cars().len();
        let parts = self.fe.usedcars_showparts;
        // (hidden, enabled) per name, in the order of `names`
        let state: [(bool, Option<bool>); 9] = if n == 0 {
            [(true, Some(false)), (true, None), (false, None), (true, None), (true, None), (true, None), (true, None), (false, Some(false)), (false, Some(false))]
        } else if parts {
            [(true, None), (false, Some(true)), (true, None), (false, None), (false, None), (true, None), (true, None), (false, Some(true)), (false, Some(true))]
        } else {
            [(false, Some(true)), (true, None), (false, None), (true, None), (true, None), (true, None), (true, None), (false, Some(true)), (false, Some(true))]
        };
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                let (hide, enable) = state[which];
                l.set_hidden_flag(index, hide);
                if let Some(en) = enable {
                    l.set_disabled_flag(index, !en);
                }
            }
        }
    }

    /// **`garage.lay`'s buttons, from the garage screen's tick 0x50F970** (§4.112): which are
    /// enabled (0x50D880) and which of the two that share a place is shown (0x50D910).
    ///
    /// ```text
    /// 0x50F98B  car = [this+0x2D4]->slot35()       ; the garage car on show, or null
    /// 0x50F997  listed = car[+0x691]               ; on the used-car market
    /// 0x50F9A7  ID_CHOOSECARBOX, ID_CARSETUP, ID_PAINTBUTTON, ID_SETTINGSBUTTON  enabled = car
    /// 0x50FA1A  career = Game.GameType == 4
    /// 0x50FA2B  ID_USEDCARSBUTTON, ID_NEWCARSBUTTON                              enabled = career
    /// 0x50FA65  ID_PARTSBUTTON, ID_REPAIRBUTTONGARAGE                            enabled = car && career
    /// 0x50FA9A  career:  ID_REMOVEFMBUTTON hidden = !(car && listed)
    ///                    ID_SELLCARBUTTON  enabled = car, hidden = car && listed
    /// 0x50FAF9  else:    ID_REMOVEFMBUTTON hidden; ID_SELLCARBUTTON disabled, shown
    /// ```
    ///
    /// "Game type 4" is the port's career (`car_settings_delegate` reads it the same way), and the
    /// car on show is the career's selected garage car.
    pub(crate) fn bind_garage(&mut self) {
        const NAMES: [&str; 10] = [
            "ID_CHOOSECARBOX",
            "ID_CARSETUP",
            "ID_PAINTBUTTON",
            "ID_SETTINGSBUTTON",
            "ID_USEDCARSBUTTON",
            "ID_NEWCARSBUTTON",
            "ID_PARTSBUTTON",
            "ID_REPAIRBUTTONGARAGE",
            "ID_REMOVEFMBUTTON",
            "ID_SELLCARBUTTON",
        ];
        if self.fe.layout_name.as_deref() != Some("garage") {
            return;
        }
        let career = self.career.is_some() && self.mission.is_none();
        let owned = self.career.as_ref().and_then(|c| c.career.garage.get(c.car_sel));
        let car = owned.is_some();
        let listed = owned.is_some_and(|c| c.asking_price.is_some());
        // (hidden, enabled) per name, in the order of `NAMES`
        let state: [(Option<bool>, bool); 10] = [
            (None, car),
            (None, car),
            (None, car),
            (None, car),
            (None, career),
            (None, career),
            (None, car && career),
            (None, car && career),
            (Some(!(career && car && listed)), true),
            (Some(career && car && listed), career && car),
        ];
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        for (index, which) in l.widgets_named(&NAMES) {
            let (hide, enable) = state[which];
            if let Some(h) = hide {
                l.set_hidden_flag(index, h);
            }
            l.set_disabled_flag(index, !enable);
        }
    }

    /// `carsetup.lay`'s gear-ratio adjuster (`ID_GEARRATIOPANEL`, embedding `gearratio.lay`'s
    /// eight `Gear0`…`Gear7` `−`/`+`/digit rows) is **race-class cars only**. The screen's tick,
    /// **0x4F1AC0** (`career.md` §11.2c/§9's own citation for this function), reaches this gate
    /// at 0x4F1B80 right after the install/uninstall button state at its head:
    ///
    /// ```text
    /// 0x4F1B80  ecx = this->car                        ; [this+0x284]
    /// 0x4F1B86  eax = 0x529FD0(ecx)                     ; the car's `nfs5.car` catalogue record
    /// 0x4F1B8B  cmp dword ptr [eax+0x1AC], 0x1F8        ; record.class_string_id == 504
    /// 0x4F1B95  jne 0x4F1BF3                            ; not RACE CLASS -> leave unloaded
    /// 0x4F1B97  if [this+0x294] (cached panel) != 0: skip (already loaded)
    /// 0x4F1BA1  push "ID_GEARRATIOPANEL"; 0x4D1A60 / 0x528B80 resolve it, dynamic_cast to
    ///           NFS5.PANEL (0x528FE0/0x5A134B), cache at [this+0x294]
    /// 0x4F1BE5  call [panel_vtable+0x6C]                ; panel->open() (0x4CEA90)
    /// 0x4F1BEE  call 0x4CEC60(panel)                    ; panel->show()
    /// ```
    ///
    /// `+0x1AC` is `CarCatalogEntry::class_string_id` (`nfs-formats/src/fedata.rs`'s own
    /// confirmed offset: 501 CLASS 3, 502 CLASS 2, 503 CLASS 1, **504 RACE CLASS**). A panel with
    /// `filename=` never draws before this open+show runs — `draw_panel`'s own comment notes
    /// `[panel+0x14C]` stays null until then — so for every car in classes 501/502/503 (every
    /// dealer/garage car in the shipped `nfs5.car`, 88 of 93 racing records; only the five
    /// factory race cars — `'56 550 A Spyder`, `'78 935/78 "Moby Dick"`, `'98 911 GT1`, `'97 911
    /// GT2`, `'98 911 GT3 Cup` — carry 504) the panel is simply never populated: `carsetup.lay`'s
    /// eight `ID_OUTLINERECTANGLE` boxes behind it (§20…§27) are all that ever draws for
    /// "ПЕРЕДАТОЧНЫЕ ЧИСЛА" — a bare box, no `−`/`+`, no digit — exactly `cmp_carsetup.jpg`'s
    /// original side for the reference `'50 356 1100 Cabriolet` (class 501). The port always
    /// loaded and drew `gearratio.lay`'s content regardless of the car's class, so a 356's stock
    /// gearbox showed a live `0.00` slider on every row where the original leaves the row bare.
    /// `carsetup.lay` in fact carries **two** `ID_GEARRATIOPANEL`s (§19 `state=normal` 70×110,
    /// §77 `state=hidden` 244×110) — the port's generic `state=hidden` handling already
    /// suppresses §77 on its own; this gate is what was missing for §19.
    pub(crate) fn bind_carsetup_gearratio(&mut self) {
        if self.fe.layout_name.as_deref() != Some("carsetup") {
            return;
        }
        let race_class = self.garage_car().is_some_and(|(e, _)| e.class_string_id == 504);
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        for (index, _) in l.widgets_named(&["ID_GEARRATIOPANEL"]) {
            l.set_hidden_flag(index, !race_class);
        }
    }

    /// `location.lay`/`locationsetup.lay`'s `круги` row, `ID_LAPSSELECTOR`, is **disabled** on a
    /// point-to-point track (there is no lap count to choose — `fe_laps` already forces `NUM_LAPS`
    /// to 1 for one). The shared screen-refresh helper **0x50CC30** (also `carsetup.lay`'s own
    /// tick, 0x4F1AC0, calls it — 12 direct callers in all) ends:
    ///
    /// ```text
    /// 0x50cd45  edi = Game.GameType's own current selection (0 single race, 2 knockout, …)
    /// 0x50ce36  cmp edi, 2 ; jne 0x50ce64            ; not Knockout — idx stays 0 (xor eax,eax)
    /// 0x50ce3b  …          idx = Knockout.TrackNumber's current selection (Knockout only)
    /// 0x50ce64  push eax(idx) ; push edi(mode) ; call 0x4FBF70   ; Mode_isCircuit(mode, idx)
    /// 0x50ce7b  push 0x5dc3c8("ID_LAPSSELECTOR") ; call 0x50D880(name, circuit, 0)  ; the same
    ///           enable/disable setter `bind_garage`/`bind_usedcars_panels` already call
    /// ```
    ///
    /// `Mode_isCircuit(mode, idx)` reads `Track1` for mode 0 (`location.lay`'s own row) and
    /// `Track<idx+1>` for mode 2 (`locationsetup.lay`'s currently highlighted round) — modes 1/3
    /// never reach here with the widget present (`locationQK.lay` carries no `ID_LAPSSELECTOR` at
    /// all, confirmed live on both sides, pass 10). Ported keyed on the same `game_type`/`ko_sel`
    /// rows `fe_laps`/`accessors.rs`'s `Knockout.TrackNumber(nTRACKLAPS)` arm already use for
    /// `NUM_LAPS` and the round table's own laps cell.
    pub(crate) fn bind_laps_selector(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let Some((index, _)) = l.widgets_named(&["ID_LAPSSELECTOR"]).into_iter().next() else { return };
        let Some(m) = self.fe.menu.as_ref() else { return };
        let idx = if self.fe.game_type == 2 { m.ko_sel } else { 0 };
        let track = m.ko_track(idx).to_string();
        let circuit = self.records.track(&track).is_some_and(|t| t.circuit);
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        l.set_disabled_flag(index, !circuit);
    }

    /// `repair.lay`'s ten damage bars are **hidden** when their category has nothing in it.
    ///
    /// 0x50F4AD…0x50F609 is ten copies of one block:
    ///
    /// ```text
    /// 0x50F4BC  ok = [this + 0x284 + 4k]->slot27(&[this + 0x2AC + 4k])   ; the category accessor
    /// 0x50F4C1  sete al                                                  ; hide = !ok
    /// 0x50F4CC  0x50D910(screen, "ID_BAR<K>", hide)
    /// ```
    ///
    /// in the order ENGINE, BRAKES, TRANSMISSION, SUSPENSION, SHOCKS, BODYWORK, SWAYBARS, TIRES,
    /// COSMETIC, OVERALL — the same order as [`QUICKFIX_NAMES`]. So a category the car has no
    /// part of does not draw an empty bar in the original: **the bar is not there at all**. This
    /// port had been drawing a full-length one for `BODYWORK` on a 356, which has no part of
    /// category 6 — the one row `RE/AUDIT.md` §10 recorded as "blank with state". §4.112.
    ///
    /// `OVERALL` is the tenth and is never hidden here: 0x4EF960 combines the other categories
    /// itself and always answers.
    pub(crate) fn bind_repair_bars(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let names: Vec<String> = QUICKFIX_NAMES.iter().map(|k| format!("ID_BAR{k}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let want = l.widgets_named(&refs);
        if want.is_empty() {
            return;
        }
        let has: Vec<bool> = QUICKFIX_NAMES
            .iter()
            .map(|k| *k == "OVERALL" || self.category_has_parts(*k))
            .collect();
        tracing::debug!(hidden = ?QUICKFIX_NAMES.iter().zip(&has).filter(|(_, h)| !**h).map(|(k, _)| *k).collect::<Vec<_>>(), "repair bars");
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                l.set_hidden_flag(index, !has[which]);
            }
        }
    }

    /// Whether the garage car has any installed part of the category `<K>DAMAGE` names — what
    /// decides whether 0x50F4BC's accessor answers at all (§4.112).
    fn category_has_parts(&self, key: &str) -> bool {
        let Some(c) = self.career.as_ref() else { return true };
        let name = format!("{key}DAMAGE");
        let Some(cat) = PART_DAMAGE_CATEGORY.iter().find(|(n, _)| *n == name).map(|(_, c)| *c) else { return true };
        let idx = c.car_sel.min(c.career.garage.len().saturating_sub(1));
        let Some(owned) = c.career.garage.get(idx) else { return true };
        owned.part_state.iter().any(|(id, _)| c.tables.parts.iter().any(|p| p.id == *id && p.kind == cat))
    }
}
