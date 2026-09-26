//! The front end's 3D rooms (`RE/notes/frontend-3d.md`).
//!
//! A `.lay` screen's `room=` names one of six room classes, each of which loads a model from
//! `FEData/Models` and turns the player's car on a turntable in front of the camera. The rig — where
//! the room sits, how fast the car turns, the camera's vertical angle and the whole lighting set —
//! is a table of numbers in each class's constructor, and the class's own tuning overlay
//! (0x4E7950) names every one of them, so the meanings below are the game's own labels.

/// One room's rig, exactly as its constructor sets it. CONFIRMED.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Room {
    /// the `room=` id a `.lay` screen names
    pub id: &'static str,
    /// the models in `FEData/Models`, drawn together (only the dealership has more than one)
    pub models: &'static [&'static str],
    /// `X` `Y` `Z` — where the room and the car sit in front of the camera
    pub pos: [f32; 3],
    /// `XRot` `YRot` `ZRot`, in turns
    pub rot: [f32; 3],
    /// `Spin` — seconds per turn of the turntable; 0 stands still
    pub spin: f32,
    /// `V.A.` — the camera's vertical angle in degrees
    pub view_angle: f32,
    /// `AR` `AG` `AB`, 0…255
    pub ambient: [f32; 3],
    /// `SR` `SG` `SB`, 0…255
    pub sun: [f32; 3],
    /// `SDirX` `SDirY` `SDirZ`
    pub sun_dir: [f32; 3],
    /// `SStr` and `SAng`
    pub sun_strength: f32,
    pub sun_angle: f32,
    /// `cardif` — the car's own distance from the room. **Dead** in the shipped build: only the
    /// class's tuning overlay reads it (0x4E7C31), so the car stands exactly where the room does.
    pub car_distance: f32,
    /// How far round the room's own models are turned. The base class turns them with the car
    /// (`None`, 0x4EA0E3); the dealership pins its three sections at half a turn and spins only the
    /// car (0x4E8D24, 0x4E8D46, 0x4E8D68).
    pub models_turn: Option<f32>,
}

/// The six registered rooms ([0x6909BC]); `ID_TESTROOM` is what an empty `room=` reads back as
/// (0x4E0BE0) and shares `ID_NONE`'s rig.
pub const ROOMS: [Room; 6] = [
    Room {
        id: "ID_GARAGE",
        models: &["garage"],
        pos: [0.0, -1.1, 4.8],
        rot: [0.0, 0.0, 0.0],
        spin: 30.0,
        view_angle: 37.5,
        ambient: [55.0, 65.0, 55.0],
        sun: [120.0, 120.0, 120.0],
        sun_dir: [-0.14, -0.98, 0.114],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 3.5,
        models_turn: None,
    },
    Room {
        id: "ID_DEALER",
        models: &["dealer2a", "dealer2b", "dealer2c"],
        pos: [0.0, -1.1, 4.8],
        rot: [0.0, 0.0, 0.0],
        spin: 30.0,
        view_angle: 37.5,
        ambient: [110.0, 110.0, 110.0],
        sun: [110.0, 110.0, 110.0],
        sun_dir: [0.0, -0.99, 0.15],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 5.0,
        models_turn: Some(0.5),
    },
    Room {
        id: "ID_FACTORY",
        models: &["factory"],
        pos: [0.0, -1.1, 4.8],
        rot: [0.0, 0.0, 0.0],
        spin: 30.0,
        view_angle: 37.5,
        ambient: [110.0, 120.0, 110.0],
        sun: [50.0, 50.0, 50.0],
        sun_dir: [0.0, -0.99, 0.10],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 5.0,
        models_turn: None,
    },
    Room {
        id: "ID_VICTORY",
        models: &["victory"],
        pos: [0.0, -1.1, 5.2],
        rot: [0.0, 0.0, 0.0],
        spin: 30.0,
        view_angle: 32.0,
        ambient: [75.0, 75.0, 75.0],
        sun: [110.0, 110.0, 110.0],
        sun_dir: [0.0, -0.98, 0.20],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 4.5,
        models_turn: None,
    },
    Room {
        id: "ID_TRACK",
        // `"track"` into `[+0x314]` and `"trackwall"` into `[+0x318]` (0x4EA409, 0x4EA444); the
        // draw is its own, `TrackRoom`
        models: &["track", "trackwall"],
        // the fly-in's start, `[+0x324…+0x32C]` and `[+0x33C]` (0x4EA45C…0x4EA476) — `TrackRoom`
        // moves it; `Spin` is dead here (no read of `[+0x270]` in 0x4EA320…0x4EB0C0)
        pos: TRACK_FLY_FROM,
        rot: [TRACK_FLY_XROT[0], 0.0, 0.0],
        spin: 20.0,
        view_angle: 25.0,
        ambient: [55.0, 65.0, 76.0],
        sun: [115.0, 115.0, 115.0],
        sun_dir: [-0.29, -1.5, 0.29],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 1.2,
        models_turn: None,
    },
    Room {
        id: "ID_NONE",
        models: &[],
        pos: [0.0, -1.2, 4.2],
        rot: [-0.01, 0.0, 0.0],
        spin: 30.0,
        // the empty room sets no `V.A.` of its own, so the screen keeps the last one; the port
        // falls back to the garage's
        view_angle: 37.5,
        ambient: [64.0, 64.0, 64.0],
        sun: [159.0, 159.0, 159.0],
        sun_dir: [0.0, -0.99, 0.10],
        sun_strength: 50.0,
        sun_angle: 0.8,
        car_distance: 1.0,
        models_turn: None,
    },
];

/// The room a screen's `room=` names; an empty id is `ID_TESTROOM`, which shares `ID_NONE`'s rig.
pub fn room(id: &str) -> Option<&'static Room> {
    let id = if id.trim().is_empty() { "ID_NONE" } else { id.trim() };
    let id = if id == "ID_TESTROOM" { "ID_NONE" } else { id };
    ROOMS.iter().find(|r| r.id == id)
}

impl Room {
    /// The turntable angle in turns at `ms` milliseconds: one whole turn every `spin` seconds
    /// (`YRot = now / (ticksPerSecond · Spin)`, 0x4EA0AE, and the model spin setter multiplies by
    /// −360 to make degrees).
    pub fn spin_turns(&self, ms: u64) -> f32 {
        if self.spin <= 0.0 {
            return 0.0;
        }
        -(ms as f32) / (1000.0 * self.spin)
    }

    /// Where the room (and its car) sits `zoom` of the way into a door: `(X·f, Y·f − zoom·0.8,
    /// Z·f + zoom·0.3)` with `f = 1 − zoom` (0x4E9FFF…0x4EA051) — `zoom` is `[room+0x2E8]`, the
    /// 3D car widget's door zoom (0x4C05E1 → 0x4E7800), 0 at rest.
    pub fn zoomed_pos(&self, zoom: f32) -> [f32; 3] {
        let f = 1.0 - zoom;
        [self.pos[0] * f, self.pos[1] * f - zoom * 0.8, self.pos[2] * f + zoom * 0.3]
    }
}

/// The channels a room's own tick turns on its model at tick `now` — the fans. `FEGarage3DRoom`'s
/// slot 1 **0x4E9C40** and `FEFactory3DRoom`'s **0x4E9990** write channel 1 = `now · 1/128` and
/// channel 2 = `now · 1/192` (`[0x5B273C]`, `[0x5B71AC]`) into the model through 0x443F30 — about
/// **z** in the garage (the float goes to the vector's third slot, `[esp+0x18]` after the two
/// pushes) and about **x** in the factory (its first, `[esp+0x10]`) — and `Fan01` / `Fan02` are
/// bound to channels 1 and 2 in both models: one turn a second and one every second and a half.
/// The other rooms' ticks turn nothing (the base `FE3DRoom`'s draw writes channels 8 and 9,
/// 0x4EA147…0x4EA1D2, which no room article is bound to).
pub fn room_channels(id: &str, now: u32) -> Vec<(u8, [f32; 3])> {
    // fild; fmul float; fstp float
    let (a, b) = ((now as f64 * 0.0078125) as f32, (now as f64 * (1.0f32 / 192.0) as f64) as f32);
    match id {
        "ID_GARAGE" => vec![(1, [0.0, 0.0, a]), (2, [0.0, 0.0, b])],
        "ID_FACTORY" => vec![(1, [a, 0.0, 0.0]), (2, [b, 0.0, 0.0])],
        _ => Vec::new(),
    }
}

/// `XOff` and `YOff` of the dealership (`[+0x2D4]` = 10.53, `[+0x2D8]` = −2.1, its constructor
/// 0x4E8B35 / 0x4E8B3F): how far one showroom section stands from the next, in the original's
/// camera space — along `x` and along `z`.
pub const DEALER_OFFSET: [f32; 2] = [10.53, -2.1];

/// How long the dealership slides from one car to the next: `[+0x318] = 0x100` ticks (0x4E95AD).
pub const DEALER_SLIDE_TICKS: u32 = 0x100;

/// The dealership's row of showrooms — `FEDealership3DRoom`, vtable 0x5B6F6C.
///
/// The dealership is not one room with three models in it: `dealer2a`, `dealer2b` and `dealer2c`
/// are **sections of a corridor**, one per car of `DealerCar.Car`, and the camera slides along it
/// as the car changes. Its state (the constructor 0x4E89F0 sets `cur = shown = ` the accessor's
/// index, `sliding = 0`):
///
/// | field | | |
/// |---|---|---|
/// | `cur` | `[+0x30C]` | the accessor's current index (slot 10), read by the tick |
/// | `shown` | `[+0x310]` | the index the row stood at before the slide |
/// | `sliding` | `[+0x308]` | a slide is running |
/// | `start` | `[+0x314]` | its start tick |
/// | `progress` | `[+0x31C]` | `(now − start) / 0x100`, clamped to 0…1 |
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DealerRow {
    pub cur: i32,
    pub shown: i32,
    pub sliding: bool,
    pub start: u32,
    pub progress: f32,
}

/// What [`DealerRow::layout`] places: the car at `pos`, the sections `first ..= last` about it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DealerLayout {
    /// `(X, Y, Z)` after the slide or the door zoom — the current car's place
    pub pos: [f32; 3],
    /// the section loop's bounds: −1 … 1, widened to 2 or −2 on the side a slide comes from
    pub first: i32,
    pub last: i32,
    /// `shown − cur` while a slide runs (±1), else 0 — where the car being left stands
    pub diff: i32,
}

impl DealerRow {
    pub fn new(index: i32) -> Self {
        DealerRow { cur: index, shown: index, sliding: false, start: 0, progress: 0.0 }
    }

    /// The tick, slot 1 **0x4E93B0**. `now` is `TIMER_getTicks` (128 Hz), `index` the accessor's
    /// current row (slot 10). Returns `true` on the tick a slide starts — the moment the original
    /// builds its second car `[+0x2F8]` from the **previous** car's name and paint, which it keeps
    /// at `[+0x320…+0x32C]` from every earlier tick (0x4E9624…0x4E967F), and copies the current
    /// car's roof state to it (0x4E9580…0x4E9596).
    ///
    /// ```text
    /// 0x4E93C6  sliding and now − start > 0x100  -> sliding = 0, shown = cur
    /// 0x4E93F7  not sliding: cur = accessor.slot10();  cur != shown -> start a slide:
    /// 0x4E9423    (not on a low-end card, 0x4E35C0: the second car)
    /// 0x4E959B    sliding = 1, start = now, duration = 0x100
    /// 0x4E95C1  sliding: progress = clamp((now − start) / duration, 0, 1)   (fild / fidiv)
    /// ```
    pub fn tick(&mut self, now: u32, index: i32) -> bool {
        if self.sliding && now.wrapping_sub(self.start) as i32 > DEALER_SLIDE_TICKS as i32 {
            self.sliding = false;
            self.shown = self.cur;
        }
        let mut started = false;
        if !self.sliding {
            self.cur = index;
            if self.cur != self.shown {
                self.sliding = true;
                self.start = now;
                started = true;
            }
        }
        if self.sliding {
            let p = (now.wrapping_sub(self.start) as i32 as f64 / DEALER_SLIDE_TICKS as f64) as f32;
            self.progress = p.clamp(0.0, 1.0);
        }
        started
    }

    /// The draw's placement, slot 15 **0x4E8C70** (0x4E8E50…0x4E9096), for a machine that is not
    /// a low-end card (0x4E35C0 false — the port's case; on one the slide is dropped at once,
    /// 0x4E8E9A). `base` is `(X, Y, Z)`, `zoom` the door zoom `[+0x2E8]`. A slide of one car
    /// moves the camera along the row in two legs, `x` and `z` one after the other:
    ///
    /// ```text
    /// shown − cur = 1  (back one car)   a = min(p, 0.6)·1.6666666     c = clamp((p − 0.6)·2.5, 0, 1)
    ///                                   X −= (1 − a)·XOff             Z −= (1 − c)·YOff      last = 2
    /// shown − cur = −1 (on one car)     a = min(p, 0.4)·2.5           c = clamp((p − 0.4)·1.6666666, 0, 1)
    ///                                   X += (1 − c)·XOff             Z += (1 − a)·YOff      first = −2
    /// any other difference (a wrap)     shown = cur, sliding = 0 — a cut, no slide (0x4E9043)
    /// not sliding                       the door zoom, as the base room's (0x4E9052…0x4E9092)
    /// ```
    ///
    /// The x87 keeps the intermediates in extended precision and stores `X`, `Z` as floats; the
    /// port computes in `f64` and rounds once.
    pub fn layout(&mut self, base: [f32; 3], zoom: f32) -> DealerLayout {
        let [mut x, mut y, mut z] = base.map(|v| v as f64);
        let (xo, yo) = (DEALER_OFFSET[0] as f64, DEALER_OFFSET[1] as f64);
        let (mut first, mut last, mut diff) = (-1, 1, 0);
        let p = self.progress as f64;
        // the two constants are floats in the image: 0x5B5FBC = 1.6666666, 0x5B7028 = 2.5
        let k53 = 1.666_666_6_f32 as f64;
        if self.sliding {
            match self.shown - self.cur {
                1 => {
                    let a = p.min(0.6_f32 as f64) * k53;
                    let c = ((p - 0.6_f32 as f64) * 2.5).clamp(0.0, 1.0);
                    x -= (1.0 - a) * xo;
                    z -= (1.0 - c) * yo;
                    last = 2;
                    diff = 1;
                }
                -1 => {
                    let a = p.min(0.4_f32 as f64) * 2.5;
                    let c = ((p - 0.4_f32 as f64) * k53).clamp(0.0, 1.0);
                    x += (1.0 - c) * xo;
                    z += (1.0 - a) * yo;
                    first = -2;
                    diff = -1;
                }
                _ => {
                    self.shown = self.cur;
                    self.sliding = false;
                }
            }
        } else {
            let zm = zoom as f64;
            let f = 1.0 - zm;
            x *= f;
            y = y * f - zm * 0.8_f32 as f64;
            z = z * f + zm * 0.3_f32 as f64;
        }
        DealerLayout { pos: [x as f32, y as f32, z as f32], first, last, diff }
    }
}

impl DealerLayout {
    /// The sections the loop draws (0x4E90BB…0x4E9143, again at 0x4E9246…0x4E92B8): for each `i`
    /// in `first ..= last` the row index `cur + i` — skipped when negative — takes model 0
    /// (`dealer2a`) at index 0, model 2 (`dealer2c`) at `count − 1` **or beyond**, model 1
    /// (`dealer2b`) between, and stands at `(X + i·XOff, Y, Z + i·YOff)`. `count` is the
    /// accessor's row count (slot 9, 0x4E8CCF).
    pub fn sections(&self, cur: i32, count: i32) -> Vec<(usize, [f32; 3])> {
        let mut out = Vec::new();
        for i in self.first..=self.last {
            let at = cur + i;
            if at < 0 {
                continue;
            }
            let model = if at == 0 {
                0
            } else if at >= count - 1 {
                2
            } else {
                1
            };
            // fild i; fmul YOff; fadd Z / fmul XOff; fadd X (0x4E90DC…0x4E9103)
            let (fi, [x, y, z]) = (i as f64, self.pos.map(|v| v as f64));
            out.push((model, [(x + fi * DEALER_OFFSET[0] as f64) as f32, y as f32, (z + fi * DEALER_OFFSET[1] as f64) as f32]));
        }
        out
    }

    /// Where the car being left stands while the row slides: `(X + d·XOff, Y, Z + d·YOff)` with
    /// `d = shown − cur` (0x4E91C0…0x4E91F5, 0x4E92FF…0x4E9396).
    pub fn previous_car(&self) -> Option<[f32; 3]> {
        (self.diff != 0).then(|| {
            let (d, [x, y, z]) = (self.diff as f64, self.pos.map(|v| v as f64));
            [(x + d * DEALER_OFFSET[0] as f64) as f32, y as f32, (z + d * DEALER_OFFSET[1] as f64) as f32]
        })
    }
}

/// The track room's fly-in, from its constructor 0x4EA390: `(X, Y, Z)` from `[+0x324…+0x32C]`
/// to `[+0x330…+0x338]`, `XRot` from `[+0x33C]` to `[+0x340]` (0x4EA45C…0x4EA49A).
pub const TRACK_FLY_FROM: [f32; 3] = [0.0, -4.8, 6.2];
pub const TRACK_FLY_TO: [f32; 3] = [0.0, -3.2, 1.9];
pub const TRACK_FLY_XROT: [f32; 2] = [-0.04, 0.0];
/// …over 0x12C = 300 ticks (0x4EA6B2).
pub const TRACK_FLY_TICKS: u32 = 0x12C;
/// `[+0x2F0]`: the frames of a `<art>.fsh` the picture panels cycle through (0x4EA3A0).
pub const TRACK_FRAMES: i32 = 5;

/// One picture the track room's tick writes into the wall's panels: `frame` of the track's
/// `<art>.fsh` into texture page `page` (0-based), with the start / finish marker over it on
/// frame 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelBlit {
    pub page: usize,
    pub frame: usize,
}

/// `FETrackSelect3DRoom` (vtable 0x5B7334) — the location screens' room: the floor `track` and a
/// four-sided box of `trackwall`, whose twelve prism panels (`Panel1a…12a`, rotation channels
/// 20…31) show the selected track's pictures and flip a third of a turn every 4.5 s, and which
/// turns a quarter each time the track changes. Its state:
///
/// | field | | |
/// |---|---|---|
/// | `fly_start`, `fly_done` | `[+0x31C]` `[+0x320]`, `[+0x31D]` | the fly-in |
/// | `pos`, `xrot` | `[+0x278…+0x280]`, `[+0x284]` | the room's place |
/// | `yrot` | `[+0x288]` | its turn, 0 … 1 |
/// | `target`, `reached` | `[+0x30C]`, `[+0x308]` | the quarter it turns to, and the last it reached |
/// | `last` | `[+0x310]` | the tick before, for the turn's rate |
/// | `track`, `rev`, `mir` | `[+0x304]`, `[+0x347]` / `[+0x345]`, `[+0x346]` / `[+0x344]` | what the tick last saw |
/// | `face` | `[+0x300]` | which of the panels' three faces is to the front |
/// | `filled`, `frame` | `[+0x2F4]`, `[+0x2EC]` | the face and the frame written last |
/// | `flipping`, `flip_start` | `[+0x2FC]`, `[+0x2F8]` | a flip running, or the hold's start |
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRoom {
    pub fly_start: Option<u32>,
    pub fly_done: bool,
    pub pos: [f32; 3],
    pub xrot: f32,
    pub yrot: f32,
    pub target: i32,
    pub reached: i32,
    pub last: u32,
    pub track: Option<usize>,
    pub rev: Option<bool>,
    pub mir: Option<bool>,
    pub face: i32,
    pub filled: i32,
    pub frame: i32,
    pub flipping: bool,
    pub flip_start: u32,
}

impl TrackRoom {
    /// The constructor 0x4EA390: the fly-in's start place, `[+0x2EC] = [+0x2F4] = −1`, the turn
    /// and the face at 0, and both clocks at `now`.
    pub fn new(now: u32) -> Self {
        TrackRoom {
            fly_start: None,
            fly_done: false,
            pos: TRACK_FLY_FROM,
            xrot: TRACK_FLY_XROT[0],
            yrot: 0.0,
            target: 0,
            reached: 0,
            last: now,
            track: None,
            rev: None,
            mir: None,
            face: 0,
            filled: -1,
            frame: -1,
            flipping: false,
            flip_start: now,
        }
    }

    /// The draw's fly-in, 0x4EA666…0x4EA740: it starts on the first draw that finds the room's
    /// transition settled (0x4E0E20: `[+0x12C] == 2`), then `t = min(now − start, 300) · 0.0033333`
    /// and the place is `(1 − t)·from + t·to`, `XRot` likewise; at 300 it stops for good.
    pub fn fly(&mut self, now: u32, settled: bool) {
        if self.fly_start.is_none() && settled {
            self.fly_start = Some(now);
        }
        let Some(start) = self.fly_start.filter(|_| !self.fly_done) else { return };
        let mut e = now.wrapping_sub(start) as i32;
        if e >= TRACK_FLY_TICKS as i32 {
            e = TRACK_FLY_TICKS as i32;
            self.fly_done = true;
        }
        // fild; fmul [0x5B4A64] (1/300 as a float) — the rest in the x87's precision
        let t = e as f64 * (1.0f32 / 300.0) as f64;
        let lerp = |a: f32, b: f32| ((1.0 - t) * a as f64 + t * b as f64) as f32;
        self.pos = std::array::from_fn(|k| lerp(TRACK_FLY_FROM[k], TRACK_FLY_TO[k]));
        self.xrot = lerp(TRACK_FLY_XROT[0], TRACK_FLY_XROT[1]);
    }

    /// The tick, slot 1 **0x4EA9B0**. `track` is the `nfs5.trk` row 0x4FC0B0 answers for the game
    /// type, `rev` / `mir` its direction; `back` is `[0x690A84]`, which the location arrows set —
    /// 0 by an `…INC`, 1 by a `…DEC`. Returns the pictures to write and the panels' turn, the
    /// value channels 20…31 take as `(a, 0, 0)` (0x4EAF4A…0x4EB0A4).
    pub fn tick(&mut self, now: u32, track: usize, rev: bool, mir: bool, back: bool) -> (Vec<PanelBlit>, f32) {
        // 0x4EAA0F…0x4EAA3E: the first tick takes the direction as it is
        let (was_mir, was_rev) = (*self.mir.get_or_insert(mir), *self.rev.get_or_insert(rev));
        // 0x4EAA44: a new track (but not the first one seen), or a new direction
        let mut changed = false;
        if (self.track.is_some_and(|t| t != track)) || was_mir != mir || was_rev != rev {
            self.rev = Some(rev);
            self.mir = Some(mir);
            self.filled = if self.face < 0 { self.face + 3 } else { self.face };
            self.target += if back { 1 } else { -1 };
            if self.target < 0 {
                self.target += 4;
            }
            if self.target > 3 {
                self.target -= 4;
            }
            self.frame = 4;
            changed = true;
            if !self.flipping {
                self.flipping = true;
                self.flip_start = now;
            }
        }
        self.track = Some(track);
        // 0x4EAAF0…0x4EAC66: turn toward `target / 4` at 1/350 turn a tick
        if self.reached != self.target {
            let mut t = self.target as f32 * 0.25;
            if t == 0.0 && back {
                t = 1.0;
            }
            if self.yrot == t {
                self.reached = self.target;
            } else {
                let below = self.yrot < t;
                let step = now.wrapping_sub(self.last) as i32 as f64 * (1.0f32 / 350.0) as f64;
                self.yrot = if back { (step + self.yrot as f64) as f32 } else { (self.yrot as f64 - step) as f32 };
                // a step past the target lands on it
                if (self.yrot < t && !below) || (self.yrot > t && below) {
                    self.yrot = t;
                }
                while self.yrot > 1.0 {
                    self.yrot -= 1.0;
                }
                while self.yrot < 0.0 {
                    self.yrot += 1.0;
                }
            }
        }
        // 0x4EAC6B…0x4EAE7B: fill the faces up to the one after the front one
        let mut blits = Vec::new();
        let next = if self.face + 1 > 2 { self.face + 1 - 3 } else { self.face + 1 };
        let mut again = changed;
        while self.filled != next || again {
            again = false;
            self.filled += 1;
            if self.filled > 2 {
                self.filled -= 3;
            }
            self.frame += 1;
            if self.frame >= TRACK_FRAMES {
                self.frame = 0;
            }
            // face 1 → page 3, face 2 → page 2, face 0 → page 1 (0x4EAD0C…0x4EAD32)
            let page = match self.filled {
                1 => 3,
                2 => 2,
                _ => 1,
            };
            blits.push(PanelBlit { page, frame: self.frame as usize });
        }
        // 0x4EAE80…0x4EAF4A: the panels' turn — a third a face, 0.001 for face 0; a flip of 64
        // ticks after a hold of 0x200
        let third = 0.3334_f32 as f64;
        // `fild; fmul; fstp` — the face's turn is stored as a float before the flip adds to it
        let mut a = if self.face == 0 { 0.001_f32 } else { (self.face as f64 * third) as f32 };
        if self.flipping {
            // `fst [esp+0x14]` keeps the flip's fraction as a float too
            let mut f = (now.wrapping_sub(self.flip_start) as i32 as f64 * 0.015625) as f32;
            if f > 1.0 {
                f = 1.0;
                self.face += 1;
                if self.face > 2 {
                    self.face = 0;
                }
                self.flipping = false;
                self.flip_start = now;
            }
            a = (f as f64 * third + a as f64) as f32;
        } else if now.wrapping_sub(self.flip_start) as i32 > 0x200 {
            self.flipping = true;
            self.flip_start = now;
        }
        self.last = now;
        (blits, a)
    }
}

/// Where the track room's start / finish marker goes on frame 0 (0x4EADC1…0x4EAE34): at
/// `(|marker.x|, marker.y)` of the marker's own `misc` offsets, or — on a mirrored track, with
/// the marker flipped too — at `|picture.x| + picture.w − |marker.x| − marker.w`.
pub fn track_marker_at(picture_x: i16, picture_w: u16, marker_x: i16, marker_y: i16, marker_w: u16, mir: bool) -> (i64, i64) {
    let mx = (marker_x as i32).abs() as f64;
    let x = if mir { (picture_x as i32).abs() as f64 + picture_w as i16 as f64 - mx - marker_w as i16 as f64 } else { mx };
    // `fld; call _ftol` — truncation
    (x as i64, marker_y as i64)
}

/// The file `FEVictory3DRoom`'s constructor **0x4EB1C0** gives its movie. It picks
/// `%ser4pc-av.mad` / `er3` / `er2` by the era (0x5032C0 against 0x502990(0x20B / 0x20A / 0x209))
/// with `sprintf` into its buffer — and then the `sprintf` at 0x4EB331 writes `%ser1pc-av.mad`
/// into the same buffer on **every** path (0x4EB31B is where all three branches land). So the
/// room plays `er1pc-av.mad` whatever the era: an original bug, kept.
pub const VICTORY_MOVIE: &str = "er1pc-av.mad";

/// The victory room's screen: the `MOVIE` object 0x4E1EB0 that 0x4EB1C0 opens over
/// [`VICTORY_MOVIE`] — 256 × 256, **no sound** (its sixth argument, `[+0x75]`, is 0) and drawing
/// into a surface it is handed (the seventh, 1) — which the room's tick **0x4EB3E0** renders into
/// the room model's texture page **5** each frame (`[R3DCar+0x78] + 0x104`, 0x447A90) and then
/// refreshes (0x443C80(5)).
///
/// Its render, slot 0 **0x4E25A0**: the ticks since the last call are added to a millisecond
/// clock `[+0x4C]` (from the second call on, `[+0x58]`), the frame the clock has reached is
/// `_ftol(clock / period)` with the period the header's `+0x0C` / 65536 ms (0x4E2054), and — the
/// movie having no sound — it may move **one frame a call at most**, the clock pulled back to that
/// frame (0x4E261D…0x4E2631). A step releases the chunk and takes the next video chunk (0x4E2640)
/// and decodes it (0x4E2520); past the last one the stream is reopened (0x4E2380: frame and clock
/// 0, the first chunk) and that is decoded, so the movie loops. The frame the constructor decoded
/// (0x4E2127) is on screen until the first step. Every call then blits the current frame at (0, 0)
/// of the page at its own size (0x56A920 at 0x4E27FF).
pub struct VictoryMovie {
    data: Vec<u8>,
    chunks: Vec<(usize, usize)>,
    decoder: nfs_formats::mad::MadDecoder,
    /// the chunk on screen, `[+0x34]`, as an index into `chunks`
    at: usize,
    /// `[+0x50]`, `[+0x4C]`, `[+0x48]`, `[+0x58]`
    frame: i64,
    clock_ms: f32,
    period_ms: f32,
    started: bool,
    pub width: u32,
    pub height: u32,
}

impl VictoryMovie {
    /// Open a `.mad` and decode its first frame (0x4E1EB0 → 0x4E2520 at 0x4E2127).
    pub fn open(tables: nfs_formats::mad::MadTables, data: Vec<u8>) -> Option<Self> {
        let mut chunks = Vec::new();
        let mut at = 0usize;
        for c in nfs_formats::mad::chunks(&data) {
            if c.is_video() {
                chunks.push((at, c.data.len()));
            }
            at += c.data.len();
        }
        let &(off, len) = chunks.first()?;
        let header = nfs_formats::mad::MadHeader::parse(&data[off..off + len]).ok()?;
        let mut decoder = nfs_formats::mad::MadDecoder::new(tables, &header);
        let f = decoder.decode_frame(&data[off..off + len]);
        if f.width == 0 || f.height == 0 {
            return None;
        }
        // `fild [chunk+0x0C]; fmul 1/65536` (0x4E2054…0x4E2062)
        let period_ms = (header.period as f64 * 1.52587890625e-05) as f32;
        Some(VictoryMovie { data, chunks, decoder, at: 0, frame: 0, clock_ms: 0.0, period_ms, started: false, width: f.width as u32, height: f.height as u32 })
    }

    /// One call of the render 0x4E25A0 with `dt_ms` since the last. True when a frame was decoded.
    pub fn tick(&mut self, dt_ms: f32) -> bool {
        let mut target = self.frame;
        if self.started {
            self.clock_ms += dt_ms;
            target = (self.clock_ms / self.period_ms.max(1e-3)) as i64;
            if target > self.frame + 1 {
                target = self.frame + 1;
                self.clock_ms = target as f32 * self.period_ms;
            }
        }
        self.started = true;
        if target <= self.frame {
            return false;
        }
        self.frame += 1;
        self.at += 1;
        if self.at >= self.chunks.len() {
            // 0x4E2722 → 0x4E2380: the stream reopened
            self.at = 0;
            self.frame = 0;
            self.clock_ms = 0.0;
        }
        let (off, len) = self.chunks[self.at];
        let _ = self.decoder.decode_chunk(&self.data[off..off + len]);
        true
    }

    /// The frame on screen, RGBA8, `width` × `height`.
    pub fn rgba(&self) -> Vec<u8> {
        self.decoder.rgba()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_track_room_fills_the_next_face_and_flips_every_four_and_a_half_seconds() {
        let mut r = TrackRoom::new(0);
        // the first tick fills faces 0 and 1 with frames 0 and 1, and turns nothing
        let (b, a) = r.tick(1, 7, false, false, false);
        assert_eq!(b, [PanelBlit { page: 1, frame: 0 }, PanelBlit { page: 3, frame: 1 }]);
        assert_eq!(a, 0.001);
        assert_eq!((r.yrot, r.target), (0.0, 0));
        // the hold: 0x200 ticks, then a flip of 64
        assert!(r.tick(0x200, 7, false, false, false).0.is_empty());
        assert!(!r.flipping);
        r.tick(0x201, 7, false, false, false);
        assert!(r.flipping);
        let (_, a) = r.tick(0x201 + 32, 7, false, false, false);
        assert!((a - (0.001 + 0.5 * 0.3334)).abs() < 1e-5);
        r.tick(0x201 + 65, 7, false, false, false);
        assert_eq!((r.face, r.flipping), (1, false));
        // face 1 in front: face 2 gets frame 2
        let (b, _) = r.tick(0x201 + 66, 7, false, false, false);
        assert_eq!(b, [PanelBlit { page: 2, frame: 2 }]);
        // a new track, stepped on: a quarter back, frame 0 on the face after the front one, a flip
        let (b, _) = r.tick(0x201 + 67, 8, false, false, false);
        assert_eq!(r.target, 3);
        assert_eq!(b, [PanelBlit { page: 2, frame: 0 }]);
        assert!(r.flipping);
        assert!(r.yrot > 0.99, "turned a hair back from 0, wrapped to just under 1");
    }

    #[test]
    fn the_garage_and_factory_fans_turn_once_a_second_and_once_in_a_second_and_a_half() {
        assert_eq!(room_channels("ID_GARAGE", 128), [(1, [0.0, 0.0, 1.0]), (2, [0.0, 0.0, (128.0f64 * (1.0f32 / 192.0) as f64) as f32])]);
        assert_eq!(room_channels("ID_FACTORY", 192)[1].1[0], (192.0f64 * (1.0f32 / 192.0) as f64) as f32);
        assert!(room_channels("ID_VICTORY", 128).is_empty());
    }

    #[test]
    fn the_track_room_flies_in_over_300_ticks_once_settled() {
        let mut r = TrackRoom::new(0);
        r.fly(10, false);
        assert_eq!(r.pos, TRACK_FLY_FROM);
        r.fly(20, true);
        r.fly(170, true);
        assert!((r.pos[2] - (6.2 + 1.9) / 2.0).abs() < 1e-4);
        r.fly(400, true);
        assert!(r.fly_done);
        assert!(r.pos.iter().zip(TRACK_FLY_TO).all(|(a, b)| (a - b).abs() < 1e-5));
        assert!(r.xrot.abs() < 1e-6);
    }

    #[test]
    fn the_dealership_is_a_row_of_sections_one_per_car() {
        let mut r = DealerRow::new(0);
        let l = r.layout([0.0, -1.1, 4.8], 0.0);
        // the first car: nothing before it, `dealer2a` under it and the next section beside
        let s = l.sections(0, 5);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], (0, [0.0, -1.1, 4.8]));
        assert_eq!(s[1].0, 1);
        assert!((s[1].1[0] - 10.53).abs() < 1e-5 && (s[1].1[2] - 2.7).abs() < 1e-5);
        // the last car: `dealer2c` from `count − 1` on, twice
        let s = l.sections(4, 5);
        assert_eq!(s.iter().map(|s| s.0).collect::<Vec<_>>(), [1, 2, 2]);
        // one section alone before the first of a one-car list
        assert_eq!(l.sections(0, 1).iter().map(|s| s.0).collect::<Vec<_>>(), [0, 2]);
    }

    #[test]
    fn the_dealership_slides_two_legs_over_256_ticks() {
        let mut r = DealerRow::new(0);
        assert!(!r.tick(100, 0));
        // on one car: the slide starts, the camera still where the old car was
        assert!(r.tick(200, 1));
        let l = r.layout([0.0, 0.0, 0.0], 0.0);
        assert_eq!((l.first, l.last, l.diff), (-2, 1, -1));
        assert!((l.pos[0] - 10.53).abs() < 1e-5 && (l.pos[2] + 2.1).abs() < 1e-5);
        assert_eq!(l.previous_car(), Some([0.0, 0.0, 0.0]));
        // the z leg is over at 0.4, the x leg at 1.0
        r.tick(200 + 0x66, 1);
        let l = r.layout([0.0, 0.0, 0.0], 0.0);
        assert!(l.pos[2].abs() < 0.01 && l.pos[0] > 10.0);
        r.tick(200 + 0x100, 1);
        assert!(r.sliding);
        // 0.6 · 1.6666666 falls a hair short of 1, in the original too
        assert!(r.layout([0.0; 3], 0.0).pos.iter().all(|v| v.abs() < 1e-5));
        r.tick(200 + 0x101, 1);
        assert!(!r.sliding && r.shown == 1);
        // a wrap from the first car to the last is a cut
        r.tick(1000, 5);
        let l = r.layout([0.0; 3], 0.0);
        assert_eq!((l.diff, r.sliding, r.shown), (0, false, 5));
    }

    #[test]
    fn every_shipped_room_id_resolves() {
        for id in ["ID_GARAGE", "ID_DEALER", "ID_FACTORY", "ID_VICTORY", "ID_TRACK", "ID_NONE", "ID_TESTROOM", ""] {
            assert!(room(id).is_some(), "{id}");
        }
        assert_eq!(room("ID_GARAGE").unwrap().models, ["garage"]);
        assert_eq!(room("").unwrap().id, "ID_NONE");
    }

    #[test]
    fn the_turntable_takes_spin_seconds_per_turn() {
        let g = room("ID_GARAGE").unwrap();
        assert_eq!(g.spin, 30.0);
        assert!((g.spin_turns(30_000) + 1.0).abs() < 1e-6, "one turn in thirty seconds");
        assert!((g.spin_turns(15_000) + 0.5).abs() < 1e-6);
        // the track wall turns faster
        assert_eq!(room("ID_TRACK").unwrap().spin, 20.0);
    }

    #[test]
    fn the_room_pulls_toward_the_camera_as_a_door_is_zoomed_into() {
        let g = room("ID_GARAGE").unwrap();
        assert_eq!(g.zoomed_pos(0.0), g.pos);
        let out = g.zoomed_pos(1.0);
        assert_eq!(out, [0.0, -0.8, 0.3]);
    }
}
