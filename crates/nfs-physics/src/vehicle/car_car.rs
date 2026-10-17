//! Car-against-car collision (0x498FF0…): the broad phase, the pair test, the separation and the impulse (a move out of the parent module, no logic changed).

use super::*;

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
pub(crate) fn separate(a: &mut Vehicle, mass_a: f32, b: &mut Vehicle, mass_b: f32) -> Option<(Vec3, Vec3)> {
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
        // 0x496A35…0x496A66: `[0x657408]` and either car's `car+0xDAC` — a car of a positional race that
        // has a remote car in the pair — scale the impulse by `[0x5B2694]` = 0.2 (a remote car's own
        // position comes from its owner's packet, so a bump is only felt a fifth)
        let jj = j * 0.5 * if a.net_car || b.net_car { f32::from_bits(0x3E4C_CCCD) } else { 1.0 };
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
        // `Damage`'s crash response (0x496F8E / 0x496FB9) runs **inside** the solver, so on the
        // rad/s spin (the original's own `Damage` entry reads 2π times the turns/s that the tick
        // ends with — a live sign hit: (2.50, 1.43, 5.06) at the call, (0.40, 0.23, 0.81) after —
        // `RE/notes/REFERENCE-2026-10-17-SIGN.md`); it is converted back to turns/s as 0x498FF0
        // does when the pair returns, and the clamp waits for the whole pair (`collide_cars_with`).
        a.angular_velocity = w_a;
        b.angular_velocity = w_b;
        // 0x496F1E / 0x496F25: `word car+0x418` steps for both bodies
        a.airborne_count = a.airborne_count.wrapping_add(1);
        b.airborne_count = b.airborne_count.wrapping_add(1);
        // 0x496F2C…0x496F88: both cars' `car+0x440` = the midpoint of their positions
        let mid = (a.position + b.position) * 0.5;
        a.last_contact = mid;
        b.last_contact = mid;
        // 0x496CA0…0x496CBD / 0x496DB7…0x496DCF: each body's event word names the other
        a.pair_event = if b.smackable { 0x50000 | b.material_class } else { 0x50003 };
        b.pair_event = if a.smackable { 0x50000 | a.material_class } else { 0x50003 };
        a.impact = a.impact.max(eff_a);
        b.impact = b.impact.max(eff_b);
        // `car+0x430` is assigned (0x496B42…0x496C3E), the word's kind is 5
        a.contact_mag = eff_a;
        b.contact_mag = eff_b;
        a.contact_kind = ContactKind::PropOrCar;
        b.contact_kind = ContactKind::PropOrCar;
        // The damage normal is not the contact normal (read 2026-09-26): the local 0x496F38 hands
        // 0x494DF0 is `A.pos − B.pos` (0x496CC3…0x496CF5), normalised in place (0x5328D0 at
        // 0x496D4D) — or `(0, 1, 0)` when the contact normal is exactly zero (0x496CF9…0x496D39),
        // which picks no zone. So A's points from B toward A, the same sense as a wall's (a hit
        // on the nose reads `z < −0.1`, zone 1), and B takes it negated (0x496F93…0x496FB9). The
        // port handed A the contact normal from A toward B, which put a nose hit on the tail.
        let dn = if ni == Vec3::ZERO { Vec3::Y } else { (a.position - b.position).normalize() };
        a.damage_by_normal(eff_a, dn, 2, mass_a);
        b.damage_by_normal(eff_b, -dn, 2, mass_b);
        a.angular_velocity /= TAU;
        b.angular_velocity /= TAU;
        Some((eff_a, eff_b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first impulse of a live hit on the original (alps SIGN 0 hit by car 993 at 13.9 m/s, the
    /// solver's entry at tick 6254, `REFERENCE-2026-10-17-SIGN.md`): the numerator at 0x4969DF read
    /// 13.8968 = `(vB − vA)·n` plus the spin terms (−0.0066), the quotient `j` = 11.3507, and the
    /// sign's velocity at the `Damage` call 11.35 along `n` — the denominator 1.2243 is this
    /// function's `den` to four digits. (An earlier note's "closing speed 11.8" was `j` times a
    /// guessed denominator, not a measurement.)
    #[test]
    fn the_impulse_of_a_live_sign_hit_is_the_originals() {
        let (ma, mb) = (0.625f32, 10.3125);
        let mut sign = Vehicle::bare(Vec3::new(-158.848, 3.5715, 193.4924), Mat3::IDENTITY, 0.0, 80.0, 0.0, 0.0);
        sign.smackable = true;
        let mut car = Vehicle::bare(Vec3::new(-157.82, 2.2570, 196.0780), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        car.velocity = Vec3::new(-5.1303, -0.85415, -12.8956);
        car.angular_velocity = Vec3::new(-0.035889, 0.0000917, 0.0061115) / TAU;
        let pt = Vec3::new(-158.23468, 1.9708, 194.00198);
        let ni = Vec3::new(-0.369168, -0.0465407, -0.928197);
        pair_impulse(&mut sign, ma, &mut car, mb, pt, ni).unwrap();
        // the sign's velocity change along n is j (·0.5·invM·1.25 = 1.0 here), plus the crash
        // response's lift (y only, `n.y` = −0.047 of it)
        let dj = sign.velocity.dot(ni);
        assert!((dj - 11.3507).abs() < 0.4, "{dj}");
    }
}
