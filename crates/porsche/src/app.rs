//! The application state: `App` (everything one running instance owns), the front end's `FrontEnd` and the screen-flow types around them.
//! The `impl App` blocks live in the sibling modules of `app/`, grouped by subsystem.

use crate::*;
mod menu;
mod fe_lists;
mod fe_colour;
mod fe_choices;
mod profile;
mod chronicle;
mod fe_cars;
mod fe_rows;
mod fe_pictures;
mod audio;
mod net;
mod race_end;
mod opponents;
mod results;
mod race_stages;
mod race_start;
mod loading;
mod simulate;
mod session;
mod replay;
mod render_scene;
mod car_lights;
mod mirror;
mod scratch;
mod effects;
mod camera;
mod fe_room;
mod fe_flow;
mod fe_screens;
mod fe_callbacks;
mod input;
mod frame;
mod events;
mod scripted;
mod fe_seq;
mod idle;
mod quickfix;
pub(crate) use quickfix::*;
pub(crate) use scripted::*;
pub(crate) use fe_flow::*;
pub(crate) use effects::*;
pub(crate) use car_lights::*;
pub(crate) use camera::*;
pub(crate) use replay::*;
pub(crate) use simulate::*;
pub(crate) use race_start::*;
pub(crate) use results::*;
pub(crate) use race_end::*;
pub(crate) use net::*;
pub(crate) use audio::*;
pub(crate) use fe_cars::*;
pub(crate) use chronicle::*;
pub(crate) use fe_lists::*;
pub(crate) use fe_colour::*;
pub(crate) use fe_choices::*;
pub(crate) use menu::*;

pub(crate) struct App {
    pub(crate) paths: GamePaths,
    /// the front end's screens and per-screen state — see [`FrontEnd`]
    pub(crate) fe: FrontEnd,
    pub(crate) career: Option<CareerUi>,
    /// opponents of the current career event (None = quick-race opponents)
    pub(crate) career_opponents: Option<Vec<CareerOpponent>>,
    /// `OpponentCar.Car`'s selection for the race being set up (`modes.md` §1.1a)
    pub(crate) opp_choice: u32,
    /// the `nfs5.car` id of the car the player picked, so the opponent rule can match era/class
    pub(crate) player_car_id: Option<u16>,
    pub(crate) pad_stick_latched: bool,
    /// Factory Driver mission in progress (skill test state and the mission record).
    pub(crate) skill: Option<SkillTest>,
    /// The race's smackables — the mission scene's cones and the track's own placed `kSmackable`
    /// articles (signs, hydrants, pipes, sawhorses …), car records in every race (0x4A17F0,
    /// `nfs_game::smackables`); built afresh by each [`App::start_race`].
    pub(crate) smackables: nfs_game::smackables::RaceSmackables,
    pub(crate) mission: Option<career_ui::MissionStart>,
    pub(crate) prev_car_pos: Vec3,
    /// Deferred world edits from the simulation (applied before the next scene rebuild).
    pub(crate) world_actions: Vec<WorldAction>,
    /// Poses before the last physics tick (player, opponents) for render-side interpolation:
    /// frames arrive at up to 120 Hz while the state advances at 64 Hz, so drawing the latest
    /// state directly makes the car (and the chase camera bolted to it) stutter.
    pub(crate) render_prev: Option<(Vec3, Quat)>,
    /// the recorded machine's own car's pose (`car_pos` / `car_rot` are the camera's car's: in a network replay
    /// viewing another human they are that human's), and its pose before the last physics tick
    pub(crate) local_pose: (Vec3, Quat),
    pub(crate) local_prev: Option<(Vec3, Quat)>,
    /// Seconds since the previous frame (camera low-pass time constants).
    pub(crate) frame_dt: f32,
    /// The sub-millisecond rest of the front end's clock step (`frame.rs`): the original's clock is
    /// the free-running 128 Hz `TIMER_getTicks` (0x53C2E0), not a per-frame count, so the port's
    /// whole-millisecond step must not lose the fraction (120 fps rounds 8.33 ms to 8, 4 % slow).
    pub(crate) fe_ms_carry: f32,
    pub(crate) paused: bool,
    /// 64 Hz ticks of wall time since the pause began. `[0x606AC8]` — the counter the HUD and the
    /// dialogs animate on — is advanced by one per pass of the frame driver's tick loop until it
    /// reaches the clock `[0x5E99B8]` (0x415B3E…0x415B4D; snapped when more than 16 behind,
    /// 0x4154D4…0x4154E9), and the pause only raises `[0x606A98]` (0x415819…0x415824) inside that
    /// loop, so the counter runs on while paused. This port's tick stops, so the pause dialogs add
    /// this to it.
    pub(crate) pause_ticks: f64,
    /// the pauses so far folded in, so `tick + pause_offset + pause_ticks` is a clock that, like
    /// `[0x606AC8]`, never runs back when a pause ends
    pub(crate) pause_offset: f64,
    /// What `hud_clock` adds so that `[0x606AC8]` is **kept across a pass**: the clock it is set
    /// from, `[0x5E99B8]`, is zeroed only when a session installs the timer (0x4A4700 → 0x4A4752;
    /// 0x4A4760 removes it at the session's end) and runs on through every pass, and both the pass
    /// setup (0x414F2B…0x414F3B) and `RaceFrame`'s entry (0x4152EB / 0x415312) set `[0x606AC8]` to
    /// it. So a restart or the automatic replay goes on from the clock the last pass reached
    /// (`+= tick` there), and a new session starts it at 0 (`= −pause_offset`). APPROXIMATION: the
    /// timer also counts the wall time the original spends loading the session's track (installed
    /// before 0x468FB0) and setting a pass up, which this port does not model — its own loading is
    /// not the original's.
    pub(crate) hud_clock_base: f64,
    /// the race HUD trays' slide: state and the `[0x606AC8]` stamp of its last transition
    pub(crate) tray_slide: (u8, u64),
    /// `HTime`'s and `HMultiTime`'s own state in a mission's playback (0x425A87…0x425ADA); outside
    /// one it follows `tray_slide`
    pub(crate) time_trays: (u8, u64),
    /// the `+5` elements' own states: `HTrackMap` (`[0x60CA34]`) and `HRearview` (`[0x60CA3C]`)
    pub(crate) map_slide: (u8, u64),
    pub(crate) mirror_slide: (u8, u64),
    /// what the HUD toggle keys F5 / F6 / F7 / F8 / "2" change (`HudKeys`)
    pub(crate) hud_keys: HudKeys,
    /// `HReplay`'s own state (`nfs_game::replay::BarState`): built once per session with the HUD
    /// (0x424F90 / 0x424FA6), kept by a restart and the automatic replay
    pub(crate) replay_bar: nfs_game::replay::BarState,
    /// The single input mailbox `[0x610A6C]` (0x435410), as far as the bar is concerned: a
    /// `WM_LBUTTONDOWN` over the bar latches its position here and nothing more (the port used to
    /// answer it at once, `replay.md` §18.11's gap); `App::replay_bar_frame`, the pump's once-per-
    /// `RaceFrame` call (0x415520 → 0x4264CC), is the only reader — it takes the latch and clears
    /// it every frame, event or none, exactly as 0x435410 does
    pub(crate) bar_press: Option<(f32, f32)>,
    /// diagnostic: `NFS_HUD_KEYS=tick:key,…` (key `F5` `F6` `F7` `F8` `2` `pause` or
    /// `pauserestart`) — the HUD toggle keys pressed on those game ticks (`hud_key`), for a
    /// `--screenshot` of their effect
    pub(crate) hud_key_script: Vec<(u64, ScriptedHudKey)>,
    /// **TEST GLUE**: the races of a scripted session (`NFS_AUTOTEST_RACES`) begun after the first
    pub(crate) scripted_race_no: u32,
    /// **TEST GLUE**: the pause `hud_key_script`'s hold ends in the pause menu's quit (`NFS_HUD_KEYS`'s
    /// `pausequit`) — the race left for the front end as `leave_race` does
    pub(crate) scripted_quit: bool,
    /// diagnostic: a pause `hud_key_script` opened, left after 64 pause ticks
    pub(crate) scripted_pause: bool,
    /// diagnostic: the pause `hud_key_script`'s hold ends in the pause menu's restart (command 3,
    /// 0x415721) rather than its continue (0x415701) — `NFS_HUD_KEYS`'s `pauserestart` key, for the
    /// restart's audio pick count (`replay.md` §18.12)
    pub(crate) scripted_restart: bool,
    /// diagnostic: a scripted pause press in a network race asked to leave (`net_pause_press`)
    pub(crate) scripted_net_leave: bool,
    /// `0x5E8B88[]` — the race-order array every car is registered in (the human, the opponents,
    /// the cops, the traffic), re-sorted at the end of every even tick (`nfs_game::race_order`)
    pub(crate) race_order: nfs_game::race_order::RaceOrder,
    /// whether the audio has been put into its pause state (0x4AB150 / 0x4AB200)
    pub(crate) audio_paused: bool,
    /// The results page whose billboard is open (not closing) this frame, set by `rebuild_scene`.
    pub(crate) results_billboard: Option<u32>,
    /// Which billboard is up: 0 none, 1 the pause menu, 2 its quit question, 10 + page a results
    /// page. `HBillBoard`'s state setter 0x41ADD0 plays the front end's sound 4 (0x4AADB0) on
    /// entering state 1 (opening, 0x41ADFA) and state 2 (closing, 0x41AE0E), so every change of
    /// this key is one of those.
    pub(crate) billboard_key: u32,
    pub(crate) knockout: Option<KnockoutSeries>,
    /// `FEOpponentCarAccessor`'s seven embedded cars (`this + 0x24 + i·0x760`, `modes.md` §1.1a):
    /// what the last 0x4F4FA0 drew — each opponent's record and appearance. Refilled by every
    /// quick race's set-up (0x4EBC2D) and the knockout start (0x4DDF58), loaded with the profile
    /// (0x4F56F0); a knockout round takes entry `i − 1` for opponent `i` (0x4DE1E7). Empty until
    /// the first draw: the constructor's own draw (0x4F4EA8, in 0x4F4E50) is not made, and that is
    /// safe — the only readers of the seven cars are 0x4EBBA0 (0x4EBC8B, after its own draw at
    /// 0x4EBC2D), the knockout round 0x4DE150 (0x4DE1E7, after the series' draw 0x4DDF58) and the
    /// profile save 0x4F56C0 (which would write the constructor's clock-seeded random cars where
    /// the port writes none until a race has drawn them; xrefs read 2026-09-30).
    pub(crate) opp_cars: Vec<nfs_game::save::OpponentCarSave>,
    /// the race running is a knockout round (`Game.GameType` 2 at the launch, `[0x690AD4]` at its
    /// end) — set by `knockout_round`, taken by the race's end
    pub(crate) knockout_round: bool,
    pub(crate) player_grid_override: Option<u32>,
    /// The `fe.txt` half of a network launch as the host sent it (`rand_seed`, `time`, `weather`,
    /// `fog`): the option-table values a player's race reads in place of its own file's
    /// (`network.md` §11.7). Empty outside a network race — every session re-reads `fe.txt`
    /// (0x4B6C1C).
    pub(crate) net_fe_txt: std::collections::HashMap<&'static str, u32>,
    /// `[0x657408] != 0 && [0x65740C] != 0` — `COMMSPOSITIONAL` and `NOCOLLISION` both set: the
    /// collision pass 0x498FF0 then skips the car–car half (`network.md` §11.7). Set by a network
    /// launch, clear for every other.
    pub(crate) net_nocollision: bool,
    /// a network race's other players' grid places (`g − 1`), in room order
    pub(crate) net_seat_grids: Vec<Option<u32>>,
    /// a network race's other players' car records (0x4EEDA0's, at their race indices), in room
    /// order — the seat's car, colour, names, set-up, roof and options
    pub(crate) net_seat_cars: Vec<nfs_net::launchcar::CarSlot>,
    /// the local car's own launch record (`NetRace::own_words`) — what the replay's `.rpl` writes at its slot
    pub(crate) net_own_words: Vec<(u32, Vec<u32>)>,
    /// The input-exchange race's controller samples for this tick — what the frame driver latched
    /// (0x415EF0) at an even tick: this car's, and each network seat's with its race index
    /// (`network.md` §11.17). `None` outside an exchange race and on an odd tick.
    pub(crate) exchange_in: Option<ExchangeIn>,
    /// A network race played back from a file (`replay.md` §18.15): who the humans are and which lane of
    /// the stream is whose. `None` for every other race.
    pub(crate) replay_net: Option<ReplayNetRun>,
    /// `DERBY [0x657400]` of a network race the stream settled (`net_race_apply`): 0x416310's knockout arm
    pub(crate) net_derby: bool,
    /// **The launch's cheat flags of a network race** `(RALLY [0x6573F8], HEAVY [0x6573FC], DERBY [0x657400])` as
    /// the host's stream carried them (0x4ED5D0's `0x19` / `0x1A` / `0x1B` records, 0x4B4B80 on every machine):
    /// `None` outside a network race, where the flags are this machine's own cheat bits (0 / 8 / 7)
    pub(crate) net_flags: Option<(bool, bool, bool)>,
    /// a network race's replay: which human car the camera follows, an index into the humans by race
    /// index (`[0x606858]`, the bar's previous / next car cells)
    pub(crate) replay_view: usize,
    /// the traffic cars' records of the network race settled (`net_race_apply`), for the replay file
    pub(crate) net_traffic: Vec<nfs_net::options::TrafficCar>,
    /// **The C runtime's `rand()` state** (`_getptd()+0x14`, 0x5A17C9: `s = s·0x343FD + 0x269EC3`,
    /// `(s >> 16) & 0x7FFF`). The thread starts on 1 (`_initptd`, 0x5A5454) and the one `srand`
    /// (0x5A17BC, called at 0x418100 by `HMissionSummary`'s constructor, i.e. every race load)
    /// seeds it from `time(0)`. Only the network launch's draws use it here (`network.md` §11.7):
    /// the grid draw 0x4B5008 and the `.DRIVER` draws 0x4EEDAC before it. **CONFIRMED
    /// unobservable** (2026-09-30; the ~66 call sites of 0x5A17C9 — the FE widgets'
    /// glints, 0x4F4FA0's opponent draws, the market 0x4F82C0, the mission opponents 0x4EC1A0,
    /// 0x4EEDA0's `.DRIVER`, the grid 0x4B4DA0 — all share the one state; its only `srand` is
    /// `srand(time(0))` at 0x418100 in `H321Go`'s constructor, run at every race load, so what a
    /// race draws never depends on what the front end drew): the original's state also moves with
    /// every other `rand()` of the program, which this port keeps on generators of their own
    /// except the front end's widget glints and the used-car trim, which use the shared
    /// `layout::glints::crt_rand`. Before the first race the state is the deterministic 1-seeded
    /// sequence, which only the glints' variants and timing show.
    pub(crate) crt_rand: nfs_game::movie_overlay::CRand,
    /// Drive the track backwards (`REVERSE`): `_bstart` scene, reversed slice route, `<track>1.lsp`.
    pub(crate) backwards: bool,
    /// `MIRROR`: world drawn x-flipped, steering negated (modes.md §2.1, MEDIUM on the consumers).
    pub(crate) mirror: bool,
    /// Quick Knockout (`RACE_TYPE 3`): laps = opponents, the last car is out on every lap.
    pub(crate) quick_knockout: bool,
    pub(crate) player_eliminated: bool,
    /// the player's `0xB7` finish time has been sent for this race (0x4168A9, once: the state-2
    /// arm of 0x416310 runs on one tick)
    pub(crate) net_time_sent: bool,
    /// `car+0xD8A` of the local car — the packed pad byte `[0x5E9664]` its controller copy 0x4991E0 took at the last
    /// 32 Hz run (0x499216): the block's `+0x03` (`NetPlay::send`), so the remote horn and handbrake travel
    pub(crate) net_pad_buttons: u8,
    pub(crate) records: Records,
    /// recording and playback of the controller stream — see [`Playback`]
    pub(crate) replay: Playback,
    /// Seed of the shared RNG for this race; travels with a replay so playback re-simulates the
    /// same traffic, police and AI.
    pub(crate) traffic_seed: u32,
    /// `[0x65B940]`, the name-box cheat bits (`nfs_game::cheats`); read through 0x4D8790
    pub(crate) cheat_bits: u32,
    /// The profile name the original stores in a track record (`player.lst` → `LoadInfo`).
    pub(crate) player_name: String,
    /// `nfs5.car` id of the car being driven, for the replay snapshot's car slot.
    pub(crate) car_id: u16,
    /// headless run (`--autotest`): the bot may reset itself when stuck
    pub(crate) scripted: bool,
    pub(crate) stuck_ticks: u32,
    /// detail level (`[0x657D70]` → 0.5 / 0.67 / 0.83 / 1.0) scaling the `.key` draw distance
    pub(crate) detail: f32,
    /// `SPLITTIMES` (`[0x657DD8]`): 1 record lap, 2 best lap, 3 last lap — circuits only; the
    /// record time is always shown on point-to-point tracks (`HRecordTime` 0x42A200, hud.md §6.3).
    pub(crate) records_path: std::path::PathBuf,
    /// The post-race records table (`HTrackRecords`, `nfs_game::records::hud_records`), built as
    /// the result pages are (0x433A20 at 0x426C4F); `None` when the race has none to show.
    pub(crate) hud_records: Option<nfs_game::records::HudRecords>,
    /// `HTrackRecords +0x124`: its record speech has been said (0x4339E9…0x4339F9)
    pub(crate) records_spoken: bool,
    /// `HPauseMenu` 0x42A440: the highlighted button — 0 continue, 1 race again, 2 quit
    pub(crate) pause_sel: u8,
    /// The result records of the race that reached race over (0x417010 → `Race_finalize(2)`
    /// 0x416AC0), `None` before it. The post-race pages are up while they exist.
    pub(crate) results: Option<RaceResults>,
    /// Race over this tick: `[0x606A8C] = 1` ends `RaceFrame`'s loop (0x415D17) and the session loop
    /// (0x4B6ED5…0x4B6F18) restarts the race as its automatic replay — done by the tick's caller,
    /// which owns the event loop (`App::start_auto_replay`).
    pub(crate) auto_replay_due: bool,
    /// The mission's verdict and result text, fixed once at the race end as
    /// `HMissionSummary::evaluate` 0x4293F0 does
    pub(crate) mission_result: Option<(bool, Option<u32>)>,
    /// `HTime +0xE0`: set to 16 on a cone hit (0x431830) and counted down once per HUD draw,
    /// i.e. per rendered frame (`HTime` draw 0x4316F0, 0x4317C9…0x4317E0 — reached whether or not
    /// the element is shown); the time shows in the alert colour while it is not 0 (0x431730)
    pub(crate) cone_flash: u32,
    /// The mission HUD's cone count, `[0x60CA04]` and `HMissionSummary+0x140` / `HTime+0xE4`
    /// (`nfs_game::skill::HudCones`, 0x425950)
    pub(crate) hud_cones: nfs_game::skill::HudCones,
    /// the tick the current post-race page opened
    pub(crate) results_page_tick: u64,
    /// the quit confirmation (`HYesNoOverlay`, 0x426154) when it is up: 0 yes, 1 no
    pub(crate) pause_confirm: Option<u8>,
    /// The in-race dialog button under the pointer as the last draw found it — `HButtonVisual`'s
    /// `+0x100` (0x41B3CE): `(dialog, button)`, dialog 0 the pause menu, 1 the quit question, 2 the
    /// mission summary. A release reads it (0x41B300), as the original's does.
    pub(crate) hud_hover: Option<(u8, usize)>,
    /// diagnostic: `NFS_REPLAY_PRESS=tick:cell,…` — the pointer put on a replay bar cell and pressed
    /// once the playback reaches that tick, for scripted runs (`App::run_bar_script`); cells 7 and
    /// 8 are `HReplaySave`'s two buttons, 9 and 10 the overwrite question's yes and no
    pub(crate) bar_script: Vec<(u64, usize)>,
    /// diagnostic: `NFS_REPLAY_HOVER=tick:cell,…` — the pointer put in a replay bar cell's middle
    /// (0…6) without a press, or off the bar (`off`: the screen's top-left corner), once the
    /// playback reaches that tick — for stills of the bar's hover, tooltip and auto-hide
    pub(crate) hover_script: Vec<(u64, Option<usize>)>,
    /// `HReplaySave` `[0x60C9B0]` and the overwrite question it raises
    pub(crate) replay_save: ReplaySave,
    /// Index into [`App::result_pages`] of the page that is up. 0x427273 queues `HRaceSummary` and
    /// then `HTrackRecords` (0x4272FF) for game types 0, 1, 2 and 5; 0x426DF9 queues the summary,
    /// `HTournamentStandings` and the records for a tournament; the next opens when the one before
    /// closes.
    pub(crate) results_page: u8,
    /// the post-race page the billboard animation last counted, its frames since it opened, and
    /// the tick it opened at (`HBillBoard +0x11C`, stamped by setState(1) at 0x41ADF4)
    pub(crate) page_seen: Option<u8>,
    pub(crate) page_frames: u32,
    pub(crate) page_open_tick: Option<u64>,
    /// frames the page on top has been shrinking for (`HBillBoard` state 2)
    pub(crate) page_closing: Option<u32>,
    /// Every racer's body-damage total at the start, `[0x60C9C8 + i·4]` — taken by the HUD's
    /// race-start arm 0x426A80(0) and judged against at the finish (`career::clean_race`).
    /// Index 0 is the player, `i + 1` opponent `i`.
    pub(crate) start_damage: [i32; 8],
    pub(crate) track: TrackAssets,
    pub(crate) car: CarAssets,
    pub(crate) screenshot: Option<String>,
    /// **TEST GLUE** (not in the original): a still's render target, `(physical W, physical H,
    /// scale)` from `NFS_SHOT_SIZE=WxH` (default 1280x800) and `NFS_SHOT_SCALE=<s>` (default 1) —
    /// `Some` in a `--screenshot` / `NFS_RACE_LOADING` / `NFS_LOADING_SHOT` run, where the still is
    /// rendered at that size whatever the window is. The HUD and the loading picture are built for it.
    pub(crate) shot_target: Option<(u32, u32, f32)>,
    /// `--layout-sweep`: load every `.lay` in the game, draw it once, and report every widget that
    /// produced no sprite at all. The port's own measure of what the front end does not draw.
    pub(crate) layout_sweep: bool,
    /// `fe.txt`, for the multiple-choice accessors' saved indices.
    pub(crate) registry: nfs_formats::fetxt::Registry,
    /// **0x4B73F0**'s language index, read once from the install's registry exports
    /// (`GamePaths::language`): `[0x690710]` / the `LANGUAGE` option (0x4E0598, 0x4D53A9), the HUD's
    /// and the car files' block (0x41E1FE, 0x508C78).
    pub(crate) language: u16,
    /// The choices the player has changed on the options screens this session, by `fe.txt` key.
    /// The original writes them straight back into the registry slot; the port keeps them here
    /// rather than rewriting the read-only game directory.
    pub(crate) choices: std::collections::HashMap<String, usize>,
    /// The profile's race-HUD option tables (`<Name>1.hud` / `<Name>2.hud`, [`hud_cfg`]), loaded lazily: the
    /// 14 Display options live in the record of (`Display.ScreenLayout`, resolution).
    pub(crate) hud_cfg: std::cell::RefCell<HudCfgCache>,
    /// scripted drive before the screenshot: seconds of full throttle (steer right for the last third)
    pub(crate) autotest: Option<f32>,
    /// **0x538040's state** `[0x5DE910…0x5DE924]` — the generator the front end calls "rand" and
    /// the race shares: the sky's jitter (0x45EEA1 / 0x45F3D3), the weather (0x5561D0), the speech
    /// wrappers' variants (0x4A62A0…0x4A6680) and `I_MUSIC`'s pick (0x4AD87E). One state for the
    /// whole program, starting at the image's `.data` words (what `0x538110(0)` leaves). Every
    /// session's race load reseeds it from `time(0)` — the HUD factory 0x424460 (0x4B6878) builds
    /// `HMissionSummary` in every race (0x424CF6), and its constructor ends with
    /// `0x538110(time(0))` (0x428C25…0x428C2D) — so what the front end or the track load's 0x44F240
    /// (`x = draw; seed(12345); …; seed(x)`, 0x46906A) left never reaches the race; a restart or
    /// the automatic replay runs on without a reseed. *(Until 2026-09-26 the sky and the weather
    /// each drew from a state of their own starting at the `.data` words, the announcer from one
    /// seeded from the clock at start-up, never again.)* The front end's own draws (the opponents'
    /// cars, the market, the knockout's personalities) take generators of their own —
    /// clock-seeded like 0x4E3182's, so unobservable (`crt_rand`'s note).
    pub(crate) fe_rng: nfs_game::opponents::FeRng,
    /// The next `start_race` is another pass of the session loop (0x4B6D9F: "race again", the
    /// automatic replay), not a new session's race load 0x4B67B0.
    pub(crate) race_pass: bool,
    /// diagnostic: `NFS_AUTOTEST_RACES=n` — a scripted session of `n` races, each `--autotest`
    /// seconds long: race → the automatic replay over it → leave → the next race (the session
    /// loop 0x4B6D88 and the front end's round trip), so the second race's `--record` rows can be
    /// compared with a fresh run's. What is left of the session, this race included.
    pub(crate) session_races: u32,
    /// the automatic replay of the session's current race has been run
    pub(crate) session_replayed: bool,
    /// The start-up movies while they play (0x4DD600 / 0x4DD4A0 → 0x4DC850): they hold the window
    /// and renderer, and hand them back in `intro_done` for `resume_inner` to go on with.
    pub(crate) intro: Option<intro_run::IntroRun>,
    pub(crate) intro_done: Option<(Arc<Window>, Renderer)>,
    /// `FEMainScreen`'s idle re-roll (0x510210 → 0x4DC140): the minute is up, the song is stopped,
    /// the movie 0x4DD4A0 plays and the wait for the buttons follow before `MusicPlay("Menu*")`
    pub(crate) idle_movie_due: bool,
    /// the `intro` that runs is that idle one: its end hands the window and renderer back to the
    /// front end instead of resuming the start-up
    pub(crate) idle_movie_running: bool,
    pub(crate) window: Option<Arc<Window>>,
    pub(crate) renderer: Option<Renderer>,
    pub(crate) world: Option<TrackWorld>,
    pub(crate) car_model: Option<CarModel>,
    /// the player's car's LOD class and translation channels (`R3DCar+0x2C0`, `+0x6D0`)
    pub(crate) car_lod: nfs_game::world::CarLodState,
    /// the cockpit view's eye point from the player car's `.tpg`, when it has one
    pub(crate) driver_cam: Option<nfs_assets::car::DriverCam>,
    pub(crate) sky: Option<Sky>,
    pub(crate) scene: Scene,
    pub(crate) camera: Camera,
    pub(crate) input: InputState,
    pub(crate) gamepads: Gamepads,
    pub(crate) car_pos: Vec3,
    pub(crate) car_rot: Quat,
    pub(crate) specs: CarSpecs,
    pub(crate) vehicle: Option<Vehicle>,
    /// Index into `camera_cycle`.
    pub(crate) camera_mode: u8,
    /// Camera slot 0 on **`AnimCam`** (view 0x0E), the race's opening animation
    /// (`nfs_game::camera::anim_cam`): 0x4640B0 puts every pass's slot on it, 0x462F30 plays one
    /// of the track's `CmAn` paths and gives the slot `car[n].camera[0]` when it is over.
    pub(crate) anim_cam: AnimCamSlot,
    /// `[0x62893C]`, the `CmAn` rotation counter 0x471A50 steps before each pick — a `.bss` word
    /// nothing resets, so it runs on across races and sessions.
    pub(crate) anim_cam_counter: u32,
    /// The track's `CmAn` animations (0x471850 at the track load).
    pub(crate) cam_anims: Vec<nfs_formats::crp::AnimTrack>,
    /// The views the player cycles through, from the registry's `car[0].camera`
    /// (`nfs_game::camera::cycle_from_registry`): the shipped list is BumperCam2, BumperCam,
    /// TailCam0, HeliCam2.
    pub(crate) camera_cycle: Vec<u16>,
    /// the skid marks (`WLineBundle`, `nfs_game::skidmarks`, `RE/notes/particles.md` §0)
    pub(crate) skid: nfs_game::skidmarks::SkidMarks,
    /// one dynamic mesh per drawn strip, `MAX_CAPACITY` vertices each
    pub(crate) skid_pool: Vec<nfs_renderer::MeshId>,
    /// the `SKDP` quad and its material, uploaded with the track
    pub(crate) particle_draw: Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// the `SHAD` blob every car casts on the ground (0x43B820)
    pub(crate) shadow_draw: Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// One dynamic four-vertex mesh per shadow on screen — the quad 0x43B5B0 rebuilds every frame.
    /// the car shadows' dynamic meshes, pooled by the patch's `n` (index 1…6)
    pub(crate) shadow_meshes: [Vec<nfs_renderer::MeshId>; 7],
    /// `CARG`, the glare slot 0 of every light kind in the table at 0x5CD73C
    pub(crate) glare_draw: Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// the glare's second sprite, the table's `+0x10` column: `SPIK` for every kind but 4, `GLNT`
    pub(crate) glare_star: [Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>; 2],
    /// the lamp draws' dynamic meshes and materials (`LampPools`)
    pub(crate) lamp_pools: LampPools,
    /// `GameData/Render/flare.dat` (0x4522B0) and a sprite for every texture its records name
    pub(crate) flare_defs: Vec<nfs_formats::flare::FlareDef>,
    pub(crate) flare_sprites: std::collections::HashMap<String, (nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// the sun's flare textures (`SUN_FLARE`'s two layers) as sky-pass sprites: additive, no depth
    /// test (`sun_items`)
    pub(crate) sun_sprites: std::collections::HashMap<String, (nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// the track's particle systems (`nfs_game::track_particles`) and their textures out of
    /// `Render/particle.fsh`
    pub(crate) track_particles: Option<nfs_game::track_particles::TrackParticles>,
    /// 0x488CC0's process-wide walk counter `[0x628C4C]` (0 at process start, never reset).
    pub(crate) walk_calls: u32,
    pub(crate) track_particle_defs: Vec<nfs_formats::particle_dat::ParticleDef>,
    pub(crate) track_particle_sprites: std::collections::HashMap<(String, u32), (nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// every car's `carpart.dat` effects (`nfs_game::car_effects`, `RE/notes/particles.md` §11),
    /// made with the track's systems, whose generator they draw from; their definitions until then
    pub(crate) car_effects: Option<nfs_game::car_effects::CarEffects>,
    pub(crate) car_effect_defs: Vec<nfs_formats::particle_dat::ParticleDef>,
    /// the weather layer `[0x627E08]` (`nfs_game::weather`, `RE/notes/particles.md` §7): made with
    /// the track's particle systems, whose generator `[0x5CEAE0]` it draws from too
    pub(crate) weather: Option<nfs_game::weather::Weather>,
    /// The layer a restart or the automatic replay carries into its pass, to be primed again
    /// rather than made (0x4690A0 → 0x459260; it is deleted only at the session's end, 0x4690F0 →
    /// 0x458BB0).
    pub(crate) weather_kept: Option<nfs_game::weather::Weather>,
    /// its draws, snow then rain: a dynamic mesh of `CAPACITY` triangles each and its material
    pub(crate) weather_draws: [Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>; 2],
    /// the windscreen drops' draw (the splat object, `particles.md` §7.5): `common.fsh`'s `SPLT`
    /// and a dynamic mesh of `SPLAT_SLOTS` triangles
    pub(crate) splat_draw: Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    /// the particle streaks of 0x454EC0 (the cars' sparks, `nfs_game::track_particles::Streak`):
    /// a pool of dynamic triangle lists of `STREAK_CAP` triangles and an untextured, vertex-coloured
    /// material per blend mode 0…2
    pub(crate) streak_draw: Option<(Vec<nfs_renderer::MeshId>, [nfs_renderer::MaterialId; 3])>,
    /// this frame's track walk over the `ef`-bearing articles (`TrackWorld::article_draws`, by
    /// `.crp` article index) — what the flares, the effect systems and the projectors run from
    pub(crate) article_draws: Vec<Option<nfs_game::world::ArticleDraw>>,
    /// the draw distance `[0x6197CC]` for the camera's slice (`keys.far_distance`) — the main view's
    /// `+0x124`, the walk's footprint and its non-mid-ground distance rule; the far plane itself
    /// is the camera object's 700 (0x4677FC)
    pub(crate) draw_distance: f32,
    /// where the track's near list, the merged `+0xC` (track, cars, skid marks) and the port's
    /// no-`Base` fallback sit in `scene.items` together, for the mirror's own walk to replace
    /// (`Self::render_mirror`, which builds the cars' and skid marks' entries again for its own eye)
    pub(crate) track_item_range: (usize, usize),
    /// the flares', particles' and streaks' entries of the main pass in `scene.items` (the mirror's pass
    /// replaces them with its own, `Self::effect_items`)
    pub(crate) effect_item_range: (usize, usize),
    /// this frame's cars' and skid marks' own `+0xC` entries (`round(d²)`-keyed, `RE/RENDERING.md`
    /// "The view's lists"), the main pass's
    pub(crate) blended_dyn_items: Vec<(u32, nfs_renderer::DrawItem)>,
    /// the scene build's reusable buffers (`scratch.rs`)
    pub(crate) scratch: scratch::SceneScratch,
    /// `live_order`'s last answer under the `(progress, tick)` pairs it was ranked from, and the
    /// buffer the next key is built in (`race_end.rs`)
    pub(crate) order_cache: std::cell::RefCell<Option<(Vec<u64>, Vec<usize>)>>,
    pub(crate) order_key: std::cell::RefCell<Vec<u64>>,
    /// the far plane of the view whose pass is being built (700, the mirror's 65000), for the
    /// depth-bias pulls of the shadows, the lamps and the skid marks (`cam[+0xC]`)
    pub(crate) view_far: f32,
    /// the main pass's mesh use and pose, from which the mirror's own car pass continues
    pub(crate) pool_used: PoolUse,
    pub(crate) dyn_pose: Option<DynPose>,
    /// **TEST GLUE**: the interpolation fraction `NFS_SHOT_SEQ` sets for each of its sub-tick frames
    pub(crate) test_alpha: Option<f32>,
    /// where the sky pass's items sit in `scene.items` (before the track's), for the mirror's own
    /// sky pass to replace
    pub(crate) sky_item_range: (usize, usize),
    /// a particle system drew this frame, leaving the alpha reference at 1 (0x454456)
    pub(crate) particles_left_ref_one: bool,
    /// trackside cameras and the triggers that switch them (`RE/notes/race-logic.md` §3.3)
    pub(crate) track_cameras: Option<nfs_game::cameras::TrackCameras>,
    pub(crate) slices: Option<SliceTable>,
    pub(crate) race: Option<RaceProgress>,
    /// the other cars on the road — opponents, traffic, police — see [`RoadCars`]
    pub(crate) road: RoadCars,
    pub(crate) racing_line: Option<nfs_formats::ais::RacingLine>,
    /// network play — see [`Network`]
    pub(crate) net: Network,
    pub(crate) num_opponents: u32,
    pub(crate) player_grid: u32,
    /// `car+0xB54` per racer (player first): metres covered, `fistp(min(car+0xAF8, NUM_LAPS) ·
    /// [[0x628BC0]+4] · 6.0)` — refreshed only when `gGameTick % 64 == 1` and there is more than
    /// one racer (0x417010…0x4170A2); `HPosition`'s distance / time gaps are taken from it
    pub(crate) hud_metres: Vec<i32>,
    /// The rear-view mirror's camera for this frame (AUDIT §O), and the texture it renders into.
    pub(crate) rear_mirror: Option<Camera>,
    pub(crate) mirror_tex: Option<TextureId>,
    /// `HWrongWay`'s blink: `+0x14` ticks counted, `+0x18` the tick of the last draw, `+0x24`
    /// visible. Every draw adds the ticks since the last one, and the visible half flips off once
    /// the count passes 64, the hidden half back on once it passes 32, the count restarting
    /// (0x436F86…0x436FBD). Only the HUD reset 0x426A80 → 0x436F60 zeroes it (visible, 0, 0) —
    /// nothing does when the car turns round — so the first draw after a reset, at a tick past
    /// 448, flips straight to hidden. This port blinked on `tick % 96 < 64`.
    pub(crate) wrong_way_blink: (u64, u64, bool),
    /// `--record path`: per-32Hz-tick CSV of controls and vehicle state (regression fixtures)
    pub(crate) record: Option<std::io::BufWriter<std::fs::File>>,
    /// **Test glue** (`NFS_NET_DUMP=<file>`): every human car's state after every tick of an
    /// input-exchange race, one line per car in race-index order, the floats as their bit patterns —
    /// what two machines' runs are diffed on (`network.md` §11.17)
    pub(crate) net_dump: Option<std::io::BufWriter<std::fs::File>>,
    /// when the last tick of an input-exchange race ran (the render's interpolation clock)
    pub(crate) exchange_tick_at: Instant,
    /// **`HInfo`'s in-race box** `[0x60C9C0]` (0x4251D5) — the `hud.loc` text it shows, if it is up
    /// (`net_race_notice`)
    pub(crate) net_notice: Option<u32>,
    /// the box up is `[0x60C9FC]` (Spacebar byte 1, 0x4250BF) — the pause action's 18190 — not
    /// `[0x60C9C0]` (byte 0, 0x4251D5): Space closes it (0x42A3D0) and its hints show both keys
    pub(crate) net_notice_space: bool,
    /// `[0x60CA08]` — the box up is the lag notice 18193, which closes itself
    pub(crate) net_lag_box: bool,
    pub(crate) num_laps: u32,
    /// A Smash Up's real `NUM_LAPS` `[0x6573F0]` (the launch's), which only the map tray's lap
    /// counter shows — `min(car+0xB20 + 1, NUM_LAPS)` over the laps the lap code never counts
    /// (0x432F92; 0x416310's knockout arm ends before it, 0x4164C0), i.e. `1/N`. `num_laps` itself
    /// stays 99 there so the port's lap tracking never ends the race. `None` in any other race.
    pub(crate) derby_hud_laps: Option<u32>,
    /// autotest: steer toward the centreline (pure pursuit) instead of scripted input
    pub(crate) follow_road: bool,
    pub(crate) car_half_width: f32,
    /// The chase camera's slot state (0x461E80): the eye's offset from the car `+0x08`, the view it
    /// was built for, and the `+0xC1` settle count SetSlotView starts at 0x32 (0x465848).
    pub(crate) chase_offset: Option<(Vec3, u16)>,
    pub(crate) chase_settle: u8,
    /// The bumper / driver slot's look-around yaw `+0xC6` in degrees (0x4611C0,
    /// `camera::look_step`) and the view it belongs to; a view switch starts it again at 0
    /// (SetSlotView 0x4658B2).
    pub(crate) look_yaw: Option<(i16, u16, bool)>,
    /// Look left, right and forward — input bits 0x14 / 0x15 / 0x16 as `RaceFrame` last read them
    /// (`[0x5E99B4]` / `[0x5E99AC]` / `[0x5E99A8]`, 0x415AFA…0x415B2C): held levels, not read
    /// while the pause is on (0x415982), so they keep their value through it.
    pub(crate) look_held: [bool; 3],
    /// The player car's lamp byte `car+0xD87` (`lamp_command`).
    pub(crate) lamp_bits: u8,
    /// The pause orbit (0x4617C0): slot 0's yaw `+0xC6` / step `+0xC8` and its offset.
    pub(crate) orbit: Orbit,
    /// `[0x60CA00]`: the whole HUD draw 0x425950 skipped — the dialogs, the pause menu with them.
    /// `I_TOGGLEPAUSEMENU` (F1, bit 0x0E, read on the press at 0x41596E) flips it through
    /// 0x425750, but only while a dialog is up (`[0x60C9C4]` not empty, `hud_dialog_up`); the
    /// HUD's race start clears it (0x4252E0).
    pub(crate) hud_hidden: bool,
    /// The player car's headlight ramp, `[car+0x64]+0x110` (`HeadRamp`).
    pub(crate) head_ramp: HeadRamp,
    /// The game tick the camera slot was last updated for. The dispatcher 0x463C30 is a **64 Hz**
    /// scheduler entry (`Sched_insert(Sc64, 0x463C30, 0)` at 0x414F8C, run by every `GameTick`
    /// 0x414FF0), so the slot's followers (the chase / heli offset, the driver's head lag, the
    /// look-around yaw, BlimpCam, CircleCam, SplineCam, 0x462B80's zoom) step once per game tick,
    /// not once per drawn frame: a frame applies `tick − camera_tick` updates.
    pub(crate) camera_tick: u64,
    /// `[+0x18]` of the four `OwnedCar.*` colour accessors (`owned_colour_slot`), the list row each
    /// keeps beside its colour `[+0x2C]`. Built once at 0x4F1EE0 with the index 0
    /// (0x4B91F0 → 0x4BC240(0)), written only by slot 11 (0x4B92B0 → 0x4BC240) and reset to 0 when
    /// the list shrinks to it (0x4BC310) — the paint-shop screen's car sync 0x511220 writes the
    /// colour alone (0x4B9580), so a saved car's colour never becomes a row.
    pub(crate) colour_idx: [std::cell::Cell<usize>; 4],
    /// The last game ticks' [`CamTick`]s: a frame that spans several ticks steps the
    /// chase / heli follower once per tick against that tick's own pose, as `Sc64` does. Not capped:
    /// 0x4152E0's catch-up loop (0x415BD4, `[0x606AA0] <= [0x606A9C]`) has no bound on the ticks a
    /// frame runs, so every tick the camera has not stepped yet stays (trimmed after each camera pass).
    pub(crate) cam_hist: std::collections::VecDeque<CamTick>,
    /// The roll-over camera's side, `+0xC3` bit 0 (0x463CC6…0x463CFB), latched on the first
    /// tilted update and dropped once the car is upright again (`+0xC2` = 0, 0x463D03).
    pub(crate) flip_side: Option<bool>,
    /// The chase / heli slot's eye `+0x24` as its update left it (before the build's clamps, which
    /// work on a copy, 0x4651BF…0x4651DB) — the "old eye" of the next update's ceiling test.
    pub(crate) slot_eye: Option<Vec3>,
    /// The playback's wander of the aim point (`camera::ReplayWander`, 0x464C2B…0x464E42) and the
    /// (chosen camera, view word, trackside switch count) it was started under: SetSlotView
    /// 0x465790 zeroes it, which the port takes as a change of that key, and at a pass's start
    /// (`camera_slot_reset`). Read 2026-09-30: 0x465790 has five callers, all covered —
    /// 0x413960 twice (the trackside switch: the count; and the mode-3 cop view 0x0B → 0x0A, guarded
    /// by `[0x627F3C] == 10`, a word change), 0x4659E0 from 0x413EA0 (a replay's start) and 0x414850
    /// (a race's start: `camera_slot_reset`), and 0x462F30's two (`AnimCam`, view 0x0E, ending in
    /// SetSlotView(0x0A): the view word changes). So every SetSlotView either starts a pass or
    /// changes one of the three — CONFIRMED, no APPROXIMATION left.
    pub(crate) replay_wander: (nfs_game::camera::ReplayWander, (u16, u16, u32)),
    /// CircleCam's slot words (`camera::CircleCam`), the view they belong to and the trackside
    /// switch count they were started under: SetSlotView resets the count on a view change and on
    /// every enabled switch.
    pub(crate) circle: (nfs_game::camera::CircleCam, u16, u32),
    /// the camera slot's tracker slice `[0x627F5C]` (the draw distance keys on it)
    pub(crate) cam_slice: Option<usize>,
    pub(crate) wall_hits: u32,
    pub(crate) resets: u32,
    pub(crate) hud: Option<Hud>,
    /// current car's HUD dial records (km/h, tach) from `nfs5.car`
    pub(crate) dials: Option<(nfs_formats::fedata::DialSpec, nfs_formats::fedata::DialSpec, nfs_formats::fedata::DialSpec)>,
    /// player body paint (catalogue set 0 by default)
    pub(crate) paint: [u8; 3],
    /// the garage car's full appearance when a career race set `paint` from one — used while
    /// `paint` still equals its exterior
    pub(crate) owned_paint: Option<Paint>,
    /// that garage car's race number (`FECar+0x688`), with `owned_paint`
    pub(crate) owned_decal: Option<i8>,
    /// everything that makes a sound — see [`AudioState`]
    pub(crate) snd: AudioState,
    /// `.sim` name — also the name of the car's `.spl` engine-audio parameters
    pub(crate) sim_name: String,
    /// `SKILL [0x6573F4]` 0..2: `fe.txt skill` for a command-line race, `Game.Difficutly` (`Menu::difficulty`) for a layout launch (0x4EB470), the event's in career
    pub(crate) race_skill: u8,
    pub(crate) last_frame: Instant,
    /// what the window last showed, for the front end's idle redraw (`idle.rs`)
    pub(crate) presented: Option<idle::Presented>,
    /// the image of the last front-end frame rendered, kept under `NFS_FE_IDLE_VERIFY`
    pub(crate) idle_check: Option<Vec<u8>>,
    pub(crate) accumulator: Duration,
    pub(crate) tick: u64,
    /// The track map's cop blips, `[0x5CD158 + 4·i]` (colour, 0xC00000FF in `.data`) and
    /// `[0x60FDA8 + 4·i]` (stamp, 0 in `.bss`) by registration index `i` — written only by the
    /// map loop (0x43333E…0x433377) and never reset, so they carry over from race to race.
    pub(crate) cop_blips: Vec<(u32, i32)>,
    pub(crate) frames: u32,
    pub(crate) fps_timer: Instant,
}

/// The front end's half of [`App`]: the menu and the layout screens it shows, the 3D room and
/// the cars standing in it, the pointer, and every piece of per-screen state the original keeps
/// in its screen and widget objects — the open combo, a dragged or held scrollbar, the chronicle
/// column, the used-car and parts rows, the repair basket. Split out of `App` on 2026-09-21;
/// the fields kept their names apart from dropping a redundant `fe_` (`fe_room` is `room`),
/// so `self.x` became `self.fe.x`.
pub(crate) struct FrontEnd {
    pub(crate) menu: Option<Menu>,
    /// menu kept for returning after a race
    pub(crate) menu_template: Option<Menu>,
    /// `Factory.ProfileNum` — which of the six factory-team profiles `teamprofile.lay` shows. The
    /// accessor is a counted list (0x4BC1F0 at 0x5196D0: `[+0x14]` = 6 rows, the index `[+0x18]`
    /// starting at 0), stepped by the screen's `PROFILEINC` / `PROFILEDEC` (0x519A10 / 0x519A40:
    /// slots 0x54 / 0x50, wrapping). *(Corrected 2026-09-25: this said seven profiles, a default
    /// of 6 from 0x4BA310, and no control to move it — the 6 is the row count.)*
    pub(crate) factory_profile: usize,
    /// where the pointer is, in **design** coordinates (640x480), or `None` when it is outside
    /// the window. The front end is mouse-driven (`RE/notes/controls.md` §4.39); this is what
    /// feeds `LayoutScreen::set_cursor`.
    pub(crate) cursor: Option<(f32, f32)>,
    /// the front end's 3D room and the car standing in it (`RE/notes/frontend-3d.md`), each keyed
    /// by the model name so a screen change only reloads what actually changed
    pub(crate) room: Vec<(String, CarModel)>,
    /// the cars standing in the front end — the room's own (`usize::MAX`) and one per
    /// `NFS5.3DCAR` widget with a `positiontype` of its own — each with the model file, **body
    /// style** and paint it was built with, and the uploaded model. The last three are all part of
    /// the reload key — see `fe_room_models`.
    pub(crate) cars: Vec<(usize, String, i16, [u8; 3], CarModel)>,
    /// what `cars` was loaded for, with each car's style overlay (the widget's 0x4C0340…0x4C0486)
    pub(crate) cars_key: Vec<(usize, String, i16, Paint, Vec<[i16; 3]>, Option<i8>)>,
    /// what the front end's cars reflect: `FEDU` / `FE  ` / `FESH` of `carcmn.fsh` (0x441AF1, uploaded
    /// once) and whether `Graphics.CarChrome` builds the `CarExtEnv` passes (0x44F020 makes the word
    /// as the race's 0x44ED10 does) — the chrome flag is part of the reload key
    pub(crate) cars_env: Option<nfs_game::world::CarEnv>,
    /// milliseconds the front end has been up, for the turntable. Matches `FE3DRoom::draw`
    /// 0x4E9FF0's own `YRot` build (0x4EA09B…0x4EA0CF, `frontend-3d.md` §3 and the 2026-09-29
    /// dated paragraph): every draw reads `now = TIMER_getTicks()` (0x53C2E0 → `[0x6B7C7C]`,
    /// `timing.md`) — the OS multimedia timer's own 128 Hz counter, ticking on its own thread
    /// since `TIMER_init` at process start, not a per-frame delta — and sets
    /// `YRot = now / (TIMER_FREQ(=128,\u{a0}[0x5DEB48]) · Spin)` outright; there is no
    /// accumulator to drift. This field is the port's stand-in for that absolute clock: it is
    /// seeded from `NFS_FE_CLOCK` (the same preload `LayoutScreen::clock_ms` takes, so a still
    /// can be pinned the same way) and from there accumulates real elapsed frame time
    /// (`draw_frame`'s `dt_ms`) the way a live session approximates the original's free-running
    /// timer. A `--screenshot` capture never calls `draw_frame` (`resume_inner`'s own loop steps
    /// transitions and room fades by a fixed `dt_ms`, not this clock), so with `NFS_FE_CLOCK` set
    /// the turntable is pinned exactly, with no residual real-time noise regardless of machine
    /// load — verified 2026-09-29 (3× under a concurrent `cargo build`, 5× under a 10-way CPU
    /// stress loop, all byte-identical).
    pub(crate) ms: u64,
    /// `FEDealership3DRoom`'s slide state (`nfs_game::room::DealerRow`), while the screen's room
    /// is `ID_DEALER`
    pub(crate) dealer_row: Option<nfs_game::room::DealerRow>,
    /// the dealership's second car `[+0x2F8]` — the one being left while the row slides
    pub(crate) dealer_prev: Option<CarModel>,
    /// the room car's `car+0x3A8…+0x3B0`: half its LOD-5 model box, which `RCar::load` stores
    /// (0x445090) and the garage's shadow projector scales by (0x4E9D82 / 0x4E9D99)
    pub(crate) room_car_half: Option<Vec3>,
    /// the garage room's matrix as the previous frame drew it — what `FEGarage3DRoom`'s draw
    /// 0x4E9CE0 builds the shadow's texture matrix from, before its base draw 0x4E9FF0 moves the
    /// car (0x4D1700 at 0x4EA06A) and turns the room (`[+0x288]` at 0x4EA0CF)
    pub(crate) garage_shadow_prev: Option<Mat4>,
    /// `Render/common.fsh`'s `FESH`, the garage's shadow map (0x4E9E4B), uploaded on first use
    pub(crate) garage_shadow_tex: Option<nfs_renderer::TextureId>,
    /// `FEVictory3DRoom`'s movie `[+0x2F0]` (`nfs_game::room::VictoryMovie`) and the front-end
    /// millisecond of its last render, while the room is `ID_VICTORY`
    pub(crate) victory_movie: Option<(nfs_game::room::VictoryMovie, u64)>,
    /// `FETrackSelect3DRoom`'s state (`nfs_game::room::TrackRoom`), while the room is `ID_TRACK`
    pub(crate) track_room: Option<nfs_game::room::TrackRoom>,
    /// The room object `[fe+0xE8]` and its own transition (`[+0x12C]`, a screen's): made with the
    /// first screen (0x4D5EFD) or on the tick after the last one went (0x4D280E), opened and
    /// started in (slots 0x6C / 0x78); sent out when a screen opens on another room (0x4D1C9F…
    /// 0x4D1CF4 in 0x4D1BD0), advanced every tick (0x4D2738) and deleted once down
    /// (0x4D2744…0x4D275D). See `fe_room_tick`.
    pub(crate) room_obj: Option<(&'static nfs_game::room::Room, nfs_game::layout::Transition)>,
    /// the pictures its panels show: the `nfs5.trk` row's `+0x44` art name and that
    /// `FEData/TrackArt/<art>.fsh`'s entries, decoded, with their `misc` offsets
    pub(crate) track_art: Option<(String, Vec<(nfs_formats::fsh::DecodedImage, [u16; 4])>)>,
    /// `FEData/Data/nfs5.trk`, read once for the track room's art names
    pub(crate) trk_table: Option<Vec<nfs_formats::career::TrackInfo>>,
    /// `[0x690A84]`: 1 after a location screen's `…DEC` arrow, 0 after an `…INC` — which way the
    /// track room turns (0x4EAA8B, 0x4EAB19)
    pub(crate) track_turn_back: bool,
    /// `--layout <name>`: draw a `FEData/Layouts/<name>.lay` screen instead of the menu
    pub(crate) layout_preview: Option<nfs_game::layout::LayoutScreen>,
    /// `bottombar.lay`, which the front end loads once and draws over every screen (0x4D39D3).
    pub(crate) layout_bottom: Option<nfs_game::layout::LayoutScreen>,
    /// The front end's music pattern now playing, `[this+0x10C]` of the front-end object —
    /// empty from its constructor 0x4D1A90 (0x4D1B1D), which main runs afresh before every entry
    /// (0x4B6C4B), and emptied again as the front end ends (0x4D5273). `fe_apply_music`.
    pub(crate) music_current: String,
    /// The class of the screen object `[fe+0xE0]` — the `callbacklist=` id it was built for; 0x4D1BD0
    /// builds a new one (its constructor runs) only when the layout names another
    /// (`callback_list_of`). Empty from the front end's constructor, like `music_current`.
    pub(crate) list_current: String,
    /// `[this+0x290]` of the `ID_MAINSCREEN` object: the `TIMER_getTicks()` (128 Hz) of its last
    /// music re-roll (0x510210, 0x510259). Never written by the constructors 0x50FF40 / 0x5094f0,
    /// so it starts as whatever the heap held — APPROXIMATION: 0.
    pub(crate) main_music_stamp: i64,
    /// The process clock the 128 Hz `TIMER_getTicks()` counts from (`TIMER_init` at process start,
    /// `frontend-3d.md` §3): this port's App construction stands in for it.
    pub(crate) timer_origin: std::time::Instant,
    /// The loading screen's packs (`era0.fsh`, `loadtext.fsh`), made on the first
    /// `fe_loading_frame` — the original frees them at step 9 and reloads them next time.
    pub(crate) loading: Option<nfs_game::layout::loading::LoadingScreen>,
    /// `NFS_FE_LOADING=<step>`: draw 0x4DDA00's frame for that step instead of the screen, for a
    /// still.
    pub(crate) loading_still: Option<u32>,
    /// 0x4A4A70's surface and packs — the loading screen on the way into a race
    pub(crate) race_loading: Option<nfs_game::layout::loading::RaceLoading>,
    /// `NFS_RACE_LOADING=<step>`: draw 0x4A4A70's frame after steps 0…step instead of the screen,
    /// for a still, with the race the front end has set up
    pub(crate) race_loading_still: Option<i32>,
    /// `NFS_FE_LOADSCRN=1`: 0x4DD600's picture instead of the screen, for a still
    pub(crate) startup_still: Option<nfs_game::layout::loading::LoadingScreen>,
    /// `FEData/Locale/FEStrings.loc` and `hud.loc` (`nfs_game::layout::load_strings`), for the
    /// layout string ids and the HUD's
    pub(crate) strings: nfs_formats::locale::Strings,
    /// `Parts.AllParts`'s two collapsible sections, `[this+0x2C]` and `[this+0x2D]` — both
    /// **false** out of the constructor (0x515190, 0x515193), so the list opens as two header
    /// rows and a click on one flips it (0x51548B / 0x5154B2)
    pub(crate) parts_open: [bool; 2],
    /// the parts shop's and the advanced repair screen's baskets and current rows (§4.160)
    pub(crate) shop: parts_shop::PartsShop,
    pub(crate) layout_name: Option<String>,
    /// selected button on the previewed screen, and the screens we came from
    pub(crate) layout_sel: usize,
    /// whether `layout_sel` has actually been moved by an Up/Down press on this screen —
    /// gates the "selected" state (§4.111: `[widget+0x10C] & 8`, set only by the chronicle's
    /// five model-group buttons, 0x5077D0) so the port's own keyboard cursor — an APPROXIMATION
    /// the original has no counterpart for (`controls.md` §4.170.2, the FE is pointer-only,
    /// §4.38) — is never painted before the player has actually used it. Before any Up/Down,
    /// `layout_sel` still names the control ENTER would activate (index 0, as the original's own
    /// tab order would put the first control), but nothing highlights it — matching the
    /// original's idle screens, where no widget is ever `selected` by default. Reset to `false`
    /// wherever `layout_sel` resets to 0 (a fresh screen); set on `MenuKey::Up` / `MenuKey::Down`.
    pub(crate) layout_nav_used: bool,
    /// the combo whose list is open, by the flattened index `combo_sliders` uses
    pub(crate) layout_open_combo: Option<usize>,
    /// a list whose scrollbar knob is being dragged: which list, where the grab was, and the
    /// position it was on — `[this+0x104]` and `[this+0x108]` in the original (0x4D0973)
    pub(crate) list_drag: Option<(usize, f32, usize)>,
    /// the open combo's **own** scrollbar being dragged (§4.90's `FeListScrollBar`, not
    /// `NFS5.VerticalSlider`) — same shape as `list_drag`: which combo, the grab's y
    /// (`combo_scroll_grab`), and the position it was on
    pub(crate) combo_drag: Option<(usize, f32, usize)>,
    /// a held scrollbar arrow: which list, which way, and how long since the last step
    pub(crate) list_repeat: Option<(usize, i32, u32)>,
    /// a held **combo drop-down's own** scrollbar arrow — `FeListScrollBar::onPress` 0x4DB040
    /// (vtable 0x5B6A64, §4.101; a distinct class from `list_repeat`'s `NFS5.VerticalSlider`).
    /// Arm 1/2 sets `[this+0xDC] = ∓1`, starts `0x529220(this, 0x96, 1)` — the same 150 ms
    /// repeating timer `list_repeat`/`setting_repeat` use — stores the handle at `[+0xD8]` and
    /// steps once immediately (slot 19, the direction). Every firing whose `[ev+8]` matches that
    /// handle and whose `[+0xEC] == [+0xF0]` (0x4DB1D0) steps again; the release 0x4DB180 kills
    /// the timer with 0x5292D0 and clears `[+0xD8]`/`[+0xDC]`. `(index, region, ms since last step)`.
    pub(crate) combo_arrow_repeat: Option<(usize, u32, u32)>,
    /// the screen manager's layers and their histories, `[fe+0xD8]` (`nfs_game::layout::nav`,
    /// `controls.md` §4.166)
    pub(crate) nav: nfs_game::layout::nav::Nav,
    /// the screens of the layers under the top one, bottom first, kept loaded and covered
    pub(crate) layout_under: Vec<(String, nfs_game::layout::LayoutScreen)>,
    /// the `NFS5.GouraudIndicatorBar` the pointer took — `FeGouraudIndicatorBar`'s slot 8 asks
    /// 0x4C7DD0 for focus before it drags, and the press is what grants it (§4.81)
    pub(crate) bar_grab: Option<String>,
    /// `[0x5DBA44]` — which model group the chronicle is on, 0…4, switched by the five buttons of
    /// [`CHRONICLE_GROUP_BUTTONS`] (§4.111).
    pub(crate) chronicle_group: usize,
    /// `[FEChronicleMainBoxControl + 0x154]` — the timeline column a click has put the chronicle
    /// on, `None` while nothing has been clicked (§4.120).
    pub(crate) chronicle_col: Option<usize>,
    /// `[0x699CFC]` — which of the media list's files the picture screens draw: 0 whenever
    /// 0x5074A0 rebuilds the list (a click on a timeline cell), the `data.number` of a thumbnail
    /// released by 0x5089C0.
    pub(crate) chronicle_sel: usize,
    /// `Chronicle.Year`'s index into its range (year − 1948) once the slider or a group button
    /// has moved it (`None` = the group's earliest year, what the constructor 0x506E30 sets).
    pub(crate) chronicle_year_idx: Option<i32>,
    /// A drag of `NFS5.CHRONICLESLIDER`'s knob: the anchor x (`[helper+0x100]`) and the index it
    /// started on (`[helper+0x108]`), 0x4D0900.
    pub(crate) chronicle_slider_drag: Option<(i32, i32)>,
    /// `[ColorBox + 0x29C]` — what the colour accessor held when the editor came up (its row,
    /// slot 0x28, 0x4C3CC5), which cancel hands back through slot 11 (0x4C4A0E): a list row, or
    /// the custom colour the car carried.
    pub(crate) picker_prev: Option<Result<usize, u32>>,
    /// `[FEUsedCarsScreen + 0x290]` — whether `usedcars.lay` is on the parts panel rather than
    /// the damage one. The constructor leaves it clear (§4.113).
    pub(crate) usedcars_showparts: bool,
    /// Which market car `UsedCar.Car` is on — the used-car list's current row.
    pub(crate) usedcar_sel: usize,
    /// The dealership's `DealerCar.Car` / `.Colour` / `.IntColour` rows in a career — its own
    /// list (`fn dealer_cars`), not the quick race's catalogue index.
    pub(crate) dealer: DealerSel,
    /// `[0x699FE0]` is `DealerCar.Car` — the showcase / compare screen was opened by
    /// `SHOWCASE.DEALERCAR` (a per-screen vtable-slot-54 thunk, `0x50E5C0`) rather than
    /// `CARCOMPARE.MERGEDCAR` (`0x50DC40`) — corrected 2026-09-29, see `App::showcase_text`'s doc
    /// comment for the registration-table trail
    pub(crate) compare_dealer: bool,
    /// `Tournament.RemainingTracks`' row (its accessor's own index; `TOURNREMAININC` / `DEC`)
    pub(crate) remaining_sel: usize,
    /// Which row of `Parts.AllParts` the player has clicked — the accessor's own current index in
    /// the original, which its refresh reads back through `slot27` (§4.114).
    pub(crate) parts_sel: Option<usize>,
    /// A held `NFS5.CARSETTINGBUTTON`: the accessor it steps, its `data.step`, and how long since
    /// the last tick. Its press 0x4C1F40 starts `0x529220(this, 0x96, 1)` — the same 150 ms
    /// repeating timer the scrollbar arrows use — keeps the handle in `[this+0x14C]` and steps
    /// once immediately (0x4C1F67); 0x4C1F80 steps again on every firing whose `[ev+8]` matches
    /// that handle, and the release 0x4C1FB0 kills it with 0x5292D0 and clears the slot. §4.108.
    pub(crate) setting_repeat: Option<(String, i32, u32)>,
    /// whether the left button is held, for the scrollbar arrows' pressed frame (§4.101)
    pub(crate) mouse_down: bool,
    /// which of `repair.lay`'s ten `QUICKFIX.*` boxes are ticked — the ten bool accessors
    /// registered at 0x4F252A…0x4F258B, all of which start **false** (§4.84, §4.95)
    pub(crate) repair_ticks: std::collections::BTreeSet<&'static str>,
    /// the pads as they were when a binding row took focus — the baseline 0x51F310 copies into
    /// `[[0x69C9E8]]`, which the capture 0x51F3B0 compares against every frame (§4.117)
    pub(crate) binding_base: Option<Vec<nfs_input::bindings::PadRecord>>,
    /// `Controller.AllControllers`' row — the device `DEFAULTCONTROLS` binds (§4.103). The
    /// original's list leads with the keyboard, this port's has only the pads, so row `r` here
    /// is the original's row `r + 1`.
    pub(crate) controllers_sel: usize,
    /// `Controller.AllPads`' row (`deadzone.lay`) — the pad whose sixteen `<Axis>.Min` / `.Max`
    /// values the dead-zone accessors hold: their slot 2 (0x51BE70) swaps in that pad's own when
    /// the row changes (§4.17). The list is the analogue devices alone, so the row is the pad.
    pub(crate) pads_sel: usize,
    /// `Graphics.Resolutions`' row (COVERAGE §14.6.3): the original lists
    /// every Direct3D device mode, `"%d x %d x %d"` (0x4BC710, width/height/depth from
    /// `0x44E720(i, &out)` over `0x44E780(i)`'s modes) — this port has no such enumeration and no
    /// exclusive-fullscreen concept, so [`App::resolution_list`] lists the **current monitor's**
    /// distinct video-mode sizes instead (`winit::monitor::MonitorHandle::video_modes`), an
    /// APPROXIMATION of "the modes this machine can actually run in" rather than the original's
    /// own device-mode list, which this port cannot reproduce.
    pub(crate) resolution_sel: usize,
    /// `OwnedCars.Car`'s list as [`App::fe_owned_select`] last saw it
    pub(crate) owned_seen: Option<Vec<usize>>,
    /// the mode the pick was made for once one was made, and the front end's window size to give back
    /// after the race (see [`App::apply_resolution`])
    pub(crate) resolution_race: Option<((u32, u32), Option<winit::dpi::PhysicalSize<u32>>)>,
    /// `NFS_FE_PARTS` has been applied (see `fe_car_parts_step`)
    pub(crate) parts_forced: bool,
    /// the widget the pointer was over, for `FEControl::onMouseEnter`'s sound (0x4C7B90)
    pub(crate) last_hover: Option<usize>,
    /// the pop-up that is up or fading, and what its answer is for (§4.147)
    pub(crate) dialog: Option<(nfs_game::dialog::Dialog, DialogFor)>,
    /// `NFS_FE_DIALOG` has been applied
    pub(crate) dialog_forced: bool,
    /// an answer the tick took, waiting for `fe_dialog_answer`
    pub(crate) dialog_answer: Option<(bool, DialogFor)>,
    /// the input dialog's text when it was answered (0x514980 reads it back)
    pub(crate) dialog_input: Option<String>,
    /// `Replay.List`'s row — the file `KILLREPLAY` and `STARTREPLAY` act on
    pub(crate) replay_sel: usize,
    /// `UserList.UserList`'s selected row when a click has moved it; otherwise the row
    /// `RebuildUserList` 0x4DEF60 selects — the profile `player.lst` names
    pub(crate) user_sel: Option<usize>,
    /// `[0x5D8F2C]`, the driver picture chosen on `createplayer` (0x4CF780 reads it, 0x4CF790
    /// writes it): 8 in the image, 8 again on `CREATEPLAYERENTER` (0x51078B), and the clicked
    /// picture's `data.ournumber` on `NFS5.PLAYERDRIVERPICTURE`'s release (0x4CF940).
    /// `CREATEPLAYERDONE` hands it to 0x4DE5B0 (0x510874) as the new profile's picture.
    /// `NFS_FE_DRIVER_PICTURE=<n>` starts it at `n`, for a still of the chosen frame.
    pub(crate) driver_picture: i32,
    /// whether the end-of-era show's `MusicPlay("vic*", 2)` (0x4FD538) has been started for this
    /// opening of the screen
    pub(crate) era_music: bool,
    /// the CD player's two list selections, `CDPlayer.AvailableList` / `SelectedList` rows
    pub(crate) cd_available: Option<usize>,
    pub(crate) cd_selected: Option<usize>,
    /// `CDPlayer.Title`'s index `[this+0x18]` and the current song `[0x5D7938]` (−1 at start)
    pub(crate) cd_title: usize,
    pub(crate) cd_current: i32,
    /// the race running is a playback `STARTREPLAY` started (`[0x699FC9]`)
    pub(crate) after_replay: bool,
    /// The test drive running: `Game.GameType` 6 dealer, 7 used, 8 owned (`[0x5DC1C0]`, armed by
    /// `TESTDRIVENEW` / `TESTDRIVEUSED` / `TESTDRIVECARSETUP`), and the car record it carries
    pub(crate) test_drive: Option<(u8, Option<nfs_game::career::OwnedCar>)>,
    /// `Game.GameType` `[0x6573E8]` as the front end holds it: 0 at start-up (0x4B6C02), then
    /// what the six `SETTYPE*` handlers of `FEMainScreen` write — `QUICKRACE` 0 (0x510C30),
    /// `QUICKKNOCKOUT` 1 (0x510D10), `KNOCKOUT` 2 (0x510CA0), `MULTIPLAYER` 3 (0x510B10),
    /// `CAREERMODE` 4 (0x510B50), `FACTORYDRIVER` 9 (0x510BC0) — and the two `TESTDRIVE`s that
    /// write it (`prerace` 5, `VictoryRoom` 11)
    pub(crate) game_type: u8,
    /// `Comms.GameMode` — `SETMODENET` / `LAN` / `MODEM` / `IP` write 0 / 1 / 2 / 3
    /// (0x510D80 / 0x510E00 / 0x510E30 / 0x510E60); read by the rebuild after a network race
    pub(crate) comms_game_mode: u32,
    /// the update thread 0x4A5750's state while it is **2** (running): `Some((ticks it still runs,
    /// dialog 78 not yet re-stamped))`; `None` = state 0 (`network.md` §11.22). See `update_check_tick`
    pub(crate) update_check: Option<(u32, bool)>,
    /// `Camera.Current` — the type-4 accessor 0x4E4D40 registers (0x4E4D6A) and sets to 0
    /// (0x4E4D7C), whose selection `NFS5.CAMERASELECT`'s slot 9 0x4BF5F0 writes and whose value its
    /// draw 0x4BF410 highlights. Only those widgets read it: the launch 0x4EEDA0 takes
    /// `Camera.1`…`Camera.4` for the car's list and never `Camera.Current`, so it chooses nothing
    /// in the race — it only says which of the four `camera_list` entries `NFS5.CAMERAPICTURE`'s
    /// release (0x4BF920) writes.
    pub(crate) camera_current: usize,
    /// `Camera.1 … Camera.4` — the view ids of the car's camera list, registered by 0x4E4D40 as
    /// 0, 1, 2, 3 (`push` 0x4E4DB4, 0x4E4DF0, 0x4E4E2C, 0x4E4E64 before the `SetSelection` calls
    /// 0x4E4DB8 / 0x4E4DF4 / 0x4E4E30 / 0x4E4E68) and written by
    /// `NFS5.CAMERAPICTURE`'s slot 9 0x4BF920. The launch's per-car routine 0x4EEDA0 sends them
    /// as `.CAMERA` 0x148 for the player's slot (0x4EF02A…0x4EF0F5) — after `fe.txt`'s own
    /// `car[0].camera` in the launch list, so they are what `car[0].camera` holds in a race the
    /// front end started (`network.md` §11.12c). They persist for the run: 0x4E5D00 runs the data
    /// area's slot 0 (0x4E4D40) at every front-end entry, but the accessor constructor's
    /// registration 0x4B7B30 renames a second `Camera.1` (appends `"%d"`, 0x4B7C93) rather than
    /// replacing the first, and every reader looks the accessor up by name.
    pub(crate) camera_list: [i32; 4],
    /// When each `NFS5.CAMERAPICTURE` last took a release that assigned (`[this+0x140]`, stored at
    /// 0x4BF9EE): a release within `[0x5DEB48] / 2` (half a second) of the last is not one
    /// (0x4BF969…0x4BF984), and clears it (0x4BF99B)
    pub(crate) camera_pic_click: std::collections::HashMap<usize, Instant>,
    /// `Game.GameType` of the race running when the layout front end started it — what 0x4D3420
    /// keys the screen stack it rebuilds on once the race is over (`fe_after_race`). `None` for a
    /// race from the text menu or the command line, which go back to the text menu.
    pub(crate) race_game_type: Option<u8>,
    /// `XXDefXX` (`App::startup_side`): the start-up state 0x4DE880 saves at 0x4D3B76, which
    /// `CREATEPLAYERDONE`'s 0x4DE5B0 makes every new profile from
    pub(crate) xxdefxx: Option<nfs_game::save::SaveSide>,
    /// `nfs5.trk` / `nfs5.trn` as 0x4FBC10 reads them, for rebuilding the two track lists
    pub(crate) track_tables: nfs_game::track_list::TrackListTables,
}

/// What a front-end dialog's answer is for — each is one of the raise sites §4.147 tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogFor {
    /// `QUIT` (0x50CC10) → `FEStrings` 811; "yes" is 0x4D56E0(0), out to the system (0x50CC53)
    Quit,
    /// the start check's 858 (0x4D5AE8): "yes" clears the other part-way tournaments and runs the
    /// check again (0x4D2C94…0x4D2CA3)
    TournamentReset,
    /// a one-button notice whose answer nothing polls (813, 857)
    Notice,
    /// the pre-race screen's offer, 807 (0x505E79): "yes" sells (0x502190), "no" declines
    /// (0x4F9720); the garage index of the car
    Offer(usize),
    /// `FORFEITTOURNY` → 824 (0x506130): "yes" forfeits the tournament (0x503E40) and goes back
    /// one screen (0x4D23F0(0) → 0x4D5FB0)
    Forfeit,
    /// `KILLREPLAY` → 808 (0x50D3A0): "yes" deletes the selected `.rpl` and rebuilds the list
    /// (0x50CC7E → 0x50D3C0)
    KillReplay,
    /// the garage's `SELLCAR` → the input dialog 800 (built at 0x50F8F5: digits, eight of them):
    /// "yes" with a price lists the selected car at it (0x4F91C0) and runs the market tick
    /// 0x4F7DB0 (the garage tick 0x50FB32…0x50FC9F)
    SellPrice,
    /// a join the host turned down for a name clash: the input dialog 821 (0x5097C2) asks for
    /// another; "yes" joins again under it, anything else goes back
    NetName,
    /// `CREATE` on a games screen -> the input dialog 601 (0x509E40): "yes" sets `Comms.GameName`
    /// (15 characters at most) and creates the session (0x50A050 -> 0x509A00), then opens
    /// `creategame`; "no" only takes the dialog down
    NetGameName,
    /// a join that failed (820 full, 831 could not connect…): one button, then back
    NetFail,
    /// 817 `IDS_MPCONNECTING`, kept up while the comm state is 3 (0x5095A5); its button cancels
    /// (0x5095C0…: teardown 0x48E9C0) and the screen goes back
    NetConnecting,
    /// 78 `IDS_CHECKINGFORUPDATES`, `SETMODENET`'s one-button dialog (0x510D80 → 0x512F70(0x4E, 0) into
    /// `[+0x29C]`), kept up while the update thread's state is 2 (tick 0x5103F0…0x510493); its button
    /// deletes it and goes back one screen (0x5104A2…0x5104DA)
    UpdateCheck,
    /// the paint shop's `CHANGEDECAL` → the input dialog 251 (0x5111B0): "yes" gives the garage
    /// car the typed number, or −1 when nothing was typed (0x5112ED…0x51146F)
    Decal,
    /// `REMOVEPLAYER` → 803 (0x510770 raises the dialog the screen built at 0x51015A): "yes"
    /// deletes the selected profile (0x5103F0 → 0x4DE8A0), 804 when that fails
    RemovePlayer,
}

/// What the front end's driver 0x4D3420 reads when it comes back from a race
/// (0x4D3EB2…0x4D5202). Each field names its source.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct AfterRace {
    /// `[0x699FC9]` (getter 0x50C120): the race was a replay's playback
    pub(crate) replay: bool,
    /// `[0x657E34]`: the race was left from the pause menu
    pub(crate) aborted: bool,
    /// `Game.GameType` `[0x6573E8]`
    pub(crate) game_type: u8,
    /// 0x4DDF00, the knockout series still running — after 0x4EDD00 has run 0x4DE060
    pub(crate) knockout_active: bool,
    /// the network session `[0x628C70]` connected (0x4D4072…0x4D40A6): `Comms.GameMode` (the
    /// accessor's slot 0x28 at 0x4D4119) and 0x48CAA0 — this machine is the host
    pub(crate) net: Option<(u32, bool)>,
    /// `TOURNAMENTLASTRACE` `[0x6577D8]`
    pub(crate) last_race: bool,
    /// `TOURNAMENTENDOFERA` `[0x6577DC]`
    pub(crate) end_of_era: bool,
    /// 0x505800 = `[0x691E81]`, the era changed with this race
    pub(crate) era_changed: bool,
    /// 0x51AF90: a Factory Driver promotion is pending
    pub(crate) promotion: bool,
    /// 0x51B020: every Factory Driver mission is passed
    pub(crate) missions_done: bool,
}

/// **0x4D3420's screen stack after a race** — the ancestors bottom to top and the screen opened.
/// The manager `[this+0xD8]` is built afresh on every entry to the front end, so its one group
/// `vec[0]` (0x4D6520(0) is `&vec[0]`, 0x4D6520…0x4D6529) starts with an empty history; each
/// 0x4D5BE0 appends a name to that history (a `push_back` on `group+8`, 0x4D5C41…0x4D5C60) and the
/// closing 0x4D5DF0(`vec[0]`, name, 1) opens the screen — so the history reads bottom to top in
/// the order it is built (`nfs_game::layout::nav::Nav::push_history`). The entry screen is
/// `gamesetup`, main's argument (0x4B6C52 → 0x4B6C5B).
///
/// ```text
/// replay (0x4D3EB2)            0x4D2100(gamesetup, 0), 0x4D2100(loadreplay, 1)
/// aborted, type ∉ 5…9          0x4DE040 (the knockout cancelled), then
///   type 11 (0x4D3F34)         [E] victoryroom          else   E alone (0x4D3FF8)
/// session connected (0x4D40AC) [E, by Comms.GameMode: 0 serverlist udpip | 1 local | 2 modem |
///                              3 peertopeerip | >3 nothing (jump table 0x4D55B0)],
///                              creategame when 0x48CAA0 (host) else joingame
/// type 6 / 7 / 8               [E, tournament, garage] dealership / usedcars / carsetup
/// type 4                       !last race → [E, tournament] prerace (0x4D4757 → 0x4D4887)
///                              !end of era → [E] tournament (0x4D4763 → 0x4D4997)
///                              era changed → [E, VictoryRoom] endofera (0x4D476F)
///                              else [E] tournament (0x4D4989)
/// type 5                       [E, tournament] prerace (0x4D487E)
/// type 11 / 10                 [E] victoryroom (0x4D4A5A) / [E] tournament (0x4D4B1A)
/// type 9                       aborted → [E] factorydriver (0x4D4BE3)
///                              promotion → [E (, factorydriver unless all done)],
///                                0x4D2100(factoryvictory, 0), 0x4D2100(levelvictory, 1)
///                              missions left → [E] factorydriver (0x4D4D72)
///                              else E alone (0x4D4E36)
/// other types (0x4D4EA2)       knockout running → [E] knockoutraceinfo
///                              0 quickrace / 1 quickknockout / 2 knockout, each over [E]
///                              else E alone (0x4D517C)
/// ```
///
/// A 0x4D2100 with offset 0 opens its screen in the top layer and records it (third argument 1);
/// one with offset 1 appends a new layer (`nfs_game::layout::nav`). So the replay is `[E]` on
/// `vec[0]` and `loadreplay` alone on a second layer, and the promotion is
/// `[E, (factorydriver,) factoryvictory]` on `vec[0]` with `levelvictory` on a second layer — the
/// third member of the result. Every other arm is 0x4D5BE0s and one 0x4D5DF0(`vec[0]`, name, 1).
/// The front end's entry screen — main's 0x4D3420 call passes `"gamesetup"` (0x4B6C52).
pub(crate) const FE_ENTRY: &str = "gamesetup";

pub(crate) fn after_race_screens(a: &AfterRace) -> (Vec<&'static str>, &'static str, Option<&'static str>) {
    const E: &str = FE_ENTRY;
    if a.replay {
        return (Vec::new(), E, Some("loadreplay"));
    }
    // the promotion arm (0x4D4D40…0x4D4D68) comes after the session test, as in the match below
    if a.promotion && a.game_type == 9 && !a.aborted && a.net.is_none() {
        return (if a.missions_done { vec![E] } else { vec![E, "factorydriver"] }, "factoryvictory", Some("levelvictory"));
    }
    let (stack, screen) = after_race_single(a);
    (stack, screen, None)
}

fn after_race_single(a: &AfterRace) -> (Vec<&'static str>, &'static str) {
    const E: &str = FE_ENTRY;
    if a.aborted && !(5..=9).contains(&a.game_type) {
        return if a.game_type == 11 { (vec![E], "victoryroom") } else { (Vec::new(), E) };
    }
    if let Some((mode, host)) = a.net {
        let mut stack = vec![E];
        match mode {
            0 => stack.extend(["serverlist", "udpip"]),
            1 => stack.push("local"),
            2 => stack.push("modem"),
            3 => stack.push("peertopeerip"),
            _ => {}
        }
        return (stack, if host { "creategame" } else { "joingame" });
    }
    match a.game_type {
        6 => (vec![E, "tournament", "garage"], "dealership"),
        7 => (vec![E, "tournament", "garage"], "usedcars"),
        8 => (vec![E, "tournament", "garage"], "carsetup"),
        4 if a.last_race && a.end_of_era && a.era_changed => (vec![E, "VictoryRoom"], "endofera"),
        4 if a.last_race => (vec![E], "tournament"),
        4 | 5 => (vec![E, "tournament"], "prerace"),
        11 => (vec![E], "victoryroom"),
        10 => (vec![E], "tournament"),
        9 if a.aborted => (vec![E], "factorydriver"),
        9 if !a.missions_done => (vec![E], "factorydriver"),
        9 => (Vec::new(), E),
        _ if a.knockout_active => (vec![E], "knockoutraceinfo"),
        0 => (vec![E], "quickrace"),
        1 => (vec![E], "quickknockout"),
        2 => (vec![E], "knockout"),
        _ => (Vec::new(), E),
    }
}

#[cfg(test)]
mod after_race_tests {
    use super::{AfterRace, after_race_screens};

    /// the one-layer arms: history under the screen, no second layer
    fn run(a: AfterRace) -> (Vec<&'static str>, &'static str) {
        let (history, screen, layer) = after_race_screens(&a);
        assert_eq!(layer, None, "{screen}");
        (history, screen)
    }

    /// 0x4D3420 after a race: the single-player types go back to their own set-up screen over
    /// `gamesetup` (0x4D4F63…0x4D5142), a running knockout to `knockoutraceinfo` (0x4D4EA2), a
    /// pause-menu quit to `gamesetup` alone (0x4D3FF8) and a replay to `loadreplay` (0x4D3EB2).
    #[test]
    fn single_player_returns() {
        assert_eq!(run(AfterRace { game_type: 0, ..Default::default() }), (vec!["gamesetup"], "quickrace"));
        assert_eq!(run(AfterRace { game_type: 1, ..Default::default() }), (vec!["gamesetup"], "quickknockout"));
        assert_eq!(run(AfterRace { game_type: 2, ..Default::default() }), (vec!["gamesetup"], "knockout"));
        assert_eq!(run(AfterRace { game_type: 1, knockout_active: true, ..Default::default() }), (vec!["gamesetup"], "knockoutraceinfo"));
        assert_eq!(run(AfterRace { game_type: 0, aborted: true, ..Default::default() }), (vec![], "gamesetup"));
        assert_eq!(run(AfterRace { game_type: 11, aborted: true, ..Default::default() }), (vec!["gamesetup"], "victoryroom"));
        assert_eq!(run(AfterRace { game_type: 3, ..Default::default() }), (vec![], "gamesetup"));
        // 0x4D2100(gamesetup, 0, 1), then 0x4D2100(loadreplay, 1, 1): a layer of its own
        assert_eq!(
            after_race_screens(&AfterRace { replay: true, aborted: true, game_type: 6, ..Default::default() }),
            (vec![], "gamesetup", Some("loadreplay"))
        );
    }

    /// The network return (0x4D40AC…0x4D4342): the protocol screen by `Comms.GameMode` through the
    /// jump table 0x4D55B0, then the room — before any game-type test, so even a test drive's.
    #[test]
    fn network_returns() {
        let net = |mode, host| run(AfterRace { game_type: 3, net: Some((mode, host)), ..Default::default() });
        assert_eq!(net(0, true), (vec!["gamesetup", "serverlist", "udpip"], "creategame"));
        assert_eq!(net(1, false), (vec!["gamesetup", "local"], "joingame"));
        assert_eq!(net(2, true), (vec!["gamesetup", "modem"], "creategame"));
        assert_eq!(net(3, false), (vec!["gamesetup", "peertopeerip"], "joingame"));
        assert_eq!(net(4, false), (vec!["gamesetup"], "joingame"));
        // a pause-menu quit of a network race is caught first (0x4D3EF1)
        assert_eq!(run(AfterRace { game_type: 3, aborted: true, net: Some((1, true)), ..Default::default() }), (vec![], "gamesetup"));
    }

    /// Test drives (0x4D4347…0x4D4736, 0x4D487E, 0x4D4A5A) and the career's three gates
    /// (0x4D4757 / 0x4D4763 / 0x4D476F); an aborted type 5…9 takes the same way.
    #[test]
    fn career_returns() {
        let t = |game_type, aborted| run(AfterRace { game_type, aborted, ..Default::default() });
        assert_eq!(t(6, true), (vec!["gamesetup", "tournament", "garage"], "dealership"));
        assert_eq!(t(7, false), (vec!["gamesetup", "tournament", "garage"], "usedcars"));
        assert_eq!(t(8, false), (vec!["gamesetup", "tournament", "garage"], "carsetup"));
        assert_eq!(t(5, true), (vec!["gamesetup", "tournament"], "prerace"));
        assert_eq!(t(11, false), (vec!["gamesetup"], "victoryroom"));
        assert_eq!(t(10, false), (vec!["gamesetup"], "tournament"));
        assert_eq!(t(4, false), (vec!["gamesetup", "tournament"], "prerace"));
        assert_eq!(t(4, true), (vec![], "gamesetup"));
        let c = |last_race, end_of_era, era_changed| run(AfterRace { game_type: 4, last_race, end_of_era, era_changed, ..Default::default() });
        assert_eq!(c(true, false, false), (vec!["gamesetup"], "tournament"));
        assert_eq!(c(true, true, true), (vec!["gamesetup", "VictoryRoom"], "endofera"));
        assert_eq!(c(true, true, false), (vec!["gamesetup"], "tournament"));
    }

    /// Factory Driver (0x4D4BDA…0x4D4E9D).
    #[test]
    fn factory_driver_returns() {
        let f = |aborted, promotion, missions_done| run(AfterRace { game_type: 9, aborted, promotion, missions_done, ..Default::default() });
        assert_eq!(f(true, true, false), (vec!["gamesetup"], "factorydriver"));
        // 0x4D2100(factoryvictory, 0, 1) on `vec[0]`, then 0x4D2100(levelvictory, 1, 1)
        let p = |missions_done| after_race_screens(&AfterRace { game_type: 9, promotion: true, missions_done, ..Default::default() });
        assert_eq!(p(false), (vec!["gamesetup", "factorydriver"], "factoryvictory", Some("levelvictory")));
        assert_eq!(p(true), (vec!["gamesetup"], "factoryvictory", Some("levelvictory")));
        assert_eq!(f(false, false, false), (vec!["gamesetup"], "factorydriver"));
        assert_eq!(f(false, false, true), (vec![], "gamesetup"));
    }

    /// The rebuilt manager: BACK walks the rebuilt history (0x4D5FB0), and a promotion's
    /// `levelvictory` is a second layer on which BACK is off (0x4D6320).
    #[test]
    fn rebuilt_history_walks_back() {
        use nfs_game::layout::nav::{Nav, Shown};
        let build = |a: AfterRace| {
            let (history, screen, layer) = after_race_screens(&a);
            let mut n = Nav::new();
            for h in history {
                n.push_history(h);
            }
            n.show_first(screen);
            if let Some(l) = layer {
                n.open(l, 1, true);
            }
            n
        };
        let mut n = build(AfterRace { game_type: 6, ..Default::default() });
        assert_eq!(n.current(), Some("dealership"));
        assert!(n.back_enabled());
        assert_eq!(n.back(0), Shown::Screen("garage".into()));
        assert_eq!(n.back(0), Shown::Screen("tournament".into()));
        assert_eq!(n.back(0), Shown::Screen("gamesetup".into()));
        assert!(!n.back_enabled());
        let n = build(AfterRace { game_type: 9, promotion: true, ..Default::default() });
        assert_eq!(n.current(), Some("levelvictory"));
        assert!(!n.back_enabled() && !n.one_layer());
        let n = build(AfterRace { game_type: 0, aborted: true, ..Default::default() });
        assert_eq!(n.current(), Some("gamesetup"));
        assert!(!n.back_enabled());
    }

    /// `HInfo`'s lag arm (0x427C7E…0x427EDF): the thresholds are the original's — 0x100 ticks (4 s) behind is a
    /// lost link, 0x20 (0.5 s) shows «Связь...», and the box closes above −0x10.
    #[test]
    fn the_lag_notice_opens_at_half_a_second_and_closes_at_a_quarter_of_that_above() {
        use super::{lag_step, LagStep};
        assert_eq!(lag_step(-0x101, true, false), LagStep::Lost);
        assert_eq!(lag_step(-0x101, false, false), LagStep::Show, "not connected: the second look, 0x427E15");
        assert_eq!(lag_step(-0x100, true, false), LagStep::Show);
        assert_eq!(lag_step(-0x21, true, false), LagStep::Show);
        assert_eq!(lag_step(-0x20, true, false), LagStep::States);
        assert_eq!(lag_step(0, true, false), LagStep::States);
        // with the box up it holds until the lag is back above −0x10
        assert_eq!(lag_step(-0x21, true, true), LagStep::Hold);
        assert_eq!(lag_step(-0x20, true, true), LagStep::Hold);
        assert_eq!(lag_step(-0x10, true, true), LagStep::Hold);
        assert_eq!(lag_step(-0x0F, true, true), LagStep::Close);
    }
}

/// Original game tick: 1/64 s (see module docs).
pub(crate) const GAME_TICK: Duration = Duration::from_nanos(15_625_000);

/// Knockout series (`RE/notes/modes.md` §5, CONFIRMED): `n` races, the last-placed car drops out
/// after each race, the grid is the previous finishing order, the player finishing last ends the
/// series.
///
/// **The series is the block `[0x690410]` and nothing else.** Everything a round needs besides it
/// is read live at the round's launch, as the original reads it: the round's track, laps,
/// traffic and direction from the row `Track<raceIndex+1>.*` (0x4FC20E formats `Track%d` with
/// `d = Knockout_raceIndex() + 1`, 0x4DDF10 at 0x4FC21B — so a row edited between rounds is
/// raced as edited), and opponent `i`'s car from entry `i − 1` of `OpponentCar.Car`'s seven
/// (0x4F5850 at 0x4DE1E7, `App::opp_cars`), which the series' start filled (0x4DDF58) and a quick
/// race in between refills (0x4EBC2D). *(Until 2026-09-26 the port kept its own copy of the rows
/// and the cars from the start, and a lossy copy of the block: no `last_pos`, the eliminated cars'
/// grid places left as they were instead of `−1 − i`.)*
pub(crate) struct KnockoutSeries {
    /// `[0x690410]`, 0x84 bytes (`modes.md` §5.2) — what the `Knockout` module saves verbatim
    pub(crate) state: nfs_formats::sav::KnockoutState,
    /// the text menu's status line (port-only; the layout front end has `knockoutraceinfo`)
    pub(crate) message: String,
}

/// One page of the post-race queue `[0x60C9C4]` (0x426C90): each opens when the one before closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultPage {
    /// every queued page has closed (the last one timed out) — nothing is drawn
    Closed,
    /// `HRaceSummary`
    Summary,
    /// the second `HCongrats` with the tournament place, queued after the summary on a
    /// tournament's last race when the place is 1–3 (0x426E5D)
    TournamentPlace(usize),
    /// `HTournamentStandings`, queued after the summary in a tournament (0x426EB9)
    Standings,
    /// `HInfo` 0x4282F0 with a text id — 18160 "You must place in the top 3…", queued after the
    /// standings on a tournament's last race when the player is not in the top three (0x426F52)
    Notice(u32),
    /// `HTrackRecords`, queued last (0x426F71, 0x4272FF)
    Records,
    /// `HMissionSummary` 0x428940 — the only page of a factory-driver mission (0x426FB8)
    MissionSummary,
}

/// **The result records** — what `Race_finalize(2)` 0x416AC0 leaves in every racer's
/// `car+0xCC4…+0xD2C` at race over (0x417010), kept past the restart that turns the race into its
/// automatic replay (`replay.md` §10: only 0x416AC0 writes `+0xCD4` / `+0xCD8` / `+0xC98`, and
/// the replay's car reset touches none of them). Everything the pages, the HUD tally 0x427660,
/// the mission summary and the race end read comes from here, not from the replay running under
/// the pages.
#[derive(Debug, Clone)]
pub(crate) struct RaceResults {
    /// by driver index (0 the player, `i + 1` opponent `i`)
    pub(crate) racers: Vec<nfs_game::race::RacerResult>,
    /// the ranking table 0x608B38 at race over, as driver indices
    pub(crate) order: Vec<usize>,
    /// `career::clean_race` of each racer's `+0xC98` against its start damage `[0x60C9C8]`
    pub(crate) clean: [bool; 8],
    /// the mission's `(pass, time, cones)` as the race left it. Settled 2026-09-30: a page
    /// rebuilt later reads the same numbers, so capturing them once is exact. `HMissionSummary`
    /// 0x4293F0 reads the records (`+0xCD4` 0x4294B8, `+0xCD8` 0x4294DE, `+0xC98` 0x434FCD — kept
    /// through the replay), the option globals `[0x65783C]` MINRANK / `[0x657824]` TIMELIMIT /
    /// `[0x657830]` CONEPENALTY / `[0x65782C]` CONEIMIT (registered in the option table at
    /// 0x4B27xx, written only by the mission launch's option set, nothing else — a data-reference
    /// scan finds no other writer) and the summary's cone tally `[this+0x140]`, whose writers are
    /// its constructor (0x428992, 0), 0x429830 (0, from 0x426A80(0) at a race's start, 0x426B3B —
    /// `0x426A80(REPLAY == 2)` at the replay's start passes 1 and skips it, 0x415345) and 0x429820
    /// (`inc`, from the HUD frame 0x425950 at 0x425DE7, which counts only while `[0x657424]` is 0 or
    /// 1 — never in the replay, 0x425D7F). The debug override `[0x60C9EC]` (0x427AF0, 0x429569 /
    /// 0x42957E) is never set by this port.
    pub(crate) mission: Option<(bool, u64, usize)>,
    /// the tick the queue was built at — `0x426A80(1)` runs at the replay's `RaceFrame` start,
    /// 0x41533A, before its first tick
    pub(crate) opened: u64,
    /// the race's `HCongrats` heads the queue — `0x426C90(0)`; the bar's 0xE queues the pages
    /// again with `0x426C90(1)`, which leaves it out (0x426CF2)
    pub(crate) congrats: bool,
}

impl RaceResults {
    pub(crate) fn times(&self) -> [i32; 8] {
        let mut t = [0; 8];
        for (i, r) in self.racers.iter().take(8).enumerate() {
            t[i] = r.time;
        }
        t
    }
}

/// World edits requested by the simulation.
pub(crate) enum WorldAction {
    Show(Vec<usize>, Vec<usize>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuKey {
    Up,
    Down,
    Left,
    Right,
    Enter,
    /// Backspace / East button: decline the finale, sell the selected car.
    Back,
}
