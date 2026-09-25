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
//!   the `.edg` segments, restitution 0.1, no friction, heuristic yaw (sign table MEDIUM).
//! * Body–ground when tilted (`BodyGroundCollide` 0x496610 + `GroundImpulse` 0x495F30, §6.3, HIGH) and
//!   car–car (`0x496860`, `traffic.md` §21.6, CONFIRMED) are the original's impulses, not approximations.
//! * Drafting (`DraftFactor` 0x49D9B0, CONFIRMED): the caller fills `draft` from the other racers
//!   and it scales the drag.
//! * Induction boost (`0x49E6C4`) and misfire (§4.3) are modelled; the caller fills `misfire_roll`
//!   from the shared LCG.
//! * The per-part condition terms of the damage getters (`physics.md` §10) are `specs.wear`: the
//!   career fills a scale and an offset per group from the parts on the car.

use crate::collision::{PatchTracker, TriangleGrid};
use crate::specs::{CarSpecs, CIRCLE_SOFTNESS, GRAVITY, GRIP_DRY, GRIP_WET, SURFACE_BOUNCE};
use glam::{Mat3, Vec3};
use std::f32::consts::TAU;

/// A front wheel's angle in 1/256 turn from its steering value: `max_steer · steer / 17`.
/// `set_wheel_steer` scales the lock to 17 (`y · 17 / max_steer`), so this undoes it. One function
/// for the tyre and the renderer.
pub fn steer_angle_256(max_steer: f32, steer: f32) -> f32 {
    max_steer * steer / 17.0
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
    /// clutch arm, 2 the rev arm, 3 `diff == 0`, 4 the misfire arm, 5 over target.
    pub engine_arm: u8,
    /// `DraftFactor` 0x49D9B0's result, which scales the drag. 1.0 = clean air; the caller sets it
    /// from the other racers each tick with [`draft_factor`].
    pub draft: f32,
    /// `car+0x54C` — the induction boost state 0..1 (0x49E6C4), which the turbo sound reads.
    pub boost: f32,
    /// The `[0, 1)` draw the misfire test compares the engine damage against (§4.3). The original
    /// takes it from the shared LCG every engine step; the caller fills it from the same stream.
    pub misfire_roll: f32,
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
    pub race_state: u8,
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
    pub ignition_cut: u32,
    pub throttle_raw: u8,
    pub brake_raw: u8,
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
    /// The body's own ground tracker `car+8` and the plane `car+0x3E8` / `car+0x400` 0x499520
    /// keeps from it ([`PatchTracker`]): 0x49AF70 runs it at the head of every physics tick
    /// ([`Self::step32`]); `Car_ground`'s tilted arm, the 8 m ceiling rule and `Susp_update`'s
    /// no-patch fallback read it.
    pub tracker: PatchTracker,
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
            engine_arm: 0,
            draft: 1.0,
            boost: 0.0,
            misfire_roll: 1.0,
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
            race_state: 0,
            rally: false,
            tcs: false,
            tiny: false,
            ignition_cut: 0,
            throttle_raw: 0,
            brake_raw: 0,
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
            last_wall_class: 5,
            crash_sound: false,
            airborne_count: 0,
            landing_sound: None,
            rollover_sound: false,
            travel_dir: orientation.row(2),
            wheels: [Wheel::default(); 4],
            accel_local: Vec3::ZERO,
            accel_world: Vec3::ZERO,
            lateral_filtered: 0.0,
            long_filtered: 0.0,
            gravity_scale: 1.16,
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
            tracker: PatchTracker::new(Vec3::new(position.x, position.y - wheel_radius, position.z)),
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

    /// The original's 16-bit LCG, drawn inline at every call site: `prod = state · 123456789`,
    /// `state = prod & 0xFFFF`, `rand16 = (prod >> 8) & 0xFFFF` (`traffic.md` §14.7). The original
    /// keeps **one** stream for the whole game; this crate sits below `nfs-game`, so it carries its
    /// own state with the same arithmetic.
    /// `ClampAngVel`'s limit in turns/s (0x4964D0, 0x49BBA0): 0.75; 2.25 for a `.TINYCAR`
    /// (`carinfo+0x518`, tested first, 0x4964E0); **6.75** for a smackable (`car+0x52C & 0x80`)
    /// whose collision mass `car+0x354` is under 0.75 (0x4964F4…0x496512, `[0x5B49BC]`).
    pub(crate) fn ang_limit(&self) -> f32 {
        if self.tiny {
            2.25
        } else if self.smackable && self.collision_mass < 0.75 {
            6.75
        } else {
            0.75
        }
    }

    fn rand16(&mut self) -> u32 {
        let prod = self.rng.wrapping_mul(123_456_789);
        self.rng = prod & 0xFFFF;
        (prod >> 8) & 0xFFFF
    }

    /// `(rand16 · n) >> 16` — the original's "random below n".
    fn rand_below(&mut self, n: u32) -> u32 {
        (self.rand16() * n) >> 16
    }

    fn rand01(&mut self) -> f32 {
        self.rand16() as f32 * 1.525_878_9e-5
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
        a.max(b) + 0.25 * a.min(b)
    }

    /// 0x40F7DB…0x40F865, the reset request: the command `car+0xD8B == 5` (the AI's, or a human's
    /// button byte `>> 3`, 0x499344) goes through to `0x40F540(car, 2, 0)` only while the car is
    /// racing (`car+0xB50 == 0`), the game tick is past 0x300, no hold is running and
    /// `car+0x35C` is under 27.6 (`[0x5B2734]`); then the hold is 0xA0. The hold counts down on
    /// every pass. (`car+0x540` / `+0x544`, also required to be 0, never leave 0 in a race —
    /// `physics.md` §4.1c; `car+0xD89`, the other request, is only ever cleared.)
    pub fn reset_request(&mut self, requested: bool, game_tick: u64) -> bool {
        let go = requested
            && self.race_state == 0
            && game_tick > 0x300
            && self.reset_hold == 0
            && self.horizontal_speed() < 27.6;
        if go {
            self.reset_hold = 0xA0;
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
        // undo the automatic's reverse swap at the end of the controls step to get the bytes back
        let (d7f, d80) = if self.automatic && !self.ai_driven && self.gear == 0 { (self.brake, self.throttle) } else { (self.throttle, self.brake) };
        if self.reverse_latch { d7f > 0.0 } else { d80 > 0.0 }
    }

    pub fn v_long(&self) -> f32 {
        self.v_local.z
    }

    /// `Car_processInput` 0x49DD20 (human car; CONFIRMED). Runs at 32 Hz before the physics step.
    fn process_input(&mut self, c: &Controls, specs: &CarSpecs) {
        self.throttle_raw = c.throttle;
        self.brake_raw = c.brake;
        self.steer_target = c.steer.clamp(-126, 126);
        self.handbrake = c.handbrake;
        // steering ramp: at most 12 per physics tick
        let d = self.steer_target - self.steer;
        let ramp = if self.rally || self.tiny { 18 } else { 12 };
        self.steer += d.clamp(-ramp, ramp);
        self.steer = self.steer.clamp(-127, 127);
        // pedals 0..1 (divisor 252)
        let mut thr = (self.throttle_raw as f32 / 252.0).min(1.0);
        let mut brk = (self.brake_raw as f32 / 252.0).min(1.0);
        if self.ticks_alive < 0x40 || !self.race_started {
            thr = 0.0;
            brk = 1.0;
        }
        // 0x49DD29…0x49DD78: the alive count steps on; a dead car — or a human one that has
        // finished (`car+0xB50 >= 2`) — gets throttle 0, the brake full, the downshift flag, the
        // shift timer and the reverse latch cleared, and no gearbox
        if self.engine_alive != 0 {
            self.engine_alive += 1;
        }
        if self.engine_alive == 0 || (!self.ai_driven && self.race_state >= 2) {
            self.downshift_blip = false;
            self.shift_timer = 0;
            self.reverse_latch = false;
            self.throttle = 0.0;
            self.brake = 1.0;
            return;
        }
        // gear selection. An AI car leaves neutral or reverse in its own pedal routine
        // (0x40C051…0x40C0AB): past tick 0x1C0 it is put straight into 1st — `car+0xD82` and
        // `+0xD7E` both 2 — with a shift timer of `4 + (rand·8 >> 16)`, one draw; a human car's
        // automatic is 0x49DD20's (below, after the request)
        if self.ai_driven && self.gear <= 1 && self.game_tick > 0x1C0 {
            self.shift_timer = 4 + self.rand_below(8);
            self.gear = 2;
            self.requested_gear = 2;
        }
        // `Car_applyInput`'s tail (0x499435…0x499475): 0x413890 turns the packet's code into a
        // gear — code 2 (`& 0xF8 == 0x10`) one up while below `specs[0x50] − 1`, code 3 (`== 0x18`)
        // one down while above 0 — counted from the **current** gear, and a downshift is refused
        // while the rpm `car+0xDB4` is past the redline `specs[0x94]`
        if !self.ai_driven {
            let g = self.gear;
            let req = match c.command {
                2 if g + 1 < specs.gear_slots => g + 1,
                3 if g > 0 => g - 1,
                _ => g,
            };
            if !(self.rpm > specs.redline && req < g) {
                self.requested_gear = req;
            }
        }
        // 0x49DD20 for a manual human car (0x49DEDC / 0x49DEE1), after the start tick (0x49DDAE):
        // the request is taken as it stands — the previous gear kept, the downshift flag set when
        // going down into a forward gear, the shift timer loaded with `specs[0x54]`
        if !self.ai_driven && self.game_tick > 0x1C0 {
            if self.automatic {
                self.automatic_request();
            }
            let (g, req) = (self.gear, self.requested_gear);
            if req != g {
                if !self.automatic {
                    self.downshift_blip = g > req && req > 1;
                    self.prev_gear = g;
                    self.gear = req;
                    self.shift_timer = specs.shift_ticks;
                } else if req < 2 {
                    // 0x49DEFA: reverse or neutral is taken as it stands
                    self.downshift_blip = false;
                    self.prev_gear = g;
                    self.gear = req;
                    self.shift_timer = specs.shift_ticks;
                } else if req == 2 && g < 2 {
                    // 0x49DF24…0x49DF7B: out of reverse / neutral, the highest gear whose
                    // `redline · specs[0x170 + 4g]` is below the forward speed `car+0xD60`, else 1st
                    let mut pick = 2;
                    for k in 2..specs.gear_slots {
                        if specs.redline * specs.mps_per_rpm[k] < self.v_local.z {
                            pick = k;
                        }
                    }
                    self.downshift_blip = false;
                    self.prev_gear = g;
                    self.gear = pick;
                    self.shift_timer = specs.shift_ticks;
                }
            }
        }
        // in reverse an automatic swaps the pedals: the throttle drives backwards (0x49DD20's tail)
        if self.automatic && !self.ai_driven && self.gear == 0 {
            std::mem::swap(&mut thr, &mut brk);
        }
        self.throttle = thr;
        self.brake = brk;
    }

    /// 0x49DDDE…0x49DEC2 — an automatic human car's own requests: first gear until tick 0x1C4,
    /// then, while the request stands and the car is racing (`car+0xB50 == 0`), reverse when it is
    /// stopped (`car+0x35C < 0.2`) with the brake past 0x80 and no throttle in a forward gear, and
    /// first again when it is stopped in reverse with the throttle past 0x80 — each once, through
    /// the latch `car+0xD98`, which a request that differs from the gear clears.
    /// `car+0x540` / `+0x544` are not carried.
    fn automatic_request(&mut self) {
        if self.game_tick < 0x1C4 {
            self.requested_gear = 2;
            return;
        }
        if self.requested_gear != self.gear || self.race_state != 0 {
            self.reverse_latch = false;
            return;
        }
        let (a, b) = (self.v_local.x.abs(), self.v_local.z.abs());
        let stopped = a.max(b) + 0.25 * a.min(b) < 0.2;
        if stopped && self.brake_raw > 0x80 && self.throttle_raw == 0 && self.gear > 1 && !self.reverse_latch {
            self.requested_gear = 0;
            self.reverse_latch = true;
        } else if stopped && self.throttle_raw > 0x80 && self.gear == 0 && self.reverse_latch {
            self.requested_gear = 2;
            self.reverse_latch = false;
        }
    }

    /// `AutoShift` 0x49DAF0 (CONFIRMED).
    fn auto_shift(&mut self, specs: &CarSpecs) {
        let r = self.rand01();
        let up = specs.redline * (0.9 + 0.1 * r);
        let down = specs.redline * 0.8;
        let mut top = specs.gear_slots - 1;
        if specs.redline * specs.mps_per_rpm[specs.gear_slots - 2] > specs.top_speed {
            top -= 1;
        }
        let vz = self.v_local.z.abs();
        let g = self.gear;
        // **Not `min(1.2·x, rpm)`.** 0x49DBFD forms `x = |vL.z| · ratio[gear]` and stores it,
        // 0x49DC05 multiplies a *copy* by `[0x5B2480]` = 1.2 and compares that with the rpm, and
        // 0x49DC16's `jne` keeps the stored **`x`** when `1.2·x <= rpm` — only the other way round
        // does 0x49DC18 replace it with the rpm. So when the headroom test passes the answer is
        // `x`, not `1.2·x`. Corrected 2026-09-21.
        let x = vz * specs.ratio_rpm_per_mps[g];
        let rpm_now = if x * 1.2 > self.rpm { self.rpm } else { x };
        let ratio_prev = if g >= 1 { specs.ratio_rpm_per_mps[g - 1] } else { 0.0 };
        let mut new_gear = g;
        if g >= 2 && self.throttle >= 0.5 && rpm_now.abs() < specs.redline && vz * ratio_prev < down && g - 1 > 1 && self.slip_tangent.abs() < 0.1 {
            new_gear = g - 1; // kick-down
        } else if g >= 2 && rpm_now >= up && g < top {
            new_gear = g + 1;
        } else if g > 2 && vz * ratio_prev < down {
            new_gear = g - 1;
        }
        if new_gear != g {
            // `car+0xD83`: set on the two downshifts (0x49DC92, 0x49DCEE), cleared on the upshift
            // (0x49DCCC)
            self.downshift_blip = new_gear < g;
            self.prev_gear = g;
            self.gear = new_gear;
            self.shift_timer = specs.shift_ticks;
        }
    }

    /// `Engine_update` 0x49E440 → drive − drag (HIGH; clutch-slip branch simplified).
    /// `0x49E6C4` — the induction boost state `car+0x54C` (CONFIRMED). A turbo (`induction == 1`)
    /// spools toward `(rpm − redline·0.25)/(redline·0.5)` at ±0.05 a tick; a supercharger
    /// (`induction == 2`) toward `(rpm − redline/6)/(redline·⅔)`, rising instantly and falling by
    /// 0.1. Both are capped by `rpm/redline·0.5` — scaled by the throttle for the turbo — and the
    /// whole update is skipped while the gearbox is shifting. Nothing in the drive path reads it;
    /// its consumers are the turbo sound (0x4AA72E) and the bleed-off at 0x49F075.
    fn boost_update(&mut self, specs: &CarSpecs) {
        let (lo, span) = match specs.induction {
            1 => (specs.redline * 0.25, specs.redline * 0.5),
            2 => (specs.redline / 6.0, specs.redline * (2.0 / 3.0)),
            _ => return,
        };
        if specs.redline <= 0.0 {
            return;
        }
        let x = self.rpm - lo;
        let target = if x <= 0.0 { 0.0 } else { (x / span).min(1.0) };
        let mut cap = self.rpm / specs.redline * 0.5;
        if specs.induction == 1 {
            cap *= self.throttle;
        }
        let target = (target * self.throttle).min(cap);
        if self.shift_timer > 0 {
            return;
        }
        self.boost = if specs.induction == 2 {
            if target >= self.boost { target } else { target.max(self.boost - 0.1) }
        } else if target >= self.boost {
            target.min(self.boost + 0.05)
        } else {
            target.max(self.boost - 0.05)
        };
    }

    fn engine_update(&mut self, specs: &CarSpecs, rear_contact: bool) -> f32 {
        self.boost_update(specs);
        let vz = self.v_local.z;
        let a_drag = specs.drag * vz * vz * vz * self.draft;
        let gear = self.gear;
        let neutral = gear == 1;
        let rpm_target = if neutral || !rear_contact {
            ((specs.redline + 250.0) * self.throttle).min(specs.redline + 250.0)
        } else {
            ((specs.redline + 100.0) * self.throttle).min(specs.redline + 100.0)
        };
        // The rev limiter of `physics.md` §4.1: crossing the redline pins the rpm just above it and
        // arms an ignition cut of a few ticks, whose length is a small random draw — longer at a
        // standing start, where the original gives a human car 10–29 ticks so the launch stutters.
        if self.rpm > specs.redline && self.rpm < specs.redline + 250.0 {
            self.rpm = specs.redline + 50.0;
            self.ignition_cut = if gear > 1 && rear_contact {
                self.rand_below(2) + 3 * gear as u32
            } else if self.ticks_alive < 0x1A0 && !self.ai_driven {
                self.rand_below(20) + 10
            } else {
                self.rand_below(2) + 2
            };
        }
        let mut throttle = self.throttle;
        let mut rpm_target = rpm_target;
        if self.ignition_cut > 0 {
            throttle = 0.0;
            rpm_target = 0.0;
            self.ignition_cut -= 1;
        }
        // 0x49E843 / 0x49E849: both axles' clutch-slip states are cleared at the top of every call,
        // **before** the neutral and shift early-outs — so the flag is per-tick, and a car sitting
        // in neutral or mid-shift is never "spinning". This port used to set it only inside the
        // block below, so on those paths it kept the previous tick's value.
        self.rear_spin = false;
        // `misfire = engineDamage/300 > rand01` (§4.3), drawn at the top of the routine (0x49E4BD)
        let misfire = self.engine_wear(specs) / 300.0 > self.misfire_roll;
        // **The no-drive path, 0x49EF36…0x49F075**, entered in neutral, mid-shift or with the rear
        // off the ground (0x49E84F / 0x49E85D / 0x49E869):
        //
        // * a misfire takes 100 off (0x49EF40);
        // * below the target and not shifting, +250, up to the target (0x49EF66…0x49EF87);
        // * shifting, unless it is out of neutral (`car+0xD81 == 1`, 0x49EF94): with the downshift
        //   flag the blip `[0x5D0C70 + 4·req]` (250, 200, 175, 150, 125) — `[0x5D0C90]` (200 … 100)
        //   with the brake past 0x40 — up to the redline (0x49EFA1…0x49EFFD), without it 100 off, or
        //   200 in fourth and up (0x49EFFF…0x49F018);
        // * otherwise, at or above the target, 200 off, down to the target (0x49F03B…0x49F06F);
        //
        // never below 0, and the boost bleeds 0.1 on the way out (0x49F075).
        if neutral || self.shift_timer > 0 || !rear_contact {
            let shifting = self.shift_timer > 0;
            if shifting {
                self.shift_timer -= 1;
            }
            let mut rpm = self.rpm;
            if misfire {
                rpm -= 100.0;
            } else if rpm < rpm_target && !shifting {
                rpm = (rpm + 250.0).min(rpm_target);
            } else if shifting && self.prev_gear != 1 {
                if self.downshift_blip {
                    const BLIP: [f32; 8] = [0.0, 0.0, 250.0, 200.0, 175.0, 150.0, 125.0, 0.0];
                    const BLIP_BRAKING: [f32; 8] = [0.0, 0.0, 200.0, 175.0, 150.0, 125.0, 100.0, 0.0];
                    let t = if self.brake_raw > 0x40 { &BLIP_BRAKING } else { &BLIP };
                    rpm = (rpm + t[self.requested_gear.min(7)]).min(specs.redline);
                } else {
                    rpm -= if gear >= 4 { 200.0 } else { 100.0 };
                }
            } else if rpm >= rpm_target {
                rpm = (rpm - 200.0).max(rpm_target);
            }
            self.rpm = rpm.max(0.0);
            self.boost = (self.boost - 0.1).max(0.0);
            return -a_drag;
        }
        if self.automatic {
            self.auto_shift(specs);
        }
        let gear = self.gear;
        // 0x49E898…0x49E8C4: while a shift runs without the downshift flag the wheel rpm is still
        // the **previous** gear's
        let ratio_gear = if self.shift_timer > 0 && !self.downshift_blip { self.prev_gear } else { gear };
        let rpm_wheel = specs.ratio_rpm_per_mps[ratio_gear] * vz;
        let rpm_c = self.rpm.clamp(0.0, specs.redline);
        let t = specs.torque_at(rpm_c);
        let mut a_drive = t * specs.accel_per_torque[gear] * (1.0 - self.engine_wear(specs) / 300.0) * (1.0 - 0.005 * self.part_wear(&specs.zone_masks.gearbox, &specs.wear.gearbox));
        let mut diff = rpm_target - rpm_wheel.abs();
        if diff.abs() < 125.0 && rpm_target >= specs.redline - 300.0 {
            diff = 0.0;
        }
        let slip = self.rpm - rpm_wheel.abs();
        // (`misfire`, above: a damaged engine stumbles, which stops the clutch model pulling the revs
        // down and kills the drive when it is over its target. The draw comes from the shared LCG,
        // so the caller fills `misfire_roll` each tick.)
        // **The clutch-slip block of `Engine_update` 0x49E440, read to the instruction on
        // 2026-09-21** (`physics.md` §4.1a). The note this used to follow had three comparisons the
        // wrong way round, and the port had simplified the rest; together they left the rear axle
        // "spinning" on about 80 % of full-throttle ticks at every speed up to 197 km/h in seventh,
        // which is the constant wheelspin squeal that was reported in play.
        //
        // Entered on any of (0x49EA1B…0x49EA8B):
        //
        // * `slip > 250` (`[0x5B4A60]`), not shifting and not in neutral — the neutral and shift
        //   early-outs above have already returned, so only the slip test remains here;
        // * a forward gear while rolling **backwards**, `v < −0.1` (0x49EA52 against `[0x5B2444]`,
        //   `test ah,1 ; jne` — taken when st0 < arg);
        // * reverse while rolling **forwards**, `v > 0.1` (0x49EA69, `test ah,0x41 ; je`).
        //
        // The note had the last two as `v ≥ −0.1` / `v ≤ 0.1` — "moving with the gear", which is
        // nearly always. A fourth disjunct, `DB8 == 1 && ignitionCut == 0` (0x49EA76), is dead:
        // 0x49E849 cleared `DB8` at the top of this very call and nothing in the image writes 1.
        let clutch = slip > 250.0 || (gear > 1 && vz < -0.1) || (gear == 0 && vz > 0.1);
        self.engine_arm = if clutch { 1 } else if diff > 0.0 { 2 } else if diff == 0.0 { 3 } else if misfire { 4 } else { 5 };
        if clutch {
            // `allow`, 0x49EA91…0x49EB44: the most the revs may fall this tick. The loads are the
            // wheels' grip limits `+0xAC` (the wheel block is at `car+0x7B8`, so wheel 0's is
            // `+0x864`) — all four for a car with any front drive (`specs[0x5C] > 0`, against 120),
            // the rear two otherwise (against 100) — and the raw throttle byte `[0xD7C]` divides,
            // floored at 0.3. The port had `100·(1 − throttle) + 100`.
            let drive = (15.0 * a_drive).abs();
            let base = if specs.front_drive > 0.0 {
                120.0 - (drive - (self.wheels[0].limit + self.wheels[1].limit + self.wheels[2].limit + self.wheels[3].limit))
            } else {
                100.0 - (drive - (self.wheels[2].limit + self.wheels[3].limit))
            };
            let allow = base / (self.throttle_raw as f32 / 254.0).max(0.3) + 100.0 * (1.0 - throttle);
            // 0x49EB2C…0x49EB61: a quick gearbox (`specs[0x54] ≤ 4` ticks) above first gear bleeds
            // a sixth of the slip off first, whichever branch follows
            if specs.shift_ticks <= 4 && gear > 2 {
                self.rpm -= slip / 6.0;
            }
            // 0x49EB67: **on the throttle** (≥ 0.5) and not misfiring, the clutch pulls the revs
            // down hard — by `min(slip, allow)` — and the axle is marked spinning only when the
            // slip is past an eighth of the redline (`specs[0x94] · [0x5B272C]`, 0x49E473),
            // not past 250. **Off** the throttle the revs only drift down by a sixth (0x49EBE3).
            // The port had these two branches swapped, so at full throttle the revs never came
            // down to the wheels.
            if throttle >= 0.5 && !misfire {
                if slip > specs.redline * 0.125 {
                    self.rear_spin = true;
                }
                self.rpm -= slip.min(allow);
            } else {
                self.rpm -= slip / 6.0;
            }
        } else if diff > 0.0 {
            // The rev arm, 0x49EC47…0x49EC8B (and its boosted twin 0x49ECD8…0x49ED45): the revs
            // gain `min(−slip, specs[0x170 + 4·gear] · 81920)` — `[0x5B4A50]` — and that is the
            // **whole** delta. `[esp+0xC]` is the slip: 0x49EA0D loads `car+0xDB4` and subtracts
            // the signed wheel rpm. In reverse (`car+0xD82 == 0`) the `test al, al` at 0x49EC4C
            // turns the `min` into a `max`, the ratio there being negative.
            //
            // **`specs[0x98]` is not a rev rate** (`RE/PHYSICS.md` +0x98, read 2026-09-23): its
            // only read in the whole of `Engine_update` (0x49E440…0x49F400, 1045 instructions) is
            // 0x49ECA0, inside the induction arm gated on `car+0x548`. The port added
            // `+ rev_rate · diff` here, which is in no instruction; removed 2026-09-23.
            let cap = 81920.0 * specs.mps_per_rpm[gear];
            let step = if gear == 0 { (-slip).max(cap) } else { (-slip).min(cap) };
            self.rpm += step;
            // the floor the arm ends on (0x49ED24…0x49ED3F on the boosted side, and the port has
            // carried it on both): the revs never sit below the wheels'
            if rpm_wheel.abs() > self.rpm {
                self.rpm = rpm_wheel.abs();
            }
        } else if diff == 0.0 {
            self.rpm = rpm_wheel.abs();
        } else if misfire {
            // over target with a misfire: the revs drop and the engine makes nothing
            self.rpm -= 100.0;
            a_drive = 0.0;
        } else {
            // **Over target, 0x49ED94…0x49EDE2 — a ladder of three, not a fall and a snap.**
            // `slip > [0x5B2484]` (200) takes 200 off; `slip < [0x5B4A4C]` (−200) **adds** 200,
            // stepping the revs *up* toward wheels that are turning faster than the engine; only
            // between the two does the engine take the wheel rpm outright (0x49EDDE). This port
            // had the first rung and the snap but not the middle one, so lifting off at speed
            // pulled the revs to the wheels in one jump where the original walks them up.
            if slip > 200.0 {
                self.rpm -= 200.0;
            } else if slip < -200.0 {
                self.rpm += 200.0;
            } else {
                self.rpm = rpm_wheel.abs();
            }
            a_drive *= throttle;
            self.rpm = self.rpm.min(rpm_target.max(rpm_wheel.abs()));
        }
        if self.rpm < 0.0 {
            self.rpm = 0.0;
            return -vz * 32.0 - a_drag;
        }
        self.rpm = self.rpm.min(1.25 * specs.redline);
        // Drive is proportional to throttle (the original scales `a_drive` by the throttle in the
        // over-target branch and by the clutch model otherwise; a linear throttle map is used for the
        // launch/rev-up branches — MEDIUM).
        // 0x49EE0B: RALLY scales the drive by 1.25 · min(1 + |slip tangent|, 2) — in the original
        // on the throttle-scaled branch, which this port folds into its final line
        let rally_k = if self.rally { 1.25 * (1.0 + self.slip_tangent.abs()).min(2.0) } else { 1.0 };
        a_drive * throttle * rally_k - a_drag
    }

    /// `LoadTransfer` 0x4A1900 (CONFIRMED).
    fn load_transfer(&mut self, specs: &CarSpecs) {
        let g_eff = self.frame.row(1).y * specs.grip * GRAVITY;
        for (i, w) in self.wheels.iter_mut().enumerate() {
            let tbl = if w.wet { &GRIP_WET } else { &GRIP_DRY };
            // AI cars (`0x52C & 8` / `0xDA8`) read a remapped μ column (0x4A196F..0x4A19BA, HIGH):
            // dry class 1 → column 0, wet class 0 → column 1
            let col = match (self.traction_control, w.wet, specs.class) {
                (true, false, 1) => 0,
                (true, true, 0) => 1,
                (_, _, c) => c,
            };
            let mu = tbl[w.surface & 15][col];
            let share = if i < 2 { specs.front_weight } else { 1.0 - specs.front_weight };
            w.limit = mu * share * g_eff * 0.5;
        }
        let mut dz = self.accel_local.z * specs.weight_transfer * (-1.0 / GRAVITY) * g_eff * (-0.2) / specs.transfer_div;
        if self.throttle > 0.0 && self.v_local.z >= 0.0 {
            dz *= (1.0 - 10.0 * self.slip_tangent.abs()).max(0.0);
        }
        self.wheels[0].limit -= dz;
        self.wheels[1].limit -= dz;
        self.wheels[2].limit += dz;
        self.wheels[3].limit += dz;
        if self.rally {
            self.wheels[0].limit *= 1.1;
            self.wheels[1].limit *= 1.1;
        }
        let dx = self.accel_local.x * specs.weight_transfer * (-1.0 / GRAVITY) * g_eff * 0.2 / specs.transfer_div;
        let dx2 = dx / specs.lateral_div;
        self.wheels[0].limit -= dx;
        self.wheels[2].limit -= dx;
        self.wheels[1].limit += dx2;
        self.wheels[3].limit += dx2;
        self.gravity_scale = 1.16;
        let vz = self.v_local.z;
        if vz > 5.0 {
            let aero = 1.0 - 0.005 * self.part_wear(&specs.zone_masks.aero, &specs.wear.aero);
            let df = specs.downforce_front * vz * aero;
            let dr = specs.downforce_rear * vz * aero;
            self.wheels[0].limit += df;
            self.wheels[1].limit += df;
            self.wheels[2].limit += dr;
            self.wheels[3].limit += dr;
            if self.cos_tilt < 0.939 {
                self.gravity_scale += 8.0 * dr;
            }
        }
        for w in &mut self.wheels {
            w.limit = w.limit.max(0.0);
        }
    }

    /// Part damage (`0x4938A0` family, HIGH): mean zone damage over the part's zone mask (the
    /// installed-part condition terms are not modelled: stock parts, `1.5 − 0.005·100 = 1`).
    /// A part group's damage as the original's getters report it: the mean zone damage under the
    /// group's mask scaled by `wear.scale`, plus the damage the parts carried in from earlier races
    /// (`wear.offset`) — `Σ (1.5 − 0.005·condition)·mean + state` (`physics.md` §10).
    pub fn part_wear(&self, mask: &[bool; 10], w: &crate::specs::PartWear) -> f32 {
        w.scale * self.part_damage(mask) + w.offset
    }

    pub fn part_damage(&self, mask: &[bool; 10]) -> f32 {
        let (mut sum, mut n) = (0.0, 0);
        for (d, m) in self.damage.iter().zip(mask.iter()) {
            if *m {
                sum += d;
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { sum / n as f32 }
    }

    /// `0x4938A0`, the engine figure: unlike the rest of the family it starts from **zone 1**
    /// (`fld [car+0x4E8]`, 0x4938AE) and counts from 1 (0x4938A9), so it is
    /// `(zone1 + Σ part terms) / (1 + n)` and always divides (0x493946). The misfire, the drive
    /// force and HTextDamage's "drivetrain" row read it. This port used the bare sum.
    pub fn engine_wear(&self, specs: &crate::specs::CarSpecs) -> f32 {
        (self.damage[1] + self.part_wear(&specs.zone_masks.engine, &specs.wear.engine)) / (1 + specs.wear.engine.count) as f32
    }

    /// `0x493BC0`, the suspension figure: the part terms of types 4, 5 and 8 summed, then divided by
    /// their count when it is above 1 (0x493C78…0x493C7E). The brakes, gearbox, aero and tyre getters
    /// (0x493A10, 0x493AA0, 0x493B30, 0x493980) have no outer division.
    pub fn suspension_wear(&self, specs: &crate::specs::CarSpecs) -> f32 {
        let w = self.part_wear(&specs.zone_masks.suspension, &specs.wear.suspension);
        let n = specs.wear.suspension.count;
        if n > 1 { w / n as f32 } else { w }
    }

    /// `0x493960`, the steering figure: `(zone2 + zone0) · 0.5` (`car+0x4EC`, `+0x4E4`, `[0x5B23F0]`).
    pub fn steering_damage(&self) -> f32 {
        (self.damage[2] + self.damage[0]) * 0.5
    }

    /// `0x493C90`, the body figure: zones 0…8 summed and scaled by `[0x5B49B0]` = 1/12 — nine zones
    /// over twelve, as the binary has it (0x493C94 `mov ecx, 9`).
    pub fn body_damage(&self) -> f32 {
        self.damage[..9].iter().sum::<f32>() * (1.0f32 / 12.0)
    }

    /// Average body damage in per cent (10 zones, `0x434FA0`).
    pub fn damage_avg(&self) -> f32 {
        self.damage.iter().sum::<f32>() / 10.0
    }

    /// `Damage_byNormal` 0x494DF0 (CONFIRMED): needs an effect > 10; the zone is chosen from the
    /// contact normal in the body frame (`x = n·right, y = n·up, z = n·forward`).
    ///
    /// The roof zone is gated: 0x494E69 takes it only when `car+0x52C & 0x80` is **clear**, and
    /// that bit marks a smackable ([`Self::smackable`]; it read "traffic" here until 2026-09-25 —
    /// the traffic's bit is 0x10, `ai.md` §6).
    ///
    /// `flag` is `Damage`'s fourth argument — 0 from the body-ground contact (0x49684A), 1 from the
    /// wall collider (0x495563), 2 from the car-car solver (0x496F8E) — and `mass` is the car's
    /// collision mass `car+0x354`; both only feed [`Self::crash_response`].
    pub fn damage_by_normal(&mut self, effect: f32, n_world: Vec3, flag: u8, mass: f32) {
        if effect <= 10.0 {
            return;
        }
        let m = self.orientation;
        let (x, y, z) = (n_world.dot(m.row(0)), n_world.dot(m.row(1)), n_world.dot(m.row(2)));
        let zone = if y < -0.5 && !self.smackable {
            8
        } else if x.abs() < 0.1 {
            if z < -0.1 { 1 } else if z > 0.1 { 5 } else { return }
        } else if z.abs() < 0.1 {
            if x > 0.1 { 7 } else { 3 }
        } else if z < -0.1 {
            if x > 0.1 { 0 } else { 2 }
        } else if x > 0.1 {
            6
        } else {
            4
        };
        self.damage_zone(effect, zone);
        self.crash_response(effect, zone, flag, mass);
    }

    /// `Damage` 0x494500's tail (0x49481A…0x494DD9), which runs whatever the damage switch says
    /// and only for a non-zero `flag`: a knock to the body's spin, and on a heavy impact a hop.
    ///
    /// `a` is the amount, doubled for a `.TINYCAR` (0x494863) — the ×0.5 of a knockout run over
    /// the network (`[0x6573E8] == 3 && [0x657408]`, 0x494826) has no race in this port. Past 80
    /// (`[0x5B26D4]`) the amount is taken ×0.125 (`[0x5B272C]`), the body is lifted 0.1 m when it
    /// sits within 0.1 of the ground (`car+0x424`, 0x4948FA), the velocity's `y` gains `a/3` —
    /// for a smackable of collision mass under 1, `max(mass, 0.1 + 0.05·rand) · a · 0.33`
    /// (0x494964…0x4949D7) — the knock is `min(a/32, 2)` with each axis ×1.2 (×3.0 tiny,
    /// 0x49494E), and a car-car hit (`flag == 2`) with the world spin under 0.5 turns/s on `x`
    /// and `z` draws a new one (0x494A0A…0x494C14): `x, z ∈ [−2.5, 2.5)`, `y ∈ [−1, 1)`, or
    /// `[−10, 10)` and `[−5, 5)` for a smackable or a tiny car. Otherwise the knock is `min(a/32, 0.33)`.
    ///
    /// The knock goes on in the body frame (0x494C45…0x494D16): pitch `−k/2` for the front zones
    /// 0…2 and `+k/2` for the rest, yaw and roll `+k/2` for zones 2…4 and `−k/2` for the rest.
    /// Unless the other body was a car (`car+0x434`, which only the car-car solver writes,
    /// 0x496CC9 — MEDIUM that nothing else leaves it set), the body then drops by the height of
    /// `(−sin 2πroll · hx, −cos 2πroll · hy, 0)` over 32 (0x494D3C…0x494D92). The wall's
    /// call ends in `ClampAngVel` 0x49BBA0 (0x494DD4); the car-car solver clamps on its own.
    ///
    /// It also loads the reset countdown `car+0x1100 = 0xC0` (0x4948E6) and, past 120 on a car
    /// that is not tiny, kills the car (`car+0xDB0 = 0`, 0x4948A6…0x4948C1), and sets
    /// [`Self::airborne_count`] to 1 (0x4948D5). Not carried, each without a counterpart in this
    /// port: `+0x1104 = 1`, `+0x450 = 0`
    /// (0x4948D5…0x4948F0) and `+0x42C = 1` on a car that is **not** a smackable (0x49488C…0x49489A,
    /// `ebx` = 1 from 0x494516 — read 2026-09-25; `traffic.md` §21.7 listed it as a clear). A
    /// smackable's `+0x42C` is the solver's ([`Self::knocked_over`]).
    fn crash_response(&mut self, amount: f32, zone: usize, flag: u8, mass: f32) {
        if flag == 0 {
            return;
        }
        let mut a = amount;
        if self.tiny {
            a *= 2.0;
        }
        let (k, m) = if a > 80.0 {
            if !self.tiny && a > 120.0 {
                if self.engine_alive != 0 {
                    self.engine_kill = 1;
                }
                self.engine_alive = 0;
            }
            a *= 0.125;
            if self.height_marker < 0.1 {
                self.position.y += 0.1;
            }
            let k = (a * 0.031_25).min(2.0);
            let m = if self.tiny { 3.0 } else { 1.2 };
            self.velocity.y += if self.smackable && mass < 1.0 {
                let t = self.rand16() as f32 * 0.05 * 1.525_878_9e-5 + 0.1;
                mass.max(t) * a * 0.33
            } else {
                a * 0.333_333_34
            };
            self.reset_countdown = 0xC0;
            // 0x4948D5: `word car+0x418 = 1` (`ebx`)
            self.airborne_count = 1;
            let w = self.angular_velocity;
            if flag == 2 && w.x.abs() < 0.5 && w.z.abs() < 0.5 {
                let big = self.smackable || self.tiny;
                let (h, v) = if big { (20.0, 10.0) } else { (5.0, 2.0) };
                let x = self.rand16() as f32 * h * 1.525_878_9e-5 - h * 0.5;
                let y = self.rand16() as f32 * v * 1.525_878_9e-5 - v * 0.5;
                let z = self.rand16() as f32 * h * 1.525_878_9e-5 - h * 0.5;
                self.angular_velocity = Vec3::new(x, y, z);
            }
            (k, m)
        } else {
            ((a * 0.031_25).min(0.33), 1.0)
        };
        let o = self.orientation;
        let w = self.angular_velocity;
        let (mut pitch, mut yaw, mut roll) = (w.dot(o.row(0)), w.dot(o.row(1)), w.dot(o.row(2)));
        let d = m * k * 0.5;
        pitch += if zone <= 2 { -d } else { d };
        if (2..=4).contains(&zone) {
            yaw += d;
            roll += d;
        } else {
            yaw -= d;
            roll -= d;
        }
        if flag != 2 {
            let (sr, cr) = (roll * std::f32::consts::TAU).sin_cos();
            let u = Vec3::new(-sr * self.half_extents.x, -cr * self.half_extents.y, 0.0);
            self.position.y += (u.x * o.row(0).y + u.y * o.row(1).y) * 0.031_25;
        }
        self.angular_velocity = o.transpose() * Vec3::new(pitch, yaw, roll);
        if flag != 2 {
            let lim = self.ang_limit();
            self.angular_velocity = self.angular_velocity.clamp(Vec3::splat(-lim), Vec3::splat(lim));
        }
    }

    /// `Damage` 0x494500 (CONFIRMED): `a = min(amount·0.5, 100)` (the ×1.5 branch belongs to the
    /// `[0x657400]` mode flag — off here, MEDIUM); the zone takes the maximum and each ring
    /// neighbour is raised to the mean of its two neighbours.
    pub fn damage_zone(&mut self, amount: f32, zone: usize) {
        if !self.damage_enabled || zone > 9 {
            return;
        }
        let a = (amount * 0.5).min(100.0);
        // crash sound for the corner / roof zones when the damage crosses 30
        if matches!(zone, 0 | 2 | 4 | 6 | 8) && a > 30.0 && self.damage[zone] < 30.0 {
            self.crash_sound = true;
        }
        self.damage[zone] = self.damage[zone].max(a);
        if zone <= 7 {
            for j in [(zone + 7) % 8, (zone + 1) % 8] {
                let other = if j == (zone + 1) % 8 { (j + 1) % 8 } else { (j + 7) % 8 };
                let v = 0.5 * (self.damage[zone] + self.damage[other]);
                self.damage[j] = self.damage[j].max(v);
            }
        }
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
    /// Not modelled: tyre damage (`LIM = LIM0`), the per-car `.ABS` / `.TRACTION` registry flags
    /// (both off) and the `RALLY` flag. `traction_control` stands for the AI-driven flag `car+0xDA8`.
    fn tyre(&mut self, i: usize, specs: &CarSpecs) {
        let front = i < 2;
        let ai = self.traction_control;
        let rally = self.rally;
        let tcs = self.tcs;
        let slip_tangent = self.slip_tangent;
        let rear_spin = self.rear_spin;
        let handbrake = self.handbrake;
        let vz = self.v_local.z;
        let throttle = self.throttle;
        let throttle_raw = self.throttle_raw;
        let brake_raw = self.brake_raw;
        let gear = self.gear;
        let class = specs.class;
        let k_class = CIRCLE_SOFTNESS[class];
        let tyre_damage = self.part_wear(&specs.zone_masks.tyres, &specs.wear.tyres);
        let w = &mut self.wheels[i];
        let ang = steer_angle_256(specs.max_steer, w.steer);
        let (mut u, mut v) = (w.slip_vel[0], w.slip_vel[2]);
        if ang != 0.0 {
            let a = ang / 256.0 * TAU;
            let (sn, cs) = a.sin_cos();
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
        let limit = w.limit * (1.0 - 0.005 * tyre_damage);
        // §2.3 over-limit decision: lock (Slide) or clamp, for braking-type demands / handbrake only
        if (opposing && d.abs() > limit) || (handbrake && !front && vz.abs() > 0.5) {
            let lock = if handbrake {
                true
            } else if 2.0 * (16.0 - limit) < vz || brake_raw < 0xEC || (vz.abs() < 5.0 && !rear_spin) {
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
                let comb = a.max(b) + 0.25 * a.min(b);
                let lim2 = limit * (1.0 - 1.0 / k_class);
                w.slip = if comb > limit { (comb - limit) / 4.0 } else { 0.0 };
                w.slip = (0.1 * w.slip).min(1.0);
                let s = if comb > 0.01 { lim2 / comb } else { 0.0 };
                // safety net (not original): a tick's impulse may not reverse the slip velocity
                w.lateral = (s * u).clamp(-2.0 * a, 2.0 * a);
                w.longitudinal = (s * v).clamp(-2.0 * b, 2.0 * b);
                w.locked = true;
                w.sliding = true;
                w.demand = d;
                return;
            }
            d = d.clamp(-limit, limit);
        }
        w.demand = d;
        // §2.4 lateral force from the slip angle (1/256 turn)
        let lat = if v == 0.0 {
            0.0
        } else {
            let a = u.atan2(-0.5 * v) / TAU; // turns
            256.0 * if v < 0.0 { a } else if u > 0.0 { 0.5 - a } else if u < 0.0 { -0.5 - a } else { a }
        };
        // 0x4A24E8 branches on `wheel+0xC0`, which is **not** a sliding flag: 0x410BB2 (car init)
        // and 0x49E345 (reset) set it to 1 for wheels 0 and 1 and 0 for wheels 2 and 3 and nothing
        // in the image ever writes it again, so it selects the lateral curve **by axle**. The front
        // pair follows `min(|lat|, 16)·load/12`, the rear `min(|lat|, 2)·load·0.5` with a floor of
        // one unit — a deliberate understeer bias of about six to one at small slip
        // (`physics.md` §5.2).
        if front {
            let f = lat.signum() * lat.abs().min(16.0) * limit / 12.0;
            w.lateral = if u.abs() + v.abs() < 32.0 { u.signum() * u.abs().min(f.abs()) } else { f };
        } else {
            let m = if lat.abs() <= 1.0 { 1.0 } else { lat.abs().min(2.0) };
            let f = lat.signum() * m * limit * 0.5;
            w.lateral = u.signum() * u.abs().min(f.abs());
        }
        w.longitudinal = d;
        // §3 friction circle
        let lon_w = w.longitudinal.abs() * if rally { 0.5 } else { 0.9 };
        let lat_a = w.lateral.abs();
        let comb = lat_a.max(lon_w) + 0.25 * lat_a.min(lon_w);
        let spin = (w.longitudinal.abs() > limit && throttle_raw > 0x80) || (!front && rear_spin);
        let k = if !front && spin { 4.0 } else { k_class };
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
            let (sn, cs) = a.sin_cos();
            let (x, z) = (w.lateral, w.longitudinal);
            w.lateral = x * cs - z * sn;
            w.longitudinal = x * sn + z * cs;
        }
    }

    /// `Roughness` 0x499730 (car, i) — per-wheel random road-height offset, CONFIRMED:
    /// amplitude and period by surface type, a new target drawn every `period` metres of travel and
    /// interpolated linearly in between. Returns 0 while the car is tilted (`cosTilt < 0.86`).
    fn roughness(&mut self, i: usize, speed_est: f32) -> f32 {
        if self.cos_tilt < 0.86 {
            return 0.0;
        }
        let (amp, period) = match self.wheels[i].surface & 0xF {
            3 | 5 | 9 => (0.1, 2.5),
            2 | 7 => (0.03, 0.25),
            4 | 10 => (0.03, 0.5),
            _ => (0.0, 0.5),
        };
        let w = &mut self.wheels[i];
        w.rough_phase += speed_est / 32.0;
        // 0x4997E4..0x49980B: a new target is drawn when the phase has run past the period, and also
        // on every tick of a smooth surface (amp < 0.01) — which keeps `prev`/`target` at ~0 there, so
        // a wheel leaving a rough patch does not carry a stale offset.
        if w.rough_phase >= period || amp < 0.01 {
            w.rough_phase = 0.0;
            w.rough_prev = w.rough_target;
            let r01 = self.rand01();
            let w = &mut self.wheels[i];
            w.rough_target = amp * (1.0 / 6.0 - r01);
        }
        let w = &self.wheels[i];
        w.rough_prev + (w.rough_target - w.rough_prev) * (w.rough_phase / period)
    }

    /// `BodyGroundCollide` 0x496610 + `GroundImpulse` 0x495F30 (`RE/notes/suspension-collision.md`
    /// §6.3, HIGH): body corners below the contact plane take a normal impulse with surface friction
    /// `k`. `invM = 1/mass`, `invI = 1/(6·mass)` (`Car_physInit` 0x49B510, CONFIRMED). The stored
    /// angular velocity is in turns/s and is converted to rad/s the way the original does.
    /// `car+0x428` is [`Self::min_height`] from [`Self::body_height_on`].
    pub(crate) fn body_ground_collide_mass(&mut self, mass: f32, n: Vec3, plane_pt: Vec3, k: f32) {
        let m = self.orientation;
        let (hw, hh, hl) = (self.half_extents.x, self.half_extents.y, self.half_extents.z);
        let (bx, by, bz) = (m.row(0), m.row(1), m.row(2));
        let (mut count, mut depth, mut centroid) = (0u32, 0.0f32, Vec3::ZERO);
        let contact = |p: Vec3, count: &mut u32, depth: &mut f32, centroid: &mut Vec3| {
            let d = (p - plane_pt).dot(n);
            if d < 0.01 {
                *count += 1;
                *centroid += p;
                *depth = depth.min(d);
            }
        };
        if self.body_points.is_empty() {
            for sz in [-1.0, 1.0] {
                for sx in [-1.0, 1.0] {
                    for sy in [-1.0, 1.0] {
                        contact(self.position + by * (sy * hh) + bz * (sz * hl) + bx * (sx * hw), &mut count, &mut depth, &mut centroid);
                    }
                }
            }
        } else {
            let mt = m.transpose();
            for p in &self.body_points {
                contact(self.position + mt * *p, &mut count, &mut depth, &mut centroid);
            }
        }
        if count == 0 {
            return;
        }
        let c = centroid / count as f32;
        let inv_m = 1.0 / mass.max(1.0);
        let inv_i = 1.0 / (6.0 * mass.max(1.0));
        let mut w = self.angular_velocity * TAU; // rad/s
        let r = c - self.position;
        let v_c = self.velocity + w.cross(r);
        let vrel = v_c.dot(n);
        // GroundImpulse 0x495F30
        let denom = 0.25 * inv_m + inv_i * r.cross(n).length();
        let mut j = if denom > 1e-6 { -vrel / denom } else { 0.0 };
        let min_extent = hw.min(hh).min(hl);
        // a smackable's `j` is scaled by 0.3 whatever its size (0x495F30, `flags & 0x80`)
        j *= if self.smackable {
            0.3
        } else if min_extent > 1.5 {
            0.6 / min_extent
        } else {
            0.4
        };
        self.impact = self.impact.max(self.velocity.dot(n).abs());
        if k > 0.0 {
            // friction along the contact plane, limited so it cannot reverse the tangential motion
            let tang = v_c - n * vrel;
            if tang.length_squared() > 1e-8 {
                let t = -tang.normalize();
                let jf = (k * j).min(tang.length() / (0.5 * inv_m).max(1e-6));
                self.velocity += t * (jf * 0.5 * inv_m);
                w += r.cross(t) * (jf * 2.0 * inv_i);
            }
        }
        if k < 0.001 {
            self.velocity *= 0.9;
            w *= 0.9;
        } else {
            self.velocity += n * (j * 0.5 * inv_m);
            w += r.cross(n * j) * (2.0 * inv_i);
            // settle a body resting on its points:
            // `(cosTilt < 0.2 or flags & 0x80) and car+0x428 < 0.005` (§6.3), `car+0x428` being
            // [`Self::min_height`] (`BodyHeight` 0x49A900, [`Self::body_height_on`])
            if (self.cos_tilt < 0.2 || self.smackable) && self.min_height < 0.005 && vrel.abs() < 10.0 && w.abs().max_element() < 1.0 {
                w *= 0.75;
                self.velocity *= 0.95;
            }
        }
        let lift = (-depth).max(0.0);
        self.position += n * lift;
        for wh in &mut self.wheels {
            wh.world_pos += n * lift;
        }
        // ClampAngVel 0x4964D0: each component to ±0.75 turns/s for a normal car, ±2.25 for a
        // `.TINYCAR` (0x4964E0…0x4964EC, `[0x5B49C0]`)
        let lim = self.ang_limit();
        self.angular_velocity = (w / TAU).clamp(Vec3::splat(-lim), Vec3::splat(lim));
        let effect = 4.0 * self.velocity.dot(n).abs();
        self.damage_by_normal(effect, n, 0, mass);
    }

    /// `BodyHeight` 0x49A900 (car, &n, &q) — read 2026-09-25, CONFIRMED: the height of the body's
    /// lowest points above the plane through `q` with normal `n`, measured on the world collision
    /// points `car+0x1DC` (`0x49AE30`: `M·car+0x8C[i] + pos`). `d = (p − q)·n` (0x532330 /
    /// 0x532910) goes into a four-slot ascending list that starts at 1000 (0x49A917…0x49A92F,
    /// `0x447A0000`).
    ///
    /// * **Upright** — `car+0x3A0 > 0.939` (`[0x5B4A0C]`, the previous `Car_ground`'s) and not a
    ///   smackable (0x49A90A…0x49A949): the **first four** points only (the counter at
    ///   0x49A95D), which 0x49ABC0 leaves as the four lowest `BndP` points — the tyre contact set.
    ///   `car+0x424` **and** `car+0x428` both get `½·(mean + min)` of them
    ///   (`[0x5B2468]` = 0.25, `[0x5B23F0]` = 0.5, 0x49AA23…0x49AA4C).
    /// * **Tilted, or a smackable** (0x49AA57): all `car+0x32C` points; `car+0x424` = the lowest,
    ///   `car+0x428` = the mean of the three lowest (`[0x5B2680]` = 1/3, 0x49AB33…0x49AB52).
    ///
    /// `Susp_update` 0x499A70 hands it its own ground normal and `Σc/4`, the mean of the four
    /// wheel contact points (0x49A8CF…0x49A8DD; the same vector whose `.y` it returns);
    /// `Susp_AI` 0x4998C0 the tracker's normal and plane point `car+0x400` (0x499934).
    ///
    /// A body with no [`Self::body_points`] (the port's scenarios) measures its four tyre-bottom
    /// points, which is what the first four `BndP` points are on a real car (port fallback).
    pub(crate) fn body_height_on(&mut self, n: Vec3, q: Vec3) {
        let mt = self.orientation.transpose();
        let upright = self.cos_tilt > 0.939 && !self.smackable;
        let fallback = self.probe_points();
        let pts: &[Vec3] = if self.body_points.is_empty() { &fallback } else { &self.body_points };
        let pts = if upright { &pts[..pts.len().min(4)] } else { pts };
        let mut low = [1000.0f32; 4];
        for p in pts {
            let d = (self.position + mt * *p - q).dot(n);
            // 0x49A998…0x49AA0F: shift down from the first slot `d` is not above
            if let Some(k) = low.iter().position(|l| d <= *l) {
                low[k..].rotate_right(1);
                low[k] = d;
            }
        }
        if upright {
            let v = ((low[0] + low[1] + low[2] + low[3]) * 0.25 + low[0]) * 0.5;
            self.body_height = v;
            self.min_height = v;
        } else {
            self.body_height = low[0];
            self.min_height = (low[0] + low[1] + low[2]) * (1.0 / 3.0);
        }
    }

    /// `ClampAngVel` 0x49BBA0: each component of the world spin `car+0x388` to ±[`Self::ang_limit`]
    /// (0.75; 2.25 for a `.TINYCAR`; 6.75 for a smackable lighter than 0.75, 0x49BBAA…0x49BBE2) —
    /// per component, not on the length.
    fn clamp_ang_vel(&mut self) {
        let lim = self.ang_limit();
        self.angular_velocity = self.angular_velocity.clamp(Vec3::splat(-lim), Vec3::splat(lim));
    }

    /// `Susp_applyLift` 0x49AEC0 (car, dy): after `Car_ground` moved the body by `dy`, each wheel's
    /// compression `wheel+0x68` (`car+0x820 + 0xC4·i`) becomes `c = compr − dy·M[1].y` (`car+0x374`),
    /// clamped to `(i < 2 ? 0.9 : 0.75)·(specs[0x124] − specs[0x120])` above 0 and to
    /// `−specs[0x124]` below (0x49AEE0…0x49AF50). `travel` is `(specs[0x120], specs[0x124])`;
    /// `None` is the smackable's early return (0x49AEC5).
    fn susp_apply_lift(&mut self, dy: f32, travel: Option<(f32, f32)>) {
        let Some((probe, bump)) = travel else { return };
        let up_y = self.orientation.row(1).y;
        for (i, w) in self.wheels.iter_mut().enumerate() {
            let c = w.compression - dy * up_y;
            w.compression = if c > 0.0 { c.min(if i < 2 { 0.9 } else { 0.75 } * (bump - probe)) } else { c.max(-bump) };
        }
    }

    /// `Car_ground` 0x49BD40 from the gravity on (0x49BE53…0x49C344), the part a car and a
    /// smackable share — read 2026-09-25, CONFIRMED. The caller has run `Susp_update` 0x499A70 (a
    /// car) or `Susp_AI` 0x4998C0 (a smackable, `car+0x52C & 0x80`), set [`Self::cos_tilt`]
    /// (`car+0x3A0 = N·M[1]`), [`Self::body_height`] (`car+0x424`) and [`Self::min_height`]
    /// (`car+0x428`), and works out `dh = (h − car+0x41C)·rateHz`. `n` / `q` are the tracker's
    /// patch normal and plane point (`car+0x400`), `surf` the surface nibble `car+0x2C & 0xF`,
    /// `mass` the body's for `BodyGroundCollide`, `travel` [`Self::susp_apply_lift`]'s.
    ///
    /// * gravity `0.30625 · nTicks · car+0x84` while `car+0x428 > 0.005` or the car is a
    ///   `.TINYCAR` (0x49BE59…0x49BE97); `vy` is the velocity after it, `closing = dh − vy`
    ///   (0x49BF36);
    /// * the **tilted test** — `cosTilt < 0.939` (0x49BF48), or any smackable (0x49BF55) — asks
    ///   `car+0x424 < 0.001` (0x49BF62). When it holds: `BodyGroundCollide` 0x496610 with the
    ///   surface's friction `0x5D0B94[surf][0]`, **0.1** instead when `cosTilt < 0.707` on a car
    ///   that is not a smackable (0x49BFC0…0x49BFE8); a car left upside down (`cosTilt < 0`) dies
    ///   (`car+0xDB0 = 0`, 0x49C01B); and when `cosTilt < 0.707` and `vy < −2` a bounce
    ///   `vel.y −= 0x5D0B98[surf] · vy` with ×0.95 on `vel.x`, `vel.z` and the spin
    ///   (0x49C038…0x49C08D) — `vy` is reloaded at 0x49BF87 and is the value **before** the
    ///   impulse, not `closing`;
    /// * otherwise (upright, or the test failed) the **upright arm** 0x49C19A: `closing > 6` and
    ///   `car+0x424 < −0.05` is a hard landing — `lift = −0.05 − car+0x424`, `car+0x424 = −0.05`,
    ///   `pos.y += lift`, `Susp_applyLift(lift)`, `car+0xAD0 = closing`,
    ///   `vel.y = 0.4 · 0x5D0B98[surf] · closing + dh`, `ClampAngVel` (0x49C1D6…0x49C270);
    ///   `closing ≤ 6` and `car+0x424 < 0.005` sets the body on the plane — `pos.y −= car+0x424`,
    ///   `car+0x424 = 0`, `vel.y = dh` unless both `vy` and `dh` are positive, then the larger —
    ///   and `Susp_applyLift(−car+0x424_old)` (0x49C27D…0x49C321);
    /// * then always (0x49C0F4…0x49C149) the **8 m ceiling rule**: when the enclosure test
    ///   0x473330 finds a patch more than 3.5 m overhead with a non-zero word, the body is rising
    ///   and `pos.y − car+0x404 > 8`, `vel.y = −vel.y`.
    ///
    /// Also (read 2026-09-25): the hard landing runs 0x493DD0 for the human car
    /// ([`Self::landing_reset`]) and pushes the landing event ([`Self::landing_sound`]); both
    /// landings clear [`Self::airborne_count`] when upright; in the tilted arm a car upside down
    /// (`cosTilt < −0.86`) with damage on, roof damage `car+0x504` under 40 and still racing
    /// (`car+0xB50 == 0`) pushes the roll-over event and has its roof damage **set to 40**
    /// (0x49C095…0x49C0E6); last, [`Self::travel_dir`] (`car+0x348`). Dead, so not carried:
    /// `car+0x40C = (dh − car+0x420 + 7·car+0x40C)/8`, `car+0x420 = dh` (0x49BF08…0x49BF30) and
    /// `car+0xAD0 = closing` — nothing but the resets (0x40F461 / 0x40F473, 0x49B6BC / 0x49B6D4,
    /// 0x410D10…0x410D1C) touches them; nor is `car+0x88 = tick` (0x49C105), which has no reader found.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn car_ground_body(&mut self, dh: f32, n: Vec3, q: Vec3, surf: usize, mass: f32, travel: Option<(f32, f32)>, grid: &TriangleGrid) {
        if self.min_height > 0.005 || self.tiny {
            self.velocity.y -= GRAVITY / 32.0 * self.gravity_scale;
        }
        let vy = self.velocity.y;
        let closing = dh - vy;
        let surf = surf & 15;
        if (self.cos_tilt < 0.939 || self.smackable) && self.body_height < 0.001 {
            let k = if self.cos_tilt < 0.707 && !self.smackable { 0.1 } else { SURFACE_BOUNCE[surf][0] };
            self.body_ground_collide_mass(mass, n, q, k);
            // 0x49C005…0x49C01B: left upside down by the contact, the car is dead
            if self.cos_tilt < 0.0 {
                if self.engine_alive != 0 {
                    self.engine_kill = 2;
                }
                self.engine_alive = 0;
            }
            if self.cos_tilt < 0.707 && vy < -2.0 {
                self.velocity.y -= SURFACE_BOUNCE[surf][1] * vy;
                self.velocity.x *= 0.95;
                self.velocity.z *= 0.95;
                self.angular_velocity *= 0.95;
            }
            // 0x49C095…0x49C0E6: rolled over with damage on (`[0x657A2C]`), roof under 40, still
            // racing — the roll-over event, and the roof damage is set to 40
            if self.damage_enabled && self.cos_tilt < -0.86 && self.damage[8] < 40.0 && self.race_state == 0 {
                self.rollover_sound = true;
                self.damage[8] = 40.0;
            }
        } else if closing > 6.0 {
            if self.body_height < -0.05 {
                // hard landing: the human's 0x493DD0 (`flags & 1`, 0x49C1C4), the event
                if self.is_human() {
                    self.landing_reset();
                }
                self.landing_sound = Some(closing);
                let lift = -0.05 - self.body_height;
                self.body_height = -0.05;
                self.position.y += lift;
                self.susp_apply_lift(lift, travel);
                // 0x49C223…0x49C23B
                if self.cos_tilt > 0.9 {
                    self.airborne_count = 0;
                }
                // `car+0x430` is not written here — the original stores the closing speed in the
                // dead `car+0xAD0`; the event is [`Self::landing_sound`]
                self.velocity.y = 0.4 * SURFACE_BOUNCE[surf][1] * closing + dh;
                self.clamp_ang_vel();
            }
        } else if self.body_height < 0.005 {
            // resting / soft landing: follow the ground
            let old = self.body_height;
            self.position.y -= old;
            self.body_height = 0.0;
            // 0x49C2B1…0x49C2C4
            if self.cos_tilt > 0.86 {
                self.airborne_count = 0;
            }
            self.height_marker = 0.0;
            self.velocity.y = if vy <= 0.0 || dh <= 0.0 { dh } else { vy.max(dh) };
            self.susp_apply_lift(-old, travel);
        }
        // the 8 m ceiling rule; the enclosure test walks the whole quadtree cell, so no depth limit
        if self.velocity.y > 0.0
            && self.position.y - q.y > 8.0
            && grid.ceiling(self.position.x, self.position.y, self.position.z, 3.5, 1.0e6).is_some_and(|h| h.surface & 0xFFFF != 0)
        {
            self.velocity.y = -self.velocity.y;
        }
        // `car+0x348` (0x49C14F…0x49C341, 0x5328D0)
        self.travel_dir = if self.velocity == Vec3::ZERO { self.orientation.row(2) } else { self.velocity.normalize() };
    }

    /// `flags & 1`, the human-driven car — in this port's fields, one that is neither AI-driven,
    /// traffic nor a smackable (the port keeps no flags word).
    pub fn is_human(&self) -> bool {
        !self.ai_driven && !self.is_traffic && !self.smackable
    }

    /// `PostHit` 0x493E40 (car) — read 2026-09-25, CONFIRMED: after a wall (0x494351) or a solid
    /// prop (0x498588) hit, `vel *= 0.998` (`0x3F7F7CEE`, 0x493E4D), the body-frame velocity
    /// `car+0xD58…+0xD60` from the body rows `car+0x364…` (0x493E5A…0x493E8D), the speed estimate
    /// `car+0x35C` (the port works it out where it is read), and **for the human**
    /// (`car+0x52C & 4`, 0x493EF1 — the human's flags 0x105 carry it, an opponent's 0x10B does not)
    /// 0x493DD0 ([`Self::landing_reset`]). *(Corrected 2026-09-25: the notes had "AI cars call
    /// 0x493DD0".)*
    fn post_hit(&mut self) {
        self.velocity *= 0.998;
        self.v_local = self.orientation * self.velocity;
        if self.is_human() {
            self.landing_reset();
        }
    }

    /// 0x493DD0 (car) — read 2026-09-25, CONFIRMED: the body-frame velocity `car+0xD58` /
    /// `+0xD60` from the physics frame rows `car+0x490` / `+0x4A8` (0x493DD6…0x493DFC; the `|vz|`
    /// it works out goes nowhere), the slip tangent `car+0xDC0 = 0`, every wheel's airborne flag
    /// `wheel+0x84 = 0` (`car+0x870 + 0xC4·i`) and the wheelspin state `car+0xDB8 = 0`
    /// (0x493E1A…0x493E37). Callers: `Car_ground`'s hard landing for the human (0x49C1CE),
    /// [`Self::post_hit`] for the human (0x493EF6) and the crash counter 0x40F740 (0x40F8DB) —
    /// the last not wired: it runs for the human only when the impact `car+0x430` is non-zero, no
    /// other car is recorded (`car+0x434 == 0`) and the impact is above `[0x5B2730]` =
    /// **6553600.0** (0x40F891…0x40F8DB), which no hit in the game reaches.
    pub fn landing_reset(&mut self) {
        let f = self.frame;
        self.v_local.x = f.row(0).dot(self.velocity);
        self.v_local.z = f.row(2).dot(self.velocity);
        self.slip_tangent = 0.0;
        for w in &mut self.wheels {
            w.airborne = false;
            w.contact = true;
        }
        self.rear_spin = false;
    }

    /// `Susp_update` 0x499A70 + `Car_ground` 0x49BD40 (CONFIRMED, `RE/notes/suspension-collision.md` §3–4).
    /// Wheel reference points (tyre bottoms) probe the ground; pitch/roll rates are driven by the
    /// front/rear and left/right contact height differences; the body follows the contact plane.
    fn ground_contact(&mut self, grid: &TriangleGrid, specs: &CarSpecs) {
        let h = self.susp_update(grid, specs);
        let up = self.orientation.row(1);
        self.car_ground_rest(h, up, grid, specs);
    }

    /// `Susp_update` 0x499A70 (car, &normalOut) → the ground height `(Σc/4).y`
    /// (`RE/notes/suspension-collision.md` §3): the four wheel probes, the compression, the
    /// suspension vectors, the ground normal ([`Self::ground_normal`]), the pitch / roll rates,
    /// `car+0x418` and `BodyHeight` 0x49A900 into `car+0x424` / `+0x428`.
    fn susp_update(&mut self, grid: &TriangleGrid, specs: &CarSpecs) -> f32 {
        let m = self.orientation; // rows = body axes in world space
        let mt = m.transpose();
        let up = m.row(1);
        let s_sign = if self.cos_tilt > 0.0 { 1.0 } else { -1.0 };
        let mut contacts = [Vec3::ZERO; 4];
        let mut susp_y = [0f32; 4];
        let mut n_air = 0;
        let mut heights = [0f32; 4];
        let probes = self.probe_points();
        for i in 0..4 {
            // the world collision point `car+0x1DC + 12i` (0x49AE30: `pos + Σ p_k·M[k]`) of body
            // point i, which 0x49ABC0 left as wheel i's ground reference (0x499AD4…0x499B13)
            let world_pt = self.position + mt * probes[i];
            let probe = world_pt + Vec3::new(0.0, specs.probe_offset * up.y, 0.0);
            // the wheel's tracker (0x499B64 `updateZone(probe, 1)`); its zone word — 0 with no
            // patch — is the wheel's surface `wheel+0x8C` on **both** branches (0x499B86…0x499BA7,
            // `RALLY` turning nibbles 1 and 10 into 2)
            self.wheels[i].tracker.update_zone(grid, probe);
            let nibble = (self.wheels[i].tracker.word & 0xF) as usize;
            let zone = if self.rally && (nibble == 1 || nibble == 10) { 2 } else { self.wheels[i].tracker.word & 0xFFFF };
            self.wheels[i].zone = zone;
            self.wheels[i].surface = (zone & 0xF) as usize;
            // 0x494480: no surface nibble ⇒ 1; the patch plane more than 4 m from the body's
            // `car+0x334` ⇒ 2 (0x4944CB…0x4944F0, `[0x5B2404]`); else 0, the patch branch
            let patch_h = if nibble == 0 {
                None
            } else {
                self.wheels[i].tracker.patch_height(probe.x, probe.z).filter(|h| (h - self.position.y).abs() <= 4.0)
            };
            let h_i = match patch_h {
                Some(h) => {
                    // road roughness (0x499730), added on the patch branch only
                    let speed_est = {
                        let (a, b) = (self.v_local.x.abs(), self.v_local.z.abs());
                        a.max(b) + 0.25 * a.min(b)
                    };
                    let mut h = h + self.roughness(i, speed_est);
                    // the front wheels' wobble from suspension damage (0x499C8B…0x499CC1):
                    // `SuspDamage(car)·sin(6.28318·(0.5·i + wheel+0x88))·(1/1500)` — 0x493BC0,
                    // `[0x5B23F0]` 0.5, `[0x5B4A08]` 6.28318, the double `[0x5B4A00]` 1/1500
                    if i < 2 {
                        let arg = (0.5 * i as f32 + self.wheels[i].rotation) * 6.283_18;
                        h += self.suspension_wear(specs) * arg.sin() * (1.0 / 1500.0);
                    }
                    h
                }
                // no usable patch: the kept plane `car+0x3E8` / `car+0x400` (0x499BC4…0x499C2E),
                // with no roughness — the branch jumps straight to the sum at 0x499CC9
                None => self.tracker.plane_height(probe.x, probe.z),
            };
            // the plane's normal the branch used, `wheel+0x4C` (0x499D0F…0x499D12): the tracker's
            // on the patch branch, `car+0x3E8` or `(0, 1, 0)` on the fallback
            let n_i = if patch_h.is_some() { self.wheels[i].tracker.normal } else { self.tracker.fallback_normal() };
            let (vel, acc, row0) = (self.velocity, self.accel_world, m.row(0));
            let w = &mut self.wheels[i];
            heights[i] = h_i;
            contacts[i] = Vec3::new(probe.x, h_i, probe.z);
            w.world_pos = Vec3::new(probe.x, h_i + self.ride_height, probe.z);
            // compression (feeds the wheel render height only), and the wheel's stiffness: 1.0,
            // or **1.25** when the compression is past twice its cap (0x499DD6…0x499DE7 front,
            // 0x499E14…0x499E25 rear; `stiff[4]` at `[esp+0x9C]`, 1.0 each at 0x499A81)
            // `c = (contact − probe)·M[1]`, and −0.1 when `M[1].y < 0.3` (0x499D78…0x499D90,
            // `[0x5B25F0]` / `[0x5B2444]`)
            let c = if up.y < 0.3 { -0.1 } else { (h_i - probe.y) * up.y };
            let mut stiff = 1.0;
            if c > 0.0 {
                let cmax = if i < 2 { 0.9 } else { 0.75 } * (specs.bump_travel - specs.probe_offset);
                if c > 2.0 * cmax {
                    stiff = 1.25;
                }
                w.compression = c.min(cmax);
            } else {
                let c = c.max(-(if w.airborne { 1.65 } else { 1.6 }) * specs.bump_travel);
                w.compression = 0.5 * (c + w.compression);
            }
            w.height_d = h_i - probe.y;
            let d_i = w.height_d.max(-0.30625);
            let v_y = s_sign * d_i * up.y;
            if d_i < -0.2041667 {
                w.airborne = true;
                n_air += 1;
                // `.TINYCAR` keeps the whole of it (0x49A01B skips the ×0.8)
                susp_y[i] = stiff * if self.tiny { v_y } else { v_y * 0.8 };
            } else {
                // **The lateral term** (0x49A050…0x49A1A7): with the velocity's `|x|` and `|z|`
                // both over 0.01 (`[0x5B27A8]`, `test ah,0x41; jne` skips at ≤), `u = normalize(−vx,
                // 0, −vz)`, `u' = (n_i × u) × n_i` (0x532880 twice — the ground-plane direction
                // against the motion), `k = 0.0666667·|vel·M[0]|` (`[0x5B49EC]`) when the wheel
                // hung last tick (`wheel+0x84` is read before 0x49A1AC clears it) or
                // `0.0625·|car+0xD40·M[0]|` (`[0x5B26A4]`) — positive either way, the `fchs` at
                // 0x49A11F / 0x49A163 takes the absolute value — and `(k·u')·M[1]` is added to
                // `susp.y` **after** the stiffness (0x49A1DA `fadd st(1)`). Anything else leaves the
                // 0.0 pushed at 0x499F60.
                let mut lat = 0.0;
                if vel.x.abs() > 0.01 && vel.z.abs() > 0.01 {
                    let u = Vec3::new(-vel.x, 0.0, -vel.z).normalize();
                    let u2 = n_i.cross(u).cross(n_i);
                    let k = if w.airborne { 0.066_666_67 * vel.dot(row0).abs() } else { 0.0625 * acc.dot(row0).abs() };
                    lat = (u2 * k).dot(up);
                }
                w.airborne = false;
                susp_y[i] = stiff * v_y + lat;
            }
            w.contact = !w.airborne;
        }
        let h = (heights[0] + heights[1] + heights[2] + heights[3]) * 0.25;
        // ground normal from the contact points
        let f = ((contacts[0] + contacts[1]) - (contacts[2] + contacts[3])) * 0.5;
        let r = ((contacts[1] + contacts[3]) - (contacts[0] + contacts[2])) * 0.5;
        let mut n = f.normalize_or_zero().cross(r.normalize_or_zero()).normalize_or_zero();
        if n.y < 0.0 {
            n = -n;
        }
        if n.length_squared() < 0.5 {
            n = Vec3::Y;
        }
        if n.y < 0.5 {
            n_air = 4;
        }
        self.ground_normal = n;
        // 0x49A8AA…0x49A8C0: `word car+0x418 = nAir > 1` (after `nAir = 4` for a steep normal)
        self.airborne_count = u16::from(n_air > 1);
        // pitch / roll rate kinematics
        if self.cos_tilt < 0.707 {
            self.dive *= 0.5;
            self.roll_lp *= 0.5;
        } else if n_air < 4 {
            // 8, or 4 for `.TINYCAR` (0x49A438…0x49A44A)
            let div = if self.tiny { 4.0 } else { 8.0 };
            let w_body = m * self.angular_velocity; // (pitch, yaw, roll) rates in body axes
            let p_in = ((susp_y[2] + susp_y[3]) - (susp_y[0] + susp_y[1])) / div;
            let a_l = self.accel_local.z;
            // `.TINYCAR` takes the gain unfiltered (0x49A522…0x49A53E, 0x49A647…0x49A655)
            self.dive = if self.tiny { specs.dive_gain * a_l } else { 0.8 * self.dive + specs.dive_gain * a_l * if a_l > 0.0 { 0.125 } else { 0.175 } };
            let p = (p_in - self.dive).clamp(-specs.pitch_clamp, specs.pitch_clamp);
            let r_in = ((susp_y[1] + susp_y[3]) - (susp_y[0] + susp_y[2])) / div;
            self.roll_lp = if self.tiny { specs.roll_gain * self.accel_local.x } else { 0.8 * self.roll_lp + 0.25 * specs.roll_gain * self.accel_local.x };
            let rr = (r_in + (1.0 - self.throttle * 254.0 / 1275.0) * self.roll_lp).clamp(-specs.roll_clamp, specs.roll_clamp);
            let wp = (0.925 * specs.pitch_ret * w_body.x + p).clamp(-0.75, 0.75);
            let wr = (specs.roll_ret * w_body.z + rr).clamp(-0.75, 0.75);
            let wy = w_body.y.clamp(-0.75, 0.75);
            self.angular_velocity = mt * Vec3::new(wp, wy, wr);
            // 0x49A756…0x49A8A4: on two wheels — some wheel hangs but not a wheel on **each** side
            // (left = wheels 0 / 2, `car+0x83C` / `+0x9C4`; right = 1 / 3, `+0x900` / `+0xA88`) —
            // the body is shifted by `Mᵀ·v/32` (`[0x5B24B8]`), `v = (−2·sin(2π·wr)·halfW,
            // −2·cos(2π·wr)·halfH, 0)` (`[0x5B2628]` = −2, 0x5327E0 / 0x5327F0 take turns, the
            // clamped roll rate `car+0xD6C`, the half extents `car+0x3A8` / `+0x3AC`), `v.y` negated
            // when the left side hangs and `wr < 0` or the right side hangs and `wr > 0`
            let left_air = self.wheels[0].airborne || self.wheels[2].airborne;
            let right_air = self.wheels[1].airborne || self.wheels[3].airborne;
            if n_air > 0 && !(left_air && right_air) {
                let a = wr * 6.283_184_5;
                let mut v = Vec3::new(-2.0 * a.sin() * self.half_extents.x, -2.0 * a.cos() * self.half_extents.y, 0.0);
                if (left_air && wr < 0.0) || (right_air && wr > 0.0) {
                    v.y = -v.y;
                }
                self.position += mt * v * (1.0 / 32.0);
            }
        }
        // body height above the contact plane: `BodyHeight` 0x49A900 on Susp_update's normal and
        // `Σc/4` (0x49A8CF…0x49A8DD), over the body points (`Self::body_height_on`)
        let centre = (contacts[0] + contacts[1] + contacts[2] + contacts[3]) * 0.25;
        self.body_height_on(n, centre);
        self.height_marker = if self.body_height > 0.05 { self.body_height } else { 0.0 };
        h
    }

    /// `Car_ground` 0x49BD40 after its `Susp_update` call, `h` that call's height.
    fn car_ground_rest(&mut self, h: f32, up: Vec3, grid: &TriangleGrid, specs: &CarSpecs) {
        // ---- Car_ground 0x49BD40 from 0x49BDBD on (`RE/notes/suspension-collision.md` §4.2) ----
        // `Lod_shouldUpdate` 0x414B10 hands a racer the full rate: 0x417190 leaves `car+0x78` 0
        // for the human (`flags & 1`, 0x4171A5) and the octagonal distance to the nearest racer
        // `0x5E8E2C[]` for anyone else, which for a racer is itself (0); the player also has the
        // force flag `car+0x10B8` (0x406E7F). Every car this runs for is a racer, so
        // `rateHz = 32`, `nTicks = 1` always (0x5CC700 / 0x5CC71C at index 6).
        let dh = (h - self.ground_h) * 32.0;
        self.ground_h = h;
        // `car+0x2C & 0xF`, the car tracker's zone word (0x49BF96, 0x49C244)
        let surf = (self.tracker.word & 15) as usize;
        // physics frame (0x49BA30): row0 body x, row1 ground normal, row2 body z, re-orthogonalised
        self.cos_tilt = self.ground_normal.dot(up);
        // the tilted arm's `BodyGroundCollide` takes the car tracker's normal (0x474420,
        // 0x49BFB3) and the patch centre `car+0x400` (0x49BF77); the ceiling rule `car+0x404`
        let (tn, tq) = (self.tracker.normal, self.tracker.plane_point);
        // the physics frame 0x49BA30 comes before the arms (0x49BE4E), so 0x493DD0 reads it
        let n = self.ground_normal;
        let mut z = self.orientation.row(2);
        let mut x = self.orientation.row(0);
        if n.dot(up) <= 0.9 {
            x = z.cross(n).normalize_or_zero();
            z = n.cross(x).normalize_or_zero();
        }
        self.frame = Mat3::from_cols(x, n, z).transpose();
        self.car_ground_body(dh, tn, tq, surf, specs.mass, Some((specs.probe_offset, specs.bump_travel)), grid);
    }

    /// 32 Hz step: input, forces, velocity integration, ground contact (`Car_simulate`).
    pub fn step32(&mut self, controls: &Controls, specs: &CarSpecs, grid: &TriangleGrid) {
        // 0x49AF70 (`+0xE0C`, priority 6, ahead of `Car_simulate` / `Car_ground` at 0x1E): the car
        // tracker at the position and, on a new patch, the plane 0x499520 keeps
        self.tracker.track(grid, self.position);
        self.ticks_alive += 1;
        self.process_input(controls, specs);
        let m = self.frame;
        // 0x628F9C = !(w0.air && w1.air && bodyHeight > 0.05), likewise for the rear axle
        let front_contact = !(self.wheels[0].airborne && self.wheels[1].airborne && self.body_height > 0.05);
        let rear_contact = !(self.wheels[2].airborne && self.wheels[3].airborne && self.body_height > 0.05);
        self.v_local = m * self.velocity;
        self.w_local = m * self.angular_velocity;
        let vl = self.v_local;
        // **Signed, not `|vz|`.** 0x49F25C loads `car+0xD60` with no `fabs` and 0x49F262 compares
        // it with `[0x5B2400]` = 5.0; 0x49F270's `test ah,0x41; jne` zeroes the ratio when
        // `vz <= 5`. So driving **backwards** the slip tangent is always 0 in the original, where
        // the port's `abs()` kept computing a ratio. It gates the kick-down and scales the
        // longitudinal demand, so the difference shows up in reverse. Corrected 2026-09-21.
        self.slip_tangent = if vl.z > 5.0 { vl.x / vl.z } else { 0.0 };
        // contact-patch slip velocities (×−16) with the yaw/roll term
        let yaw = specs.yaw_lever * self.angular_velocity.y * 32.0 * -0.5;
        let speed = vl.x.abs().max(vl.z.abs()) + 0.25 * vl.x.abs().min(vl.z.abs());
        // The two arms are the other way round: 0x49F2F0 compares the speed with `[0x5B241C]` =
        // 10.0 and 0x49F301's `je` goes to the **`+70 … ×0.0125`** arm when `speed >= 10`, while
        // the fall-through at 0x49F303 is `+30 … ×0.025` for `speed < 10`. (Both give `1.0·yaw`
        // at exactly 10, so the curve is continuous either way and only the branch order tells
        // them apart.) `physics.md` §4 had it inverted and the port followed.
        let roll = if speed < 10.0 { (speed + 30.0) * yaw * 0.025 } else { (speed + 70.0) * yaw * 0.0125 };
        for (i, w) in self.wheels.iter_mut().enumerate() {
            let lat = -16.0 * vl.x + if i < 2 { roll } else { -roll };
            w.slip_vel = [lat, -16.0 * vl.y, -16.0 * vl.z];
        }
        let gravity_l = m * Vec3::new(0.0, -GRAVITY, 0.0);
        // steering angle law. Both float→int steps here are **bare `fistp`** — 0x49F3EE for the
        // gravity offset and 0x49F4C9 for the damage pull — so they round to nearest (ties to
        // even), not truncate. `physics.md` writes them as `int()` and the port used `as i32`.
        // Corrected 2026-09-21; see `RE/CONVENTIONS.md`.
        let g_off = fistp(gravity_l.x * 8.0).clamp(-64, 64);
        // damage pull (0x49F160): uneven front-corner damage plus suspension-damage noise every 4th tick
        let mut pull = (self.damage[2] - self.damage[0]) * 0.25;
        if self.ticks_alive.is_multiple_of(4) {
            let r = ((self.ticks_alive.wrapping_mul(1103515245).wrapping_add(12345) >> 16) & 0xFFFF) as f32 / 65535.0 - 0.5;
            pull += r * 0.5 * if speed > 15.0 { speed / 15.0 } else { 1.0 } * self.suspension_wear(specs) * 0.01;
        }
        self.damage_pull = pull;
        self.set_wheel_steer(specs, (self.steer + fistp(pull) + g_off).clamp(-127, 127) as f32, speed, vl.z);
        let a_drive = self.engine_update(specs, rear_contact);
        if !front_contact && !rear_contact {
            self.accel_local = Vec3::ZERO;
            for w in &mut self.wheels {
                w.slip = 0.0;
                w.lateral = 0.0;
                w.longitudinal = 0.0;
            }
        } else {
            let mut a_brake = specs.brake_decel * self.brake * (1.0 - 0.005 * self.part_wear(&specs.zone_masks.brakes, &specs.wear.brakes));
            a_brake = a_brake.min(vl.z.abs() * 32.0);
            let sign = -vl.z.signum();
            let a_brake = a_brake * if vl.z == 0.0 { 0.0 } else { sign };
            let a_bf = a_brake * specs.front_brake;
            self.load_transfer(specs);
            let f = specs.front_drive;
            let (l0, l1, l2, l3) = (self.wheels[0].limit, self.wheels[1].limit, self.wheels[2].limit, self.wheels[3].limit);
            let fs = (l0 + l1).max(1e-6);
            let rs = (l2 + l3).max(1e-6);
            self.wheels[0].demand = a_drive * f * l0 / fs + 0.5 * a_bf;
            self.wheels[1].demand = a_drive * f * l1 / fs + 0.5 * a_bf;
            self.wheels[2].demand = a_drive * (1.0 - f) * l2 / rs + 0.5 * (a_brake - a_bf);
            self.wheels[3].demand = a_drive * (1.0 - f) * l3 / rs + 0.5 * (a_brake - a_bf);
            for w in &mut self.wheels {
                w.lateral = 0.0;
                w.longitudinal = 0.0;
            }
            if front_contact {
                self.tyre(0, specs);
                self.tyre(1, specs);
            }
            if rear_contact {
                self.tyre(2, specs);
                self.tyre(3, specs);
            }
            let w = &self.wheels;
            let fx: f32 = w.iter().map(|w| w.lateral).sum();
            let fz: f32 = w.iter().map(|w| w.longitudinal).sum();
            self.accel_local = Vec3::new(fx, 0.0, fz * specs.inv_grip);
            // yaw torque (signs per 0x49F994..0x49F9E3; MEDIUM)
            let torque_y = ((w[1].lateral + w[0].lateral) - (w[2].lateral + w[3].lateral)) * specs.inv_yaw_lever
                - ((w[2].longitudinal + w[0].longitudinal) - w[1].longitudinal - w[3].longitudinal) * specs.inv_yaw_lever;
            // **Stability assist 0x49F0C0, corrected 2026-09-21.** Two gates, both of which the
            // port had wrong.
            //
            // The lateral term: 0x49F11F compares `|car+0xD58|` with 1.0 and 0x49F12A's
            // `test ah,0x41; je` jumps **straight to the add** when `|x| > 1`; only the
            // `|x| <= 1` path falls through to the second test, `[car+0x374] < 0.833`
            // (0x49F132 / 0x49F13D). So the condition is `|vL.x| > 1 || M[1].y < 0.833`, and the
            // port asked for `< 1`.
            //
            // The longitudinal term is not unconditional either: 0x49F0E3 is
            // `cmp byte [ecx+0xD80], 0x40; jae 0x49F104`, so `−4.9 · M[2].y` is added only while
            // the **brake byte is under 64** of 255.
            let mut local = self.accel_local;
            if vl.x.abs() > 1.0 || self.orientation.row(1).y < 0.833 {
                local.x += -19.6 * m.row(0).y;
            }
            if self.brake_raw < 0x40 {
                local.z += -4.9 * m.row(2).y;
            }
            let acc = m.transpose() * local;
            self.accel_world = acc;
            let angacc = m.transpose() * Vec3::new(0.0, torque_y, 0.0);
            self.velocity += acc * DT32;
            self.angular_velocity.x += angacc.x * DT32;
            self.angular_velocity.z += angacc.z * DT32;
            // the yaw step is 1/48 of the angular acceleration, 1/32 for `.TINYCAR` (0x49FAD4)
            self.angular_velocity.y += angacc.y * if self.tiny { 1.0 / 32.0 } else { 1.0 / 48.0 };
            self.lateral_filtered = 0.25 * fx + 0.75 * self.lateral_filtered;
            self.long_filtered = 0.9 * self.long_filtered + 0.1 * self.accel_local.z;
            // 0x49FB3A: under `RALLY` the velocity loses `0.002 · min(|car+0xDC0|, 5)` of itself
            // each step (0x532360 over `car+0x33C`, `[0x5B2400]` 5, `[0x5B4A30]` 0.002)
            if self.rally {
                self.velocity *= 1.0 - 0.002 * self.slip_tangent.abs().min(5.0);
            }
            // **Coast yaw damping / handbrake — both conditions were inverted, 2026-09-21.**
            // 0x49FBE5 loads the speed and compares it with `[0x5B2408]` = 30.0; 0x49FBF3's
            // `test ah,1; je` **bails** when `speed >= 30`, so the damping is a *low*-speed
            // effect. 0x49FBFE then loads `specs[0xF4]` against `[0x5B2464]` = 2.5 and
            // 0x49FC0C's `test ah,0x41; jne` bails when it is `<= 2.5`, so it needs a grip
            // multiplier **above** 2.5. The port asked for `> 30` and `<= 2.5`, which is the
            // opposite pair on both counts.
            //
            // The handbrake arm is the same shape: 0x49FCFD compares the speed with 1.0 and
            // 0x49FD05's `je` bails when `speed >= 1`, so `vel *= 0.5` is the stop-it-dead
            // behaviour at a standstill, not a braking force at speed.
            if self.gear > 1 && self.throttle < 0.25 && speed < 30.0 && specs.grip > 2.5 {
                let k = 1.0 - 0.05 * self.angular_velocity.y.abs();
                self.velocity.x *= k;
                self.velocity.z *= k;
            } else if self.handbrake && speed < 1.0 {
                self.velocity *= 0.5;
            }
        }
        // Car_simulate gate: not upright ⇒ damp (0x49FD30)
        if self.cos_tilt < 0.5 {
            self.velocity *= 0.95;
            if self.cos_tilt < 0.2 {
                self.angular_velocity.y *= 0.98;
            }
        }
        self.ground_contact(grid, specs);
    }

    /// 64 Hz integration (`0x49B7A0` / `0x49B8E0`) and wheel rotation (`0x4A2790`).
    pub fn step64(&mut self, specs: &CarSpecs) {
        self.game_tick = self.game_tick.saturating_add(1);
        self.integrate64();
        // wheel rotation
        let vz = self.v_local.z;
        for (i, wh) in self.wheels.iter_mut().enumerate() {
            let driven = if i < 2 { specs.front_drive >= 0.01 } else { specs.front_drive <= 0.99 };
            let v = if driven && self.gear != 1 && !wh.locked && specs.ratio_rpm_per_mps[self.gear] != 0.0 {
                self.rpm / specs.ratio_rpm_per_mps[self.gear]
            } else {
                vz
            };
            if !wh.locked {
                wh.rotation = (wh.rotation + v / specs.circumference / 64.0).rem_euclid(1.0);
            }
        }
    }

    /// The two shared 64 Hz integrators every car record runs (`car+0xE24` / `+0xE28`):
    /// `0x49B7A0` `pos += vel · 1/64` and `0x49B8E0`'s rotation by `ω/64`.
    pub fn integrate64(&mut self) {
        self.position += self.velocity * DT64;
        let w = self.angular_velocity * DT64; // turns
        // 0x49B8E0 builds three axis rotations and multiplies them **X, then Y, then Z**:
        // 0x532600(M_y, ω_y/64), 0x5325A0(M_x, ω_x/64), 0x532660(M_z, ω_z/64) — each angle in turns
        // ([0x5B2828] = 1/64, and 0x5327E0/0x5327F0 are `sin`/`cos` of `angle × 2π`) — then
        // 0x49B968 `0x532470(M_x, M_y, T)` = `T = M_x · M_y`, 0x49B991 `Δ = T · M_z` and 0x49B9A6
        // `car+0x364 = car+0x364 · Δ`, in place through the aliasing guard at 0x532482.
        //
        // The three matrices are stored row-major and act on **row** vectors, so each is the
        // transpose of glam's same-named rotation: `M_x = [[1,0,0],[0,c,s],[0,-s,c]]`,
        // `M_y = [[c,0,-s],[0,1,0],[s,0,c]]`, `M_z = [[c,s,0],[-s,c,0],[0,0,1]]`. A row of
        // `car+0x364` therefore maps to `Δᵀ · row`, and
        // `Δᵀ = (M_x M_y M_z)ᵀ = M_zᵀ M_yᵀ M_xᵀ` = **`Rz · Ry · Rx`** in glam's convention.
        // This was `Ry · Rx · Rz` until 2026-09-21 — the right three matrices in the wrong order,
        // which agrees to first order in ω/64 and drifts in the second.
        let rot = Mat3::from_rotation_z(w.z * TAU) * Mat3::from_rotation_y(w.y * TAU) * Mat3::from_rotation_x(w.x * TAU);
        // rows are body axes in world space: rotate each row
        let m = self.orientation;
        self.orientation = Mat3::from_cols(rot * m.row(0), rot * m.row(1), rot * m.row(2)).transpose();
    }

    /// `Car_wallCollide` 0x495020 + `WallImpulse` 0x494000 (CONFIRMED): the 8 body corners are swept
    /// over the last tick against the `.edg` edges; a crossing reflects the normal velocity with
    /// restitution 0.1 (no friction), pushes the body 1.05× back onto the edge line and sets a
    /// heuristic yaw rate.
    ///
    /// `patch_height` answers the height of the drivable patch that contains a corner in plan, or
    /// `None` when there is none — `CollisionGrid::find_patch_height`. 0x495020 skips a corner only
    /// when **both** halves of its conjunction hold: the patch under it is drivable
    /// (`(T.word & 0xF) != 0`, 0x49527A) **and** that patch's height is within 4 m of the car's own
    /// y (`|h − car+0x334| <= [0x5B2404]`, 0x4952B9…0x4952D9). So a car sailing over drivable road
    /// is still tested against the walls, and a car that has dropped away from it is too.
    pub fn collide_walls(&mut self, walls: &crate::walls::WallSet, patch_height: &dyn Fn(Vec3) -> Option<f32>, _unused: f32, _b: f32, _f: f32) -> bool {
        let m = self.orientation;
        let (hw, hh, hl) = (self.half_extents.x, self.half_extents.y, self.half_extents.z);
        let (bx, by, bz) = (m.row(0), m.row(1), m.row(2));
        let mut corners = Vec::with_capacity(8);
        for sz in [-1.0, 1.0] {
            for sx in [-1.0, 1.0] {
                corners.push(self.position - by * hh + bz * (sz * hl) + bx * (sx * hw));
                corners.push(self.position + by * hh + bz * (sz * hl * 0.5) + bx * (sx * hw));
            }
        }
        let v_tick = self.velocity / 32.0;
        let mut max_j: f32 = 0.0;
        let mut hit = false;
        for p in corners {
            if patch_height(p).is_some_and(|h| (h - self.position.y).abs() <= 4.0) {
                continue;
            }
            let p0 = p - v_tick;
            let Some((n2, q, class)) = walls.segment_hit(p0, p) else { continue };
            self.last_wall_class = class;
            let n = Vec3::new(n2.x, 0.0, n2.y);
            let push = Vec3::new(q.x - p.x, 0.0, q.y - p.z) * 1.01 * 1.05;
            let j = -self.velocity.dot(n);
            if j <= 0.0 {
                continue;
            }
            self.velocity += n * (1.1 * j);
            self.position += push;
            // `car+0x354` only matters to a smackable's crash response (0x494964)
            self.damage_by_normal(3.0 * j, n, 1, self.collision_mass);
            for w in &mut self.wheels {
                w.world_pos += push;
            }
            // WallSpin 0x493F10 (sign table MEDIUM): spin away from the wall
            if !(j >= 15.0 && self.v_local.z >= 20.0) {
                let side = n.dot(bx);
                let spin = 0.25 * j.abs().min(side.abs());
                self.angular_velocity.y = -spin * side.signum() * self.v_local.z.signum();
            }
            self.post_hit();
            max_j = max_j.max(j);
            hit = true;
        }
        self.impact = self.impact.max(max_j);
        hit
    }

    /// `Car_propPass` 0x498DE0: the props whose AABB meets the car's position ± 5 m (0x473870 with
    /// 0x486510 — the tree query's 3 m leaf search is not reproduced, every prop is tried), each
    /// through 0x498280 — `|d·x| ≤ r + hx` and `|d·fwd| ≤ r + hz` for `d` the x-z offset to its
    /// centre — and then its slot 8 (0x487290) on the car's velocity `car+0x33C`: **2** (solid, and
    /// every prop without pieces) takes the cylinder's push 0x498AA0 or the box's 0x498690; **1**,
    /// the hit that starts a breakable's pieces (`Breakable::answer` on `tick`, the game tick), is
    /// the event `car+0x438 = 0x50000 | car+0x44C` with `car+0x430 = 15` at the car's position
    /// (0x498F21…0x498F56) and no push; **0** (its pieces started) lets the car through. The
    /// answer is the last hit's impact and its class — the event word `0x50000 | class` the sound
    /// takes (0x498DAD, 0x498A3B); for a start the class is `car+0x44C`, which only the smackable
    /// setup 0x4A12D7…0x4A136D writes, so a racing car's is 0 (MEDIUM: its allocation's zero). The
    /// `car+0x43C` hold that would skip the pass (0x498DE9) never runs: every writer in the image
    /// stores 0 (`suspension-collision.md` §4.2 item 10; corrected 2026-09-25 from "not carried").
    pub fn collide_props(&mut self, props: &[crate::props::Prop], tick: u32) -> Option<(f32, PropEvent)> {
        let mut hit = None;
        for p in props {
            if !p.near(self.position, 5.0) {
                continue;
            }
            let c = p.centre();
            let d = Vec3::new(c.x - self.position.x, 0.0, c.z - self.position.z);
            let (bx, bz) = (self.orientation.row(0), self.orientation.row(2));
            let r = p.radius();
            if d.dot(bx).abs() > r + self.half_extents.x || d.dot(bz).abs() > r + self.half_extents.z {
                continue;
            }
            match p.answer(tick, self.velocity) {
                2 => {}
                1 => {
                    self.impact = self.impact.max(15.0);
                    hit = Some((15.0, PropEvent::Prop(0)));
                    continue;
                }
                // a drive-through object (`BUSH`): above 2 m/s (`car+0x35C`, 0x498ED7…0x498EE8) the
                // event 0x6000F at 15 (0x498EF6…0x498F1A) and no push
                3 => {
                    if self.horizontal_speed() > 2.0 {
                        self.impact = self.impact.max(15.0);
                        hit = Some((15.0, PropEvent::Bush));
                    }
                    continue;
                }
                _ => continue,
            }
            let before = self.impact;
            self.impact = 0.0;
            let touched = match &p.shape {
                crate::props::PropShape::Cylinder { radius } => self.prop_cylinder(c, *radius),
                crate::props::PropShape::Box { corners, .. } => self.prop_box(corners),
            };
            if touched {
                hit = Some((self.impact, PropEvent::Prop(p.class)));
            }
            self.impact = self.impact.max(before);
        }
        hit
    }

    /// 0x498AA0: `d = car − centre` in 3D (nothing under 0.01), `n = d/|d|`; the car's reach along
    /// `n` is `min(hx / |n·x|, hz / |n·fwd|)` (10 for an axis within 0.1 of square); when the reach
    /// plus the radius exceeds the x-z distance, the solid hit 0x498380 with the object's surface
    /// point `centre + n·R` and the car's `pos − n·reach` — the normal turned round first when the
    /// car is past 5 m/s (`car+0x35C`), its heading within 60° of `n` and the reach under 0.75.
    fn prop_cylinder(&mut self, c: Vec3, radius: f32) -> bool {
        let d = self.position - c;
        if d.length() <= 0.01 {
            return false;
        }
        let mut n = d.normalize();
        let dist = Vec3::new(d.x, 0.0, d.z).length();
        let (bx, bz) = (self.orientation.row(0), self.orientation.row(2));
        let ax = n.dot(bx).abs();
        let rx = if ax > 0.1 { self.half_extents.x / ax } else { 10.0 };
        let az = n.dot(bz).abs();
        let rz = if az > 0.1 { self.half_extents.z / az } else { 10.0 };
        let reach = rx.min(rz);
        if reach + radius <= dist {
            return false;
        }
        if self.horizontal_speed() > 5.0 && self.velocity.normalize_or_zero().dot(n) > 0.5 && reach < 0.75 {
            n = -n;
        }
        let a = c + n * radius;
        let b = self.position - n * reach;
        self.solid_hit(n, a, b);
        true
    }

    /// 0x498690: the car's footprint edges in x-z — corners `pos ± hz·fwd ± hx·right` in the order
    /// `+F−R`, `+F+R`, `−F−R`, `−F+R`, edges (2,0), (0,1), (1,3), (3,2) (`0x5D0B6C`) — against the
    /// box (0x486FB0): two crossings take the crossing's point and normal at once; one takes the
    /// deepest of the car's body points behind it, `−(p − point)·n` above −0.01, when that is
    /// under the best so far (from 999) and under `max(0.1, 0.125·car+0x35C)`. The contact goes to
    /// `BodyGroundCollide` 0x496610 with `k = 0.1`.
    fn prop_box(&mut self, corners: &[Vec3; 4]) -> bool {
        let (f, r) = (self.orientation.row(2) * self.half_extents.z, self.orientation.row(0) * self.half_extents.x);
        let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
        let p = [flat(self.position + f - r), flat(self.position + f + r), flat(self.position - f - r), flat(self.position - f + r)];
        let limit = (self.horizontal_speed() * 0.125).max(0.1);
        let mut best_depth = 999.0f32;
        let mut contact: Option<(Vec3, Vec3)> = None;
        let mt = self.orientation.transpose();
        let body: Vec<Vec3> = self.body_points.iter().map(|q| self.position + mt * *q).collect();
        for (a, b) in [(2usize, 0usize), (0, 1), (1, 3), (3, 2)] {
            let (count, point, n) = crate::props::box_edge_test(corners, p[a], p[b]);
            if count > 1 {
                contact = Some((point, n));
                break;
            }
            if count == 1 {
                let mut d = -99.0f32;
                for q in &body {
                    let v = -(*q - point).dot(n);
                    if v > -0.01 && v > d {
                        d = v;
                    }
                }
                if d < best_depth && d > -0.01 && d < limit {
                    best_depth = d;
                    contact = Some((point, n));
                }
            }
        }
        let Some((point, n)) = contact else { return false };
        self.body_ground_collide_mass(self.sim_mass, n, point, 0.1);
        true
    }

    /// 0x498380, the solid hit: `m = n·((a − b)·n)` moves the car and its wheels' x and z; the impact
    /// `car+0x430 = |3·(v·n)|`; the velocity gains `32·m` and, when that raises the horizontal
    /// estimate past the old `car+0x35C`, it and the impact are scaled back by the ratio; WallSpin
    /// 0x493F10 and PostHit 0x493E40 as the wall collider has them; past 10 `Damage(…, zone, 1)`
    /// with the zone from `z = n·fwd`, `x = n·right` (0x4985A7…0x49867A).
    fn solid_hit(&mut self, n: Vec3, a: Vec3, b: Vec3) {
        let m = n * (a - b).dot(n);
        self.position += m;
        for w in &mut self.wheels {
            w.world_pos.x += m.x;
            w.world_pos.z += m.z;
        }
        let old = self.horizontal_speed();
        let vclose = -self.velocity.dot(n);
        let mut impact = (3.0 * vclose).abs();
        self.velocity += m * 32.0;
        let new = self.horizontal_speed();
        if new > 0.0 {
            let k = old / new;
            if k < 1.0 {
                self.velocity *= k;
                impact *= k;
            }
        }
        if !(vclose >= 15.0 && self.v_local.z >= 20.0) {
            let side = n.dot(self.orientation.row(0));
            let spin = 0.25 * vclose.abs().min(side.abs());
            self.angular_velocity.y = -spin * side.signum() * self.v_local.z.signum();
        }
        self.post_hit();
        self.impact = self.impact.max(impact);
        if impact > 10.0 {
            let (z, x) = (n.dot(self.orientation.row(2)), n.dot(self.orientation.row(0)));
            let zone = if z < -0.95 {
                1
            } else if z > 0.95 {
                5
            } else if x < -0.95 {
                3
            } else if x > 0.95 {
                7
            } else if z < 0.0 {
                if x < 0.0 { 0 } else { 2 }
            } else if x < 0.0 {
                6
            } else {
                4
            };
            self.damage_zone(impact, zone);
            self.crash_response(impact, zone, 1, self.collision_mass);
        }
    }

    /// Reset onto the track at a slice pose (the game's "reset car" 0x40F540 mode 3 — placement only,
    /// the original's exact conditions are not reproduced).
    pub fn reset_to(&mut self, position: Vec3, orientation: Mat3) {
        self.position = position;
        self.orientation = orientation;
        self.frame = orientation;
        self.velocity = Vec3::ZERO;
        self.angular_velocity = Vec3::ZERO;
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
        // 0x40F59D: every reset clears the crash countdown; 0x40F400 the `+0x418` word (0x40F43C)
        self.reset_countdown = 0;
        self.airborne_count = 0;
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
    /// trackers' slice (0x49B183, the port's trackers keep none), `car+0x35C = 0` and
    /// `car+0x7E = 0` (0x49B403 / 0x49B409 — the speed estimate is recomputed each tick here),
    /// the wheels' `+0x34` / `+0x40` seeded with the world body points (0x49B439…0x49B47D,
    /// overwritten by the first `Susp_update`), and the slice the frame reads: the tracker's after
    /// `updateZone(L)` (0x49B1F6) — the port takes the slice it was given (APPROXIMATION).
    #[allow(clippy::too_many_arguments)]
    pub fn place_at_slice(&mut self, grid: &TriangleGrid, specs: &CarSpecs, centre: Vec3, slice_fwd: Vec3, offset: Vec3, opponent: bool, flip: bool) {
        let absolute = opponent && self.game_tick > 0x1C0 && offset != Vec3::ZERO;
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
        self.ground_h = self.susp_update(grid, specs);
        self.position.y -= self.body_height;
        self.body_height = 0.0;
        self.ground_h = self.susp_update(grid, specs);
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
        self.ground_h = self.susp_update(grid, specs);
        self.position.y -= self.body_height;
        self.body_height = 0.0;
        for w in &mut self.wheels {
            w.compression = 0.0;
            w.airborne = false;
            w.contact = true;
        }
    }

    /// 0x40F740's countdown (0x40FB13…0x40FBA2, 32 Hz, every car but traffic): while it runs it
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
            self.reset_countdown = 0xC0;
        }
    }

    /// The alive count's part of 0x49E220, which every reset through 0x40F540 runs (0x40F694):
    /// 60 when the car was alive, 1 when it was dead (0x49E31E…0x49E3E5); and the smoothed
    /// steering it clears (0x49E361 / 0x49E367).
    pub fn rearm_after_reset(&mut self) {
        self.engine_alive = if self.engine_alive != 0 { 0x3C } else { 1 };
        self.steer_smoothed = [0.0; 2];
    }

    /// Re-orthonormalise the orientation (every 32 ticks in the original, 0x4A4810).
    pub fn renormalise(&mut self) {
        let z = self.orientation.row(2).normalize_or_zero();
        let x = self.orientation.row(1).cross(z).normalize_or_zero();
        let y = z.cross(x);
        self.orientation = Mat3::from_cols(x, y, z).transpose();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn brake_lamps_follow_the_latch() {
        // 0x43937A: the throttle byte while the reverse latch is set, the brake byte otherwise
        let specs = specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.7, 0.3);
        v.automatic = true;
        v.gear = 0;
        v.reverse_latch = true;
        (v.throttle, v.brake) = (0.0, 1.0); // swapped: the raw throttle key is down
        assert!(v.brake_lamp());
        v.reverse_latch = false; // the raw throttle is not the brake byte
        assert!(!v.brake_lamp());
    }

    use super::*;
    use crate::collision::{Triangle, TriangleGrid};

    /// `0x4A1472…0x4A14A6`: the half extents are half the span of the body points, per axis.
    #[test]
    fn the_half_extents_are_half_the_body_points_span() {
        let specs = specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.7, 0.3);
        v.body_points = vec![
            Vec3::new(-0.8, 0.0, -2.0),
            Vec3::new(0.9, 0.0, -2.0),
            Vec3::new(0.9, 1.2, 2.2),
            Vec3::new(-0.8, 1.2, 2.2),
        ];
        v.fit_half_extents();
        assert_eq!(v.half_extents, Vec3::new(0.85, 0.6, 2.1));
        // no points, no change — 0x4A13CD's loop runs `car+0x32C` times and that is zero
        let before = v.half_extents;
        v.body_points.clear();
        v.fit_half_extents();
        assert_eq!(v.half_extents, before);
    }

    /// A racer's body (0x445090 + 0x4421BE): the origin `car+0x330` goes `M[1]·car+0x3AC` above
    /// where the car was placed, the model is drawn back there (`R3DCar` slot 5 0x43A410), and
    /// the world points — `pos + Mᵀ·p` of the lowered points — are where they were.
    #[test]
    fn a_racers_origin_is_lifted_and_drawn_back_down() {
        let specs = specs_993();
        let tilt = Mat3::from_rotation_z(0.2).transpose();
        let mut v = Vehicle::new(Vec3::new(1.0, 2.0, 3.0), tilt, &specs, 0.7, 0.3);
        assert_eq!(v.draw_origin(), v.position, "no body yet: drawn at the origin");
        let ground = [Vec3::new(-0.7, 0.0, 1.2), Vec3::new(0.7, 0.0, 1.2), Vec3::new(-0.7, 0.0, -1.1), Vec3::new(0.7, 0.0, -1.1)];
        let before: Vec<Vec3> = ground.iter().map(|p| v.position + tilt.transpose() * *p).collect();
        let half = Vec3::new(0.9, 0.63, 2.1);
        v.set_car_body(ground.iter().map(|p| *p - Vec3::Y * half.y).collect(), half);
        assert!((v.position - (Vec3::new(1.0, 2.0, 3.0) + tilt.row(1) * 0.63)).length() < 1e-6);
        assert!((v.draw_origin() - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-6);
        for (p, b) in v.body_points.iter().zip(&before) {
            assert!((v.position + tilt.transpose() * *p - *b).length() < 1e-5);
        }
        // setting it again does not lift twice
        v.set_car_body(v.body_points.clone(), half);
        assert!((v.draw_origin() - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-6);
    }

    /// A plane tilted by `grade` (rise per run) about x, so +z is uphill.
    fn slope_grid(grade: f32) -> TriangleGrid {
        let s = 2000.0;
        let y = |z: f32| z * grade;
        let (a, b, c, d) = (
            Vec3::new(-s, y(-s), -s),
            Vec3::new(s, y(-s), -s),
            Vec3::new(s, y(s), s),
            Vec3::new(-s, y(s), s),
        );
        TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 100.0)
    }

    /// 0x49ED94…0x49EDE2's middle rung: over its target with the **wheels** turning far faster
    /// than the engine (`slip < [0x5B4A4C]` = −200), the revs step **up** by 200 — they do not
    /// snap to the wheel rpm, which is what this port did until 2026-09-23.
    #[test]
    fn over_target_steps_up_toward_the_wheels() {
        let specs = specs_993();
        let grid = flat_grid();
        let idle = Controls { throttle: 0, brake: 0, steer: 0, handbrake: false, command: 0 };
        let mut v = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.7, 0.3);
        v.race_started = true;
        v.ticks_alive = 0x100;
        // settle onto the plane: the arm only runs with the rear axle in contact
        for t in 0..64 {
            if t % 2 == 0 {
                v.step32(&idle, &specs, &grid);
            }
            v.step64(&specs);
        }
        // rolling fast in third with the throttle shut and the engine at a standstill
        v.gear = 3;
        v.velocity = Vec3::new(0.0, 0.0, 40.0);
        v.rpm = 0.0;
        v.step32(&idle, &specs, &grid);
        assert_eq!(v.engine_arm, 5, "over its (zero) target");
        assert!(v.rpm >= 199.0 && v.rpm <= 201.0, "one rung up, not a snap to the wheels: {}", v.rpm);
    }

    /// Diagnostic: how fast a 993 gets going from rest, on the flat and up a 14% climb.
    #[test]
    #[ignore]
    fn launch_profile() {
        for grade in [0.0f32, 0.14] {
            let specs = specs_993();
            let grid = slope_grid(grade);
            let mut v = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.7, 0.3);
            v.ai_driven = true;
            v.game_tick = 0x400;
            v.race_started = true;
            let c = Controls { throttle: 255, brake: 0, steer: 0, handbrake: false, command: 0 };
            for t in 0..(5 * 64) {
                if t % 2 == 0 {
                    v.step32(&c, &specs, &grid);
                }
                v.step64(&specs);
                if t % 32 == 0 {
                    println!("grade {grade:.2} t={:.1}s  {:5.1} km/h  rpm {:6.0} gear {} contact {} arm {}", t as f32 / 64.0, v.speed() * 3.6, v.rpm, v.gear, v.wheels.iter().filter(|w| w.contact).count(), v.engine_arm);
                }
            }
        }
    }

    fn flat_grid() -> TriangleGrid {
        let s = 2000.0;
        let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
        TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 100.0)
    }

    pub(super) fn specs_993() -> CarSpecs {
        // 993 Carrera 3.6 values from GameData/Simulation/CarData/993coupe36.sim (RE/PHYSICS.md)
        let mut sim = nfs_formats::sim::CarSim {
            name: "993".into(),
            mass_kg: 1370.0,
            wheelbase_mm: 2272.0,
            unknown_48: 0,
            induction: 0,
            gear_count: 6,
            unknown_54: 6,
            unknown_58_pct: 90.0,
            unknown_5c_pct: 0.0,
            reverse_ratio: -2.857,
            neutral: 0.0,
            gear_ratios: [3.818, 2.15, 1.56, 1.242, 1.024, 0.82],
            final_drive: 3.444,
            tyre_width_mm: 245.0,
            tyre_aspect: 45.0,
            rim_inches: 16.0,
            top_speed_mph: 168.0,
            redline_rpm: 6700.0,
            unknown_98_pct: 20.0,
            torque_curve: [122.0, 138.0, 150.0, 165.0, 201.0, 209.0, 209.0, 209.0, 224.0, 240.0, 243.0, 240.0, 232.0, 207.0, 161.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            unknown_tail: [0.0; 22],
        };
        sim.unknown_tail = [46.0, 1.9, 30.0, 16.5, 0.0, 9.5, 60.0, 1.0, 1.1, 77.5, 75.0, 1.0, 0.0, 110.0, 3.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        CarSpecs::from_sim(&sim, false)
    }

    fn run(v: &mut Vehicle, specs: &CarSpecs, grid: &TriangleGrid, c: Controls, seconds: f32) {
        let ticks = (seconds * 64.0) as u32;
        for t in 0..ticks {
            if t % 2 == 0 {
                v.step32(&c, specs, grid);
            }
            v.step64(specs);
            if t % 32 == 0 {
                v.renormalise();
            }
        }
    }

    #[test]
    fn derived_specs_match_notes() {
        let s = specs_993();
        assert!((s.circumference - 1.97).abs() < 0.05, "circumference {}", s.circumference);
        assert!((s.ratio_rpm_per_mps[2] - 3.444 * 3.818 * 60.0 / s.circumference).abs() < 1e-3);
        assert!((s.top_speed - 75.0).abs() < 0.1);
        assert!(s.drag > 0.0);
        assert_eq!(s.gear_slots, 8);
    }

    #[test]
    fn full_throttle_accelerates_and_shifts() {
        let specs = specs_993();
        let grid = flat_grid();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
        // settle (brake forced for the first 0x40 ticks)
        run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
        let c = Controls { throttle: 254, ..Default::default() };
        let mut t100 = None;
        for t in 0..(64 * 20) {
            if t % 2 == 0 {
                v.step32(&c, &specs, &grid);
            }
            v.step64(&specs);
            if t100.is_none() && v.velocity.length() >= 100.0 / 3.6 {
                t100 = Some(t as f32 / 64.0);
            }
        }
        let t100 = t100.expect("never reached 100 km/h");
        eprintln!("0-100 km/h in {t100:.2} s; after 20 s: {:.1} km/h gear {} rpm {:.0}", v.velocity.length() * 3.6, v.gear, v.rpm);
        assert!(t100 > 3.0 && t100 < 9.0, "0-100 in {t100}");
        assert!(v.gear >= 5);
        assert!(v.velocity.z > 0.0 && v.velocity.x.abs() < 1.0 && v.position.y < 1.0);
    }

    #[test]
    fn steering_turns_the_car() {
        let specs = specs_993();
        let grid = flat_grid();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
        run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
        run(&mut v, &specs, &grid, Controls { throttle: 254, ..Default::default() }, 5.0);
        let heading0 = v.forward();
        run(&mut v, &specs, &grid, Controls { throttle: 120, steer: 126, ..Default::default() }, 3.0);
        let heading1 = v.forward();
        let turn = heading0.cross(heading1).y;
        eprintln!("turn (cross.y) after 3 s of right steer: {turn:.3}, speed {:.1} km/h", v.speed() * 3.6);
        assert!(turn.abs() > 0.2, "car did not turn");
        // positive steer turns the nose toward body +x (cross.y > 0). Whether +x is screen-right
        // depends on the model frame (RE/notes/car-model.md: +X = car's left) — the input layer maps it.
        assert!(turn > 0.0, "positive steer should turn toward +x (cross.y = {turn})");
    }
}

/// Car–car contact `0x498FF0` → `0x4980D0` → impulse `0x496860` (`RE/notes/suspension-collision.md`
/// §6.4, HIGH for the linear impulse). The bodies are tested as oriented boxes in the ground plane
/// (separating axes = both cars' right/forward axes; the original's 9-axis OBB test is 3-D, MEDIUM);
/// the contact normal is the axis of least penetration pointing from `a` to `b`.
///
/// Impulse: `vrel = (vA − vB)·n`, `j = vrel / (½·invM_A + ½·invM_B)`, `j *= ½`, applied as
/// `A.vel += n·j·invM_A` / `B.vel −= n·j·invM_B` (× 1.25 for hard hits, effect ≥ 80). The angular
/// terms of the original use the rigid-body inertia this model does not carry; the yaw response is
/// the wall-hit heuristic instead (MEDIUM). The bodies are then pushed apart along the centre line
/// by the penetration depth. Returns the effect magnitude when a contact was resolved.
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

/// The broad phase in front of the pair test (0x498FF0, 0x499077…0x499101): each of `|Δy|`, `|Δx|`
/// and `|Δz|` under the sum of the two bounding radii `car+0x3B4` — `|car+0x3A8…+0x3B0|`, the half
/// extents' length: `RigidBody_init` 0x49B510 stores it at 0x49B5CD in every `Car_reset`, over
/// the load-time `√(|h|² + h_z²)` of 0x4451A0 (`car-model.md` §12).
fn near(a: &Vehicle, b: &Vehicle) -> bool {
    let r = a.half_extents.length() + b.half_extents.length();
    let d = (b.position - a.position).abs();
    d.y < r && d.x < r && d.z < r
}

/// The face centres `0x496FD0` samples, in its order (0x497006…0x497103).
const FACES: [Vec3; 6] = [Vec3::X, Vec3::Y, Vec3::Z, Vec3::NEG_X, Vec3::NEG_Y, Vec3::NEG_Z];
/// And the corners (0x497113…0x4971EB, 0x4975EE…0x4976C6).
const CORNERS: [Vec3; 8] = [
    Vec3::new(1.0, 1.0, 1.0),
    Vec3::new(1.0, 1.0, -1.0),
    Vec3::new(1.0, -1.0, 1.0),
    Vec3::new(1.0, -1.0, -1.0),
    Vec3::new(-1.0, 1.0, 1.0),
    Vec3::new(-1.0, 1.0, -1.0),
    Vec3::new(-1.0, -1.0, 1.0),
    Vec3::new(-1.0, -1.0, -1.0),
];

/// `0x496FD0(A, B, &point, &normal)` — whether the two bodies' boxes (`car+0x330` centred, rows
/// `car+0x364…`, half extents `car+0x3A8…`) overlap, and if so where and along what: **28 point
/// tests**, the first hit deciding.
///
/// 1. B's six face centres against A (0x495600): the point is the face centre, the normal B's
///    face normal.
/// 2. A's six against B (0x495890): A's face normal **negated** (0x4959DC).
/// 3. B's eight corners against A (0x495B30), then 4. A's eight against B (0x495D30): the point
///    is the corner; the normal is one of the other box's axes. When the bodies' velocities
///    differ by more than 15 m/s on some axis (`[0x5B243C]`, 0x4971FB…0x497297) it is the axis —
///    each turned to face away from the corner's side — most aligned with the corner owner's
///    velocity (0x4972A7…0x4974C3); otherwise the axis of least penetration `h − |q|`, turned the
///    same way (0x4974C4…0x4975EC, 0x497ACD).
///
/// A point is inside when every coordinate is within the half extent, both ends inclusive. The
/// normal always points from B toward A, which is the way `0x496860` wants it.
pub(crate) fn box_contact(a: &Vehicle, b: &Vehicle) -> Option<(Vec3, Vec3)> {
    let ra = [a.orientation.row(0), a.orientation.row(1), a.orientation.row(2)];
    let rb = [b.orientation.row(0), b.orientation.row(1), b.orientation.row(2)];
    let (ha, hb) = (a.half_extents, b.half_extents);
    let d = b.position - a.position;
    // a point of B's box (`s` in B's unit coordinates) in A's frame, and the other way round
    let b_in_a = |s: Vec3| -> Vec3 {
        let p = rb[0] * (hb.x * s.x) + rb[1] * (hb.y * s.y) + rb[2] * (hb.z * s.z) + d;
        Vec3::new(p.dot(ra[0]), p.dot(ra[1]), p.dot(ra[2]))
    };
    let a_in_b = |s: Vec3| -> Vec3 {
        let p = ra[0] * (ha.x * s.x) + ra[1] * (ha.y * s.y) + ra[2] * (ha.z * s.z) - d;
        Vec3::new(p.dot(rb[0]), p.dot(rb[1]), p.dot(rb[2]))
    };
    let inside = |q: Vec3, h: Vec3| q.x.abs() <= h.x && q.y.abs() <= h.y && q.z.abs() <= h.z;
    let world = |pos: Vec3, r: &[Vec3; 3], h: Vec3, s: Vec3| pos + r[0] * (h.x * s.x) + r[1] * (h.y * s.y) + r[2] * (h.z * s.z);
    for s in FACES {
        if inside(b_in_a(s), ha) {
            return Some((world(b.position, &rb, hb, s), rb[0] * s.x + rb[1] * s.y + rb[2] * s.z));
        }
    }
    for s in FACES {
        if inside(a_in_b(s), hb) {
            return Some((world(a.position, &ra, ha, s), -(ra[0] * s.x + ra[1] * s.y + ra[2] * s.z)));
        }
    }
    // the corner's normal: `axes` the other box's rows, `q` the corner in its frame, `flip` the
    // side rule (B's corner: an axis is turned where `q > 0`; A's: where `q < 0`), `v` the corner
    // owner's velocity
    let fast = {
        let dv = (b.velocity - a.velocity).abs();
        dv.max_element() > 15.0
    };
    let corner_normal = |axes: &[Vec3; 3], q: Vec3, h: Vec3, flip: &dyn Fn(f32) -> bool, v: Vec3| -> Vec3 {
        let q = q.to_array();
        let turned: [Vec3; 3] = std::array::from_fn(|i| if flip(q[i]) { -axes[i] } else { axes[i] });
        if fast {
            let dots: [f32; 3] = std::array::from_fn(|i| turned[i].dot(v).abs());
            let k = if dots[0] > dots[1] && dots[0] > dots[2] {
                0
            } else if dots[1] > dots[2] {
                1
            } else {
                2
            };
            turned[k]
        } else {
            let h = h.to_array();
            let p: [f32; 3] = std::array::from_fn(|i| h[i] - q[i].abs());
            let k = if p[0] < p[1] && p[0] < p[2] {
                0
            } else if p[1] < p[2] {
                1
            } else {
                2
            };
            turned[k]
        }
    };
    for s in CORNERS {
        let q = b_in_a(s);
        if inside(q, ha) {
            return Some((world(b.position, &rb, hb, s), corner_normal(&ra, q, ha, &|x| x > 0.0, b.velocity)));
        }
    }
    for s in CORNERS {
        let q = a_in_b(s);
        if inside(q, hb) {
            return Some((world(a.position, &ra, ha, s), corner_normal(&rb, q, hb, &|x| x < 0.0, a.velocity)));
        }
    }
    None
}

/// `0x497B10` runs its separation up to this many times.
const SEPARATION_PASSES: usize = 0x20;

/// `0x497B10(A, B, &point, &normal)`: up to 32 passes (0x497D42…0x4980B7) of `0x496FD0`; while
/// the boxes overlap both bodies move by the pass's push, and the first overlap fixes the push
/// (0x497E03): with a horizontal closing speed `|vA − vB|` over **60 m/s** (`[0x5B49CC]` = 3600 on
/// the squares) and either the centres within √0.05 of each other or the closing direction
/// within `acos 0.85` of the line to B, each body backs up along its **own** velocity by
/// `v / (32 · |vA − vB|)` (0x497E4B…0x497FC0); otherwise the two move apart along the line of
/// centres, A by `mB / (mA + mB) / 32`, B by `mA / (mA + mB) / 32` (0x497FC5…0x498079). The
/// contact is the last overlapping pass's; none when the first pass finds the boxes clear.
fn separate(a: &mut Vehicle, mass_a: f32, b: &mut Vehicle, mass_b: f32) -> Option<(Vec3, Vec3)> {
    let mut contact = None;
    let (mut push_a, mut push_b) = (Vec3::ZERO, Vec3::ZERO);
    for _ in 0..SEPARATION_PASSES {
        let Some(c) = box_contact(a, b) else { break };
        if contact.is_none() {
            let dv = a.velocity - b.velocity;
            let d = b.position - a.position;
            let along = if dv.x * dv.x + dv.z * dv.z > 3600.0 {
                if d.x * d.x + d.z * d.z < 0.05 {
                    true
                } else {
                    d.normalize_or_zero().dot(dv.normalize_or_zero()) > 0.85
                }
            } else {
                false
            };
            if along {
                let s = dv.length() * 0.5;
                let k = -(if s == 0.0 { 1.0 } else { 0.5 / s });
                push_a = a.velocity * k * 0.031_25;
                push_b = b.velocity * k * 0.031_25;
            } else {
                let n = d.normalize_or_zero();
                let sum = mass_a + mass_b;
                push_a = n * (mass_b / sum) * -0.031_25;
                push_b = n * (mass_a / sum) * 0.031_25;
            }
        }
        contact = Some(c);
        a.position += push_a;
        b.position += push_b;
        for w in &mut a.wheels {
            w.world_pos += push_a;
        }
        for w in &mut b.wheels {
            w.world_pos += push_b;
        }
    }
    contact
}

/// `CarPair_collide` 0x4980D0 without the walls: see [`collide_cars_with`].
pub fn collide_cars(a: &mut Vehicle, mass_a: f32, b: &mut Vehicle, mass_b: f32) -> Option<f32> {
    collide_cars_with(a, mass_a, b, mass_b, &mut |_| {})
}

/// `CarPair_collide` 0x4980D0: the separation (`0x497B10`); only when the boxes overlapped,
/// `CarCarImpulse` 0x496860 at the contact point and normal it left (`RE/notes/traffic.md` §21.6),
/// then each car's walls (`walls`, 0x495020) — A's, then B's (0x49810E…0x498126); then up to **four**
/// more rounds (`ebx = 4`, 0x49810F): separate again, and while the boxes still overlap and the
/// impulse still acts (0x496860 returns 0 on a separating pair, 0x498164), the walls again
/// (0x49816C…0x49817F). The answer is the last impulse's effect (`car+0x430` is overwritten by each).
/// Not carried: the props test 0x498DE0 beside each wall call (this port's props are the skill
/// test's cones, handled on their own).
pub fn collide_cars_with(a: &mut Vehicle, mass_a: f32, b: &mut Vehicle, mass_b: f32, walls: &mut dyn FnMut(&mut Vehicle)) -> Option<f32> {
    collide_pair(a, mass_a, b, mass_b, walls).map(|(ea, eb)| ea.max(eb))
}

/// [`collide_cars_with`], answering each body's own impact `car+0x430` from the last impulse
/// (A's, B's) — what each car's post-impact pass 0x410280 then reads for itself. Both are 0 when
/// the boxes overlapped but the first impulse found the pair separating.
pub fn collide_pair(a: &mut Vehicle, mass_a: f32, b: &mut Vehicle, mass_b: f32, walls: &mut dyn FnMut(&mut Vehicle)) -> Option<(f32, f32)> {
    if !near(a, b) {
        return None;
    }
    let (pt, ni) = separate(a, mass_a, b, mass_b)?;
    // the first impulse's answer is not tested (0x498109 → 0x49810E): the walls follow regardless
    let mut effect = pair_impulse(a, mass_a, b, mass_b, pt, ni).unwrap_or((0.0, 0.0));
    walls(a);
    walls(b);
    for _ in 0..4 {
        let Some((pt, ni)) = separate(a, mass_a, b, mass_b) else { break };
        let Some(e) = pair_impulse(a, mass_a, b, mass_b, pt, ni) else { break };
        effect = e;
        walls(a);
        walls(b);
    }
    // ClampAngVel 0x4964D0 once the pair is done — 0x498FF0 calls it for both cars when 0x4980D0
    // returns (0x499152 / 0x499158)
    let clamp = |w: Vec3, lim: f32| w.clamp(Vec3::splat(-lim), Vec3::splat(lim));
    a.angular_velocity = clamp(a.angular_velocity, a.ang_limit());
    b.angular_velocity = clamp(b.angular_velocity, b.ang_limit());
    Some(effect)
}

/// `CarCarImpulse` 0x496860 at `pt` along `ni` (from B toward A); `None` when the pair is
/// separating (`j < 0`, 0x4969DF) and nothing happens. The answer is (A's, B's) `car+0x430`.
///
/// The masses are `car+0x354`. Every branch that compares one with a constant — 0.65, 0.75, 3.0,
/// and the `4 − mA` below — is in the original's units (kg / 128); a pair of cars given in kg
/// never meets any of them, which is why the racers' pairs can pass kg. A pair with a smackable
/// has to pass the original's units for both.
///
/// The smackable (`car+0x52C & 0x80`) branches, read 2026-09-25:
/// * 0x496985…0x4969DB: on a smackable's **first** contact (`car+0x7C == 0`) `j` is made
///   non-negative (`fchs` when below 0) and `car+0x7C = 1` — so the first touch always pushes.
/// * 0x4969F8…0x496A2B: a contact that goes on (`j ≥ 0`) marks a smackable whose class
///   `car+0x44C` is **0x10** (a pylon) as hit, `car+0x42C = 1`.
/// * 0x496ADD…0x496B42 / 0x496B97…0x496BEF: a smackable lighter than **3.0** takes
///   `car+0x430 = min((4 − mA)·S, |P|·invM)` — **`mA` for both sides** (0x496B1F and 0x496BCC both
///   read `[esi+0x354]`, A's mass), where the other rule is `mOther > 0.65 ? min(S, |P|·invM) : 9`.
/// * 0x496DDB…0x496E08 / 0x496E83…0x496EA6: a smackable lighter than **0.75** turns with
///   `1.5 / m` instead of 3.0 / 1.5.
///
/// And for every body (0x496E0A…0x496E25, 0x496EAC…0x496EC7): the angular factor is 3.0 only when
/// `car+0x430` is **above** 80 (`test ah, 0x41` — at exactly 80 it is 1.5), while the linear
/// ×1.25 takes 80 itself (0x496C4A…0x496C5A, `test ah, 1`). The port had `≥ 80` for both.
fn pair_impulse(a: &mut Vehicle, mass_a: f32, b: &mut Vehicle, mass_b: f32, pt: Vec3, ni: Vec3) -> Option<(f32, f32)> {
    // `CarCarImpulse` 0x496860, symmetric and with no special case for traffic; the solver works in
    // rad/s, so the stored turns/s angular velocities are scaled by 2π around it exactly as 0x498FF0
    // does. `ni` points from B toward A.
    let n = -ni;
    // `car+0x358` = 1/mass (0x49B652); the port's kg masses are never below 1
    let (inv_a, inv_b) = (1.0 / mass_a.max(1e-6), 1.0 / mass_b.max(1e-6));
    let (inv6_a, inv6_b) = (inv_a / 6.0, inv_b / 6.0);
    let (r_a, r_b) = (pt - a.position, pt - b.position);
    let (c_a, c_b) = (r_a.cross(ni), r_b.cross(ni));
    let (w_a, w_b) = (a.angular_velocity * TAU, b.angular_velocity * TAU);
    let vrel = b.velocity.dot(ni) - a.velocity.dot(ni) - w_a.dot(c_a) + w_b.dot(c_b);
    let den = 0.5 * (c_a.length_squared() * inv6_a + c_b.length_squared() * inv6_b + inv_b + inv_a);
    let mut j = if den > 1e-9 { vrel / den } else { 0.0 };
    // a smackable's first contact (0x496985 / 0x4969BC)
    for body in [&mut *a, &mut *b] {
        if body.smackable && !body.disturbed {
            j = j.abs();
            body.disturbed = true;
        }
    }
    if j < 0.0 {
        return None;
    }
    // a pylon that is struck is down for the count (0x496A08 / 0x496A23)
    for body in [&mut *a, &mut *b] {
        if body.smackable && body.material_class == 0x10 {
            body.knocked_over = true;
        }
    }
    {
        let jj = j * 0.5;
        let imp = ni * jj;
        // impact magnitude 0x430: `min(4·|Δv|, |vclose·(mA+mB)|/m)`, floored at 9 when the other body
        // is lighter than 0.65 mass units; a smackable under 3.0 has its own rule
        let vclose = a.velocity.dot(ni) - b.velocity.dot(ni);
        let p = vclose * (mass_a + mass_b);
        let sp = 4.0 * (a.velocity - b.velocity).length();
        let own = |body: &Vehicle, m: f32, inv: f32, other: f32| -> f32 {
            if body.smackable && m < 3.0 {
                ((4.0 - mass_a) * sp).min(p.abs() * inv)
            } else if other > 0.65 {
                sp.min(p.abs() * inv)
            } else {
                9.0
            }
        };
        let eff_a = own(a, mass_a, inv_a, mass_b);
        let eff_b = own(b, mass_b, inv_b, mass_a);
        a.velocity += imp * inv_a * if eff_a >= 80.0 { 1.25 } else { 1.0 };
        b.velocity -= imp * inv_b * if eff_b >= 80.0 { 1.25 } else { 1.0 };
        let turn = |body: &Vehicle, m: f32, eff: f32| -> f32 {
            if body.smackable && m < 0.75 {
                1.5 / m
            } else if eff > 80.0 {
                3.0
            } else {
                1.5
            }
        };
        let (t_a, t_b) = (turn(a, mass_a, eff_a), turn(b, mass_b, eff_b));
        let w_a = w_a + r_a.cross(imp) * (t_a * inv6_a);
        let w_b = w_b - r_b.cross(imp) * (t_b * inv6_b);
        // back to turns/s; `Damage`'s crash response (0x496F8E / 0x496FB9) runs on those, and
        // the clamp waits for the whole pair (`collide_cars_with`). The damage normals keep this port's convention
        // (A's from A toward B); which vector 0x496F38 hands 0x494DF0 was not settled — MEDIUM.
        a.angular_velocity = w_a / TAU;
        b.angular_velocity = w_b / TAU;
        // 0x496F1E / 0x496F25: `word car+0x418` steps for both bodies
        a.airborne_count = a.airborne_count.wrapping_add(1);
        b.airborne_count = b.airborne_count.wrapping_add(1);
        a.impact = a.impact.max(eff_a);
        b.impact = b.impact.max(eff_b);
        a.damage_by_normal(eff_a, n, 2, mass_a);
        b.damage_by_normal(eff_b, -n, 2, mass_b);
        Some((eff_a, eff_b))
    }
}

#[cfg(test)]
mod car_car_tests {
    use super::*;

    fn car(pos: Vec3, vel: Vec3, yaw: f32) -> Vehicle {
        let specs = super::tests::specs_993();
        let rot = Mat3::from_rotation_y(yaw).transpose();
        let mut v = Vehicle::new(pos, rot, &specs, 0.8, 0.3);
        v.velocity = vel;
        v
    }

    /// `0x497B10`: each pass moves the pair 1/32 m apart along the line of centres, split by mass,
    /// until `0x496FD0` finds the boxes clear — at most 32 passes, so a deep interlock is not
    /// cleared in one call.
    #[test]
    fn separation_moves_a_thirty_second_a_pass() {
        let mut a = car(Vec3::ZERO, Vec3::ZERO, 0.0);
        let x = a.half_extents.x * 2.0 - 0.05;
        let mut b = car(Vec3::new(x, 0.0, 0.0), Vec3::ZERO, 0.0);
        assert!(box_contact(&a, &b).is_some(), "the pair has to start overlapping");
        let before = b.position.x - a.position.x;
        collide_cars(&mut a, 1.0, &mut b, 3.0);
        assert!(box_contact(&a, &b).is_none());
        let moved = (b.position.x - a.position.x) - before;
        // two passes of 1/32 clear a 5 cm overlap; A (the lighter) takes three quarters of each
        assert!((moved - 2.0 / 32.0).abs() < 1e-4, "moved {moved}");
        assert!((a.position.x + 2.0 * 0.75 / 32.0).abs() < 1e-4, "{:?}", a.position);
    }

    /// `0x496FD0`'s first hit is a face centre of B inside A, with B's face normal — here B's −x
    /// face, pointing from B toward A.
    #[test]
    fn box_contact_samples_faces_first() {
        let a = car(Vec3::ZERO, Vec3::ZERO, 0.0);
        let x = a.half_extents.x * 2.0 - 0.05;
        let b = car(Vec3::new(x, 0.0, 0.0), Vec3::ZERO, 0.0);
        let (p, n) = box_contact(&a, &b).unwrap();
        assert!((n - Vec3::NEG_X).length() < 1e-5, "{n:?}");
        assert!((p.x - (x - b.half_extents.x)).abs() < 1e-5, "{p:?}");
    }

    #[test]
    fn head_on_equal_mass_shares_velocity_and_separates() {
        let mut a = car(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 8.0), 0.0);
        let mut b = car(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -8.0), 0.0);
        let hit = collide_cars(&mut a, 1000.0, &mut b, 1000.0);
        assert!(hit.is_some());
        // j = vrel/(invM) * 0.5 with equal masses: each car loses half the closing speed along n
        assert!((a.velocity.z - 0.0).abs() < 1e-3, "{:?}", a.velocity);
        assert!((b.velocity.z - 0.0).abs() < 1e-3, "{:?}", b.velocity);
        assert!(b.position.z - a.position.z > 3.0, "pushed apart");
    }

    #[test]
    fn separated_boxes_do_not_touch() {
        let mut a = car(Vec3::ZERO, Vec3::Z, 0.0);
        let mut b = car(Vec3::new(4.0, 0.0, 0.0), Vec3::ZERO, 0.0);
        assert!(collide_cars(&mut a, 1000.0, &mut b, 1000.0).is_none());
        // side by side just outside the widths
        let mut c = car(Vec3::new(2.5, 0.0, 0.0), Vec3::ZERO, 0.0);
        assert!(collide_cars(&mut a, 1000.0, &mut c, 1000.0).is_none());
    }

    #[test]
    fn the_impulse_conserves_momentum_and_clamps_the_spin() {
        let (ma, mb) = (2000.0f32, 1000.0f32);
        let mut a = car(Vec3::ZERO, Vec3::new(0.0, 0.0, 12.0), 0.0);
        let mut b = car(Vec3::new(0.6, 0.0, 3.0), Vec3::new(0.0, 0.0, 2.0), 0.4);
        a.angular_velocity = Vec3::new(0.0, 0.3, 0.0);
        let p0 = a.velocity * ma + b.velocity * mb;
        assert!(collide_cars(&mut a, ma, &mut b, mb).is_some());
        let p1 = a.velocity * ma + b.velocity * mb;
        // below the 80 "hard hit" threshold both sides take the same impulse, so momentum is conserved
        assert!((p1 - p0).length() < 1.0, "{p0:?} -> {p1:?}");
        // and each angular-velocity component stays inside ClampAngVel's ±0.75 turns/s
        for w in [a.angular_velocity, b.angular_velocity] {
            assert!(w.abs().max_element() <= 0.75 + 1e-6, "{w:?}");
        }
    }

    #[test]
    fn heavier_car_moves_less() {
        let mut a = car(Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0), 0.0);
        let mut b = car(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, 0.0);
        collide_cars(&mut a, 2000.0, &mut b, 1000.0);
        assert!(a.velocity.z > 5.0 && b.velocity.z > 0.0 && b.velocity.z < 10.0, "{:?} {:?}", a.velocity, b.velocity);
    }
}

#[cfg(test)]
mod damage_tests {
    use super::*;

    #[test]
    fn zone_from_normal_and_ring_propagation() {
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        // hitting the front (normal points backwards into the car → z < −0.1 in the body frame)
        v.damage_by_normal(60.0, Vec3::new(0.0, 0.0, -1.0), 0, 1.0);
        assert_eq!(v.damage[1], 30.0, "front zone takes min(60·0.5, 100)");
        assert_eq!(v.damage[0], 15.0, "ring neighbours get the mean of their neighbours");
        assert_eq!(v.damage[2], 15.0);
        assert_eq!(v.damage[5], 0.0);
        // small effects do nothing
        v.damage_by_normal(5.0, Vec3::new(0.0, 0.0, 1.0), 0, 1.0);
        assert_eq!(v.damage[5], 0.0);
        // roof
        v.damage_by_normal(300.0, Vec3::new(0.0, -1.0, 0.0), 0, 1.0);
        assert_eq!(v.damage[8], 100.0);
        assert!((v.damage_avg() - (30.0 + 15.0 + 15.0 + 100.0) / 10.0).abs() < 1e-4);
        assert!(v.part_damage(&specs.zone_masks.brakes) > 0.0);
        assert_eq!(v.part_damage(&[false; 10]), 0.0);
    }
}

#[cfg(test)]
mod body_ground_tests {
    use super::*;

    /// A car dropped upside down onto flat ground is pushed out of the surface, loses its downward
    /// speed and has its angular velocity clamped (`BodyGroundCollide` 0x496610 / `ClampAngVel`).
    #[test]
    fn upside_down_car_is_pushed_out_and_settles() {
        let specs = tests::specs_993();
        let flipped = Mat3::from_rotation_z(std::f32::consts::PI).transpose();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.2, 0.0), flipped, &specs, 0.8, 0.3);
        v.velocity = Vec3::new(0.0, -8.0, 0.0);
        v.angular_velocity = Vec3::new(3.0, -4.0, 5.0);
        v.cos_tilt = -1.0;
        v.body_height = 0.0;
        let plane = Vec3::ZERO;
        v.body_ground_collide_mass(specs.mass, Vec3::Y, plane, 0.5);
        assert!(v.velocity.y > -8.0, "downward speed reduced: {:?}", v.velocity);
        assert!(v.position.y >= 0.2, "pushed out of the ground: {}", v.position.y);
        assert!(v.angular_velocity.abs().max_element() <= 0.75 + 1e-6, "ω clamped: {:?}", v.angular_velocity);
        assert!(v.impact > 0.0);
        // a light landing leaves too little residual normal speed to damage the body
        // (`Damage_byNormal` needs an effect > 10, i.e. `4·|vel·n| > 10`)
        assert!(v.damage.iter().all(|d| *d == 0.0));
    }

    /// A hard drop onto the roof does damage the roof zone.
    #[test]
    fn hard_roof_impact_damages_the_roof() {
        let specs = tests::specs_993();
        let flipped = Mat3::from_rotation_z(std::f32::consts::PI).transpose();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.2, 0.0), flipped, &specs, 0.8, 0.3);
        v.velocity = Vec3::new(0.0, -30.0, 0.0);
        v.cos_tilt = -1.0;
        v.body_height = 0.0;
        v.body_ground_collide_mass(specs.mass, Vec3::Y, Vec3::ZERO, 0.5);
        assert!(v.damage[8] > 0.0, "roof zone damaged: {:?}", v.damage);
    }

    /// `Damage`'s crash response (0x49481A): a heavy wall hit on the front hops the car
    /// (`a·0.125/3` on the velocity's `y`, doubled for `.TINYCAR`) and pitches it back; the
    /// body-ground contact (flag 0) does neither; a light hit only knocks the spin.
    #[test]
    fn crash_response_follows_the_flag() {
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.damage_by_normal(200.0, Vec3::new(0.0, 0.0, -1.0), 0, 1.0);
        assert_eq!(v.velocity, Vec3::ZERO);
        assert_eq!(v.angular_velocity, Vec3::ZERO);
        v.damage_by_normal(200.0, Vec3::new(0.0, 0.0, -1.0), 1, 1.0);
        assert!((v.velocity.y - 200.0 * 0.125 / 3.0).abs() < 1e-3, "{:?}", v.velocity);
        // zone 1 (front): pitch −m·k/2 with k = min(25/32, 2), m = 1.2, then clamped to 0.75
        let k = 25.0f32 / 32.0;
        assert!((v.angular_velocity.x + 1.2 * k * 0.5).abs() < 1e-4, "{:?}", v.angular_velocity);
        assert!((v.angular_velocity.y + 1.2 * k * 0.5).abs() < 1e-4);
        let mut t = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
        t.tiny = true;
        t.damage_by_normal(200.0, Vec3::new(0.0, 0.0, -1.0), 1, 1.0);
        assert!((t.velocity.y - 400.0 * 0.125 / 3.0).abs() < 1e-3);
        let mut l = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
        l.damage_by_normal(20.0, Vec3::new(0.0, 0.0, 1.0), 1, 1.0);
        assert_eq!(l.velocity, Vec3::ZERO);
        // zone 5 (rear): pitch +k/2, yaw −k/2 with k = min(20/32, 0.33)
        assert!((l.angular_velocity.x - 0.165).abs() < 1e-5, "{:?}", l.angular_velocity);
        assert!((l.angular_velocity.y + 0.165).abs() < 1e-5);
    }

    /// 0x40F740's countdown: a heavy hit starts it; an upright live car stops it; a dead car runs
    /// it out and is reset, and the reset re-arms the alive count at 1.
    #[test]
    fn crash_countdown_and_dead_car() {
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.cos_tilt = 1.0;
        v.damage_by_normal(100.0, Vec3::new(0.0, 0.0, -1.0), 1, 1.0);
        assert_eq!(v.reset_countdown, 0xC0);
        assert!(v.engine_alive != 0, "100 does not kill");
        assert!(!v.reset_countdown_tick(false));
        assert_eq!(v.reset_countdown, 0, "upright and alive: stopped");
        v.damage_by_normal(130.0, Vec3::new(0.0, 0.0, -1.0), 1, 1.0);
        assert_eq!(v.engine_alive, 0, "past 120: dead");
        let mut n = 0;
        while !v.reset_countdown_tick(false) {
            n += 1;
            assert!(n < 0xC0);
        }
        assert_eq!(n, 0xBF);
        v.reset_to(Vec3::ZERO, Mat3::IDENTITY);
        assert_eq!(v.engine_alive, 1);
        v.reset_countdown_start(Vec3::Y, false);
        assert_eq!(v.reset_countdown, 0, "upright on the slice: no restart");
        v.orientation = Mat3::from_rotation_z(std::f32::consts::FRAC_PI_2);
        v.reset_countdown_start(Vec3::Y, false);
        assert_eq!(v.reset_countdown, 0xC0, "on its side: started");
    }

    /// 0x40F7EB's gates: not before tick 0x301, not at 27.6 on `car+0x35C`, not once finished, and
    /// a request that goes through holds the next one off for 0xA0 passes.
    #[test]
    fn reset_request_gates_and_hold() {
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        assert!(!v.reset_request(true, 0x300));
        v.velocity = Vec3::new(27.6, 0.0, 0.0);
        assert!(!v.reset_request(true, 0x400));
        // the estimate of (20, 20) is 25, under 27.6, though the true speed is 28.3
        v.velocity = Vec3::new(20.0, 0.0, 20.0);
        assert!(v.reset_request(true, 0x400));
        assert_eq!(v.reset_hold, 0x9F);
        for _ in 0..0x9F {
            assert!(!v.reset_request(true, 0x400));
        }
        assert!(v.reset_request(true, 0x400), "the hold has run out");
        v.race_state = 2;
        v.reset_hold = 0;
        assert!(!v.reset_request(true, 0x400), "finished");
    }

    /// No body point below the plane: nothing happens.
    #[test]
    fn airborne_car_is_untouched() {
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::new(0.0, 10.0, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.velocity = Vec3::new(1.0, -2.0, 3.0);
        let before = (v.position, v.velocity, v.angular_velocity);
        v.body_ground_collide_mass(specs.mass, Vec3::Y, Vec3::ZERO, 0.5);
        assert_eq!((v.position, v.velocity, v.angular_velocity), before);
    }
}

#[cfg(test)]
mod roughness_tests {
    use super::*;

    #[test]
    fn roughness_by_surface_and_tilt() {
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.cos_tilt = 1.0;
        // smooth surfaces (asphalt = 1) have no roughness at all
        v.wheels[0].surface = 1;
        for _ in 0..200 {
            assert_eq!(v.roughness(0, 30.0), 0.0);
        }
        // a rough surface (3) draws offsets within ±amplitude (0.1 m) and varies over distance
        v.wheels[1].surface = 3;
        let mut seen: Vec<f32> = Vec::new();
        for _ in 0..200 {
            let r = v.roughness(1, 30.0);
            assert!(r.abs() <= 0.1 + 1e-6, "amplitude {r}");
            seen.push(r);
        }
        assert!(seen.iter().any(|r| *r != 0.0), "some offset is produced");
        // a tilted car gets none (cosTilt < 0.86)
        v.cos_tilt = 0.5;
        assert_eq!(v.roughness(1, 30.0), 0.0);
    }

    #[test]
    fn drafting_only_behind_a_close_car_at_speed() {
        // §4.3 / 0x49D9B0: nothing below 15 m/s, nothing outside v/3 metres or 7.7 degrees, and at
        // most half the drag
        let fwd = Vec3::Z;
        let me = Vec3::ZERO;
        assert_eq!(draft_factor(me, fwd, 10.0, &[Vec3::new(0.0, 0.0, 5.0)]), 1.0, "too slow");
        assert_eq!(draft_factor(me, fwd, 30.0, &[Vec3::new(0.0, 0.0, 15.0)]), 1.0, "the car is 15 m away, the window is v/3 = 10");
        assert_eq!(draft_factor(me, fwd, 30.0, &[Vec3::new(5.0, 0.0, 5.0)]), 1.0, "45 degrees off the heading");
        let close = draft_factor(me, fwd, 30.0, &[Vec3::new(0.0, 0.0, 2.0)]);
        assert!(close < 1.0 && close >= 0.5, "drag cut to {close}");
        // directly behind is no help at all
        assert_eq!(draft_factor(me, fwd, 30.0, &[Vec3::new(0.0, 0.0, -2.0)]), 1.0);
    }

    #[test]
    fn the_turbo_spools_and_bleeds() {
        // 0x49E6C4: a turbo rises 0.05 a tick toward (rpm - redline/4)/(redline/2), capped by
        // rpm/redline/2 x throttle, and falls the same way
        let mut specs = tests::specs_993();
        specs.induction = 1;
        specs.redline = 6000.0;
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.throttle = 1.0;
        v.rpm = 5000.0;
        for _ in 0..40 {
            v.boost_update(&specs);
        }
        let held = v.boost;
        assert!(held > 0.4 && held <= 1.0, "spooled to {held}");
        // off the throttle it falls back
        v.throttle = 0.0;
        for _ in 0..40 {
            v.boost_update(&specs);
        }
        assert!(v.boost < 0.01, "bled to {}", v.boost);
        // a supercharger rises at once and falls by 0.1
        specs.induction = 2;
        v.throttle = 1.0;
        v.rpm = 5000.0;
        v.boost_update(&specs);
        assert!(v.boost > 0.4, "instant: {}", v.boost);
        // and a naturally aspirated engine never boosts
        specs.induction = 0;
        v.boost = 0.0;
        v.boost_update(&specs);
        assert_eq!(v.boost, 0.0);
    }

    #[test]
    fn a_damaged_engine_misfires() {
        // §4.3: `misfire = engineDamage/300 > rand01`, and a misfire over the rev target kills the
        // drive
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        // an undamaged engine never misfires, whatever the draw
        v.misfire_roll = 0.0;
        assert_eq!(v.part_damage(&specs.zone_masks.engine), 0.0);
        // a wrecked one misfires on a low draw and not on a high one
        v.damage = [300.0; 10];
        assert!(v.part_damage(&specs.zone_masks.engine) / 300.0 > 0.0);
        let dmg = v.part_damage(&specs.zone_masks.engine) / 300.0;
        assert!(dmg > 0.5 && dmg <= 1.0, "engine damage {dmg}");
    }

    #[test]
    fn the_engine_figure_counts_zone_one_and_divides_by_one_more_part() {
        // 0x4938A0: `(zone1 + Σ part terms) / (1 + n)`; 0x493BC0: `Σ / n` only when n > 1
        let mut specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.damage = [0.0; 10];
        v.damage[1] = 40.0;
        let sum = v.part_wear(&specs.zone_masks.engine, &specs.wear.engine);
        assert_eq!(v.engine_wear(&specs), (40.0 + sum) / 2.0);
        v.damage = [20.0; 10];
        specs.wear.suspension.count = 1;
        let one = v.suspension_wear(&specs);
        specs.wear.suspension.count = 3;
        assert_eq!(v.suspension_wear(&specs) * 3.0, v.part_wear(&specs.zone_masks.suspension, &specs.wear.suspension));
        assert_eq!(one, v.part_wear(&specs.zone_masks.suspension, &specs.wear.suspension));
    }
}

/// A bare x87 `fistp`: round to nearest, **ties to even** — which is neither Rust's `f32::round`
/// (ties away from zero) nor `as i32` (truncate). `RE/CONVENTIONS.md` has the rule and the list of
/// the 141 sites in the executable that use it instead of `_ftol`.
fn fistp(v: f32) -> i32 {
    let r = v.round();
    if (v - v.trunc()).abs() == 0.5 && (r as i32) % 2 != 0 { r as i32 - v.signum() as i32 } else { r as i32 }
}

#[cfg(test)]
mod steer_taper_tests {
    use super::*;

    /// `physics.md` §4 item 4, corrected 2026-09-21: the `1.75 − 0.025·speed` factor applies
    /// **below** 30 m/s (0x49F5F0 compares, 0x49F5FB's `test ah,1; je` skips when `speed >= 30`).
    ///
    /// This used to assert a closure it defined itself — a copy of the formula, not the port — so
    /// it could never catch the port drifting. It now drives [`Vehicle::set_wheel_steer`] at full
    /// lock and reads the wheel back (2026-09-21, found by the unused `use super::*`).
    /// 0x49F6CC: `car+0xAC8` follows the wheel angle half way each step; a reset clears it.
    #[test]
    fn smoothed_steering() {
        let specs = super::tests::specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.set_wheel_steer(&specs, 127.0, 10.0, 10.0);
        let a = v.wheels[0].steer;
        assert!((v.steer_smoothed[0] - a * 0.5).abs() < 1e-5);
        v.set_wheel_steer(&specs, 127.0, 10.0, 10.0);
        assert!((v.steer_smoothed[0] - a * 0.75).abs() < 1e-5);
        v.rearm_after_reset();
        assert_eq!(v.steer_smoothed, [0.0; 2]);
    }

    #[test]
    fn the_steering_taper_is_a_low_speed_boost_not_a_high_speed_cut() {
        let specs = super::tests::specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        // the wheel at full lock, less the fixed offset, at one speed (below 80 m/s, so the
        // `1/(v·0.0125)` arm stays out of it)
        let mut lock = |speed: f32| {
            v.set_wheel_steer(&specs, 127.0, speed, speed);
            v.wheels[0].steer - specs.steer_offset
        };
        let (rest, at30, at70) = (lock(0.0), lock(30.0), lock(70.0));
        assert!(rest > at70, "the factor is a boost below 30 m/s: {rest} at rest vs {at70} at 70 m/s");
        assert!((at30 - at70).abs() < 1e-4, "and flat from 30 m/s up: {at30} vs {at70}");
        // the old reading went negative, which is what took the steering away at speed
        assert!(at70 > 0.0);
        // where the 20-unit clamp does not bite, the boost at rest is exactly 1.75
        if specs.max_steer * 1.75 <= 20.0 {
            assert!((rest / at70 - 1.75).abs() < 1e-3, "rest/fast = {}", rest / at70);
        }
    }

    /// A bare `fistp` rounds to nearest with ties to even — 0x49F3EE and 0x49F4C9 both use it
    /// where `physics.md` §4 writes `int()`.
    #[test]
    fn the_fistp_helper_rounds_to_nearest_with_ties_to_even() {
        assert_eq!(super::fistp(0.4), 0);
        assert_eq!(super::fistp(0.6), 1);
        assert_eq!(super::fistp(-0.6), -1);
        // ties go to the even neighbour, both signs
        assert_eq!(super::fistp(0.5), 0);
        assert_eq!(super::fistp(1.5), 2);
        assert_eq!(super::fistp(2.5), 2);
        assert_eq!(super::fistp(-0.5), 0);
        assert_eq!(super::fistp(-1.5), -2);
        assert_eq!(super::fistp(-2.5), -2);
        // and it is not truncation: this is the case `as i32` gets wrong
        assert_ne!(super::fistp(0.9), 0.9_f32 as i32);
    }

    /// And the roll split: `speed < 10 → (speed+30)·0.025`, otherwise `(speed+70)·0.0125`
    /// (0x49F2F0 / 0x49F301). Continuous at 10, which is why only the branch order settles it.
    #[test]
    fn the_roll_factor_arms_meet_at_ten_metres_a_second() {
        let r = |speed: f32| if speed < 10.0 { (speed + 30.0) * 0.025 } else { (speed + 70.0) * 0.0125 };
        assert!((r(9.99) - 1.0).abs() < 0.001);
        assert!((r(10.0) - 1.0).abs() < 1e-6);
        // below 10 the slope is twice as steep as above it
        assert!((r(0.0) - 0.75).abs() < 1e-6);
        assert!((r(50.0) - 1.5).abs() < 1e-6);
    }
}

#[cfg(test)]
mod car_ground_tests {
    use super::*;
    use crate::collision::Triangle;

    /// A flat plane at y = 0, drivable (surface 1).
    fn flat() -> TriangleGrid {
        let s = 50.0;
        let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
        TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 10.0)
    }

    #[test]
    fn a_wheel_off_every_patch_reads_surface_zero_and_the_kept_plane() {
        // road (surface 3) only in front of z = 0; the rear wheels hang over nothing
        let s = 50.0;
        let (a, b, c, d) = (Vec3::new(-s, 0.0, 0.0), Vec3::new(s, 0.0, 0.0), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
        let grid = TriangleGrid::build(vec![Triangle::new(a, b, c, 3), Triangle::new(a, c, d, 3)], 10.0);
        let specs = super::tests::specs_993();
        let mut v = Vehicle::bare(Vec3::new(0.0, 0.0, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        v.tracker.plane_point = Vec3::new(0.0, -0.5, 0.0);
        v.wheels[2].surface = 5;
        v.ground_contact(&grid, &specs);
        assert_eq!(v.wheels[0].surface, 3);
        assert_eq!(v.wheels[1].surface, 3);
        assert_eq!(v.wheels[2].surface, 0, "no patch: the tracker's zone word 0");
        assert_eq!(v.wheels[3].surface, 0);
        // the rear wheels stood on the kept plane `car+0x400` (0.5 m down: hanging)
        assert!(v.wheels[2].airborne && v.wheels[3].airborne && !v.wheels[0].airborne);
    }

    /// 0x49B090: the offset in the slice frame, then the drop by `car+0x424` puts the wheel quad
    /// on the road and the origin `car+0x3AC` above it; an opponent after tick 0x1C0 takes the
    /// offset as a world position.
    #[test]
    fn place_at_slice_drops_the_body_onto_the_road() {
        let grid = flat();
        let specs = super::tests::specs_993();
        let quad = [(-0.66, 1.2), (0.66, 1.2), (-0.66, -1.1), (0.66, -1.1)];
        let mut pts: Vec<Vec3> = quad.iter().map(|&(x, z)| Vec3::new(x, 0.0, z)).collect();
        pts.push(Vec3::new(0.0, 1.26, 0.0));
        pts.iter_mut().for_each(|p| p.y -= 0.631);
        body_points_setup(&mut pts);
        let mut v = Vehicle::bare(Vec3::new(0.0, 3.0, 0.0), Mat3::IDENTITY, 2.3, 1300.0, 0.8, 0.3);
        v.set_car_body(pts, Vec3::new(0.91, 0.631, 2.1));
        v.game_tick = 0;
        v.place_at_slice(&grid, &specs, Vec3::new(5.0, 0.0, 5.0), Vec3::Z, Vec3::new(1.5, 0.5, 0.0), true, false);
        assert!((v.position - Vec3::new(6.5, 0.631, 5.0)).length() < 1e-4, "{:?}", v.position);
        assert!((v.draw_origin().y).abs() < 1e-4);
        assert_eq!(v.body_height, 0.0);
        v.game_tick = 0x1C1;
        v.place_at_slice(&grid, &specs, Vec3::new(5.0, 0.0, 5.0), Vec3::Z, Vec3::new(-2.0, 0.0, 7.0), true, true);
        assert!((v.position - Vec3::new(-2.0, 0.631, 7.0)).length() < 1e-4, "{:?}", v.position);
        assert!((v.orientation.row(2) - Vec3::NEG_Z).length() < 1e-5, "flipped: forward negated");
    }

    /// 0x410430: the skill start puts `car+0x330` on the trigger (on the pad here), turns the
    /// frame onto the patch normal with the trigger's `+0xC8` as forward, and one `Susp_update`
    /// lifts the body until the wheel quad stands on the pad — the origin half the box up.
    #[test]
    fn place_at_trigger_stands_the_car_on_the_pad() {
        let grid = flat();
        let specs = super::tests::specs_993();
        let quad = [(-0.66, 1.2), (0.66, 1.2), (-0.66, -1.1), (0.66, -1.1)];
        let mut pts: Vec<Vec3> = quad.iter().map(|&(x, z)| Vec3::new(x, 0.0, z)).collect();
        pts.push(Vec3::new(0.0, 1.26, 0.0));
        pts.iter_mut().for_each(|p| p.y -= 0.631);
        body_points_setup(&mut pts);
        let mut v = Vehicle::bare(Vec3::new(0.0, 3.0, 0.0), Mat3::IDENTITY, 2.3, 1300.0, 0.8, 0.3);
        v.set_car_body(pts, Vec3::new(0.91, 0.631, 2.1));
        v.place_at_trigger(&grid, &specs, Vec3::new(-6.8, 0.0, 14.0), Vec3::new(0.0, 0.0, -2.0));
        assert!((v.position - Vec3::new(-6.8, 0.631, 14.0)).length() < 1e-4, "{:?}", v.position);
        assert!(v.draw_origin().y.abs() < 1e-4);
        assert!((v.orientation.row(2) - Vec3::NEG_Z).length() < 1e-5, "forward = +0xC8, normalised");
        assert!((v.orientation.row(1) - Vec3::Y).length() < 1e-5);
        assert_eq!(v.body_height, 0.0);
    }

    /// `wheel+0x8C` keeps the whole word (0x499B84…0x499B8A); `RALLY` turns nibble 1 or 10 into
    /// the dword 2 (0x499BA7).
    #[test]
    fn the_wheel_keeps_the_whole_zone_word() {
        let s = 50.0;
        let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
        let grid = TriangleGrid::build(vec![Triangle::new(a, b, c, 0x0101), Triangle::new(a, c, d, 0x0101)], 10.0);
        let specs = super::tests::specs_993();
        let mut v = Vehicle::bare(Vec3::ZERO, Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        v.ground_contact(&grid, &specs);
        assert!(v.wheels.iter().all(|w| w.zone == 0x0101 && w.surface == 1));
        v.rally = true;
        v.ground_contact(&grid, &specs);
        assert!(v.wheels.iter().all(|w| w.zone == 2 && w.surface == 2));
    }

    #[test]
    fn body_setup_sorts_by_height_and_deals_the_lowest_four_by_quadrant() {
        // the 993's wheel quad as `BndP` stores it (model axes), a roof point and a box corner
        let mut p = vec![
            Vec3::new(0.0, 1.2, 0.0),
            Vec3::new(0.66, 0.0, -1.075),
            Vec3::new(-0.66, 0.0, -1.075),
            Vec3::new(0.9, 0.3, 2.0),
            Vec3::new(0.66, -0.001, 0.926),
            Vec3::new(-0.66, 0.0, 0.926),
        ];
        body_points_setup(&mut p);
        assert_eq!(
            &p[..4],
            &[Vec3::new(-0.66, 0.0, 0.926), Vec3::new(0.66, -0.001, 0.926), Vec3::new(-0.66, 0.0, -1.075), Vec3::new(0.66, 0.0, -1.075)]
        );
        assert_eq!(&p[4..], &[Vec3::new(0.9, 0.3, 2.0), Vec3::new(0.0, 1.2, 0.0)]);
        // a point on an axis reaches no slot, which stays zero; a second one in a slot wins
        let mut q = vec![Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(2.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, -1.0)];
        body_points_setup(&mut q);
        assert_eq!(q, vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, -1.0), Vec3::ZERO]);
    }

    #[test]
    fn body_height_reads_the_first_four_points_upright_and_all_of_them_tilted() {
        let mut v = Vehicle::bare(Vec3::new(0.0, 1.0, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        // four tyre points 1 m below the centre at 0.1, 0.2, 0.3, 0.4 above the plane, then a
        // body point that pokes 0.05 below it
        v.body_points = vec![
            Vec3::new(-0.8, -0.9, 1.2),
            Vec3::new(0.8, -0.8, 1.2),
            Vec3::new(-0.8, -0.7, -1.2),
            Vec3::new(0.8, -0.6, -1.2),
            Vec3::new(0.0, -1.05, 0.0),
        ];
        v.cos_tilt = 1.0;
        v.body_height_on(Vec3::Y, Vec3::ZERO);
        // ½·(mean + min) of the first four, into both fields (0x49AA46 / 0x49AA4C)
        let want = 0.5 * (0.25 + 0.1);
        assert!((v.body_height - want).abs() < 1e-5 && v.min_height == v.body_height, "{} {}", v.body_height, v.min_height);
        v.cos_tilt = 0.9;
        v.body_height_on(Vec3::Y, Vec3::ZERO);
        assert!((v.body_height + 0.05).abs() < 1e-5, "{}", v.body_height);
        assert!((v.min_height - (-0.05 + 0.1 + 0.2) / 3.0).abs() < 1e-5, "{}", v.min_height);
    }

    /// 0x49C095…0x49C0E6: a car on its roof with damage on and the roof under 40 has the roof set
    /// to 40 and the roll-over event raised, once.
    #[test]
    fn a_rolled_car_takes_forty_on_the_roof_once() {
        let mut v = Vehicle::bare(Vec3::new(0.0, 0.5, 0.0), Mat3::from_diagonal(Vec3::new(1.0, -1.0, -1.0)), 2.5, 1300.0, 0.8, 0.3);
        v.cos_tilt = -1.0;
        v.body_height = 0.0;
        v.min_height = 0.0;
        v.car_ground_body(0.0, Vec3::Y, Vec3::ZERO, 1, 1300.0, None, &flat());
        assert!(v.rollover_sound && v.damage[8] == 40.0);
        v.rollover_sound = false;
        v.car_ground_body(0.0, Vec3::Y, Vec3::ZERO, 1, 1300.0, None, &flat());
        assert!(!v.rollover_sound, "the roof is at 40 now");
        // the direction of travel follows the velocity, or the forward row at rest
        assert!(v.travel_dir.is_normalized());
    }

    /// 0x49C1C4…0x49C27D: a hard landing raises the event, clears the `+0x418` word when upright
    /// and, for the human, 0x493DD0 clears the wheels' airborne flags.
    #[test]
    fn a_hard_landing_resets_the_human_car() {
        let mut v = Vehicle::bare(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        v.cos_tilt = 1.0;
        v.body_height = -0.2;
        v.min_height = -0.2;
        v.velocity.y = -10.0;
        v.airborne_count = 1;
        for w in &mut v.wheels {
            w.airborne = true;
        }
        v.car_ground_body(0.0, Vec3::Y, Vec3::ZERO, 1, 1300.0, None, &flat());
        assert!(v.landing_sound.is_some_and(|c| c > 6.0));
        assert_eq!(v.airborne_count, 0);
        assert!(v.wheels.iter().all(|w| !w.airborne));
    }

    #[test]
    fn a_tilted_car_off_its_body_falls_through_to_the_resting_arm() {
        // 0x49BF62: `car+0x424 < 0.001` fails, so the upright arm 0x49C19A sets the body on the
        // plane (`car+0x424 < 0.005`, closing ≤ 6)
        let grid = flat();
        let mut v = Vehicle::bare(Vec3::new(0.0, 0.003, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        v.cos_tilt = 0.8;
        v.body_height = 0.003;
        v.min_height = 0.003;
        v.velocity = Vec3::new(5.0, -0.1, 0.0);
        v.car_ground_body(0.0, Vec3::Y, Vec3::ZERO, 1, 1300.0, None, &grid);
        assert!(v.position.y.abs() < 1e-6, "{:?}", v.position);
        assert_eq!(v.body_height, 0.0);
        assert_eq!(v.velocity, Vec3::new(5.0, 0.0, 0.0));
    }

    #[test]
    fn the_rollover_bounce_reads_the_velocity_from_before_the_impulse() {
        // 0x49C038: `vel.y −= bounce[surf] · vy` only while `vy < −2`; a body moving up gains nothing
        let grid = flat();
        let far = Vec3::new(0.0, -100.0, 0.0); // no body point reaches this plane: no impulse
        let mut v = Vehicle::bare(Vec3::new(0.0, 10.0, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        v.cos_tilt = 0.5;
        v.body_height = 0.0;
        v.min_height = 0.0;
        v.velocity = Vec3::new(0.0, 3.0, 0.0);
        v.car_ground_body(0.0, Vec3::Y, far, 1, 1300.0, None, &grid);
        assert_eq!(v.velocity.y, 3.0);
        v.velocity = Vec3::new(1.0, -4.0, 0.0);
        v.car_ground_body(0.0, Vec3::Y, far, 1, 1300.0, None, &grid);
        // surface 1's bounce is 0.7: −4 + 0.7·4
        assert!((v.velocity.y - (-4.0 + 0.7 * 4.0)).abs() < 1e-5, "{:?}", v.velocity);
        assert!((v.velocity.x - 0.95).abs() < 1e-6);
    }

    #[test]
    fn a_hard_landing_clamps_the_spin_per_component() {
        // 0x49BBA0 clamps each of ω's components to ±0.75, not its length
        let grid = flat();
        let mut v = Vehicle::bare(Vec3::new(0.0, -0.2, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        v.cos_tilt = 1.0;
        v.body_height = -0.2;
        v.min_height = -0.2;
        v.velocity = Vec3::new(0.0, -8.0, 0.0);
        v.angular_velocity = Vec3::new(0.7, -0.7, 2.0);
        v.car_ground_body(0.0, Vec3::Y, Vec3::ZERO, 1, 1300.0, None, &grid);
        assert_eq!(v.angular_velocity, Vec3::new(0.7, -0.7, 0.75));
        assert!((v.position.y + 0.05).abs() < 1e-6);
        assert!((v.velocity.y - 0.4 * 0.7 * 8.0).abs() < 1e-5, "{:?}", v.velocity);
    }
}
