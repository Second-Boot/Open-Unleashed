//! Vehicle model reconstructed from `Car_physicsStep` 0x49F160 and friends (`RE/notes/physics.md`).
//!
//! Conventions (as in the original): angles and angular velocities in **turns** (1.0 = 360°),
//! steering angles in 1/256 turn, body frame x = right, y = up, z = forward; orientation rows are
//! body axes in world space; the "physics frame" is x-axis / ground normal / z-axis.
//!
//! Fidelity notes (see the notes file for each):
//! * CONFIRMED: scheduling/dt, gravity, torque lookup, rpm from wheels, drive/drag/brake, auto
//!   shift, steering ramp and angle law, load transfer, friction circle, integration.
//! * HIGH/MEDIUM: engine rpm heuristics, lateral tyre curve, yaw torque signs.
//! * Suspension / ground (`Susp_update` 0x499A70, `Car_ground` 0x49BD40, `RE/notes/suspension-collision.md`
//!   §3–4, CONFIRMED): wheel reference points probe the ground, the road roughness of 0x499730 is added
//!   to each probe, pitch/roll are rate-driven kinematics, landing/rest rules with the surface bounce
//!   table.
//! * Walls (`Car_wallCollide` 0x495020 / `WallImpulse` 0x494000, CONFIRMED): 8 body corners swept against
//!   the `.edg` segments, restitution 0.1, no friction, `WallSpin`'s set yaw rate (0x493F10, exact).
//! * Body–ground when tilted (`BodyGroundCollide` 0x496610 + `GroundImpulse` 0x495F30, §6.3, HIGH) and
//!   car–car (`0x497B10` / `0x496FD0` / `0x496860`, `traffic.md` §21.6, CONFIRMED) are the original's
//!   separation, box test and impulses, not approximations.
//! * Drafting (`DraftFactor` 0x49D9B0, CONFIRMED): the caller fills `draft` from the other racers
//!   and it scales the drag.
//! * Induction boost (`0x49E6C4`) and misfire (§4.3) are modelled; the caller fills `misfire_roll`
//!   from the shared LCG.
//! * The part-damage getters (`physics.md` §10) walk `specs.parts`, the installed-part list
//!   `car+0x768`, each part over its own zone mask; the career fills it from the car's parts.

use crate::collision::{PatchTracker, TriangleGrid};
use crate::specs::{CarSpecs, CIRCLE_SOFTNESS, GRAVITY, GRIP_DRY, GRIP_WET, SURFACE_BOUNCE};
use glam::{Mat3, Vec3};
pub use car_car::*;
use std::f32::consts::TAU;

mod car_car;
mod damage;
mod drivetrain;
mod ground;
mod netcar;
mod tuning;
mod world_hits;
mod x87;
pub use netcar::Block as NetBlock;
use tuning::*;
use x87::{fistp, sign_clamp, tyre_demand_clamp};

/// [`Vehicle::race_state`], `car+0xB50` (the finish state). Every store is one of these three
/// (the app's per-tick writes, simulate.rs / net.rs); nothing reads it from a file or the wire.
#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RaceState {
    /// 0: racing.
    #[default]
    Racing = 0,
    /// 2: finished.
    Finished = 2,
    /// 3: out.
    Out = 3,
}

/// [`Vehicle::contact_kind`], `car+0x438 & 0xEFFF0000` — the kind of the tick's last contact
/// writer; the word goes to the force feedback as is ([`ContactKind::word`]).
#[repr(u32)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContactKind {
    /// 0: nothing wrote it this tick (0x40FB0D clears it).
    #[default]
    None = 0,
    /// `0x30000`, the body on the ground (0x496104).
    Ground = 0x30000,
    /// `0x40000`, a wall (0x49411E…).
    Wall = 0x40000,
    /// `0x50000`, a prop or a car (0x498A44, 0x498DB6, 0x496CB5…).
    PropOrCar = 0x50000,
    /// `0x60000`, a bush (0x498F00).
    Bush = 0x60000,
}

impl ContactKind {
    /// The word `car+0x438 & 0xEFFF0000` as the original stores it.
    #[inline]
    pub fn word(self) -> u32 {
        self as u32
    }
}

/// [`Vehicle::engine_arm`] (diagnostic only): which arm of `engine_update`'s rev/clutch
/// selection ran last.
#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EngineArm {
    /// 0: no engine step yet.
    #[default]
    None = 0,
    /// 1: the clutch arm.
    Clutch = 1,
    /// 2: the over-run (engine braking) arm.
    Overrun = 2,
    /// 3: `diff == 0`.
    AtTarget = 3,
    /// 4: a misfire under the target.
    Misfire = 4,
    /// 5: the driving arm under the target.
    Driving = 5,
}

/// A front wheel's angle in 1/256 turn from its steering value: `max_steer · steer / 17`.
/// `set_wheel_steer` scales the lock to 17 (`y · 17 / max_steer`), so this undoes it. One function
/// for the tyre and the renderer.
pub fn steer_angle_256(max_steer: f32, steer: f32) -> f32 {
    max_steer * steer / 17.0
}

/// Under `RALLY` (`[0x6573F8]`) a surface nibble 1 or 10 reads as 2 — the body tracker's
/// (0x49AFE1) and each wheel's (0x499B90, 0x499BA7).
#[inline]
pub(crate) fn rally_reads_as_two(nibble: usize, rally: bool) -> bool {
    rally && (nibble == 1 || nibble == 10)
}

/// The original's cheap two-axis norm, `max(a, b) + 0.25 · min(a, b)` (0x49BEA9…0x49BF02; the
/// operands are magnitudes). Not a true norm: the gates and the tyre's combined slip read it.
#[inline]
pub fn norm_est(a: f32, b: f32) -> f32 {
    a.max(b) + 0.25 * a.min(b)
}

pub const DT32: f32 = 0.03125;
pub const DT64: f32 = 0.015625;

/// Controller values latched at 32 Hz (`0x5E9664..`): 0..254 pedals, ±126 steer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Controls {
    pub throttle: u8,
    pub brake: u8,
    pub steer: i32,
    pub handbrake: bool,
    /// The packet's **command code**, byte 0 bits 3..7 (`controls.md`, the packer's command codes):
    /// 1 leave race, **2 shift up**, **3 shift down**, 4 abort, 5 reset, … — set on the tick a
    /// command fires and 0 otherwise.
    pub command: u8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Wheel {
    pub contact: bool,
    pub wet: bool,
    /// rotation angle, turns in [0,1)
    pub rotation: f32,
    /// The surface nibble, `zone & 0xF` — what the grip table (0x4A1962 `and eax, 0xF`),
    /// `Roughness` (0x49976F) and the car renderer (0x43F02B) read.
    pub surface: usize,
    /// `wheel+0x8C`, the **whole** zone word: `Susp_update` stores the wheel tracker's `u16`
    /// `+0x24` zero-extended (0x499B84…0x499B8A), or 2 under `RALLY` when its nibble is 1 or 10
    /// (0x499BA7 — the whole dword). The `Tyre` tests compare it whole, signed, with 1
    /// (0x4A1F7A `cmp [esi+0x8C], 1; jg`, 0x4A2373 `cmp [esi+0x8C], ebx` with `ebx = 1` from
    /// 0x4A21A5), and so does the wheel-effects feed 0x4A2880 (`== 9`, `== 6`, `== 1` at
    /// 0x4A28B9…0x4A28EC, then hands it on as the mark kind): a patch whose flags word has bits
    /// above the nibble is not "≤ 1" there even when its nibble is 1.
    pub zone: u32,
    pub lateral: f32,
    pub longitudinal: f32,
    /// contact-patch slip velocities ×(−16): lateral, vertical, longitudinal
    pub slip_vel: [f32; 3],
    pub demand: f32,
    pub limit: f32,
    /// The front wheel's steering value as `set_wheel_steer` leaves it — **not** an angle: full lock
    /// is about ±17 (the `y · 17 / max_steer` scaling), and the angle the tyre is actually turned
    /// through is [`steer_angle_256`]`(max_steer, steer)`, in 1/256 turn. This comment used to say
    /// "steer angle in 1/256 turn", and the renderer took it at its word (2026-09-21).
    pub steer: f32,
    pub slip: f32,
    pub locked: bool,
    pub sliding: bool,
    /// world position (rendering)
    pub world_pos: Vec3,
    /// `wheel+0x64`, the ground height under the probe minus the probe's height, **as stored**:
    /// 0x499F77 `fst`s it before the clamp at −0.30625, which only the copy `Susp_update` keeps
    /// on the FPU stack gets (0x499F7A…0x499F9B). *(Corrected 2026-09-25: the port stored the
    /// clamped value.)*
    pub height_d: f32,
    pub airborne: bool,
    pub compression: f32,
    /// `Roughness` 0x499730 state: previous / target offset and the distance phase
    /// (`wheel+0x74 / +0x78 / +0x7C`).
    pub rough_prev: f32,
    pub rough_target: f32,
    pub rough_phase: f32,
    /// The wheel's own ground tracker `wheel+4` (`suspension-collision.md` §1.1): `Susp_update`
    /// runs `Tracker::updateZone(probe, 1)` 0x474060 on it every pass (0x499B64) and takes the
    /// surface word, the patch normal and `point(0)` from it.
    pub tracker: PatchTracker,
}

/// The event a prop contact leaves in `car+0x438` (`sfx.md` §2): `0x50000 | class` for a prop, or
/// `0x6000F` for a drive-through object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropEvent {
    Prop(u32),
    Bush,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vehicle {
    pub position: Vec3,
    pub velocity: Vec3,
    /// rows: x-axis, up, forward
    pub orientation: Mat3,
    /// world, turns/s
    pub angular_velocity: Vec3,
    /// physics frame rows: body x, ground normal, body z
    pub frame: Mat3,
    pub ground_normal: Vec3,
    pub cos_tilt: f32,
    /// Diagnostic only: **why** the engine last died — 1 an impact past 120 (0x4948C1), 2 a
    /// body-ground contact that left the car upside down (0x49C005). Nothing reads it but the
    /// logs that chase cars which will not drive away.
    pub engine_kill: u8,
    /// Diagnostic only: which arm of `engine_update`'s rev/clutch selection ran last — 1 the
    /// clutch arm, 2 the over-run (engine braking) arm, 3 `diff == 0`, 4 a misfire under the
    /// target, 5 the driving arm under the target.
    pub engine_arm: EngineArm,
    /// `DraftFactor` 0x49D9B0's result, which scales the drag. 1.0 = clean air; the caller sets it
    /// from the other racers each tick with [`draft_factor`].
    pub draft: f32,
    /// `car+0x54C` — the induction boost state 0..1 (0x49E6C4), which the turbo sound reads.
    pub boost: f32,
    /// The `[0, 1)` draw the misfire test compares the engine damage against (§4.3). The original
    /// takes it from the shared LCG every engine step; the caller fills it from the same stream.
    pub misfire_roll: f32,
    /// The `rand16` the AI's start-gear shift timer takes (0x40C06F…0x40C09E, `4 + (r·8 >> 16)`),
    /// drawn by the AI's pedal routine from the shared stream before `Car_simulate` runs; the
    /// caller fills it for the tick it is due, and the car's own stream stands in when it does not.
    pub shift_roll: Option<u32>,
    /// 0 on ground, >0.05 airborne (car+0x424)
    pub height_marker: f32,
    pub v_local: Vec3,
    pub w_local: Vec3,
    pub slip_tangent: f32,
    pub rpm: f32,
    /// gear slot: 0 R, 1 N, 2.. forward
    pub gear: usize,
    pub prev_gear: usize,
    pub shift_timer: u32,
    /// `car+0xD7E`, the requested gear: 1 at car creation (0x410C2F), moved by the shift codes
    /// (0x499475) and taken by 0x49DD20.
    pub requested_gear: usize,
    /// `car+0xD83`, set on a manual downshift into a forward gear (0x49DEF1). Its readers — the
    /// neutral rpm blip and the previous gear's ratio while blipping (`physics.md` §4.1) — are
    /// not in this port's engine model.
    pub downshift_blip: bool,
    /// `car+0xD98`, the automatic's reverse latch (0x49DE72, 0x49DEC2).
    pub reverse_latch: bool,
    /// `gGameTick` `[0x606AC4]` (64 Hz), which 0x49DD20 gates the gearbox on: nothing before 0x1C0,
    /// first gear requested until 0x1C4. The race sets it every step; `step64` counts it on
    /// otherwise.
    pub game_tick: u64,
    /// `car+0xB50`, the finish state: 0 racing, 2 finished, 3 out.
    pub race_state: RaceState,
    /// `RALLY` `[0x6573F8]`, the race option the Dakar code sets (cheat bit 0) — global, so every
    /// car in the race has it: surfaces 1 and 10 read as 2 (0x499B90, 0x49AFE1), the steering
    /// ramps 18 a tick (0x49DFC0), the front grip limits ×1.1 (0x4A1ADD), the drive
    /// ×1.25·min(1 + |slip tangent|, 2) (0x49EE0B), the friction circle's longitudinal ×0.5 (0x4A1DC1)
    /// and the front lateral ×1.2 after it (0x4A26DE)
    pub rally: bool,
    /// `.TRACTION` `carinfo+0x20` (option 0x23B), the traction control of the friction circle
    /// 0x4A1DC0: a rear wheel over its limit is scaled back to it — no slide — while
    /// `|car+0xDC0| < 0.15` (0x4A1F38…0x4A1F6E), a spinning rear axle does not slide outright
    /// (0x4A2072), and the slide intensity is halved at the end (0x4A20AA)
    pub tcs: bool,
    /// `.TINYCAR` `carinfo+0x518`: the steering ramps 18 a tick as under `RALLY` (0x49DFCF)
    pub tiny: bool,
    /// `DERBY` `[0x657400]` (option 0x1B), a global the launch sets only for the Smash Up name-box
    /// code (cheat bit 7, 0x4ED7D9…, `modes.md`): `Damage` 0x494500 takes the amount ×1.5
    /// (`[0x5B246C]`, 0x49453C) instead of ×0.5 before the cap at 100. Its other readers are race
    /// logic outside this crate (the knockout 0x416407, the grid 0x4105E0, `HPosition` 0x42B2D5).
    pub derby: bool,
    pub ignition_cut: u32,
    pub throttle_raw: u8,
    pub brake_raw: u8,
    /// `car+0xD7F` / `+0xD80` as `Car_processInput` 0x49DD20 leaves them: the controller bytes
    /// (0x49DD8A…0x49DD98), then **throttle 0 and brake 0xFF** for a dead car or a finished human
    /// (0x49DD56 / 0x49DD71) and while `car+0xDB0 < 0x40` (0x49E00D…0x49E019), and **brake 0xFF**
    /// alone before game tick 0x1C0 (0x49E022…0x49E02E). The pedal floats are swapped in an
    /// automatic's reverse (0x49E19C…), these bytes are not. The tyre model, `LoadTransfer` and
    /// the stability assist read these; [`Self::throttle_raw`] / [`Self::brake_raw`] stay the
    /// controller's. Not carried: the finish's 0x80 / 0 pair (`car+0x540` / `+0x544`,
    /// 0x49E037…0x49E066) and `car+0xD2C == 2` → throttle 0 (0x49E074).
    pub throttle_byte: u8,
    pub brake_byte: u8,
    /// `LoadTransfer`'s wet test (0x4A191D…0x4A194D): the wet-track global `[0x606AE0]` (set
    /// for `autobahn`, 0x414DFF…0x414E11) and nothing overhead at the car
    /// (`0x473330(car+8, car+0x330, 0) == 0`). Set by the caller before [`Self::step32`];
    /// `LoadTransfer` copies it into every wheel's `+0x80` ([`Wheel::wet`]) and reads the wet
    /// grip table 0x5D0E4C when it is set.
    pub wet_track: bool,
    pub throttle: f32,
    pub brake: f32,
    pub steer_target: i32,
    pub steer: i32,
    pub handbrake: bool,
    pub automatic: bool,
    /// Before the race start the original forces effective throttle 0 / brake full (0x49DD20,
    /// game tick < 0x1C0) without touching the raw controller bytes.
    pub race_started: bool,
    /// TCS (car-info +0x24) or AI car: a traction-limited wheel clamps its demand to the grip limit
    /// instead of spinning/locking (`Tyre` 0x4A20D0, CONFIRMED).
    pub traction_control: bool,
    /// `car+0x52C & 4` is clear — the car is driven by the AI, not a human. The rev limiter's
    /// standing-start branch is the one place the original asks (`physics.md` §4.1).
    pub ai_driven: bool,
    /// The port's own marker for the stand-in bodies it builds for its kinematic traffic (the
    /// caller skips their walls). *(Corrected 2026-09-25:)* this read "`car+0x52C` bit 7, traffic";
    /// bit 0x80 is the **smackables'** (`ai.md` §6, 0x40F160) — [`Self::smackable`] — and the
    /// traffic's is 0x10, which none of the physics below tests.
    pub is_traffic: bool,
    /// `car+0x52C & 0x80` — a **smackable**: a world prop the race gives a car record of its own
    /// (0x4A17F0, `skill-tests.md` §5.3, `nfs_physics::smackable`). `Damage_byNormal` 0x494E69
    /// skips the roof zone for one, `Damage`'s crash response hops and spins it harder
    /// (0x494964, 0x494A0A), `ClampAngVel` 0x4964F4 lets a light one spin to 6.75 turns/s, the
    /// ground impulse 0x495F30 takes 0.3 of its `j`, and the car–car solver 0x496860 has its own
    /// branches for it.
    pub smackable: bool,
    /// `car+0x44C` — the smackable's material class from its `audioType` (0x4A12D0's switch;
    /// `kAudPylon` → **0x10**). 0 on every other car: only the smackable setup writes it.
    pub material_class: u32,
    /// `car+0x7C` — "disturbed": `RigidBody_init` 0x49B510 sets it, 0x4A1510 clears it on a
    /// smackable, and the car–car solver sets it again on the smackable's first contact
    /// (0x4969B8 / 0x4969DB), forcing that contact's `j` non-negative.
    pub disturbed: bool,
    /// `car+0x42C` on a smackable — "has been hit": set by the car–car solver when the contact's
    /// `j` is not negative and the class is 0x10 (0x496A10 / 0x496A2B). The skill test's cone
    /// count is the number of these (0x425950, 0x416AC0). Never cleared during a race.
    pub knocked_over: bool,
    /// `car+0x354`, the collision mass in the original's units (kg / 128, 0x4107CD) — read here
    /// only for a smackable (`ClampAngVel` 0x4964FD compares it with 0.75, and the wall / prop
    /// hits hand it to `Damage`'s crash response, 0x494964).
    pub collision_mass: f32,
    /// Engine wheelspin state (`car+0xDB8 == 2`, clutch slipping / spinning rear axle).
    pub rear_spin: bool,
    /// Body damage per zone 0..100 (`car+0x4E4..`, `Damage` 0x494500). The ring runs **0 front-left,
    /// 1 front, 2 front-right, 3 right, 4 rear-right, 5 rear, 6 rear-left, 7 left**, 8 roof —
    /// CONFIRMED by the damage decals, whose zone-0/6/7 patches live on the page the car's left-hand
    /// geometry uses and whose 2/3/4 patches live on the right-hand page, while 1, 5 and 8 appear on
    /// both (`RE/notes/car-model.md` §11).
    pub damage: [f32; 10],
    pub damage_enabled: bool,
    /// Steering pull from uneven front damage plus suspension-damage noise (`0x49F160`).
    pub damage_pull: f32,
    /// `car+0x55C / +0x560 / +0x564`, the force-feedback words `Car_physicsStep` writes at
    /// 0x49F4F6…0x49F5DB ([`crate::force_feedback::ff_words`]). **Output only** — nothing in the
    /// simulation reads them; the driver 0x4123E0 does.
    pub ff_words: [f32; 3],
    /// Material class of the last wall touched (`[0x628C88]` word → class, see `walls::wall_class`).
    pub last_wall_class: u8,
    /// `Damage` 0x494500: a corner / roof zone crossed 30 this tick → play the crash one-shot
    /// (`RE/notes/suspension-collision.md` §6.5, `sfx.md` §4.3). Cleared by the caller.
    pub crash_sound: bool,
    /// `word car+0x418`: `Susp_update` writes `nAir > 1` into it every ground pass (0x49A8B3…
    /// 0x49A8C0); a landing clears it when upright (`cosTilt > 0.9` hard, `> 0.86` resting,
    /// 0x49C23B / 0x49C2C4 — the `flags & 0x400` exemption is dead, `physics.md` §9); a car–car
    /// contact steps it for both cars (0x496F1E / 0x496F25); `Damage`'s heavy impact sets it to 1
    /// (0x4948D5); a reset clears it (0x40F43C). Readers outside this crate: the wrong-way
    /// counter's hold (0x4A7714), the crash-hold clear (0x40F86D), the reset exit
    /// (`<= 5`, 0x40F615), the engine sound (`> 5`, 0x4A79E9), 0x412730 and 0x4124AC.
    pub airborne_count: u16,
    /// `Car_ground`'s hard-landing event `0x40F020(car, 1, −1, 0, 5, closing, 0)` (0x49C1D6…
    /// 0x49C1E6), pushed for every car: the closing speed. Taken (and cleared) by the caller.
    pub landing_sound: Option<f32>,
    /// `Car_ground`'s roll-over event `0x40F020(car, 1, −1, 4, 3, 40.0, 0)` (0x49C0CE…0x49C0DE).
    /// Taken (and cleared) by the caller.
    pub rollover_sound: bool,
    /// `Car_ground`'s tilted arm, the body–ground contact (`BodyGroundCollide` 0x496610 →
    /// `GroundImpulse` 0x495F30): the event word `car+0x438 = 0x30000 | car+0x464` (0x4960F9…
    /// 0x496104) with `car+0x430 = 4·|vel·n|` after the impulse (0x49683A…0x496844) — here the
    /// magnitude and the nibble. 0x40F740 turns it into CarBody × `0x5CC4D8[nibble]` at
    /// `vol = 0x430/8` above 1.5 unless a later contact of the tick wrote its own word. Taken (and
    /// cleared) by the caller; output only.
    pub ground_sound: Option<(f32, usize)>,
    /// `car+0x348`, the direction of travel: `normalize(vel)` at the end of every `Car_ground`, or
    /// the body's forward row `car+0x37C` when the velocity is exactly zero (0x49C14F…0x49C341).
    /// The skid-mark feed 0x4A2880 reads it (0x4A28CB…0x4A291D).
    pub travel_dir: Vec3,
    pub wheels: [Wheel; 4],
    pub accel_local: Vec3,
    /// `car+0xD40…+0xD48`, the acceleration in world axes: the physics frame's transpose times
    /// the local one after the stability assist (0x49F9E6…0x49FA1B). Written only when a wheel
    /// is on the ground — the no-contact exit 0x49F715 clears `car+0xD4C…+0xD54` and returns
    /// before it — and zeroed by the resets (0x49E379, 0x410AB5). `Susp_update`'s lateral term
    /// reads it (0x49A13C).
    pub accel_world: Vec3,
    pub lateral_filtered: f32,
    pub long_filtered: f32,
    pub gravity_scale: f32,
    pub ticks_alive: u32,
    /// `car+0xDB0`: 1 at the race init (0x410BD6), counted up every input step while non-zero
    /// (0x49DD29…0x49DD34). Zero is a dead car — 0x49DD4E holds the pedals, throttle 0 and the
    /// brake full, and skips the gearbox — and two things kill it: an impact past 120 on a car
    /// that is not tiny (`Damage` 0x4948C1) and a body-ground contact that leaves the car
    /// upside down (`car+0x3A0 < 0`, 0x49C01B). A reset re-arms it (0x49E220, 0x49E31E…0x49E3E5):
    /// 60 when it was alive, 1 when it was dead.
    pub engine_alive: u32,
    /// `car+0xAC8` / `+0xACC`: the front wheels' angle smoothed, `s = s/2 + angle/2` each step
    /// right after the angle is worked out (0x49F6CC…0x49F6F6); zero after a reset (0x49E361) and
    /// at race init (0x410BD0). The model's steer channels take it in a replay (`car-model.md`).
    pub steer_smoothed: [f32; 2],
    /// `car+0x1100`, the crash reset countdown (32 Hz ticks) that 0x40F740 runs — see
    /// [`Self::reset_countdown_tick`]. `Damage` loads it with 0xC0 on a heavy impact (0x4948E6).
    pub reset_countdown: i32,
    /// `car+0xB4C`, the hold after a reset request went through: 0xA0 ticks (0x40F841), one off
    /// every 0x40F740 pass (0x40F854…0x40F85F).
    pub reset_hold: u32,
    /// nominal wheel offsets in body space (x right, y up, z forward) — from the model
    pub wheel_offsets: [Vec3; 4],
    pub ride_height: f32,
    /// `car+0x35C` as stored — `max(|vx|, |vz|) + 0.25·min(|vx|, |vz|)` of the world velocity,
    /// written by `Car_ground` (0x49BEA9…0x49BF02), `PostHit` (0x493EEB) and zeroed by
    /// `Car_placeAtSlice` (0x49B403). `Roughness` 0x499730 reads the value held over from the
    /// previous `Car_ground` (0x4997CC), since `Susp_update` runs before the new one is stored.
    pub speed_est: f32,
    /// dive / roll low-pass states (car+0xDC4 / +0xDC8)
    pub dive: f32,
    pub roll_lp: f32,
    /// previous ground height (car+0x41C), body height above the contact plane (car+0x424), min (car+0x428)
    pub ground_h: f32,
    pub body_height: f32,
    pub min_height: f32,
    /// `car+0x3A8`, `+0x3AC`, `+0x3B0`. For a **racer** they are half the render model's LOD-5
    /// box, the box holding the model origin — **0x445090** at load (`nfs_game::world::car_body`),
    /// `× 0.4` for `.TINYCAR` — and nothing re-cuts them: `Car_physInit` 0x49B510 only writes them
    /// back where they already were (0x4107C1). *(Corrected 2026-09-25: this said every car's were
    /// half the span of the body points; that re-cut, 0x4A1472…0x4A14A6 in 0x4A1280, runs only
    /// for a smackable, `car+0x52C & 0x80` at 0x4107A1 — [`Vehicle::fit_half_extents`].)* They
    /// are the half extents the prop-footprint test reads (`RE/AUDIT.md` §B), the car–car box, the
    /// wall corners, and the ones the car's drop shadow is cut to (`R3DCar` slot 22, 0x43B220).
    pub half_extents: Vec3,
    /// `car+0x3AC` as the model is **drawn**: `R3DCar` slot 5, **0x43A410**, returns
    /// `M[1] · car+0x3AC`, and the draw 0x443490 (0x4434D7) and the view-box test 0x439F90
    /// (0x43A147) place the model at `car+0x330` minus it ([`Self::draw_origin`]); `R3DObject`'s
    /// slot 5 (0x444DC0) returns zero. It is also the `y` the `BndP` loader took off every body
    /// point (0x4421BE), so the body origin `car+0x330` sits this far above the model's ground
    /// plane. 0 until [`Self::set_car_body`] — the port's older frame, origin on the contact plane.
    pub body_lift: f32,
    /// The car's collision body points `car+0x8C` in the body frame (`BndP`,
    /// `RE/notes/car-model.md` §12), relative to the body origin: for a racer the loader's
    /// `y −= car+0x3AC` puts the wheel quad [`Self::body_lift`] below it
    /// ([`Self::set_car_body`]). `0x49ABC0` sorts them ascending by height, so the first four are
    /// the wheel contact set. Empty falls back to the eight corners of [`Self::half_extents`],
    /// which is what the port did before the records were decoded.
    pub body_points: Vec<Vec3>,
    /// The `.sim` mass the body-ground impulse divides by (`CarSpecs::mass`), kept so the prop
    /// collider can run it without the specs
    pub sim_mass: f32,
    /// last wall impulse magnitude (car+0x430)
    pub impact: f32,
    /// `car+0x438` as the car–car solver 0x496860 writes it for each body (0x496C97…0x496CBD for A,
    /// 0x496DAE…0x496DCF for B, CONFIRMED 2026-09-26): `0x50000 | other.car+0x44C` when the other
    /// body is a smackable (`car+0x52C & 0x80`), else `0x50003`; `car+0x434` = the other car, so
    /// 0x40F740 takes `idB = word & 0xFF` (0x40FA38) — CarBody × CarBody, or CarBody × the
    /// smackable's material (a cone: 0x10 Pylon). Output only; 0 until a pair contact.
    pub pair_event: u32,
    /// `car+0x430`, the event magnitude 0x40F740 reads at the tick's end, **as the tick's writers
    /// leave it** (batch A6): `GroundImpulse` / `BodyGroundCollide` assign it (0x4960E6,
    /// 0x496844), the props and the car–car solver assign it (0x4984B8, 0x498575, 0x498EF6,
    /// 0x496B42…0x496C3E), and the wall pass 0x495020 keeps the **larger** of it and the pass's
    /// own largest `3·|mag|` (0x49557D…0x495594, on every call, hit or not). Output only.
    /// 0x40F740 clears it: the app does, after the sparks ([`Self::take_contact`]).
    pub contact_mag: f32,
    /// `car+0x438 & 0xEFFF0000`, the kind of the last writer: `0x30000` the body on the ground
    /// (0x496104), `0x40000` a wall (0x49411E…), `0x50000` a prop or a car (0x498A44, 0x498DB6,
    /// 0x496CB5…), `0x60000` a bush (0x498F00). 0 when nothing wrote it this tick.
    pub contact_kind: ContactKind,
    /// `car+0x440…+0x448`, the **last contact point** in world space — read 2026-09-26, CONFIRMED.
    /// The byte scan of `+0x440` / `+0x444` / `+0x448` finds seven writers, all in the collision
    /// code: the wall pass 0x495506 (the car's own **corner** that crossed the edge, the point it
    /// hands `WallImpulse` 0x494000 — stored after the call whatever the impulse, 0x495504…0x49551E),
    /// `GroundImpulse` 0x4960FE (the **centroid** `C` of the body points under the plane, its
    /// second argument, 0x495F87 / 0x49610A…0x496119), the car–car solver 0x496F46…0x496F88 (the
    /// **midpoint** of the two cars' positions, `0.5·(A.pos + B.pos)`, into both), the prop
    /// cylinder 0x498DCA (the prop's surface point `centre + n·R`, the `a` it hands 0x498380), the
    /// prop box 0x498A4D (the car's own position `car+0x330` after `BodyGroundCollide`), and the
    /// bush / breakable-start overlaps 0x498EF0 / 0x498F33 (the car's position). Nothing clears it
    /// (not the reset 0x40F400). Readers: the force-feedback jolt 0x40F91A → 0x412230 and the
    /// player's one-shot pan 0x4A7BB1…0x4A7BDB. Output only — nothing in the physics reads it.
    pub last_contact: Vec3,
    /// Output only: the largest `3·|mag|` `WallImpulse` answered in the last wall pass
    /// ([`Self::collide_walls`]; 0 without a hit) — the `car+0x430` the wall pass keeps
    /// (0x49557D…0x495594) for 0x40F740, whose kind-4 arm fires the sparks 0x43F4C0 over 1.5
    /// (0x40F944…0x40F9B9). Nothing in the physics reads it.
    pub last_wall_effect: f32,
    /// The body's own ground tracker `car+8` and the plane `car+0x3E8` / `car+0x400` 0x499520
    /// keeps from it ([`PatchTracker`]): 0x49AF70 runs it at the head of every physics tick
    /// ([`Self::step32`]); `Car_ground`'s tilted arm, the 8 m ceiling rule and `Susp_update`'s
    /// no-patch fallback read it.
    pub tracker: PatchTracker,
    /// `car+0xDAC == 1`: a network block has come for this car (0x4A0DF0 sets it on the first one),
    /// i.e. it is a **remote** car in a positional race. `Susp_update` skips the compression block
    /// (`[0x657408]` and `+0xDAC`, 0x499D22…0x499D34) and `WheelRotation` 0x4A2790 turns the wheels by
    /// `car+0x35C / circumference` whatever the drive (0x4A2799). `vehicle/netcar.rs`.
    pub net_car: bool,
    /// LCG state (the original's `0x5D1028 × 0x5D102C`) for road roughness and the engine's noise.
    rng: u32,
}

impl Vehicle {
    pub fn new(position: Vec3, orientation: Mat3, specs: &CarSpecs, half_track: f32, wheel_radius: f32) -> Self {
        Self::bare(position, orientation, specs.wheelbase, specs.mass, half_track, wheel_radius)
    }

    /// The fields [`Self::new`] fills, from the two numbers of the specs it reads — so a body that
    /// has no `carSpecs` block (a smackable, `Car_installHandlers` 0x410D9B) can be built too.
    pub fn bare(position: Vec3, orientation: Mat3, wheelbase: f32, mass: f32, half_track: f32, wheel_radius: f32) -> Self {
        let hb = wheelbase * 0.5;
        Self {
            position,
            velocity: Vec3::ZERO,
            orientation,
            angular_velocity: Vec3::ZERO,
            frame: orientation,
            ground_normal: Vec3::Y,
            cos_tilt: 1.0,
            engine_kill: 0,
            engine_arm: EngineArm::None,
            draft: 1.0,
            boost: 0.0,
            misfire_roll: 1.0,
            shift_roll: None,
            height_marker: 0.0,
            v_local: Vec3::ZERO,
            w_local: Vec3::ZERO,
            slip_tangent: 0.0,
            rpm: 0.0,
            gear: 1,
            prev_gear: 1,
            shift_timer: 0,
            requested_gear: 1,
            downshift_blip: false,
            reverse_latch: false,
            // a car no race clock drives (the scenarios) behaves as one made on the start tick
            game_tick: 0x1C1,
            race_state: RaceState::Racing,
            rally: false,
            tcs: false,
            tiny: false,
            derby: false,
            ignition_cut: 0,
            throttle_raw: 0,
            brake_raw: 0,
            throttle_byte: 0,
            brake_byte: 0xFF,
            wet_track: false,
            throttle: 0.0,
            brake: 0.0,
            steer_target: 0,
            steer: 0,
            handbrake: false,
            automatic: true,
            race_started: true,
            traction_control: false,
            ai_driven: false,
            is_traffic: false,
            smackable: false,
            material_class: 0,
            disturbed: true,
            knocked_over: false,
            collision_mass: mass / 128.0,
            rear_spin: false,
            damage: [0.0; 10],
            damage_enabled: true,
            damage_pull: 0.0,
            ff_words: [0.0; 3],
            last_wall_class: 5,
            crash_sound: false,
            airborne_count: 0,
            landing_sound: None,
            ground_sound: None,
            rollover_sound: false,
            travel_dir: orientation.row(2),
            wheels: [Wheel::default(); 4],
            accel_local: Vec3::ZERO,
            accel_world: Vec3::ZERO,
            lateral_filtered: 0.0,
            long_filtered: 0.0,
            gravity_scale: GRAVITY_SCALE,
            ticks_alive: 0,
            engine_alive: 1,
            steer_smoothed: [0.0; 2],
            reset_countdown: 0,
            reset_hold: 0,
            wheel_offsets: [
                Vec3::new(-half_track, wheel_radius, hb),
                Vec3::new(half_track, wheel_radius, hb),
                Vec3::new(-half_track, wheel_radius, -hb),
                Vec3::new(half_track, wheel_radius, -hb),
            ],
            ride_height: wheel_radius,
            speed_est: 0.0,
            dive: 0.0,
            roll_lp: 0.0,
            ground_h: position.y - wheel_radius,
            body_height: 0.0,
            min_height: 0.0,
            half_extents: Vec3::new(half_track + 0.15, 0.6, hb + 0.9),
            body_lift: 0.0,
            body_points: Vec::new(),
            sim_mass: mass,
            impact: 0.0,
            pair_event: 0,
            last_contact: Vec3::ZERO,
            last_wall_effect: 0.0,
            contact_mag: 0.0,
            contact_kind: ContactKind::None,
            tracker: PatchTracker::new(Vec3::new(position.x, position.y - wheel_radius, position.z)),
            net_car: false,
            rng: 0x3ADE_68B1,
        }
    }

    /// The four ground probes `car+0x8C[0…3]` in the body frame: the first four [`Self::body_points`]
    /// as [`body_points_setup`] (0x49ABC0) leaves them — FL, FR, RL, RR by quadrant. A body with
    /// fewer than four points (the port's scenarios) takes its tyre bottoms, `wheel_offsets` less
    /// `ride_height` (port fallback, no original counterpart).
    fn probe_points(&self) -> [Vec3; 4] {
        if self.body_points.len() >= 4 {
            std::array::from_fn(|i| self.body_points[i])
        } else {
            std::array::from_fn(|i| {
                let off = self.wheel_offsets[i];
                Vec3::new(off.x, off.y - self.ride_height, off.z)
            })
        }
    }

    /// A racer's body as `RCar::load` leaves it (`nfs_game::world::car_body`): the `BndP` points
    /// already lowered by `car+0x3AC` (0x4421BE) and the half extents of 0x445090. The position
    /// the car was placed at is taken as the model's ground origin — where the port kept the body
    /// origin — and the origin `car+0x330` is lifted `M[1]·car+0x3AC` above it, so the model is
    /// drawn where it was ([`Self::draw_origin`]) and the physics turns about the original's point.
    pub fn set_car_body(&mut self, points: Vec<Vec3>, half_extents: Vec3) {
        self.body_points = points;
        self.half_extents = half_extents;
        self.position += self.orientation.row(1) * (half_extents.y - self.body_lift);
        self.body_lift = half_extents.y;
    }

    /// Where the model is drawn: `car+0x330 − M[1]·car+0x3AC` (`R3DCar` slot 5 0x43A410, the draw
    /// 0x443490 at 0x4434D7…0x4435A0) — [`Self::body_lift`]; the body origin itself when it is 0.
    pub fn draw_origin(&self) -> Vec3 {
        self.position - self.orientation.row(1) * self.body_lift
    }

    /// Re-cut [`Self::half_extents`] to the body points, as **0x4A1472** does: half the span of
    /// `body_points` on each axis. A car with no points keeps whatever it was built with. That
    /// re-cut is a **smackable's** (0x4A1280, reached from 0x4107B6 only when `car+0x52C & 0x80`);
    /// a racer keeps 0x445090's model box ([`Self::set_car_body`]).
    pub fn fit_half_extents(&mut self) {
        let Some(first) = self.body_points.first().copied() else { return };
        let (mut lo, mut hi) = (first, first);
        for p in &self.body_points {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        self.half_extents = (hi - lo) * 0.5;
    }

    /// `ClampAngVel`'s limit in turns/s (0x4964D0, 0x49BBA0): 0.75; 2.25 for a `.TINYCAR`
    /// (`carinfo+0x518`, tested first, 0x4964E0); **6.75** for a smackable (`car+0x52C & 0x80`)
    /// whose collision mass `car+0x354` is under 0.75 (0x4964F4…0x496512, `[0x5B49BC]`).
    pub(crate) fn ang_limit(&self) -> f32 {
        if self.tiny {
            ANG_LIMIT_TINY
        } else if self.smackable && self.collision_mass < SMACKABLE_LIGHT_MASS {
            ANG_LIMIT_SMACKABLE
        } else {
            ANG_LIMIT_DEFAULT
        }
    }

    /// The original's 16-bit LCG, drawn inline at every call site: `prod = state · 123456789`,
    /// `state = prod & 0xFFFF`, `rand16 = (prod >> 8) & 0xFFFF` (`traffic.md` §14.7). The original
    /// keeps **one** stream for the whole game; this crate sits below `nfs-game`, so it carries its
    /// own state with the same arithmetic.
    fn rand16(&mut self) -> u32 {
        let prod = self.rng.wrapping_mul(123_456_789);
        self.rng = prod & 0xFFFF;
        (prod >> 8) & 0xFFFF
    }

    /// `(rand16 · n) >> 16` — the original's "random below n".
    fn rand_below(&mut self, n: u32) -> u32 {
        (self.rand16() * n) >> 16
    }

    pub fn forward(&self) -> Vec3 {
        self.orientation.row(2)
    }
    pub fn speed(&self) -> f32 {
        self.velocity.length()
    }

    /// `car+0x35C`: `max(|vx|, |vz|) + 0.25·min(|vx|, |vz|)` of the world velocity (0x49BEA9…0x49BF02)
    /// — the estimate the original's gates read, not a true norm.
    pub fn horizontal_speed(&self) -> f32 {
        let (a, b) = (self.velocity.x.abs(), self.velocity.z.abs());
        norm_est(a, b)
    }

    /// `car+0x464`, the surface nibble of the body's own tracker as 0x49AF70 stores it
    /// (0x49AFCC…0x49AFF5, CONFIRMED): `car+0x2C & 0xF`, and under `RALLY` `[0x6573F8]` a nibble 1
    /// or 10 reads as 2. The squeal / scrape producers (0x40FC1B, 0x4100BB) and `GroundImpulse`'s
    /// event word (0x4960D6) take it — not a wheel's surface.
    pub fn surface_nibble(&self) -> usize {
        let n = (self.tracker.word & 0xF) as usize;
        if rally_reads_as_two(n, self.rally) { 2 } else { n }
    }

    /// 0x40F7DB…0x40F865, the reset request: the command `car+0xD8B == 5` (the AI's, or a human's
    /// button byte `>> 3`, 0x499344) goes through to `0x40F540(car, 2, 0)` only while the car is
    /// racing (`car+0xB50 == 0`), the game tick is past 0x300, no hold is running and
    /// `car+0x35C` is under 27.6 (`[0x5B2734]`); then the hold is 0xA0. The hold counts down on
    /// every pass. (`car+0x540` / `+0x544`, also required to be 0, never leave 0 in a race —
    /// `physics.md` §4.1c; `car+0xD89`, the other request, is only ever cleared.)
    pub fn reset_request(&mut self, requested: bool, game_tick: u64) -> bool {
        let go = requested
            && self.race_state == RaceState::Racing
            && game_tick > TICK_RESET_ALLOWED
            && self.reset_hold == 0
            && self.horizontal_speed() < RESET_SPEED_LIMIT;
        if go {
            self.reset_hold = RESET_HOLD;
        }
        if self.reset_hold > 0 {
            self.reset_hold -= 1;
        }
        go
    }
    /// The brake lamps' pedal (0x43937A…0x439392): `car+0xD7F` (throttle) while the automatic's
    /// reverse latch `car+0xD98` is set, else `car+0xD80` (brake) — the bytes as 0x49DD92…0x49E066
    /// leave them, before the reverse gear swaps the pedals for driving. So in reverse the throttle
    /// key lights them, and once the latch is gone (a race that has ended) the brake key does again.
    pub fn brake_lamp(&self) -> bool {
        if self.reverse_latch { self.throttle_byte > 0 } else { self.brake_byte > 0 }
    }

    pub fn v_long(&self) -> f32 {
        self.v_local.z
    }

    /// `Tyre` 0x4A20D0 + `FrictionCircle` 0x4A1DC0 + `Slide` 0x4A1C70 — CONFIRMED
    /// (`RE/notes/tyre-slide.md` §2–4, ported formula by formula).
    ///
    /// The steering angle law: the front wheels' angle for a given ramped steering value, damage
    /// pull included, with the high-speed taper and the ×1/(v·0.0125) above 80 m/s.
    ///
    /// Pulled out of the step so a car the caller drives itself — a network player's, whose own
    /// step never runs — can have the same wheels. [`Self::steer_wheels_now`] is the convenience
    /// form that takes the values off the vehicle.
    pub fn set_wheel_steer(&mut self, specs: &CarSpecs, steer_int: f32, speed: f32, v_fwd: f32) {
        let mut y = specs.max_steer;
        // **Below** 30 m/s, not above — `physics.md` had the condition inverted and the port
        // followed it. 0x49F5EA loads the speed, 0x49F5F0 compares it with `[0x5B2408]` = 30.0,
        // and 0x49F5FB's `test ah,1; je` **skips** the factor when C0 is clear, i.e. when
        // `speed >= 30`. So `1.75 − 0.025·speed` runs from 1.75 at rest down to 1.0 at 30 m/s
        // and the lock is flat above that — a speed-sensitive rack. The old reading drove the
        // factor negative and took all the steering away at 250 km/h.
        if speed < 30.0 {
            y *= 1.75 - 0.025 * speed;
        }
        y = y.clamp(0.0, 20.0) * 17.0 / specs.max_steer;
        let mut s0 = steer_int * (1.0 - self.damage[0] / 512.0) * (y + specs.steer_offset) / 127.0;
        let mut s1 = steer_int * (1.0 - self.damage[2] / 512.0) * (y - specs.steer_offset) / 127.0;
        if v_fwd > 80.0 {
            s0 /= v_fwd * 0.0125;
            s1 /= v_fwd * 0.0125;
        }
        self.wheels[0].steer = s0;
        self.wheels[1].steer = s1;
        self.wheels[2].steer = 0.0;
        self.wheels[3].steer = 0.0;
        self.steer_smoothed = [self.steer_smoothed[0] * 0.5 + s0 * 0.5, self.steer_smoothed[1] * 0.5 + s1 * 0.5];
    }

    /// [`Self::set_wheel_steer`] with this vehicle's own steering, speed and forward velocity.
    pub fn steer_wheels_now(&mut self, specs: &CarSpecs) {
        let (s, sp, f) = (self.steer.clamp(-127, 127) as f32, self.speed(), self.v_long());
        self.set_wheel_steer(specs, s, sp, f);
    }

    /// Front wheel `i`'s angle in **turns**, positive to the right — the same angle [`Self::tyre`]
    /// turns the contact patch by, so the drawn wheel and the simulated one cannot disagree.
    pub fn wheel_steer_turns(&self, i: usize, specs: &CarSpecs) -> f32 {
        steer_angle_256(specs.max_steer, self.wheels[i].steer) / 256.0
    }

    /// Slip velocities are ×(−16) in the steered-wheel frame; forces are m/s² of car acceleration.
    /// Not modelled: the `.ABS` arm (0x4A2308 / 0x4A23CC — `carinfo+0x24` is 0 in every race,
    /// `physics.md` §5.4). `traction_control` stands for the AI-driven flag `car+0xDA8`, `tcs` for
    /// `.TRACTION` (`carinfo+0x20`), `rally` for `RALLY` (`[0x6573F8]`).
    fn tyre(&mut self, i: usize, specs: &CarSpecs, tyre_damage: f32) {
        let front = i < 2;
        let ai = self.traction_control;
        let rally = self.rally;
        let tcs = self.tcs;
        let slip_tangent = self.slip_tangent;
        let rear_spin = self.rear_spin;
        let handbrake = self.handbrake;
        let vz = self.v_local.z;
        let throttle = self.throttle;
        let throttle_byte = self.throttle_byte;
        let brake_byte = self.brake_byte;
        let gear = self.gear;
        let class = specs.class;
        let k_class = CIRCLE_SOFTNESS[class];
        let w = &mut self.wheels[i];
        let ang = steer_angle_256(specs.max_steer, w.steer);
        let (mut u, mut v) = (w.slip_vel[0], w.slip_vel[2]);
        if ang != 0.0 {
            let a = ang / 256.0 * TAU;
            // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable sin/cos, not std; original 0x5327E0 / 0x5327F0 (x87) at 0x4A2159 / 0x4A2163.
            let (sn, cs) = nfs_math::portable::sin_cosf(a);
            let (x, z) = (u, v);
            u = x * cs - z * sn;
            v = x * sn + z * cs;
        }
        w.slip = 0.0;
        w.locked = false;
        w.lateral = 0.0;
        w.longitudinal = 0.0;
        // §2.2 demand gate: only a demand opposing the wheel motion is limited to the slip speed
        let mut d = w.demand;
        let mut opposing = false;
        if d < 0.0 && v < 0.0 {
            if throttle <= 0.25 || gear != 0 {
                d = d.max(v);
                opposing = true;
            }
        } else if d > 0.0 && v > 0.0 && (throttle <= 0.25 || gear <= 1) {
            d = d.min(v);
            opposing = true;
        }
        d *= specs.grip;
        // `Tyre` itself works on the **raw** `wheel+0xAC`, which 0x4A20DF copies to a local before
        // anything else: the over-limit test (0x4A22A7), the lock speed `2·(16 − LIM0)`
        // (0x4A2313), the clamp to `±LIM0` (0x4A238E…0x4A23C2) and both lateral curves
        // (`fmul [esp+0x18]`, 0x4A2512 / 0x4A262C) read it. Tyre damage (`0x493980`,
        // `1 − 0.005·d`) enters only `FrictionCircle` (0x4A1E62…0x4A1E84) and `Slide`
        // (0x4A1C77…0x4A1C95). *(Corrected 2026-09-26: the port used the damaged limit throughout.)*
        let lim0 = w.limit;
        let limit = lim0 * (1.0 - 0.005 * tyre_damage);
        // §2.3 over-limit decision: lock (Slide) or clamp, for braking-type demands / handbrake only
        if (opposing && d.abs() > lim0) || (handbrake && !front && vz.abs() > 0.5) {
            let lock = if handbrake {
                true
            } else if 2.0 * (16.0 - lim0) < vz || brake_byte < 0xEC || (vz.abs() < 5.0 && !rear_spin) {
                false
            } else if !ai {
                true
            } else {
                // 0x4A2373: the whole word against 1
                w.zone <= 1 && class != 2
            };
            if lock {
                // §4 Slide: kinetic friction opposing the slip velocity, no back-rotation (original)
                let (a, b) = (u.abs(), v.abs());
                let comb = norm_est(a, b);
                let lim2 = limit * (1.0 - 1.0 / k_class);
                w.slip = if comb > limit { (comb - limit) / 4.0 } else { 0.0 };
                w.slip = (0.1 * w.slip).min(1.0);
                // 0x4A1D70…0x4A1D96: `s = LIM2 / comb` above 0.01, and at or below it the `|U|`
                // still on the stack; then `Flat = s·U`, `Flon = s·V` (0x4A1D96…0x4A1DAA) with no
                // clamp. *(Corrected 2026-09-26: the port took `s = 0` and clamped each component
                // to twice its slip speed, a "safety net" with no counterpart in the binary.)*
                let s = if comb > 0.01 { lim2 / comb } else { a };
                w.lateral = s * u;
                w.longitudinal = s * v;
                w.locked = true;
                w.sliding = true;
                w.demand = d;
                return;
            }
            d = tyre_demand_clamp(d, lim0);
        }
        w.demand = d;
        // §2.4 lateral force from the slip angle (1/256 turn)
        let lat = if v == 0.0 {
            0.0
        } else {
            // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable atan2, not std; original x87 atan2 in the slip block 0x4A1D96…0x4A1DAA.
            let a = nfs_math::portable::atan2f(u, -0.5 * v) / TAU; // turns
            256.0 * if v < 0.0 { a } else if u > 0.0 { 0.5 - a } else if u < 0.0 { -0.5 - a } else { a }
        };
        // 0x4A24E8 branches on `wheel+0xC0`, which is **not** a sliding flag: 0x410BB2 (car init)
        // and 0x49E345 (reset) set it to 1 for wheels 0 and 1 and 0 for wheels 2 and 3 and nothing
        // in the image ever writes it again, so it selects the lateral curve **by axle**. The front
        // pair follows `min(|lat|, 16)·load/12`, the rear `min(|lat|, 2)·load·0.5` with a floor of
        // one unit — a deliberate understeer bias of about six to one at small slip
        // (`physics.md` §5.2).
        if front {
            let f = lat.signum() * lat.abs().min(16.0) * lim0 / 12.0;
            w.lateral = if u.abs() + v.abs() < 32.0 { u.signum() * u.abs().min(f.abs()) } else { f };
        } else {
            let m = if lat.abs() <= 1.0 { 1.0 } else { lat.abs().min(2.0) };
            let f = lat.signum() * m * lim0 * 0.5;
            w.lateral = u.signum() * u.abs().min(f.abs());
        }
        w.longitudinal = d;
        // §3 friction circle
        let lon_w = w.longitudinal.abs() * if rally { 0.5 } else { 0.9 };
        let lat_a = w.lateral.abs();
        let comb = norm_est(lat_a, lon_w);
        let spin = (w.longitudinal.abs() > limit && throttle_byte > 0x80) || (!front && rear_spin);
        let k = if !front && spin { 4.0 } else { k_class };
        // 0x4A1F25 tests `comb > LIM` alone; `comb > 0` is the port's own guard, for a case **never observed**
        // (2026-10-02 SRV-B, `RE/notes/REFERENCE-2026-10-02-SRVB.md` §5: 4 races, ≥ 350 000 tyre evaluations, a roll onto the roof —
        // `comb == 0` and `LIM < 0` never once; min LIM 0.21, min comb 0.33). Kept as a NaN guard.
        // With a limit driven below zero (`LoadTransfer` has no floor) and no force at all
        // (`comb` exactly 0: needs `d = 0` and a front `v == 0.0` or `u == 0.0`), the original
        // divides by 0 here (0x4A1F95 / 0x4A2007) and gets ±inf / NaN in both arms — `lateral` and
        // `longitudinal` are `0 · inf`. Triaged 2026-09-30 (batch A6): STILL OPEN, B-runtime. A
        // limit `μ·share·g_eff/2 − dz` is negative only when the transfer `dz` outgrows the static
        // load; the debug counters put into this branch (removed again) saw **no negative limit** in the three
        // race goldens (8 s each) and a 60 s alps race with three opponents (1921 ticks, all four
        // wheels each): the case was never reached, so nothing shows what the original's NaN would
        // do to the car (it would poison every later tick). **Re-observed on the original
        // (2026-10-01 SV, Wine, `fe = 0` alps race with three opponents, the car idling at the grid, gdb at
        // 0x4A1F25 printing `limit` = `[esp+0x18]` and `comb` = `[esp+0x14]`): 402 000 evaluations (all
        // wheels) over ~10 min of race time, minimum limit 0.0, none below it and no `comb == 0 && limit <=
        // 0` — the case is not reached by a car sitting or rolling gently; a hard landing / roll was not
        // produced (the original ran at well under 1 frame a second under the breakpoint). STILL OPEN.
        if comb > limit && comb > 0.0 {
            let hard = (tcs && !front && slip_tangent.abs() < 0.15) || (ai && (w.zone > 1 || class == 2));
            if hard {
                let s = limit / comb;
                w.slip = 0.0;
                if gear > 2 || ai {
                    w.lateral *= s;
                }
                w.longitudinal *= s;
            } else {
                let e = (comb - limit).min(limit) / k;
                let s = (limit - e) / comb;
                w.slip = (3.0 * w.slip + (if front { 0.5 } else { 12.0 }) * (comb - limit)) / 4.0;
                w.lateral *= s;
                w.longitudinal *= s;
            }
        }
        if rally && front {
            w.lateral *= 1.2;
        }
        if !front && spin && !tcs {
            w.slip = comb;
        }
        w.slip = w.slip.min(1.0);
        if tcs {
            w.slip *= 0.5;
        }
        w.sliding = w.slip > 0.5;
        if ang != 0.0 {
            let a = -ang / 256.0 * TAU;
            // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable sin/cos, not std; original 0x5327E0 / 0x5327F0 (x87) at 0x4A2159 / 0x4A2163.
            let (sn, cs) = nfs_math::portable::sin_cosf(a);
            let (x, z) = (w.lateral, w.longitudinal);
            w.lateral = x * cs - z * sn;
            w.longitudinal = x * sn + z * cs;
        }
    }

    /// 32 Hz step: input, forces, velocity integration, ground contact (`Car_simulate`).
    pub fn step32(&mut self, controls: &Controls, specs: &CarSpecs, grid: &TriangleGrid) {
        // 0x49AF70 (`+0xE0C`, priority 6, ahead of `Car_simulate` / `Car_ground` at 0x1E): the car
        // tracker at the position and, on a new patch, the plane 0x499520 keeps
        self.tracker.track(grid, self.position);
        self.ticks_alive += 1;
        self.latch_input(controls, specs);
        // `Car_simulate` 0x49FD30, read to the instruction 2026-09-26: `car+0x3A0` (the ground
        // normal's `y`, [`Self::cos_tilt`] as the previous `Car_ground` left it) `< 0.5`
        // (0x49FD3B, `test ah,1; je 0x49FE11` calls the step at `>=`) skips `Car_physicsStep`
        // 0x49F160 **entirely** — no input processing, no engine, no forces — and instead:
        // the wheelspin state `car+0xDB8`, the slip tangent `car+0xDC0` and the four wheels'
        // slip `wheel+0xB8` (`car+0x870 + 0xC4·i`) = 0 (0x49FD4C…0x49FD6D); the rpm `car+0xDB4`
        // −500, or 0 at `<= 500` (0x49FD6F…0x49FD96); and when the body height `car+0x424 < 0.5`
        // (0x49FDA2) the velocity `car+0x33C` ×0.95 (`0x3F733333`, whole vector) and, when also
        // `car+0x3A0 < 0.2`, the yaw rate `car+0x38C` and the dive / roll states `car+0xDC4` /
        // `+0xDC8` ×0.98 (0x49FDD9…0x49FE09). No gate on `car+0x52C`, `+0xDB0` or `+0xB50` here.
        // *(Corrected: the port ran the whole step on a tilted car and then damped it, with no
        // height gate, no rpm decay and the dive / roll states left alone.)*
        if self.cos_tilt < 0.5 {
            self.rear_spin = false;
            self.slip_tangent = 0.0;
            for w in &mut self.wheels {
                w.slip = 0.0;
            }
            self.rpm = if self.rpm > 500.0 { self.rpm - 500.0 } else { 0.0 };
            if self.body_height < 0.5 {
                self.velocity *= 0.95;
                if self.cos_tilt < 0.2 {
                    self.angular_velocity.y *= 0.98;
                    self.dive *= 0.98;
                    self.roll_lp *= 0.98;
                }
            }
        } else {
            self.physics_step(specs);
        }
        self.ground_contact(grid, specs);
    }

    /// `Car_physicsStep` 0x49F160, which `Car_simulate` calls on an upright car. The named steps
    /// below run in the binary's order; how each was corrected over time is in
    /// `RE/notes/physics-history.md` §1.
    fn physics_step(&mut self, specs: &CarSpecs) {
        let m = self.frame;
        // 0x628F9C = !(w0.air && w1.air && bodyHeight > 0.05), likewise for the rear axle
        let front_contact = !(self.wheels[0].airborne && self.wheels[1].airborne && self.body_height > 0.05);
        let rear_contact = !(self.wheels[2].airborne && self.wheels[3].airborne && self.body_height > 0.05);
        let vl = self.body_frame_velocity(m);
        // `car+0x35C`, the estimate `Car_ground` makes of the **world** velocity's `x` / `z`
        // (0x49BEA9…0x49BF02) — the roll factor (0x49F2EA), the steering rack (0x49F5EA) and the
        // tail's gates (0x49FB98, 0x49FBE5, 0x49FCF7) all load it; the step never rewrites it,
        // so the tail sees the value from before this tick's integration. Confirmed by disassembly
        // 2026-09-27: 0x49F2EA is `fld dword ptr [esi+0x35c]`, a bare memory load, no recompute.
        // [`Self::speed_est`] is [`car_ground_body`]'s stored `car+0x35C` from the previous tick's
        // `Car_ground` (history: `physics-history.md` §1.2).
        let speed = self.speed_est;
        self.contact_patch_slip(specs, vl, speed);
        let gravity_l = m * Vec3::new(0.0, -GRAVITY, 0.0);
        // 0x49F3D4: `Car_processInput` 0x49DD20 runs here, after `car+0xD58…+0xD6C`, `car+0xDC0`,
        // the contact-patch slip velocities and `[0x628FA0…A8]` — so its gearbox (the automatic's
        // request, the pick out of neutral, `AutoShift`) reads this tick's body-frame velocity and
        // slip tangent (history: `physics-history.md` §1.4).
        self.process_input(specs);
        self.steering_and_damage_pull(specs, front_contact, gravity_l, speed, vl);
        let a_drive = self.engine_update(specs, rear_contact);
        if !front_contact && !rear_contact {
            self.accel_local = Vec3::ZERO;
            for w in &mut self.wheels {
                w.slip = 0.0;
                w.lateral = 0.0;
                w.longitudinal = 0.0;
            }
        } else {
            self.wheel_demands(specs, vl, a_drive);
            self.axle_tyres(specs, front_contact, rear_contact);
            let torque_y = self.tyre_force_sums(specs);
            self.stability_assist(vl);
            self.integrate_forces(m, torque_y);
            self.tail_damping(specs, speed, gravity_l, vl);
        }
    }

    /// `Car_physicsStep` 0x49F160 up to 0x49F270: the body-frame velocities `car+0xD58` /
    /// `+0xD64` and the slip tangent `car+0xDC0`. Returns `car+0xD58…+0xD60`.
    fn body_frame_velocity(&mut self, m: Mat3) -> Vec3 {
        self.v_local = m * self.velocity;
        self.w_local = m * self.angular_velocity;
        let vl = self.v_local;
        // **Signed, not `|vz|`.** 0x49F25C loads `car+0xD60` with no `fabs` and 0x49F262 compares
        // it with `[0x5B2400]` = 5.0; 0x49F270's `test ah,0x41; jne` zeroes the ratio when
        // `vz <= 5`. So driving **backwards** the slip tangent is always 0 in the original
        // (history: `physics-history.md` §1.1).
        self.slip_tangent = if vl.z > 5.0 { vl.x / vl.z } else { 0.0 };
        vl
    }

    /// `Car_physicsStep` 0x49F2EA…, before `Car_processInput` at 0x49F3D4: the contact-patch slip
    /// velocities (×−16) with the yaw/roll term; `speed` is `car+0x35C` ([`Self::speed_est`]).
    fn contact_patch_slip(&mut self, specs: &CarSpecs, vl: Vec3, speed: f32) {
        // contact-patch slip velocities (×−16) with the yaw/roll term
        let yaw = specs.yaw_lever * self.angular_velocity.y * 32.0 * -0.5;
        // 0x49F2F0 compares the speed with `[0x5B241C]` = 10.0 and 0x49F301's `je` goes to the
        // **`+70 … ×0.0125`** arm when `speed >= 10`, while the fall-through at 0x49F303 is
        // `+30 … ×0.025` for `speed < 10`. (Both give `1.0·yaw` at exactly 10, so the curve is
        // continuous either way and only the branch order tells them apart; history:
        // `physics-history.md` §1.3.)
        let roll = if speed < 10.0 { (speed + 30.0) * yaw * 0.025 } else { (speed + 70.0) * yaw * 0.0125 };
        for (i, w) in self.wheels.iter_mut().enumerate() {
            let lat = -16.0 * vl.x + if i < 2 { roll } else { -roll };
            w.slip_vel = [lat, -16.0 * vl.y, -16.0 * vl.z];
        }
    }

    /// `Car_physicsStep` 0x49F3EE…0x49F5EA: the steering angle law — the gravity offset, the
    /// damage pull and its noise, the force-feedback words — into [`Self::set_wheel_steer`].
    fn steering_and_damage_pull(&mut self, specs: &CarSpecs, front_contact: bool, gravity_l: Vec3, speed: f32, vl: Vec3) {
        // steering angle law. Both float→int steps here are **bare `fistp`** — 0x49F3EE for the
        // gravity offset and 0x49F4C9 for the damage pull — so they round to nearest (ties to
        // even), not truncate; see `RE/CONVENTIONS.md` (history: `physics-history.md` §1.5).
        let g_off = fistp(gravity_l.x * 8.0).clamp(-64, 64);
        // damage pull, read to the instruction 2026-09-26 (0x49F412…0x49F4C3): `zone2·¼ − zone0·¼`,
        // and when the game tick `[0x606AC4]` is a multiple of 4 (`and 0x80000003`, 0x49F437) a
        // draw of the shared LCG (0x49F44B…0x49F473) makes the noise
        // `SuspDamage(car) · (r/65536 − 0.5) · 0.5` (0x493BC0, `[0x5B253C]`, `[0x5B23F0]`), scaled
        // by `car+0x35C · 1/15` **below** 15 m/s (`[0x5B243C]`, `[0x5B49EC]`; `test ah,1; je` skips
        // it at 15 and above; confirmed by disassembly 2026-09-27: 0x49F498 and 0x49F4AE are both
        // `fld dword ptr [esi+0x35c]`, the same stored estimate `speed` above, not a fresh
        // recompute). History: `physics-history.md` §1.6.
        let mut pull = self.damage[2] * 0.25 - self.damage[0] * 0.25;
        let mut noise_term = 0.0f32; // `[ebp+8]`, left 0 on a tick that is not a multiple of 4 (0x49F430)
        if self.game_tick.is_multiple_of(4) {
            let r = self.rand16();
            let mut noise = self.suspension_wear(specs) * (r as f32 * 1.525_878_9e-5 - 0.5) * 0.5;
            let hs = speed;
            if hs < 15.0 {
                noise = noise * hs * 0.066_666_67;
            }
            noise_term = noise;
            pull += noise;
        }
        self.damage_pull = pull;
        // 0x49F4F6…0x49F5DB: the force-feedback words, from the front wheels' slip fractions as the
        // previous `Tyre` left them (this runs before it). Output only.
        self.ff_words = crate::force_feedback::ff_words(front_contact, self.wheels[0].slip + self.wheels[1].slip, specs.grip, noise_term, pull, g_off, speed);
        self.set_wheel_steer(specs, (self.steer + fistp(pull) + g_off).clamp(-127, 127) as f32, speed, vl.z);
    }

    /// `Car_physicsStep` with ground contact: the brake force, `LoadTransfer` and each wheel's
    /// demand from the drive split (0x49F7E4…0x49F894); clears the tyre forces.
    fn wheel_demands(&mut self, specs: &CarSpecs, vl: Vec3, a_drive: f32) {
        let mut a_brake = specs.brake_decel * self.brake * (1.0 - 0.005 * self.brakes_wear(specs));
        a_brake = a_brake.min(vl.z.abs() * 32.0);
        let sign = -vl.z.signum();
        let a_brake = a_brake * if vl.z == 0.0 { 0.0 } else { sign };
        let a_bf = a_brake * specs.front_brake;
        self.load_transfer(specs);
        let f = specs.front_drive;
        let (l0, l1, l2, l3) = (self.wheels[0].limit, self.wheels[1].limit, self.wheels[2].limit, self.wheels[3].limit);
        // The drive split, read to the instruction 2026-09-26 (0x49F7E4…0x49F894): a plain
        // `fdiv` by `L0 + L1` (`wheel+0xAC` at 0x864 / 0x928) and `L2 + L3` (0x9EC / 0xAB0),
        // no floor — `a·specs[0x5C]·L0`, `L1·specs[0x5C]·a`, `(1 − specs[0x5C])·L2·a`,
        // `(1 − specs[0x5C])·L3·a` in that operand order. Since `LoadTransfer` no longer floors
        // the limits (b9141f7) a sum can be negative, and the original divides by it (history:
        // `physics-history.md` §1.7). An exactly zero sum gives the axle no drive share here, where the
        // original's x87 divide makes ±inf / NaN of it — **unreachable** (settled 2026-09-30,
        // batch A6): a sum is `2·μ·share·g_eff/2 ± dz` with `μ ≥ 0.5` in both grip tables
        // (0x5D0D50 / 0x5D0E4C), `share` the front weight 0.44…0.485 of all 86 shipped
        // `.sim`s (or its complement), `grip ≥ 1.1`, so zero needs `g_eff = up.y·grip·g` to be
        // exactly 0 (a car on its side to the last bit) or an exact cancellation with `dz`; a
        // zero or negative-sum tick was not seen in 84 s of races (the three goldens and a 60 s
        // alps race) either.
        let share = |x: f32, s: f32| if s == 0.0 { 0.0 } else { x / s };
        let (fs, rs) = (l0 + l1, l2 + l3);
        self.wheels[0].demand = share(a_drive * f * l0, fs) + 0.5 * a_bf;
        self.wheels[1].demand = share(l1 * f * a_drive, fs) + 0.5 * a_bf;
        self.wheels[2].demand = share((1.0 - f) * l2 * a_drive, rs) + 0.5 * (a_brake - a_bf);
        self.wheels[3].demand = share((1.0 - f) * l3 * a_drive, rs) + 0.5 * (a_brake - a_bf);
        for w in &mut self.wheels {
            w.lateral = 0.0;
            w.longitudinal = 0.0;
        }
    }

    /// `Car_physicsStep`: `Tyre` on each axle in contact.
    fn axle_tyres(&mut self, specs: &CarSpecs, front_contact: bool, rear_contact: bool) {
        // 0x493980, the tyre figure: the zones and the part list do not change inside the step, so
        // the four tyres share one evaluation
        let tyre_damage = if front_contact || rear_contact { self.tyres_wear(specs) } else { 0.0 };
        if front_contact {
            self.tyre(0, specs, tyre_damage);
            self.tyre(1, specs, tyre_damage);
        }
        if rear_contact {
            self.tyre(2, specs, tyre_damage);
            self.tyre(3, specs, tyre_damage);
        }
    }

    /// `Car_physicsStep` …0x49F9E3: the body-frame acceleration `car+0xD4C…+0xD54` from the tyre
    /// forces, and the yaw torque it returns.
    fn tyre_force_sums(&mut self, specs: &CarSpecs) -> f32 {
        let w = &self.wheels;
        let fx: f32 = w.iter().map(|w| w.lateral).sum();
        let fz: f32 = w.iter().map(|w| w.longitudinal).sum();
        self.accel_local = Vec3::new(fx, 0.0, fz * specs.inv_grip);
        // yaw torque, 0x49F9A0…0x49F9E3 re-read 2026-09-26: `A − B` with
        // `A = ((w1.lat + w0.lat) − (w2.lat + w3.lat)) · specs[0x1B8]` and
        // `B = ((w2.long + w0.long) − w1.long − w3.long) · specs[0x1B8]` (the wheels' `+0x90` /
        // `+0x98` at 0x848 / 0x850, 0x90C / 0x914, 0x9D0 / 0x9D8, 0xA94 / 0xA9C) — the sum is
        // exact; MEDIUM only on the sign conventions the tyre model gives `+0x90` / `+0x98`
        ((w[1].lateral + w[0].lateral) - (w[2].lateral + w[3].lateral)) * specs.inv_yaw_lever
            - ((w[2].longitudinal + w[0].longitudinal) - w[1].longitudinal - w[3].longitudinal) * specs.inv_yaw_lever
    }

    /// Stability assist 0x49F0C0 (called at 0x49F9EC), history in `physics-history.md` §1.8.
    ///
    /// The lateral term: 0x49F11F compares `|car+0xD58|` with 1.0 and 0x49F12A's
    /// `test ah,0x41; je` jumps **straight to the add** when `|x| > 1`; only the
    /// `|x| <= 1` path falls through to the second test, `[car+0x374] < 0.833`
    /// (0x49F132 / 0x49F13D). So the condition is `|vL.x| > 1 || M[1].y < 0.833`.
    ///
    /// The longitudinal term is not unconditional either: 0x49F0E3 is
    /// `cmp byte [ecx+0xD80], 0x40; jae 0x49F104`, so `−4.9 · M[2].y` is added only while
    /// the **brake byte is under 64** of 255.
    ///
    /// Re-read 2026-09-26 (0x49F0C0…0x49F157): the assist adds **into `car+0xD4C` / `+0xD54`
    /// themselves** (`fadd [ecx+0xD54]; fstp [ecx+0xD54]`, 0x49F0F8 / 0x49F14B), so the world
    /// acceleration (0x49F9F6…), the filters `+0xE08` / `+0xE04` (0x49FAFE…0x49FB34) and next
    /// tick's `LoadTransfer` and `Susp_update` all see the sum; the tilts it reads are the
    /// **body** rows `car+0x368` / `+0x380` (the orientation's `M[0].y` / `M[2].y`), not the
    /// physics frame `car+0x490`; and the byte is `car+0xD80` as 0x49DD20 leaves it — 0xFF
    /// before the start, so a car on a sloped grid is not pushed downhill. Not carried: the gate
    /// at 0x49F0C4…0x49F0E1 — skipped for a traffic body (`car+0x52C & 0x10`, which never runs
    /// `Car_simulate`) and in a network race (`[0x6573E8] == 3` with `[0x657408]`).
    fn stability_assist(&mut self, vl: Vec3) {
        if vl.x.abs() > 1.0 || self.orientation.row(1).y < SLOPE_UP_Y {
            self.accel_local.x += SLOPE_PUSH_X * self.orientation.row(0).y;
        }
        if self.brake_byte < 0x40 {
            self.accel_local.z += SLOPE_PUSH_Z * self.orientation.row(2).y;
        }
    }

    /// `Car_physicsStep` 0x49F9F6…0x49FB8F: the world acceleration `car+0xD40`, the 32 Hz velocity
    /// / angular-velocity step, the filters `+0xE08` / `+0xE04` and the `RALLY` drag.
    fn integrate_forces(&mut self, m: Mat3, torque_y: f32) {
        let local = self.accel_local;
        let acc = m.transpose() * local;
        self.accel_world = acc;
        let angacc = m.transpose() * Vec3::new(0.0, torque_y, 0.0);
        self.velocity += acc * DT32;
        self.angular_velocity.x += angacc.x * DT32;
        self.angular_velocity.z += angacc.z * DT32;
        // the yaw step is 1/48 of the angular acceleration, 1/32 for `.TINYCAR` (0x49FAD4)
        self.angular_velocity.y += angacc.y * if self.tiny { 1.0 / 32.0 } else { 1.0 / 48.0 };
        self.lateral_filtered = 0.25 * self.accel_local.x + 0.75 * self.lateral_filtered;
        self.long_filtered = 0.9 * self.long_filtered + 0.1 * self.accel_local.z;
        // 0x49FB3A: under `RALLY` the velocity loses `0.002 · min(|car+0xDC0|, 5)` of itself
        // each step (0x532360 over `car+0x33C`, `[0x5B2400]` 5, `[0x5B4A30]` 0.002)
        if self.rally {
            self.velocity *= 1.0 - 0.002 * self.slip_tangent.abs().min(5.0);
        }
    }

    /// The tail of `Car_physicsStep`, 0x49FB8F…0x49FD21: four mutually exclusive velocity
    /// scalings, each arm a `ret` (history: `physics-history.md` §1.9).
    ///
    /// * 0x49FB8F: the body's surface word `car+0x464 == 3` and `car+0x35C > 44.64`
    ///   (`[0x5B4A70]`; `test ah,0x41; jne` skips at `<=`) → the world velocity `car+0x33C`
    ///   ×0.995 (`0x3F7EB852`, 0x532360) and return — none of the arms below.
    /// * gear `car+0xD82 > 1` (signed `jle`, 0x49FBCC): the coast yaw damping. 0x49FBE5 loads
    ///   the speed and compares it with `[0x5B2408]` = 30.0; 0x49FBF3's `test ah,1; je`
    ///   **bails** when `speed >= 30`, so the damping is a *low*-speed effect. 0x49FBFE then
    ///   loads `specs[0xF4]` against `[0x5B2464]` = 2.5 and 0x49FC0C's `test ah,0x41; jne`
    ///   bails when it is `<= 2.5`, so it needs a grip multiplier **above** 2.5. Any failed
    ///   test goes to 0x49FC51, whose `cmp cl,1; jne` sends a forward gear to the handbrake.
    /// * gear `== 1`, neutral (0x49FC54…0x49FCE3): when `|[0x628FA8]| < 0.5` (gravity along
    ///   the frame's `z`, `test ah,1; je` bails at `>=`), `f = 0.998` (`0x3F7F7CEE`) if
    ///   `|car+0xD60| >= 20` and `|car+0xD94| <= 32` (`jle`), else `f = 0.99`
    ///   (`0x3F7D70A4`); the velocity `car+0x33C` **and** the angular velocity `car+0x388`
    ///   (`[ebp−0x10]`, 0x49FA8B) ×f, whole vectors, and return. Otherwise the handbrake.
    /// * 0x49FCED, the handbrake: byte `car+0xD85` and `car+0x35C < 1` → velocity ×0.5;
    ///   0x49FCFD compares the speed with 1.0 and 0x49FD05's `je` bails when `speed >= 1`, so
    ///   it is the stop-it-dead behaviour at a standstill, not a braking force at speed.
    fn tail_damping(&mut self, specs: &CarSpecs, speed: f32, gravity_l: Vec3, vl: Vec3) {
        if self.surface_nibble() == 3 && speed > SURFACE3_DAMP_SPEED {
            self.velocity *= 0.995;
        } else if self.gear > 1 && self.throttle < 0.25 && speed < 30.0 && specs.grip > 2.5 {
            let k = 1.0 - 0.05 * self.angular_velocity.y.abs();
            self.velocity.x *= k;
            self.velocity.z *= k;
        } else if self.gear == 1 && gravity_l.z.abs() < 0.5 {
            let f = if vl.z.abs() >= 20.0 && self.steer.abs() <= 32 { 0.998 } else { 0.99 };
            self.velocity *= f;
            self.angular_velocity *= f;
        } else if self.handbrake && speed < 1.0 {
            self.velocity *= 0.5;
        }
    }

    /// 64 Hz integration (`0x49B7A0` / `0x49B8E0`) and wheel rotation (`0x4A2790`).
    pub fn step64(&mut self, specs: &CarSpecs) {
        self.game_tick = self.game_tick.saturating_add(1);
        self.integrate64();
        // wheel rotation, `WheelRotation` 0x4A2790 (audited 2026-09-26): driven = front
        // `specs[0x5C] >= 0.01` / rear `<= 0.99`, neutral or undriven `vL.z / circ`, else
        // `rpm / (ratio'[gear]·circ)`, a locked wheel skipped, `·1/64`. The original wraps only
        // above 1 turn and by `ang − round(ang)` (`fistp`, 0x4A2839…0x4A2858), `rem_euclid` here —
        // the same angle modulo a turn, which is all the drawing reads. The `ratio != 0` test is the
        // port's guard and never fires (settled 2026-09-30, batch A6): a driven wheel outside
        // neutral (`gear != 1`, 0x4A2799…) divides by `ratio'[gear]·circ`, `ratio' = final_drive ·
        // gear_ratio · 60 / circ` (`specs.rs`), and all 86 shipped `.sim`s have a non-zero final
        // drive, reverse ratio and ratio in every gear up to `gear_count`, the only zero ratios being
        // the unused slots of a shorter box, which no gear selects.
        let vz = self.v_local.z;
        for (i, wh) in self.wheels.iter_mut().enumerate() {
            let driven = if i < 2 { specs.front_drive >= 0.01 } else { specs.front_drive <= 0.99 };
            // in the binary's order: `rpm / (ratio'·circ)` (0x4A281F…0x4A2829 `fdivr`) or
            // `vL.z / circ` (0x4A280E), then `· [0x5B2828]` = 1/64
            // 0x4A2799 (`car+0xDAC == 1`): a remote car's wheels turn by `car+0x35C / circumference`,
            // the locked flag not consulted
            let turns = if self.net_car {
                self.speed_est / specs.circumference
            } else if driven && self.gear != 1 && specs.ratio_rpm_per_mps[self.gear] != 0.0 {
                self.rpm / (specs.ratio_rpm_per_mps[self.gear] * specs.circumference)
            } else {
                vz / specs.circumference
            };
            if self.net_car || !wh.locked {
                wh.rotation = (turns * (1.0 / 64.0) + wh.rotation).rem_euclid(1.0);
            }
        }
    }

    /// The two shared 64 Hz integrators every car record runs (`car+0xE24` / `+0xE28`):
    /// `0x49B7A0` `pos += vel · 1/64` and `0x49B8E0`'s rotation by `ω/64`.
    ///
    /// 0x49B7A0 moves the four wheel contact points `wheel+0x40` by the same `vel/64` as the body
    /// (0x49B825…0x49B84C, skipped for a smackable, `car+0x52C & 0x80`) before the skid feed
    /// 0x4A2880 reads them — here [`Wheel::world_pos`], which the port keeps [`Self::ride_height`]
    /// above the contact. *(Corrected 2026-09-26: the port left them where `Susp_update` put them,
    /// half a physics tick behind on every other 64 Hz step.)*
    pub fn integrate64(&mut self) {
        let d = self.velocity * DT64;
        self.position += d;
        if !self.smackable {
            for w in &mut self.wheels {
                w.world_pos += d;
            }
        }
        self.orientation = orientation_step(self.orientation, self.angular_velocity * DT64);
    }

    /// 0x40F740's end of tick for the contact word: the magnitude and the kind, cleared
    /// (`car+0x430 = car+0x434 = car+0x438 = 0`, 0x40FAFB…0x40FB0D).
    pub fn take_contact(&mut self) -> (f32, ContactKind) {
        (std::mem::take(&mut self.contact_mag), std::mem::take(&mut self.contact_kind))
    }

    /// Reset onto the track at a slice pose (the game's "reset car" 0x40F540 mode 3 — placement only,
    /// the original's exact conditions are not reproduced).
    pub fn reset_to(&mut self, position: Vec3, orientation: Mat3) {
        self.position = position;
        self.orientation = orientation;
        self.frame = orientation;
        self.velocity = Vec3::ZERO;
        self.angular_velocity = Vec3::ZERO;
        self.speed_est = 0.0;
        // 0x40F694 → 0x49E220, which clears `car+0xD40…+0xD48` (0x49E379)
        self.accel_world = Vec3::ZERO;
        self.rpm = 0.0;
        self.gear = if self.automatic { 2 } else { 1 };
        self.requested_gear = self.gear;
        self.shift_timer = 0;
        for w in &mut self.wheels {
            *w = Wheel::default();
        }
        self.rearm_after_reset();
        // 0x40F59D: every reset clears the crash countdown; 0x40F400 the `+0x418` word (0x40F43C),
        // `+0x42C` (0x40F443) and the impact `+0x430` (0x40F44F)
        self.reset_countdown = 0;
        self.airborne_count = 0;
        self.knocked_over = false;
        self.impact = 0.0;
    }

    /// `Car_placeAtSlice` **0x49B090** (car, slice, &offset, dir) — read to the instruction
    /// 2026-09-25, CONFIRMED. `centre` / `slice_fwd` are the slice's `+0x04` / `+0x1C`, `offset`
    /// the slice-local `{lateral, up, along}` the caller hands in (`Grid_slot` 0x410580:
    /// `{±½ lane width, 0.5, 0}`; a reset 0x40F540: zero, or an opponent's racing-line point),
    /// `opponent` the `car+0x52C & 8` bit and `flip` the `dir != 1` flag (0x49B0B9).
    ///
    /// 1. `L` = the slice centre — or, for an opponent after tick 0x1C0 (`[0x606AC4]`) with a
    ///    non-zero offset, the offset itself as a world position (0x49B0F1…0x49B14A); the car
    ///    tracker `updateZone(L, 1)`, the plane point `car+0x400` = the patch's mean corner and
    ///    `car+0x3E8` its normal (`PatchTracker::seat`).
    /// 2. The frame `car+0x3B8`: up `N` = the tracker's patch normal (0x474420), lateral
    ///    `normalize(N × F)`, forward `normalize(lateral × N)` with `F` the slice's forward
    ///    (0x49B22F…0x49B2AD, `0x532880(a, b)` = `a × b`).
    /// 3. `car+0x330` = the slice centre + `offset.x·lateral + offset.y·N + offset.z·forward`
    ///    (0x532A00 transposes the frame, three dots 0x532910; 0x49B3B7…0x49B3F2), or `L` in the
    ///    opponent case (0x49B397). The orientation and the physics frame `car+0x490` are that
    ///    frame, lateral and forward negated when `flip` (0x417260 / 0x417310).
    /// 4. 0x49AF70 (the car tracker at the new position), then — the car alive (`car+0x7F`) —
    ///    **`Susp_update` twice with the body dropped in between**: `car+0x41C = h`,
    ///    `car+0x334 −= car+0x424` (`BodyHeight`'s height of the lowest body points above the
    ///    contact plane), `car+0x424 = 0`, `car+0x41C = h` again (0x49B493…0x49B4C7); last every
    ///    wheel's compression `+0x68`, airborne `+0x84` and `+0x70` are zeroed (0x49B4D0…0x49B4E9).
    ///
    /// So a racer's origin ends half its model box (`car+0x3AC`) above the road, whatever the
    /// offset's 0.5: the drop is along world `y`, the whole `car+0x424`. Not carried: the wheel
    /// trackers' slice (0x49B183, the port's trackers keep none), `car+0x7E = 0` (0x49B409),
    /// the wheels' `+0x34` / `+0x40` seeded with the world body points (0x49B439…0x49B47D,
    /// overwritten by the first `Susp_update`).
    ///
    /// *(Corrected 2026-09-27, `F`'s source: disassembled to the instruction. `Tracker::setSlice`
    /// **0x473210** — called on the car tracker at 0x49B0C9, right after the `slice` argument is
    /// clamped to `max(1, slice)` (0x49B09F…0x49B0A9) and before `L` is read — writes
    /// `word [tracker] = clamped_slice` (0x47321C); `Tracker::updateZone` **0x474060** (the
    /// `updateZone(L, 1)` of point 1) never touches that word (it writes only `+2`, `+8…+0xA`,
    /// `+0x24`, `+0x28`, confirmed by disassembling the whole function). So the `movsx eax, word
    /// [tracker]` at **0x49B1F6**, which indexes the slice table for `F` (0x49B20D…0x49B24D), reads
    /// back the *same* `clamped_slice` `setSlice` just stored — not some tracker state left over
    /// from an earlier, unrelated tick. `F` is therefore deterministically `slices[max(1,
    /// slice)].forward`, **independent of `L`/`offset`/`absolute`** — including in the opponent
    /// branch, where `L` is the racing-line point but `F` is still the plain `slice` argument's own
    /// forward. The earlier note here ("the tracker's after `updateZone(L)`") had the writer wrong;
    /// there is no "stale tracker" value to fall back to.
    ///
    /// This port's `slice_fwd` parameter already plays that role directly — a free input,
    /// independent of `centre`/`offset` — and every current caller passes it from the *same*
    /// `Slice` as `centre`/the offset/racing-line point came from (traced 2026-09-27: `main.rs`
    /// 9831, 9933, 21185, 21668; `police.rs` 2762, 3660; `traffic.rs` 1420, 1676 all index one
    /// `Slice` for both). So for this port's calling convention the two are never out of step —
    /// **NOT A BUG**, `vehicle.rs:3065`'s "port takes the slice it was given" approximation note
    /// closed without a behaviour change. The one residual gap is the original's `max(1, ·)` clamp,
    /// which only bites a caller that resolves slice index 0 — this API takes `Vec3`s, not an
    /// index, so a caller would have to special-case it; no current caller ever targets slice 0.)*
    #[allow(clippy::too_many_arguments)]
    pub fn place_at_slice(&mut self, grid: &TriangleGrid, specs: &CarSpecs, centre: Vec3, slice_fwd: Vec3, offset: Vec3, opponent: bool, flip: bool) {
        let absolute = opponent && self.game_tick > TICK_RACE_START && offset != Vec3::ZERO;
        // `car+0x35C = 0` (0x49B403)
        self.speed_est = 0.0;
        let l = if absolute { offset } else { centre };
        self.tracker.seat(grid, l);
        let n = self.tracker.normal;
        let lat = n.cross(slice_fwd).normalize_or_zero();
        let fwd = lat.cross(n).normalize_or_zero();
        self.position = if absolute { offset } else { centre + lat * offset.x + n * offset.y + fwd * offset.z };
        let s = if flip { -1.0 } else { 1.0 };
        self.orientation = Mat3::from_cols(lat * s, n, fwd * s).transpose();
        self.frame = self.orientation;
        self.tracker.track(grid, self.position);
        self.ground_h = self.susp_update(grid, specs, false);
        self.position.y -= self.body_height;
        self.body_height = 0.0;
        self.ground_h = self.susp_update(grid, specs, false);
        for w in &mut self.wheels {
            w.compression = 0.0;
            w.airborne = false;
            w.contact = true;
        }
    }

    /// **The skill-test start, 0x410430** (`Car_raceInit` 0x4108B2, taken instead of
    /// `Grid_slot` + `Car_placeAtSlice` when the scene has a type-2 trigger and `[0x6573EC] == 4`):
    ///
    /// 1. `car+0x330` = the trigger's position (vtable `+0x38`, 0x41045A…0x410483) — no offset;
    ///    the car tracker and the four wheel trackers `setSlice(1)` (0x473210, 0x410486…0x4104A6:
    ///    no cached patch), then 0x49AF70 tracks the car at that position, and the zone having
    ///    changed, 0x499520 takes the patch normal into `car+0x3E8` (`PatchTracker::track`).
    /// 2. The frame: forward `car+0x37C` = the trigger's `+0xC8` (0x46F0F0), up `car+0x370` =
    ///    `car+0x3E8`, right `car+0x364` = up × forward, forward = right × up (two 0x532880),
    ///    re-orthonormalised by 0x4A4810 — and copied to `car+0x3B8` and to the physics frame
    ///    `car+0x490` (0x410504…0x410528).
    /// 3. 0x49AB60 (the world body points into the wheels), then **one** `Susp_update`: `car+0x41C`
    ///    = it, `car+0x334 −= car+0x424`, `car+0x424 = 0` (0x41052F…0x41055F) — the body dropped
    ///    (or lifted) onto the pad by `BodyHeight`, as 0x49B090 does but without its second
    ///    `Susp_update`; last each wheel's `+0x68`, `+0x84`, `+0x70` are zeroed (0x41056C…0x41057A).
    ///
    /// Not carried: `car+0x80 = 0x20` (0x410521, no reader known to the port), 0x49AB60's seeds
    /// (overwritten by the first `Susp_update` here as in `place_at_slice`), the wheel trackers'
    /// reset (the port's re-find their patch each `Susp_update`).
    pub fn place_at_trigger(&mut self, grid: &TriangleGrid, specs: &CarSpecs, position: Vec3, heading: Vec3) {
        self.position = position;
        self.tracker.forget_patch();
        self.tracker.track(grid, self.position);
        let up = self.tracker.plane_normal;
        let right = up.cross(heading);
        let fwd = right.cross(up);
        self.orientation = Mat3::from_cols(right, up, fwd).transpose();
        self.renormalise();
        self.frame = self.orientation;
        self.ground_h = self.susp_update(grid, specs, false);
        self.position.y -= self.body_height;
        self.body_height = 0.0;
        for w in &mut self.wheels {
            w.compression = 0.0;
            w.airborne = false;
            w.contact = true;
        }
    }

    /// 0x40F740's countdown (0x40FB13…0x40FBA2, 32 Hz, every car — the traffic's runs in
    /// `nfs_game::traffic::TrafficSet::post_impact`, 2026-09-26): while it runs it
    /// loses a tick, eight more off the road surface (`car+0x464 == 0`); an upright car
    /// (`car+0x3A0 > 0.75`) that is alive stops it, otherwise at zero the car is reset to its
    /// slice (`0x40F540(car, 0, 0)`) — the `true` returned, and the caller resets the car before
    /// [`Self::reset_countdown_start`] runs, as the original's start test reads the new pose.
    /// `off_surface` is the caller's.
    pub fn reset_countdown_tick(&mut self, off_surface: bool) -> bool {
        let mut reset = false;
        if self.reset_countdown > 0 {
            self.reset_countdown -= 1;
            if off_surface {
                self.reset_countdown -= 8;
            }
            if self.cos_tilt > 0.75 && self.engine_alive != 0 {
                self.reset_countdown = 0;
            } else if self.reset_countdown <= 0 {
                reset = true;
            }
        }
        reset
    }

    /// 0x40FB6D…0x40FBA2: while the countdown is not running it starts at 0xC0 when the car's up
    /// is less than 0.1 along the slice's (`car+0x370 · car+0x3C4`) or the car is off the surface.
    pub fn reset_countdown_start(&mut self, slice_up: Vec3, off_surface: bool) {
        if self.reset_countdown <= 0 && (self.orientation.row(1).dot(slice_up) < 0.1 || off_surface) {
            self.reset_countdown = RESET_COUNTDOWN;
        }
    }

    /// The alive count's part of 0x49E220, which every reset through 0x40F540 runs (0x40F694):
    /// 60 when the car was alive, 1 when it was dead (0x49E31E…0x49E3E5); and the smoothed
    /// steering it clears (0x49E361 / 0x49E367).
    pub fn rearm_after_reset(&mut self) {
        self.engine_alive = if self.engine_alive != 0 { 0x3C } else { 1 };
        self.steer_smoothed = [0.0; 2];
    }

    /// `0x4A4810(M)`, the orientation's re-orthonormalisation — run by 0x49B8E0 every 32nd 64 Hz
    /// step (the countdown `car+0x80`, 0x49BA01…0x49BA1D), and by the placements (0x4104FF).
    /// Read to the instruction 2026-09-26: four passes (`ebp = 4`, 0x4A4821) of
    /// `D = Mᵀ·M − I` (0x532A00 transposes, 0x532470 multiplies row-major, the identity at
    /// `0x5D1068`), `S = I + Σ c_k·D^k` over the three coefficients at `0x5D1040` —
    /// **−0.125, 0.09375, −0.078125** (the loop stops at `0x5D104C`), `R ← R·D` each term — and
    /// `M ← M·S` (0x4A4A12). Those are a quarter of the `(1 + x)^(−½)` series, so a pass takes
    /// only a quarter of the way to `M·(MᵀM)^(−½)`: the error falls by about ¾ a pass, 0.32 over
    /// the four, and the matrix is never made exactly orthonormal. *(Corrected 2026-09-26: the
    /// port rebuilt the rows by Gram–Schmidt from the forward axis, exactly orthonormal.)* The
    /// order of the three products inside each element is the port's (0x532470 sums them in an
    /// order of its own per element).
    pub fn renormalise(&mut self) {
        self.orientation = renormalise_matrix(self.orientation);
    }
}

/// `0x4A4810(M)` on a plain matrix — the routine [`Vehicle::renormalise`] runs on the car's
/// orientation (its doc has the whole reading) and the camera's slot matrix (0x462F30 at
/// 0x463071) runs on its own.
pub fn renormalise_matrix(m: Mat3) -> Mat3 {
    type M3 = [[f32; 3]; 3];
    fn mul(a: &M3, b: &M3) -> M3 {
        let mut d = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                d[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
            }
        }
        d
    }
    const I: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    const C: [f32; 3] = [-0.125, 0.093_75, -0.078_125];
    let mut a: M3 = [m.row(0).to_array(), m.row(1).to_array(), m.row(2).to_array()];
    for _ in 0..4 {
        let t: M3 = std::array::from_fn(|i| std::array::from_fn(|j| a[j][i]));
        let mut d = mul(&t, &a);
        for (i, row) in d.iter_mut().enumerate() {
            row[i] -= 1.0;
        }
        let (mut r, mut s) = (I, I);
        for c in C {
            r = mul(&r, &d);
            for i in 0..3 {
                for j in 0..3 {
                    s[i][j] += r[i][j] * c;
                }
            }
        }
        a = mul(&a, &s);
    }
    Mat3::from_cols(Vec3::from(a[0]), Vec3::from(a[1]), Vec3::from(a[2])).transpose()
}

#[cfg(test)]
mod tests;

/// The 64 Hz orientation step `0x49B8E0` (`car+0xE28`), shared by every car record — the racers
/// through [`Vehicle::integrate64`], the cops through their own kinematic step. `w_turns` is
/// `ω/64` in turns.
///
/// 0x49B8E0 builds three axis rotations and multiplies them **X, then Y, then Z**:
/// 0x532600(M_y, ω_y/64), 0x5325A0(M_x, ω_x/64), 0x532660(M_z, ω_z/64) — each angle in turns
/// ([0x5B2828] = 1/64, and 0x5327E0/0x5327F0 are `sin`/`cos` of `angle × 2π`) — then
/// 0x49B968 `0x532470(M_x, M_y, T)` = `T = M_x · M_y`, 0x49B991 `Δ = T · M_z` and 0x49B9A6
/// `car+0x364 = car+0x364 · Δ`, in place through the aliasing guard at 0x532482.
///
/// The three matrices are stored row-major and act on **row** vectors, so each is the
/// transpose of glam's same-named rotation: `M_x = [[1,0,0],[0,c,s],[0,-s,c]]`,
/// `M_y = [[c,0,-s],[0,1,0],[s,0,c]]`, `M_z = [[c,s,0],[-s,c,0],[0,0,1]]`. A row of
/// `car+0x364` therefore maps to `Δᵀ · row`, and
/// `Δᵀ = (M_x M_y M_z)ᵀ = M_zᵀ M_yᵀ M_xᵀ` = **`Rz · Ry · Rx`** in glam's convention.
/// This was `Ry · Rx · Rz` until 2026-09-21 — the right three matrices in the wrong order,
/// which agrees to first order in ω/64 and drifts in the second.
pub fn orientation_step(orientation: Mat3, w_turns: Vec3) -> Mat3 {
    let w = w_turns;
    // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable rotations, not glam/std; original RotY/RotX/RotZ 0x532600 / 0x5325A0 / 0x532660 (x87) at 0x49B880…0x49B977.
    let rot = nfs_math::portable::mat3_rotation_z(w.z * TAU) * nfs_math::portable::mat3_rotation_y(w.y * TAU) * nfs_math::portable::mat3_rotation_x(w.x * TAU);
    // rows are body axes in world space: rotate each row
    let m = orientation;
    Mat3::from_cols(rot * m.row(0), rot * m.row(1), rot * m.row(2)).transpose()
}

/// `Lod_shouldUpdate` 0x414B10 as `Car_ground` 0x49BD40 calls it (0x49BD6B…0x49BD8C,
/// `RE/notes/suspension-collision.md` §4.1): `0x414B10(1, 3, car+0x78, car+0, &rateHz, &nTicks,
/// &periodSec, car+0x10B8)`. Row 3 of `0x5CC770` is `6 ×6, 5, 5, 4 ×12`, indexed by
/// `min(_ftol(dist) / 16, 19)` (0 when `force`): index 6 — **32 Hz**, `nTicks` 1, mask 0 — under
/// 96 m, index 5 — 16 Hz, `nTicks` 2, mask 1 — to 128 m, index 4 — 8 Hz, `nTicks` 4, mask 3 —
/// beyond (`0x5CC700` / `0x5CC71C` / `0x5CC754`). Due when `((car+0) + gGameTick/2) & mask == mask`,
/// `car+0` being `slot | 0x100` (`RigidBody_init` 0x49B510); in the first 15 game ticks it always
/// passes with (32, 1). `distance` is `car+0x78` (0x417190: the octagonal x–z distance to the
/// nearest racer, 0 for a racer), `force` `car+0x10B8` (a racer). Answers `(rateHz, nTicks)`: the
/// rate `dh` is scaled by and the 32 Hz ticks the gravity covers. Every racer takes (32, 1); only
/// the traffic, the cops and the smackables are thinned.
pub fn ground_due(distance: f32, force: bool, tick: u64, slot: usize) -> Option<(f32, i32)> {
    if tick <= 15 {
        return Some((32.0, 1));
    }
    let lod = if force { 0 } else { (distance as i32 / 16).min(19) };
    let (mask, rate, n_ticks) = match lod {
        0..=5 => (0u64, 32.0, 1),
        6 | 7 => (1, 16.0, 2),
        _ => (3, 8.0, 4),
    };
    ((((slot | 0x100) as u64 + tick / 2) & mask) == mask).then_some((rate, n_ticks))
}

/// Car–car contact `0x498FF0` → `0x4980D0` → the separation `0x497B10` ([`separate`], its box
/// test `0x496FD0` [`box_contact`] — the 28 point tests, exact) → impulse `0x496860`
/// ([`pair_impulse`], `RE/notes/traffic.md` §21.6, CONFIRMED: linear and angular, the inertia
/// term `1/(6m)`). *(This doc used to call the box test a 2-D one and the angular response a
/// heuristic — stale since 2026-09-22 / 2026-09-25; corrected 2026-09-26.)* Returns the effect
/// magnitude when a contact was resolved.
/// `DraftFactor` 0x49D9B0 — CONFIRMED. Above 15 m/s, a racer within `v/3` metres and inside ~7.7°
/// of this car's heading (`cos > 0.991`) cuts the drag: the deepest of
/// `(1 − 3·dist/v)·(cos − 0.991)·111.1·0.5` wins and the factor is `1 − that`, so the drag can fall
/// by at most half. Only the racer array `0x5E8B34` is scanned — traffic and police do not draft.
/// **0x49ABC0** (car) — read 2026-09-25, CONFIRMED: the body points `car+0x8C` (count `car+0x32C`)
/// from the model's `BndP` array (`carObj+0xFC`, count `+0xF8`, loader 0x442080), made ready for
/// the physics. A smackable (`car+0x52C & 0x80`) returns at once (0x49ABC7) — this is for cars.
///
/// 1. **Bubble sort ascending by `y`** (0x49AC3B…0x49ACBC): `n` passes over the `n − 1` pairs,
///    swapping when the second is strictly lower (`fcomp; test ah,1`) — stable for equal heights.
/// 2. **Quadrant permutation of the first four** (0x49ACC2…0x49AE21): into four slots zeroed first,
///    each of points 0…3 goes to the first slot whose test it passes, against `[0x5B23F8]` = 0:
///    slot 0 `x < 0, z > 0`; slot 1 `x > 0, z > 0`; slot 2 `x < 0, z < 0`; slot 3 `x > 0, z < 0` —
///    front-left, front-right, rear-left, rear-right in the physics frame (x right, z forward),
///    the wheel order of `Susp_update` 0x499A70. A later point overwrites an earlier one in the
///    same slot; a slot no point reaches (a point on an axis) stays the zero vector. The twelve
///    dwords are copied back over points 0…3 (`rep movsd` 0x49AE21).
///
/// The points are the loader's **as stored**: 0x442080 copies `BndP`'s `x, y, z` with no change of
/// axes (only ×0.4 for `.TINYCAR` and `y −= car+0x3AC`), and 0x49AE30 puts them in the world as
/// `pos + Σ p_k·M[k]` with the physics rows `car+0x364` — so the model's own frame (its front at
/// −z) is used unturned as the physics body frame. The four probes a car's wheels carry are the
/// model's rear wheel contacts in front and its front ones behind.
pub fn body_points_setup(points: &mut [Vec3]) {
    let n = points.len();
    for _ in 0..n {
        for i in 0..n.saturating_sub(1) {
            if points[i + 1].y < points[i].y {
                points.swap(i, i + 1);
            }
        }
    }
    if n < 4 {
        return;
    }
    let mut slots = [Vec3::ZERO; 4];
    for p in points.iter().take(4) {
        let k = if p.x < 0.0 && p.z > 0.0 {
            0
        } else if p.x > 0.0 && p.z > 0.0 {
            1
        } else if p.x < 0.0 && p.z < 0.0 {
            2
        } else if p.x > 0.0 && p.z < 0.0 {
            3
        } else {
            continue;
        };
        slots[k] = *p;
    }
    points[..4].copy_from_slice(&slots);
}

pub fn draft_factor(position: Vec3, forward: Vec3, forward_speed: f32, others: &[Vec3]) -> f32 {
    if forward_speed <= 15.0 {
        return 1.0;
    }
    let mut best = 0.0f32;
    for o in others {
        let d = *o - position;
        let dist = d.length();
        if forward_speed * (1.0 / 3.0) <= dist || dist <= 0.1 {
            continue;
        }
        let cos = (d / dist).dot(forward);
        if cos <= 0.991 {
            continue;
        }
        let v = (1.0 - 3.0 * dist / forward_speed) * (cos - 0.991) * 111.1 * 0.5;
        best = best.max(v);
    }
    1.0 - best
}

#[cfg(test)]
mod car_car_tests;

#[cfg(test)]
mod damage_tests;

#[cfg(test)]
mod body_ground_tests;

#[cfg(test)]
mod roughness_tests;

#[cfg(test)]
mod clamp_ladder_tests;

#[cfg(test)]
mod steer_taper_tests;

#[cfg(test)]
mod car_ground_tests;
