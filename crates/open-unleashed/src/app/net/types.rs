//! The network state types of the application: the session (`Network`), the launch's race (`NetRace`), the exchange's inputs and
//! seats, the lag arm's step.

use crate::*;

/// Network play, split out of [`App`] on 2026-09-21: the running session and the front end's
/// **`HInfo`'s lag arm** (0x427C7E…0x427EDF, `net_race_notice`): what one look at `lag = 2·net+0xF0 −
/// [0x6559F8] + 1` does. `Lost`: under −0x100 with the link up (0x427CD3: `cmp ecx, 0xffffff00; jge`, then
/// the connection test again) — 18182 and the teardown; else under −0x20 (0x427E1A): with no box up
/// (`[0x60CA08]`) `Show` — 18193 and the tick ends; with it up `Hold` (0x427EC2 → 0x427DAA: nothing else
/// runs); at or above −0x20 with the box up (0x427EBA → 0x427EC2): above −0x10 `Close` (vtable `+0x14`, 2,
/// the flag cleared) and on to the states, else `Hold`; with no box up `States` (0x427DB7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LagStep {
    Lost,
    Show,
    Hold,
    Close,
    States,
}

pub(crate) fn lag_step(lag: i32, connected: bool, box_up: bool) -> LagStep {
    if lag < -0x100 && connected {
        return LagStep::Lost;
    }
    if lag < -0x20 {
        return if box_up { LagStep::Hold } else { LagStep::Show };
    }
    match (box_up, lag > -0x10) {
        (false, _) => LagStep::States,
        (true, true) => LagStep::Close,
        (true, false) => LagStep::Hold,
    }
}

/// The controller samples an input-exchange race's frame driver latched for one tick (0x415EF0 →
/// 0x413870, then `Car_applyInput` 0x4991E0 reads them per car): as the cars read them, the steering
/// negated under `MIRROR` (0x4992D6).
pub(crate) struct ExchangeIn {
    /// this machine's own car (`[0x65741C]`)
    pub(crate) local: Controls,
    /// its race index
    pub(crate) local_index: u32,
    /// every network seat: `(seat, race index, sample)`
    pub(crate) seats: Vec<(usize, u32, Controls)>,
    /// the latched sample's horn bit (`b0` bit 1 = `car+0xD86`, 0x4991E0) of this machine's car and of
    /// each entry of `seats`, in the same order
    pub(crate) local_horn: bool,
    pub(crate) seat_horns: Vec<bool>,
    /// the samples as the ring holds them (`[b0, b1, b2, b3]`): what the replay records per human
    pub(crate) local_raw: nfs_net::exchange::Sample,
    pub(crate) seat_raw: Vec<nfs_net::exchange::Sample>,
}

/// What [`App::exchange_seats_step`] steps the seats with: the seats (race-index order), the player's car position, the tick, the
/// `(rally, derby)` flags, `LoadTransfer`'s wet test and the collision grid.
pub(crate) struct SeatStep<'a> {
    pub(crate) seats: &'a [(usize, u32, Controls)],
    pub(crate) local: Vec3,
    pub(crate) tick: u64,
    pub(crate) flags: (bool, bool),
    pub(crate) wet: &'a dyn Fn(Vec3) -> bool,
    pub(crate) grid: &'a nfs_physics::collision::TriangleGrid,
}

/// What [`App::place_player_on_reset`] reads of the world: the player's `.sim`, the track, the slice table, the race progress, the other
/// cars and the smackables (the free-slice search's blockers).
pub(crate) struct ResetWorld<'a> {
    pub(crate) specs: &'a CarSpecs,
    pub(crate) w: &'a TrackWorld,
    pub(crate) t: &'a SliceTable,
    pub(crate) r: &'a nfs_game::race::RaceProgress,
    pub(crate) road: &'a RoadCars,
    pub(crate) smackables: &'a nfs_game::smackables::RaceSmackables,
}

/// What the simulation reads of a human car in an input-exchange race, taken the same way for the local car
/// and for a seat (`network.md` §11.18).
pub(crate) struct ExchHuman {
    pub(crate) position: Vec3,
    pub(crate) slice: usize,
    pub(crate) laps: u32,
    /// `car+0xEC0`: the speed along the car's own heading
    pub(crate) vl: f32,
    /// `car+0xEB0`
    pub(crate) dir: f32,
    pub(crate) speed: f32,
    pub(crate) hw: f32,
    pub(crate) hl: f32,
    /// `car+0xD86`
    pub(crate) horn: bool,
}

/// 0x417800's `car+0xEB0`: −1 below −3 m/s, +1 from 0 up, otherwise the heading's sign against the slice.
pub(crate) fn exch_dir(t: &SliceTable, vl: f32, fwd: Vec3, s: usize) -> f32 {
    if vl <= -3.0 {
        -1.0
    } else if vl >= 0.0 {
        1.0
    } else {
        fwd.dot(t.slices[s].forward).signum()
    }
}

/// A network race played back (`replay.md` §18.15): the input-exchange race without the network. The
/// stream carries one lane per human car (the original's `0x606880 + 0x20·p`, `p` the race index); the
/// simulation reads each car's packet from its lane where a live race takes it from the room's frames.
pub(crate) struct ReplayNetRun {
    /// the local car's race index (`[0x65741C]`, the car the setup's car / paint / name are)
    pub(crate) local_index: u32,
    /// each seat's race index, in seat order (`u32::MAX`: a seat the launch gave no index)
    pub(crate) seats: Vec<u32>,
    /// the room's names of the seats
    pub(crate) names: Vec<String>,
}

/// A human car of an input-exchange race, by where it lives on this machine: the local player's
/// car (`self.vehicle`) or a network seat (`self.road.opponents[k]`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HumanCar {
    Local,
    Seat(usize),
}

impl ExchangeIn {
    /// The human cars in **registration order** (`Car_register` 0x40F189 fills `0x5E8B34[]` /
    /// `0x5E8E2C[]` in the order `Race_setCars` 0x4ECF00 creates the cars, i.e. by race index, the
    /// same on every machine — `network.md` §11.18). The port keeps the local car first and the
    /// seats after it, which is a different order on each machine; every list the simulation walks
    /// is built from this instead. A seat the launch left without a race index (parked) follows the
    /// humans.
    pub(crate) fn humans(&self, n_seats: usize) -> Vec<HumanCar> {
        let mut order: Vec<(u32, HumanCar)> = vec![(self.local_index, HumanCar::Local)];
        order.extend(self.seats.iter().map(|e| (e.1, HumanCar::Seat(e.0))));
        order.sort_by_key(|e| e.0);
        let mut order: Vec<HumanCar> = order.into_iter().map(|e| e.1).collect();
        for k in 0..n_seats {
            if !order.contains(&HumanCar::Seat(k)) {
                order.push(HumanCar::Seat(k));
            }
        }
        order
    }

    /// The horn of a human car (`car+0xD86`).
    pub(crate) fn horn(&self, h: HumanCar) -> bool {
        match h {
            HumanCar::Local => self.local_horn,
            HumanCar::Seat(k) => self.seats.iter().position(|e| e.0 == k).and_then(|i| self.seat_horns.get(i).copied()).unwrap_or(false),
        }
    }
}

/// lobby. `self.net` became `self.net.session`; `self.lobby` became `self.net.lobby`.
pub(crate) struct Network {
    /// `--net host[:PORT]` / `--net join:HOST[:PORT]` — network play (`netplay`, `network.md` §7).
    pub(crate) session: Option<netplay::NetPlay>,
    /// Sessions in the wait of 0x48E9C0 (0x48EA2B, `0x53C380([0x5DEB48] / 2)`): a host's teardown has sent
    /// its leave and pumps for half a second before the links are freed — the port lets the front end go on
    /// and keeps draining them until the deadline ([`App::net_leaving_step`]).
    pub(crate) leaving: Vec<(netplay::NetPlay, Instant)>,
    /// The front end's side of it, alive while one of the four network layouts is open.
    pub(crate) lobby: Option<netplay::Lobby>,
    /// the host's `0xB3` came in on a lobby screen: launch the race once the frame is drawn
    pub(crate) start_due: bool,
    /// the join outcome has been put in front of the player already (one notice per session)
    pub(crate) join_notified: bool,
    /// the lobby screen has sent our slot records (`[this+0x285]`, 0x509290 / 0x50A4F0)
    pub(crate) lobby_sent: bool,
    /// the car (`id`, name) those records last carried, to see a new selection
    pub(crate) lobby_car: Option<(i32, Vec<u8>)>,
    /// the car list's setter 0x4BC240 ran since the last tick (a step of `zMergedList.Car`, changed
    /// or not): the accessor sends its records on every call, 0x4B7F70 holding
    pub(crate) lobby_setsel: bool,
    /// what the launch stream settled for the race about to start (`network.md` §11.7)
    pub(crate) race: Option<NetRace>,
    /// `PeerToPeerIP`'s `[this+0x288]` — set by CONNECT (0x50B150, its first instruction): the screen's tick
    /// opens `joingame` once the link is up (0x50B270)
    pub(crate) connect_pending: bool,
    /// `[this+0x289]`: that `joingame` has been opened (once)
    pub(crate) connect_opened: bool,
    /// **The internet lobby sign-on** (`accountlogin`'s SIGNON 0x5109B0): the account and password typed, kept for the entry server's
    /// re-connect and for ENTER's second link (the original keeps them in `net+0x58` / `+0x98`)
    pub(crate) signon: Option<(String, String)>,
    /// the name the persona dialog (821) was answered with: the retry connects under it (0x509905…0x509978)
    pub(crate) persona: Option<String>,
    /// SIGNON's `[this+0x294]`: `accountlogin` opens `serverlist` once the link is in (0x510457)
    pub(crate) signon_latch: bool,
}

/// The race a network launch settled - the host's own choices, or what a player heard in the
/// host's `0xBC` stream - resolved the way the launch reads the option table (`network.md` §11.7).
#[derive(Debug, Clone)]
pub(crate) struct NetRace {
    pub(crate) track: Option<String>,
    pub(crate) laps: u32,
    pub(crate) backwards: bool,
    pub(crate) mirror: bool,
    pub(crate) damage: bool,
    pub(crate) skill: u8,
    /// traffic cars after 0x4ECF00's clamp to nine cars (`NUMCARS - 8`)
    pub(crate) traffic_cars: u32,
    pub(crate) positional: bool,
    pub(crate) nocollision: bool,
    /// `DERBY`: the stream said the race is a Smash Up (0x416407's knockout arm runs)
    pub(crate) derby: bool,
    /// `RALLY` `[0x6573F8]` / `HEAVY` `[0x6573FC]`: the host's cheats 0 and 8 as the stream carried them
    /// (`network.md` §11.20) — every machine's cars run on these, whatever cheat bits it has of its own
    pub(crate) rally: bool,
    pub(crate) heavy: bool,
    /// the players in the race (`ebx` of 0x4B4DA0: the players `net+0xE5` marks)
    pub(crate) players: u32,
    /// `NUMOPPONENTRACECARS` `[0x658088]` as the stream settled it: 0 for a network launch (0x4ECF00;
    /// the stream's `(0x28, 0)`, `options.rs`); the fix-up 0x4B5081 numbers that many opponent slots
    /// after the players, and the port has no AI opponent in a network race
    pub(crate) opponents: u32,
    /// `.STARTINGPOS` of this machine's own race index, as the host drew it, as the port's
    /// `grid_pos` (`g − 1`)
    pub(crate) grid: Option<u32>,
    /// the same for each other player, in room order — the seat's parking place until its
    /// packets arrive
    pub(crate) seat_grids: Vec<Option<u32>>,
    pub(crate) playlist: Option<Vec<u32>>,
    /// `fe.txt`'s `RAND_SEED` / `TIME` / `WEATHER` / `FOG` as the host's stream carried them
    pub(crate) fe_txt: Vec<(&'static str, u32)>,
    /// the traffic cars' per-slot records, in slot order (the host's own for the host)
    pub(crate) traffic: Vec<nfs_net::options::TrafficCar>,
    /// each other player's per-slot car record at its race index, in room order (the seat order)
    /// — what 0x4EEDA0 built on the owner's machine (`network.md` §11.12)
    pub(crate) seat_cars: Vec<nfs_net::launchcar::CarSlot>,
    /// `.CAMERA` 0x148 of **race index 0** — `car[0].camera`, the only list this machine's camera
    /// reads (`network.md` §11.12c): the first player of the race's, whoever that is
    pub(crate) camera0: Option<Vec<u32>>,
    /// this machine's own car record at its race index (`launch.cars[me]`), for the replay's `.rpl`
    pub(crate) own_words: Vec<(u32, Vec<u32>)>,
}

/// The layouts that speak to the network. Their `callbacklist=` ids are `ID_COMMSSERVERSCREEN`,
/// `ID_COMMSWEBGAMESSCREEN`, `ID_PEERTOPEERIP` and the modem one — see `netplay::Lobby`.
/// Matched case-insensitively, because the screens name each other in lower case
/// (`destination=peertopeerip`) while the files are mixed case.
const NETWORK_LAYOUTS: [&str; 7] = [
    "serverlist",
    "udpip",
    "local",
    "peertopeerip",
    "modem",
    "creategame",
    "joingame",
];

pub(crate) fn is_network_layout(name: &str) -> bool {
    NETWORK_LAYOUTS.iter().any(|n| n.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod net_page_tests {
    use super::*;

    /// 0x4272BF / 0x4272C9: game type 3 clears the summary's timeout (`0x41B130(0)`) — it stays until Space;
    /// every other type passes 1 and the page closes after 0x280 ticks. The other pages are unchanged.
    #[test]
    fn a_network_summary_has_no_timeout() {
        assert_eq!(App::page_timeout(ResultPage::Summary, true), None);
        assert_eq!(App::page_timeout(ResultPage::Summary, false), Some(0x280));
        assert_eq!(App::page_timeout(ResultPage::Records, true), Some(0x280));
        assert_eq!(App::page_timeout(ResultPage::TournamentPlace(1), true), Some(0xC0));
    }
}
