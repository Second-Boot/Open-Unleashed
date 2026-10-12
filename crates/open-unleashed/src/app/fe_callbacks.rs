//! The player-screen callbacks.

use crate::*;

impl App {
    /// The player screens' callbacks — the front-end controller's slots 0xDC…0xE8 and 0x108
    /// (vtable 0x5B9D64), registered by name at 0x50FF40 (§4.147). Returns whether `cb` was one.
    ///
    /// * `REMOVEPLAYER` (0x510770) raises 803; its answer is `DialogFor::RemovePlayer`.
    /// * `SETNAMETHENBACK` (0x5106C0): with a row selected, `LoadInfo(stem, 0)`; loaded, the
    ///   knockout is cancelled (0x4DE040) and the screen goes back (slot 0xB4 = 0x4D23F0(0)),
    ///   otherwise 806.
    /// * `CREATEPLAYERDONE` (0x5107A0): the name box's text; nothing when empty (0x5107F9); 801
    ///   when the name is taken (0x4DE510); 805 when the profile cannot be made (0x4DE5B0 with
    ///   the picture `[0x5D8F2C]`); then `LoadInfo` of the new stem — 806 on failure — and the
    ///   whole layer closes, 0x4D23F0(−1), with sound 4 (0x4D252B).
    /// * `CANCELCREATEPLAYER` (0x510AE0): slot 0xB4, back one history entry (0x4D5FB0).
    ///
    /// Not ported: 0x4D8680, which first tries the name as one of the seven cheat codes of the
    /// table at 0x5D9618 (sound 0xC and a bit of `[0x65B940]` when it matches); 0x4F3E40, the
    /// `DealerCar` accessors' reset `SETNAMETHENBACK` makes before loading. Each path ends in
    /// 0x505570 (`fe_tournament_refresh`). The picture is `[0x5D8F2C]` (`App::fe`'s `driver_picture`), which the
    /// eight `NFS5.PLAYERDRIVERPICTURE`s set (0x4CF940) and `CREATEPLAYERENTER` resets to 8.
    pub(crate) fn player_callback(&mut self, cb: &str, event_loop: &ActiveEventLoop) -> bool {
        use nfs_game::dialog::DialogKind;
        let dir = nfs_game::save::save_dir();
        match cb {
            "REMOVEPLAYER" => {
                self.fe_raise_dialog(DialogKind::Two, 803, [0, 0], DialogFor::RemovePlayer);
            }
            "SETNAMETHENBACK" => {
                let profiles = nfs_game::save::list_profiles(&dir);
                // 0x5106D0: no row selected goes straight to 0x505570 (0x510762)
                let Some(p) = self.user_row(&profiles).and_then(|i| profiles.get(i)).cloned() else {
                    self.fe_tournament_refresh();
                    return true;
                };
                // 0x5106D7: 0x4F3E40 before the load, whose 0x4FBC10 rebuilds the track lists for
                // the profile being left; `LoadInfo` then rebuilds them for the loaded one
                // (`apply_saved_profile`), so only a failed load (806) keeps this one
                self.car_lists_head();
                self.rebuild_track_lists();
                if self.career.as_mut().is_some_and(|c| c.switch_profile(&p)) {
                    self.fe.user_sel = None;
                    self.apply_saved_profile();
                    // 0x4DE040 after the load (0x510701): a series the file carried is cancelled here —
                    // only the start-up and after-race loads (0x4D3B52) keep it running
                    if let Some(k) = self.knockout.as_mut() {
                        k.state.active = false;
                    }
                    self.fe_back_callback(event_loop);
                } else {
                    self.fe_raise_dialog(DialogKind::One, 806, [0, 0], DialogFor::Notice);
                }
                // 0x510715 / 0x51074D: both ends run 0x505570
                self.fe_tournament_refresh();
            }
            "CREATEPLAYERDONE" => {
                let name = self.fe.layout_preview.as_ref().map(|l| l.input_text("UserList.NameInput")).unwrap_or_default();
                if name.is_empty() {
                    return true;
                }
                // 0x510804: a cheat code first — sound 0xC, its bit of `[0x65B940]` set, and the
                // name box emptied (0x510818) instead of a profile made
                if let Some(id) = nfs_game::cheats::code_of(&name) {
                    self.fe_sound(0xC);
                    // id 4 would call 0x5012E0(−1) instead; no shipped code carries it
                    if id != 4 {
                        self.cheat_bits |= 1 << id;
                    }
                    tracing::info!(id, "cheat code");
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.clear_input("UserList.NameInput");
                    }
                    self.fe_tournament_refresh(); // 0x51081B
                    return true;
                }
                if nfs_game::save::name_taken(&dir, &name) {
                    self.fe_raise_dialog(DialogKind::One, 801, [0, 0], DialogFor::Notice);
                    self.fe_tournament_refresh(); // 0x510868
                    return true;
                }
                // 0x510874: `0x4CF780()` — `[0x5D8F2C]` — is the picture 0x4DE5B0 gets; it lands in
                // `[0x5DB748]` (0x4DE82F → 0x5024F0), the file name's digit and `PlayerInfo`'s field
                let picture = self.fe.driver_picture.clamp(0, 9) as u8;
                let template = self.fe.xxdefxx.clone().unwrap_or_default();
                let made = self.career.as_mut().is_some_and(|c| c.create_profile_from(&name, picture, &template));
                if !made {
                    self.fe_raise_dialog(DialogKind::One, 805, [0, 0], DialogFor::Notice);
                    self.fe_tournament_refresh(); // 0x510961
                    return true;
                }
                self.fe.user_sel = None;
                let p = nfs_game::save::Profile::new(name, picture);
                if self.career.as_mut().is_some_and(|c| c.switch_profile(&p)) {
                    // `LoadInfo(stem, 1)`: the new file's modules and accessors — `XXDefXX`'s,
                    // the start-up options — onto the live state, as every load does, then
                    // 0x4DE040 (the series cancelled). *(Until 2026-09-26 the options of the
                    // profile that was up stayed.)*
                    self.apply_saved_profile();
                    if let Some(k) = self.knockout.as_mut() {
                        k.state.active = false;
                    }
                    // 0x4D23F0(−1): the layer's whole history, sound 4 (inside `fe_back`)
                    self.fe_back(-1, event_loop);
                } else {
                    self.fe_raise_dialog(DialogKind::One, 806, [0, 0], DialogFor::Notice);
                }
                // 0x5108F1 / 0x510929
                self.fe_tournament_refresh();
            }
            // slot 0xB4: 0x4D5FB0 opens the screen before this one in the layer's history
            "CANCELCREATEPLAYER" => self.fe_back_callback(event_loop),
            _ => return false,
        }
        true
    }

    pub(crate) fn bind_player_buttons(&mut self) {
        let Some(l) = self.fe.layout_preview.as_mut() else { return };
        let named = l.widgets_named(&["ID_CREATEPLAYEROKBUTTON", "ID_REMOVEBUTTON"]);
        if named.is_empty() {
            return;
        }
        let empty = nfs_game::save::list_profiles(&nfs_game::save::save_dir()).is_empty();
        let typed = !l.input_text("UserList.NameInput").is_empty();
        for (index, which) in named {
            l.set_disabled_flag(index, if which == 0 { !typed } else { empty });
        }
    }

    /// The row of `UserList.UserList` that is selected: the clicked one, else the profile
    /// `player.lst` names — `RebuildUserList` 0x4DEF60 selects it — which is the loaded one.
    pub(crate) fn user_row(&self, profiles: &[nfs_game::save::Profile]) -> Option<usize> {
        match self.fe.user_sel {
            Some(i) if i < profiles.len() => Some(i),
            _ => self.career.as_ref().and_then(|c| profiles.iter().position(|p| *p == c.profile)),
        }
    }

    /// `NFS5.CHARVECTACCESSORINPUTBOX`'s Return (see the key handler): commit the box and, for
    /// `Comms.IP`, put its text at the front of `Comms.IPList`.
    pub(crate) fn fe_charvect_return(&mut self, i: usize) {
        if let Some((name, text)) = self.fe.layout_preview.as_mut().and_then(|l| l.commit_input(i)) {
            if name == "Comms.IP"
                && !text.is_empty()
                && let Some(lobby) = self.net.lobby.as_mut()
            {
                lobby.commit_ip(&text);
            }
            tracing::info!(accessor = %name, value = %text, "list input box committed");
            self.bind_lists();
        }
    }
}
