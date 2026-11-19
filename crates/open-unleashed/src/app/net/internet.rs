//! The front end's side of the internet lobby (`RE/notes/lobby.md`): SIGNON's link, the server list's ENTER, the games screen's
//! CREATE / JOIN, the way back. The protocol is `nfs_net::lobby`, the link `netplay::NetPlay::start_lobby`.

use crate::*;

impl App {
    /// Whether the session in hand is an internet lobby link.
    pub(crate) fn internet_session(&self) -> bool {
        self.net.session.as_ref().is_some_and(|n| n.is_internet())
    }

    /// The name the lobby sees: the profile name, or the one the persona dialog (821) was last answered with.
    fn lobby_persona(&self) -> String {
        self.net.persona.clone().unwrap_or_else(|| self.player_name.to_string())
    }

    /// **SIGNON** (0x5109B0): `net+0x58 = 'c'` + the account, `net+0x98` the password, `net+0xA8` the profile name, then
    /// `0x48E1A0(net, 1, 0, Primary Address)` — a new link to the entry server, state 3 (817 «Connecting…»); `[this+0x294] = 1`.
    /// The address is [`netplay::lobby_address`] (PORT-DEVIATION D55).
    pub(crate) fn signon_connect(&mut self) {
        let Some((account, password)) = self.net.signon.clone() else { return };
        self.fe_session_leave();
        self.connect_lobby(None, &account, &password);
        self.net.signon_latch = true;
    }

    /// A kind-1 link to `target` (the entry server when `None`) under the stored credentials.
    fn connect_lobby(&mut self, target: Option<std::net::SocketAddr>, account: &str, password: &str) {
        let addr = netplay::lobby_address();
        let target = target.or_else(|| netplay::resolve_lobby(&addr));
        let persona = self.lobby_persona();
        match netplay::NetPlay::start_lobby(target, &persona, account, password) {
            Ok(n) => {
                tracing::info!(%addr, resolved = ?target, "lobby link (0x48E1A0 kind 1)");
                self.net.session = Some(n);
                self.net.join_notified = false;
                self.net.lobby_sent = false;
                self.net.lobby_car = None;
            }
            Err(e) => tracing::warn!(error = %e, "lobby link: no socket"),
        }
    }

    /// `accountlogin`'s tick, 0x510457: the link is in (`0xC6 0xE`) after SIGNON — `serverlist` opens.
    pub(crate) fn signon_tick(&mut self) {
        if !self.net.signon_latch || !self.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("accountlogin")) {
            return;
        }
        if self.net.session.as_ref().is_some_and(|n| n.is_internet() && n.established()) {
            self.net.signon_latch = false;
            self.fe_open("serverlist", 0, true);
        }
    }

    /// `serverlist` opens (the constructor 0x50B320 calls `0x48E1A0(1, 0, Primary)` when a `NfsNet` exists): a lobby link that is not the
    /// entry server's — the one a former visit to `udpip` left — is replaced by a new link to it. **APPROXIMATION:** the link SIGNON
    /// just made (the entry server's, in) is kept instead of being re-opened; the server sees one login less.
    pub(crate) fn serverlist_arrival(&mut self) {
        let Some((account, password)) = self.net.signon.clone() else { return };
        let entry = netplay::resolve_lobby(&netplay::lobby_address());
        let keep = self.net.session.as_ref().is_some_and(|n| n.is_internet() && !n.lost() && n.internet().is_some_and(|i| i.target == entry));
        if !keep {
            self.fe_session_leave();
            self.connect_lobby(None, &account, &password);
        }
    }

    /// **ENTER** on `serverlist` (0x50B450): the row's `a.b.c.d.port` becomes a new link (`0x48E1A0(net, 1, 0, text)`) and `udpip` opens at once.
    pub(crate) fn lobby_enter_server(&mut self) {
        let Some(target) = self.net.session.as_ref().and_then(|n| n.selected_server_target()) else { return };
        let Some((account, password)) = self.net.signon.clone() else { return };
        self.fe_session_leave();
        self.connect_lobby(Some(target), &account, &password);
        self.fe_open("udpip", 0, true);
    }

    /// **JOIN** on `udpip` (0x50B6A0): version (822), then count (286), then `0xB6` and `joingame` at once.
    pub(crate) fn lobby_join(&mut self) {
        use nfs_net::lobby::JoinRefusal;
        let row = self.net.session.as_ref().and_then(|n| n.internet().map(|i| i.sel_game));
        let Some(row) = row else { return };
        match self.net.session.as_mut().map(|n| n.join_game(row)) {
            Some(Err(JoinRefusal::WrongVersion)) => {
                tracing::info!("JOIN refused: wrong version (0x4B9E80)");
                self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 822, [0, 0], DialogFor::Notice);
            }
            Some(Err(JoinRefusal::Full)) => {
                tracing::info!("JOIN refused: the game is full (0x4B9DA0)");
                self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 286, [0, 0], DialogFor::Notice);
            }
            Some(Ok(true)) => self.fe_open("joingame", 0, true),
            _ => {}
        }
    }

    /// The game-name dialog's answer on `udpip` (0x509A00): `0xB4` with id 0, then `creategame` opens at once.
    pub(crate) fn lobby_create(&mut self, name: &str) {
        let positional = u8::from(self.net.session.as_ref().is_none_or(|n| n.positional));
        if self.net.session.as_mut().is_some_and(|n| n.create_game(name, positional)) {
            self.fe_open("creategame", 0, true);
        }
    }

    /// The create / join screens' back on an internet link (0x48E6F0 / 0x48D8A0 with `net+0xC6` set): `0xBA`, the link stays and the
    /// screen returns to `udpip`. Otherwise the session is left as before.
    pub(crate) fn fe_game_back(&mut self) {
        if self.internet_session() {
            if let Some(n) = self.net.session.as_mut() {
                n.back_to_lobby();
            }
        } else {
            self.fe_session_leave();
        }
    }

    /// The persona dialog's answer (821, 0x5097C2…0x509978): another name, and the link to the entry server goes again under it.
    pub(crate) fn lobby_persona_retry(&mut self, typed: &str) {
        self.net.persona = Some(typed.to_string());
        self.signon_connect();
    }
}
