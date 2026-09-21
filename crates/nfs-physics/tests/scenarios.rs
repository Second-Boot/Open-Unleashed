//! Reproducible physics scenarios (RE spec §21). Expected values are currently the native model's
//! own recorded outputs (self-regression); they must be replaced by measurements from the original
//! game (replay/instrumented run, see RE/DYNAMIC_ANALYSIS.md) — tolerances are documented per value.

use glam::{Mat3, Vec3};
use nfs_physics::collision::{Triangle, TriangleGrid};
use nfs_physics::specs::{CarSpecs, CIRCLE_SOFTNESS, GRIP_DRY, GRIP_WET, SURFACE_BOUNCE};
use nfs_physics::vehicle::{Controls, Vehicle};

fn flat() -> TriangleGrid {
    let s = 5000.0;
    let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
    TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 200.0)
}

fn specs_993() -> Option<CarSpecs> {
    let home = std::env::var_os("HOME")?;
    let p = std::path::PathBuf::from(home).join("Games/NFS-Porsche-Unleashed/game_info/data/GameData/Simulation/CarData/993coupe36.sim");
    let sim = nfs_formats::sim::CarSim::parse(&std::fs::read(p).ok()?).ok()?;
    Some(CarSpecs::from_sim(&sim, false))
}

fn run(v: &mut Vehicle, specs: &CarSpecs, grid: &TriangleGrid, c: Controls, seconds: f32) {
    for t in 0..(seconds * 64.0) as u32 {
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
fn scenario_full_throttle_10s() {
    let Some(specs) = specs_993() else { return };
    let grid = flat();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
    run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
    run(&mut v, &specs, &grid, Controls { throttle: 254, ..Default::default() }, 10.0);
    let kmh = v.speed() * 3.6;
    eprintln!("993 full throttle 10 s: {kmh:.1} km/h, gear {}, rpm {:.0}, distance {:.1} m", v.gear, v.rpm, v.position.z);
    assert!(kmh > 90.0 && kmh < 200.0, "speed out of the plausible band: {kmh}");
    assert!(v.position.z > 120.0 && v.position.z < 400.0, "distance {}", v.position.z);
}

#[test]
fn scenario_brake_from_speed() {
    let Some(specs) = specs_993() else { return };
    let grid = flat();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
    run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
    run(&mut v, &specs, &grid, Controls { throttle: 254, ..Default::default() }, 7.0);
    let v0 = v.speed();
    let z0 = v.position.z;
    run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 4.0);
    eprintln!("brake from {:.0} km/h: after 4 s {:.1} km/h, {:.1} m", v0 * 3.6, v.speed() * 3.6, v.position.z - z0);
    // brake_decel 9.5 m/s²: from ~26 m/s the car stops within ~3 s and ~40 m; holding the brake at a
    // standstill then selects reverse (automatic gearbox rule, 0x49DD20) so it may creep backwards.
    assert!(v.v_local.z <= 0.5, "still moving forward at {:.1} km/h", v.v_local.z * 3.6);
    assert!(v.position.z - z0 < 60.0);
}

#[test]
fn scenario_gt1_full_throttle() {
    let home = std::env::var_os("HOME").unwrap();
    let p = std::path::PathBuf::from(home).join("Games/NFS-Porsche-Unleashed/game_info/data/GameData/Simulation/CarData/GT1race.sim");
    let Ok(d) = std::fs::read(p) else { return };
    let sim = nfs_formats::sim::CarSim::parse(&d).unwrap();
    let specs = CarSpecs::from_sim(&sim, true);
    let grid = flat();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.9, 0.33);
    run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
    for t in 0..(6.0 * 64.0) as u32 {
        if t % 2 == 0 {
            v.step32(&Controls { throttle: 254, ..Default::default() }, &specs, &grid);
            if t % 64 == 0 {
                eprintln!("GT1 t={:.1} v={:.1} km/h gear={} rpm={:.0} thr={:.2} demand={:.2} lim={:.2} lon={:.2} locked={} slip={:.2}", t as f32 / 64.0, v.speed() * 3.6, v.gear, v.rpm, v.throttle, v.wheels[2].demand, v.wheels[2].limit, v.wheels[2].longitudinal, v.wheels[2].locked, v.wheels[2].slip);
            }
        }
        v.step64(&specs);
    }
    assert!(v.speed() * 3.6 > 60.0, "GT1 too slow: {:.1} km/h", v.speed() * 3.6);
}

/// How tightly the car actually turns for a given steering demand, at a speed and a steer angle the
/// AI uses on a street circuit (`RE/notes/ai.md` §9.1): the measured radius against what the
/// steering geometry asks for and what the grip allows.
#[test]
fn steady_state_cornering_radius() {
    let Some(specs) = specs_993() else { return };
    let grid = flat();
    for steer in [32i32, 63, 126] {
        let mut v = Vehicle::new(Vec3::new(0.0, 0.32, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.315);
        // get up to ~20 m/s, then hold the throttle and the steering
        run(&mut v, &specs, &grid, Controls { throttle: 254, ..Default::default() }, 6.0);
        let speed0 = v.speed();
        run(&mut v, &specs, &grid, Controls { throttle: 90, steer, ..Default::default() }, 3.0);
        let h0 = v.orientation.row(2);
        run(&mut v, &specs, &grid, Controls { throttle: 90, steer, ..Default::default() }, 1.0);
        let h1 = v.orientation.row(2);
        let yaw = h0.dot(h1).clamp(-1.0, 1.0).acos();
        let speed = v.speed();
        let radius = if yaw > 1e-3 { speed / yaw } else { f32::INFINITY };
        let lateral = if radius.is_finite() { speed * speed / radius } else { 0.0 };
        let wheel_deg = steer as f32 / 126.0 * (specs.max_steer / 256.0 * 360.0);
        let geometric = specs.wheelbase / wheel_deg.to_radians().max(1e-4);
        eprintln!(
            "steer {steer:3} ({wheel_deg:.1} deg): entry {speed0:.1} m/s, holding {speed:.1} m/s, radius {radius:.1} m (geometry asks {geometric:.1} m), lateral {lateral:.1} m/s^2, drift {:.2} m/s",
            v.v_local.x
        );
    }
}

/// What `FECar`'s constructor writes into the setup block (0x4EED2D…0x4EED80), in the order the
/// seventeen sliders sit in the record: three 50s, three zeroes, a 1.0, two 30s and eight zeroes.
const SETUP_DEFAULT: [f32; 17] = [50.0, 50.0, 50.0, 0.0, 0.0, 0.0, 1.0, 30.0, 30.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

/// **A car at its default setup is the car the `.sim` describes.** Every one of the eleven effects
/// in 0x49D290…0x49D62C is neutral at the constructor's values — `pf = pr = 1` so the top-speed
/// divisor is `(1 + 1 + 3)·0.2 = 1`, brake bias 50 gives `(0.5 − 0.5)·0.5`, shock rate 1 leaves
/// `transfer_div` at `(1 + 2)/3` of itself and then grip at `(1 + 9)·0.1` of itself, and 30 psi on
/// both axles gives `a = b = 1`, grip `×(1 + 1 + 8)·0.1` and front weight `×(2/2 + 1)·0.5`.
///
/// It holds only because the 993's `.sim` carries `transfer_div = 1` and `probe_offset = 0`; the
/// ride-height arm is an **overwrite**, so a car whose file said otherwise would move.
#[test]
fn the_default_car_setup_changes_nothing() {
    let Some(base) = specs_993() else { return };
    let mut tuned = base.clone();
    tuned.apply_setup(&SETUP_DEFAULT, true);
    assert_eq!(base, tuned);
}

/// The sliders that have a sign, in the direction the block gives them. Downforce trades top speed
/// for grip on the axle, a softer shock rate (0.7) raises the pitch and roll retention because the
/// term is `(1 − v)·0.2`, and ride height is written in metres from millimetres.
#[test]
fn the_sliders_move_what_their_names_say() {
    let Some(base) = specs_993() else { return };

    let mut s = SETUP_DEFAULT;
    s[0] = 100.0;
    s[1] = 100.0;
    let mut max_wing = base.clone();
    max_wing.apply_setup(&s, true);
    assert!(max_wing.top_speed < base.top_speed, "full downforce must cost top speed");
    assert!(max_wing.downforce_front > base.downforce_front);
    assert!(max_wing.downforce_rear > base.downforce_rear);
    // pf = pr = (100·0.02 + 1)·0.5 = 1.5, so the divisor is (1.5 + 1.5 + 3)·0.2 = 1.2
    assert!((max_wing.top_speed - base.top_speed / 1.2).abs() < 1e-4);
    assert!((max_wing.downforce_front - base.downforce_front * 1.5).abs() < 1e-6);

    let mut s = SETUP_DEFAULT;
    s[6] = 0.7;
    let mut soft = base.clone();
    soft.apply_setup(&s, true);
    assert!(soft.pitch_ret > base.pitch_ret && soft.roll_ret > base.roll_ret);
    assert!((soft.pitch_ret - (base.pitch_ret + 0.3 * 0.2)).abs() < 1e-5);
    assert!(soft.transfer_div < base.transfer_div, "a softer shock rate lowers the transfer divisor");

    let mut s = SETUP_DEFAULT;
    s[3] = 20.0;
    let mut high = base.clone();
    high.apply_setup(&s, true);
    assert!((high.probe_offset - 0.02).abs() < 1e-6, "twenty millimetres is 0.02 m, and it overwrites");

    let mut s = SETUP_DEFAULT;
    s[2] = 70.0;
    let mut rear_bias = base.clone();
    rear_bias.apply_setup(&s, true);
    // trunc(70·65536/100) = 45875, /65536 = 0.6999969, −0.5, ×0.5
    assert!((rear_bias.front_brake - (base.front_brake + 0.099_998_5)).abs() < 1e-5);
}

/// The gear sliders add to the **raw** ratios, so what moves is the whole derived chain:
/// `ratio_rpm_per_mps`, `mps_per_rpm`, `accel_per_torque` and the drag term. `Gear7` is the final
/// drive, not an eighth gear, and the neutral slot stays at zero whatever the sliders say.
#[test]
fn a_gear_slider_moves_the_derived_tables() {
    let Some(base) = specs_993() else { return };

    let mut s = SETUP_DEFAULT;
    s[10] = 0.5; // Gear1 = first gear
    let mut tall = base.clone();
    tall.apply_setup(&s, true);
    assert!((tall.gear_ratio[2] - (base.gear_ratio[2] + 0.5)).abs() < 1e-6);
    assert!(tall.ratio_rpm_per_mps[2] > base.ratio_rpm_per_mps[2]);
    assert!(tall.mps_per_rpm[2] < base.mps_per_rpm[2]);
    assert_eq!(tall.gear_ratio[1], 0.0, "the neutral slot has no slider");
    assert_eq!(tall.ratio_rpm_per_mps[1], 0.0);
    assert_eq!(tall.mps_per_rpm[1], 0.1, "and a zero ratio gives the 0.1 that 0x49D6A4 stores");

    let mut s = SETUP_DEFAULT;
    s[16] = 0.5; // Gear7 = the final drive
    let mut short = base.clone();
    short.apply_setup(&s, true);
    assert!((short.final_drive - (base.final_drive + 0.5)).abs() < 1e-6);
    for i in [0usize, 2, 3, 4, 5, 6] {
        assert!(short.gear_ratio[i] == base.gear_ratio[i], "the final drive is not a gear");
        // reverse is a negative ratio, so what grows is the magnitude
        assert!(short.ratio_rpm_per_mps[i].abs() > base.ratio_rpm_per_mps[i].abs(), "but it rescales every one");
    }
    assert!(short.drag != base.drag);
}

/// A manual gearbox is worth five per cent of acceleration in every gear — `carinfo+0xC == 0`
/// at 0x49D733. Nothing else in the block moves.
#[test]
fn a_manual_gearbox_is_worth_five_per_cent() {
    let Some(base) = specs_993() else { return };
    let mut manual = base.clone();
    manual.apply_setup(&SETUP_DEFAULT, false);
    for i in 0..base.gear_slots {
        assert!((manual.accel_per_torque[i] - base.accel_per_torque[i] * 1.05).abs() < 1e-9);
    }
    assert_eq!(manual.grip, base.grip);
}

fn parts() -> Option<Vec<nfs_formats::career::Part>> {
    let home = std::env::var_os("HOME")?;
    let p = std::path::PathBuf::from(home).join("Games/NFS-Porsche-Unleashed/game_info/data/FEData/Data/nfs5.prt");
    nfs_formats::career::parse_parts(&std::fs::read(p).ok()?).ok()
}

/// `+0xC0 = 1` is the identity in the brake, spring, shock and sway-bar arms, and that is a check on
/// the record layout rather than on the port: if `value_c0` were being read from the wrong offset,
/// the standard part would not leave the block alone.
#[test]
fn the_standard_part_is_the_identity() {
    let (Some(base), Some(all)) = (specs_993(), parts()) else { return };
    for kind in [2u16, 4, 5, 8] {
        let Some(p) = all.iter().find(|p| p.kind == kind && p.value_c0 == 1.0) else { continue };
        let mut one = base.clone();
        one.apply_parts(&[p], "993");
        // the mass and top-speed deltas are separate and may be non-zero; the tuning arms are not
        assert_eq!(one.transfer_div, base.transfer_div, "kind {kind} transfer_div");
        assert_eq!(one.lateral_div, base.lateral_div, "kind {kind} lateral_div");
        assert!((one.grip - base.grip).abs() < 1e-6, "kind {kind} grip");
        assert!((one.roll_gain - base.roll_gain).abs() < 1e-6, "kind {kind} roll_gain");
        assert!((one.pitch_ret - base.pitch_ret).abs() < 1e-6, "kind {kind} pitch_ret");
    }
}

/// The three arms whose value is not a multiplier: a spring's `+0xC4` is a ride height in
/// millimetres and **overwrites** the probe offset, a tyre's `+0xC8` **overwrites** the grip class,
/// and a brake's `+0xC0` scales the deceleration by `(c0 + 3)/4`.
#[test]
fn the_parts_that_overwrite_rather_than_scale() {
    let (Some(base), Some(all)) = (specs_993(), parts()) else { return };

    if let Some(p) = all.iter().find(|p| p.kind == 4 && p.boost == 20.0) {
        let mut lifted = base.clone();
        lifted.apply_parts(&[p], "993");
        assert!((lifted.probe_offset - 0.02).abs() < 1e-6);
    }
    if let Some(p) = all.iter().find(|p| p.kind == 9 && p.grip_class == 2) {
        let mut shod = base.clone();
        shod.apply_parts(&[p], "993");
        assert_eq!(shod.class, 2, "a tyre's +0xC8 is the grip-table column");
    }
    if let Some(p) = all.iter().find(|p| p.kind == 2 && p.value_c0 == 2.5) {
        let mut stoppers = base.clone();
        stoppers.apply_parts(&[p], "993");
        assert!((stoppers.brake_decel - base.brake_decel * 1.375).abs() < 1e-5);
    }
}

/// Four families ignore the purchased list outright (0x49CCC5…0x49CD07), and the 959 gets its front
/// weight written flat before that test — so for the 959 the parts do nothing *and* the weight
/// moves.
#[test]
fn four_families_ignore_their_parts() {
    let (Some(base), Some(all)) = (specs_993(), parts()) else { return };
    let Some(p) = all.iter().find(|p| p.kind == 2 && p.value_c0 == 2.5) else { return };
    for family in ["959", "928", "GT2", "GT3"] {
        let mut skipped = base.clone();
        skipped.apply_parts(&[p], family);
        assert_eq!(skipped.brake_decel, base.brake_decel, "{family} must ignore the part");
        assert_eq!(skipped.mass, base.mass, "{family} must ignore its mass too");
        if family == "959" {
            assert_eq!(skipped.front_weight, 0.4675);
        } else {
            assert_eq!(skipped.front_weight, base.front_weight);
        }
    }
}

/// **`specs[0x4C] = 2` is unreachable from a part.** 0x49CE09 compares the part's name with
/// `"Supercharged"` (0x5D0D04, the only occurrence of that string in the image), and the fifteen
/// records that do the job are named **`"Supercharger"`**. So in the original a supercharger part
/// adds its torque and nothing else, and a car is supercharged only if its own `.sim` says so. The
/// port compares the same literal, so it inherits the same dead branch — this test is what stops it
/// being "fixed" by accident.
#[test]
fn no_part_is_named_supercharged() {
    let Some(all) = parts() else { return };
    assert!(all.iter().any(|p| p.kind_name == "Supercharger"), "the data does have superchargers");
    assert!(!all.iter().any(|p| p.kind_name == "Supercharged"), "but not one is spelled the way 0x49CE09 compares");

    let Some(base) = specs_993() else { return };
    let Some(sc) = all.iter().find(|p| p.kind_name == "Supercharger") else { return };
    let mut boosted = base.clone();
    boosted.apply_parts(&[sc], "993");
    assert_eq!(boosted.induction, base.induction, "a supercharger part cannot change the induction");
}

/// `"Engine (Standard)"` is not `"Engine"`, so it takes the `+= 0.67 ×` arm — and every one of the
/// 38 records carries an all-zero curve, which is what makes it the revert part rather than a
/// 1.67× engine.
#[test]
fn the_standard_engine_is_a_no_op() {
    let Some(all) = parts() else { return };
    for p in all.iter().filter(|p| p.kind_name == "Engine (Standard)") {
        assert!(p.torque_curve.iter().all(|t| *t == 0.0), "part {} has a curve", p.id);
        assert_eq!(p.mass_delta, 0.0);
        assert_eq!(p.top_speed_pct, 0.0);
    }
    let (Some(base), Some(std)) = (specs_993(), all.iter().find(|p| p.kind_name == "Engine (Standard)").cloned()) else { return };
    let mut reverted = base.clone();
    reverted.apply_parts(&[&std], "993");
    assert_eq!(reverted.torque, base.torque);
}

/// **`0x49B8E0`'s rotation, transcribed instruction for instruction** and compared with what
/// `step64` does to the orientation. The reference below is the original's arithmetic in the
/// original's own terms: row-major 3×3 matrices, `dest = a1 · a2` (0x532470, whose first output
/// element is `Σ_k a1[0][k]·a2[k][0]` — 0x5324B3…0x5324C7), the three axis matrices exactly as
/// 0x5325A0 / 0x532600 / 0x532660 fill them, composed **X then Y then Z**, and applied on the
/// right: `M ← M · Δ`.
///
/// The angular velocities here are deliberately large and unequal, because the wrong composition
/// order agrees to first order and only shows in the second.
#[test]
fn the_orientation_update_is_the_originals_composition() {
    type M3 = [[f32; 3]; 3];
    fn mul(a: &M3, b: &M3) -> M3 {
        let mut o = [[0f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        o
    }
    // 0x5325A0, 0x532600, 0x532660 — the angle is in turns, 0x5327E0/0x5327F0 take `angle × 2π`
    let sc = |t: f32| ((t * std::f32::consts::TAU).sin(), (t * std::f32::consts::TAU).cos());
    let mx = |t: f32| { let (s, c) = sc(t); [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]] };
    let my = |t: f32| { let (s, c) = sc(t); [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]] };
    let mz = |t: f32| { let (s, c) = sc(t); [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]] };

    let Some(specs) = specs_993() else { return };
    let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
    // a tilted start, so that `M · Δ` and `Δ · M` cannot coincide either
    v.orientation = Mat3::from_rotation_z(0.2).transpose() * v.orientation;
    v.angular_velocity = Vec3::new(0.9, 2.3, -1.4); // turns/s
    let m0 = v.orientation;

    let mut want: M3 = [[0.0; 3]; 3];
    for i in 0..3 {
        let r = m0.row(i);
        want[i] = [r.x, r.y, r.z];
    }
    let w = v.angular_velocity * (1.0 / 64.0); // [0x5B2828]
    let delta = mul(&mul(&mx(w.x), &my(w.y)), &mz(w.z));
    let want = mul(&want, &delta);

    v.step64(&specs);
    for i in 0..3 {
        let got = v.orientation.row(i);
        for j in 0..3 {
            assert!(
                (got[j] - want[i][j]).abs() < 1e-5,
                "row {i} col {j}: port {} vs 0x49B8E0 {}",
                got[j],
                want[i][j]
            );
        }
    }
}

/// Read `nfs5.exe` and map a virtual address to a file offset, so a table the port transcribed can
/// be compared with the bytes it was transcribed from. Minimal PE walking: `e_lfanew` at 0x3C, the
/// COFF header, then the section whose virtual range holds the RVA.
struct Image {
    bytes: Vec<u8>,
    sections: Vec<(u32, u32, u32)>, // rva, size, file offset
}

impl Image {
    fn open() -> Option<Self> {
        let home = std::env::var_os("HOME")?;
        let p = std::path::PathBuf::from(home).join("Games/NFS-Porsche-Unleashed/game_info/data/nfs5.exe");
        let bytes = std::fs::read(p).ok()?;
        let u16at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
        let u32at = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let pe = u32at(0x3C) as usize;
        let n = u16at(pe + 6);
        let opt = u16at(pe + 20);
        let table = pe + 24 + opt;
        let sections = (0..n)
            .map(|i| {
                let s = table + i * 40;
                (u32at(s + 12), u32at(s + 8), u32at(s + 20))
            })
            .collect();
        Some(Self { bytes, sections })
    }

    fn at(&self, va: u32) -> usize {
        let rva = va - 0x400000;
        let (r, _, off) = *self
            .sections
            .iter()
            .find(|(r, sz, _)| rva >= *r && rva < r + sz.max(&1))
            .expect("the address is inside a section");
        (off + (rva - r)) as usize
    }

    fn f32s(&self, va: u32, n: usize) -> Vec<f32> {
        let o = self.at(va);
        (0..n).map(|i| f32::from_le_bytes(self.bytes[o + i * 4..o + i * 4 + 4].try_into().unwrap())).collect()
    }
}

/// **The four surface tables, against the bytes they were transcribed from.** These are the numbers
/// every wheel's grip and every landing bounce comes out of, they are pure data at fixed addresses,
/// and a slip in one of the 131 floats would be invisible in play and in every other test.
///
/// `GRIP_DRY` 0x5D0D50 and `GRIP_WET` 0x5D0E4C are `f32[16][3]` — surface type by car class, and the
/// wet table is the dry one with its first two columns swapped for rows 1…11, which is worth knowing
/// before trusting a hand copy of either. `SURFACE_BOUNCE` 0x5D0B94 is `f32[16][2]` and
/// `CIRCLE_SOFTNESS` 0x5D0F58 the three friction-circle exponents (`physics.md` §3, `+0x1C0`).
#[test]
fn the_surface_tables_match_the_image() {
    let Some(img) = Image::open() else { return };
    let dry = img.f32s(0x5D0D50, 48);
    let wet = img.f32s(0x5D0E4C, 48);
    for row in 0..16 {
        for col in 0..3 {
            assert_eq!(GRIP_DRY[row][col], dry[row * 3 + col], "GRIP_DRY[{row}][{col}]");
            assert_eq!(GRIP_WET[row][col], wet[row * 3 + col], "GRIP_WET[{row}][{col}]");
        }
    }
    let bounce = img.f32s(0x5D0B94, 32);
    for row in 0..16 {
        for col in 0..2 {
            assert_eq!(SURFACE_BOUNCE[row][col], bounce[row * 2 + col], "SURFACE_BOUNCE[{row}][{col}]");
        }
    }
    assert_eq!(CIRCLE_SOFTNESS.to_vec(), img.f32s(0x5D0F58, 3));
    // and the swap, because it is the thing a copy gets wrong
    for row in 1..12 {
        assert_eq!(wet[row * 3], dry[row * 3 + 1], "wet col 0 is dry col 1 on row {row}");
        assert_eq!(wet[row * 3 + 1], dry[row * 3], "wet col 1 is dry col 0 on row {row}");
        assert_eq!(wet[row * 3 + 2], dry[row * 3 + 2], "col 2 is shared on row {row}");
    }
}

/// The clutch-slip state `car+0xDB8` must not stay set at speed. `Engine_update` 0x49E440 only
/// enters its clutch block on `slip > 250` or on a car rolling **against** its gear, marks the axle
/// spinning only when the slip passes an eighth of the redline, and on the throttle pulls the revs
/// down by `min(slip, allow)` (`physics.md` §4.1a). Following a note that had those comparisons the
/// wrong way round, the port kept the rear "spinning" on about 80 % of full-throttle ticks at every
/// speed — heard in play as a constant wheelspin squeal. After the first twelve seconds of a
/// full-throttle run a 993 is past 130 km/h in fifth and sixth, and the flag must be all but gone.
#[test]
fn the_rear_axle_does_not_spin_at_speed() {
    let Some(specs) = specs_993() else { return };
    let grid = flat();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
    run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
    let full = Controls { throttle: 254, ..Default::default() };
    run(&mut v, &specs, &grid, full, 12.0);
    let (mut spin, mut n) = (0u32, 0u32);
    for t in 0..(12.0 * 64.0) as u32 {
        if t % 2 == 0 {
            v.step32(&full, &specs, &grid);
        }
        v.step64(&specs);
        if t % 32 == 0 {
            v.renormalise();
        }
        spin += v.rear_spin as u32;
        n += 1;
    }
    let share = spin as f32 / n as f32;
    eprintln!("rear axle spinning on {:.1} % of ticks from 12 s to 24 s, ending at {:.0} km/h", 100.0 * share, v.speed() * 3.6);
    assert!(v.speed() * 3.6 > 150.0, "the car should be well into its top gears");
    assert!(share < 0.10, "the rear axle is marked spinning on {:.0} % of ticks at speed", 100.0 * share);
}
