//! `Vehicle` methods split out of the parent module (a move, no logic changed).

use super::*;

impl Vehicle {
    /// `Car_wallCollide` 0x495020 + `WallImpulse` 0x494000 (CONFIRMED): each of the 8 body corners
    /// that is off the road is tested with the segment **from the car's centre to the corner**
    /// against the `.edg` edges; a crossing reflects the normal velocity with restitution 0.1 (no
    /// friction), pushes the body 1.05× back onto the edge line and sets the yaw rate `WallSpin`
    /// 0x493F10 does ([`Self::wall_spin`]).
    ///
    /// `patch_height` answers the height of the drivable patch that contains a point in plan, or
    /// `None` when there is none — `CollisionGrid::find_patch_height`. 0x495020 skips a corner only
    /// when **both** halves of its conjunction hold: the patch under it is drivable
    /// (`(T.word & 0xF) != 0`, 0x49527A) **and** that patch's height is within 4 m of the car's own
    /// y (`|h − car+0x334| <= [0x5B2404]`, 0x4952B9…0x4952D9). So a car sailing over drivable road
    /// is still tested against the walls, and a car that has dropped away from it is too.
    ///
    /// The tested segment (read to the instruction 2026-09-26, 0x4952DF…0x495410): its start is the
    /// car's position `car+0x330` when the same test passes there (`updateZone(pos, 1)`, nibble,
    /// `|h(pos) − car+0x334| <= 4`, 0x4952DF…0x49536A), else `pos − vel/32` (0x49536C…0x49538F,
    /// `updateZone` on it); the end is the corner. `Tracker::edgeHit` 0x4740D0 answers nothing when
    /// the tracker holds no patch after that (`[ecx+0x28] == 0`, 0x4740D0…0x4740DC) — so a car
    /// whose start point is off every drivable patch meets no wall. Among the crossed edges it keeps
    /// the one whose endpoint-mid height is nearest the **segment's** mid height
    /// (`(from.y + to.y)/2`, 0x474185…0x4741BF), a tie going to the first candidate in `tree`'s
    /// own query order for this segment (`0x4848B0`, wired 2026-09-28 — `RE/RENDERING.md` "The
    /// tree, built"; `tree` is the track's `TrackTree`, `insertAll(C)`-then-`insertAll(A)`-ordered).
    /// *(Until 2026-09-26 the port swept each corner from `corner − vel/32` to the corner, which
    /// misses a corner that a yawing car swings across the line, and a car turned so that the
    /// sweep ran along the wall; the original has no per-corner sweep at all.)*
    pub fn collide_walls(&mut self, walls: &crate::walls::WallSet, tree: &crate::track_tree::TrackTree, patch_height: &dyn Fn(Vec3) -> Option<f32>) -> bool {
        let m = self.orientation;
        let (hw, hh, hl) = (self.half_extents.x, self.half_extents.y, self.half_extents.z);
        let (bx, by, bz) = (m.row(0), m.row(1), m.row(2));
        // z outer, x inner, the lower corner then the upper one — the order the hits are taken in
        let mut corners = [Vec3::ZERO; 8];
        let mut k = 0;
        for sz in [-1.0, 1.0] {
            for sx in [-1.0, 1.0] {
                corners[k] = self.position - by * hh + bz * (sz * hl) + bx * (sx * hw);
                corners[k + 1] = self.position + by * hh + bz * (sz * hl * 0.5) + bx * (sx * hw);
                k += 2;
            }
        }
        // `vel/32` once, before any impulse (0x49507D); the corners too (0x4950F7…0x495254)
        let v_tick = self.velocity / 32.0;
        let on_road = |q: Vec3, y: f32| patch_height(q).is_some_and(|h| (h - y).abs() <= 4.0);
        let mut max_j: f32 = 0.0;
        let mut max_effect: f32 = 0.0;
        let mut hit = false;
        for p in corners {
            if on_road(p, self.position.y) {
                continue;
            }
            // the segment's start, from the car's position as an earlier corner's push left it
            // (`car+0x330` is read per corner, 0x4952DF), and whether the tracker then holds a
            // patch, which `edgeHit` needs (0x4740D6)
            let from = if on_road(self.position, self.position.y) {
                self.position
            } else {
                let back = self.position - v_tick;
                if patch_height(back).is_none() {
                    continue;
                }
                back
            };
            // the candidate edges, in `0x4848B0`'s own leaf-then-insertion order for this exact
            // segment (`RE/RENDERING.md` "The tree, built") — `segment_hit`'s tie-break is "first
            // in this order", matching `0x474191…0x47419D`'s own `fcomp`/`test ah,1`
            let candidates = tree.query_segment((from.x, from.z), (p.x, p.z)).filter_map(|it| match it {
                crate::track_tree::TreeItem::Wall(i) => Some(i),
                _ => None,
            });
            let Some((n2, q, class)) = walls.segment_hit(from, p, candidates) else { continue };
            self.last_wall_class = class;
            // 0x495506: `car+0x440 = p`, the corner, after `WallImpulse` whatever it answered
            self.last_contact = p;
            let n = Vec3::new(n2.x, 0.0, n2.y);
            // `push = 1.01·(q − p)`, `push.y = 0` (0x495479…0x4954A4); `WallImpulse` 0x494000 runs
            // `updateZone(p + push, 0)` on a copy of the car's tracker and, when no drivable patch
            // contains that point (`(word & 0xF) == 0`, 0x494059…0x494063), pushes by `n·0`
            // (0x494065…0x494077), else by the push; then `×1.05` (0x4940A9). The `.edg` lines are
            // sides of the drivable triangles (every segment of monaco1/5, alps, skidpad), so a
            // corner pushed back across its own wall lands on the road; a push that lands off it —
            // past a joint, onto another wall — is dropped.
            let mut push = Vec3::new(q.x - p.x, 0.0, q.y - p.z) * 1.01;
            if patch_height(p + push).is_none() {
                push = Vec3::ZERO;
            }
            let push = push * 1.05;
            let j = -self.velocity.dot(n);
            if j <= 0.0 {
                continue;
            }
            // 0x4940F0…0x4941EC: the magnitude `WallImpulse` answers with is `j` for a hit
            // (`speedEst/8 ≤ j`) and `speedEst/12` (`[0x5B49B0]`) for a scrape, `speedEst` being
            // `car+0x35C` from before the impulse
            let speed_est = self.horizontal_speed();
            let mag = if speed_est * 0.125 > j { speed_est * (1.0 / 12.0) } else { j };
            self.velocity += n * (1.1 * j);
            self.position += push;
            for w in &mut self.wheels {
                w.world_pos += push;
            }
            self.wall_spin(j, n);
            self.post_hit();
            // `WallImpulse` returns `3·|mag|` (0x494356…0x494372); 0x495540…0x495563 hands it to
            // `Damage_byNormal` (with `car+0x430` = it) only **above 20** (`[0x5B2470]`) — after
            // the impulse, the push, `WallSpin` and `PostHit`, so the crash response's knock lands
            // on the spin `WallSpin` set. `car+0x354` only matters to a smackable's crash
            // response (0x494964).
            let effect = (3.0 * mag).abs();
            if effect > 20.0 {
                self.damage_by_normal(effect, n, 1, self.collision_mass);
            }
            max_j = max_j.max(j);
            max_effect = max_effect.max(effect);
            hit = true;
        }
        self.impact = self.impact.max(max_j);
        self.last_wall_effect = max_effect;
        // 0x49557D…0x495594: `car+0x430 = max(car+0x430, the pass's largest)`, whether or not a
        // corner hit; the word (0x49411E…0x4941DE) is written by a hit only
        self.contact_mag = self.contact_mag.max(max_effect);
        if hit {
            self.contact_kind = ContactKind::Wall;
        }
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
                    // 0x498F43: `car+0x430 = 15`, the word `0x50000 | car+0x44C`
                    self.contact_mag = 15.0;
                    self.contact_kind = ContactKind::PropOrCar;
                    // 0x498F33: `car+0x440 = car+0x330`
                    self.last_contact = self.position;
                    hit = Some((15.0, PropEvent::Prop(0)));
                    continue;
                }
                // a drive-through object (`BUSH`): above 2 m/s (`car+0x35C`, 0x498ED7…0x498EE8) the
                // event 0x6000F at 15 (0x498EF6…0x498F1A) and no push
                3 => {
                    if self.horizontal_speed() > 2.0 {
                        self.impact = self.impact.max(15.0);
                        // 0x498EF6: `car+0x430 = 15`, the word `0x6000F`
                        self.contact_mag = 15.0;
                        self.contact_kind = ContactKind::Bush;
                        // 0x498EF0: `car+0x440 = car+0x330`
                        self.last_contact = self.position;
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
                // 0x4984B8 / 0x498575 (the box), 0x498A44 / 0x498DB6 (the word's `0x50000 | class`)
                self.contact_mag = self.impact;
                self.contact_kind = ContactKind::PropOrCar;
            }
            self.impact = self.impact.max(before);
        }
        hit
    }

    /// 0x498AA0, read to the instruction 2026-09-26: everything in the x-z plane. It copies the
    /// car's position and the object's centre (slot 1) and **zeroes both `y`** (0x498B2C
    /// `[esp+0x5C]` = centre.y, 0x498B34 `[esp+0x38]` = pos.y) before `d = pos − centre`
    /// (0x532330); nothing under 0.01 (`|d|`, 0x498B46…0x498B59), `n = d/|d|` (0x5328D0), the
    /// x-z distance again (0x498B6F…0x498B9E). The car's reach along `n` is
    /// `min(hx / |n·x|, hz / |n·fwd|)` (10 for an axis within 0.1 of square); when the reach plus
    /// the radius exceeds the distance, the solid hit 0x498380 with the object's surface point
    /// `centre + n·R` and the car's `pos − n·reach`, both at `y = 0` — the normal turned round
    /// first when the car is past 5 m/s (`car+0x35C`), its velocity within 60° of `n` and the
    /// **distance** (`[esp+0x5C]` at 0x498D03, not the reach in `[esp+0x58]`) under 0.75.
    /// No height term anywhere: the candidate query (0x473870 → 0x484630 / 0x486510) and
    /// 0x498280 are x-z too, so a pole over a tunnel pushes a car below it sideways.
    /// *(Corrected 2026-09-26: the port took `d` and `n` in 3D — a pole 15 m above the car pushed
    /// it along a nearly vertical normal, vy 1.1 → 58.8 m/s on monaco5 — and tested the reach
    /// against 0.75.)*
    fn prop_cylinder(&mut self, c: Vec3, radius: f32) -> bool {
        let c = Vec3::new(c.x, 0.0, c.z);
        let pos = Vec3::new(self.position.x, 0.0, self.position.z);
        let d = pos - c;
        if d.length() <= 0.01 {
            return false;
        }
        let mut n = d.normalize();
        let dist = d.length();
        let (bx, bz) = (self.orientation.row(0), self.orientation.row(2));
        let ax = n.dot(bx).abs();
        let rx = if ax > 0.1 { self.half_extents.x / ax } else { 10.0 };
        let az = n.dot(bz).abs();
        let rz = if az > 0.1 { self.half_extents.z / az } else { 10.0 };
        let reach = rx.min(rz);
        if reach + radius <= dist {
            return false;
        }
        if self.horizontal_speed() > 5.0 && self.velocity.normalize_or_zero().dot(n) > 0.5 && dist < 0.75 {
            n = -n;
        }
        let a = c + n * radius;
        let b = pos - n * reach;
        self.solid_hit(n, a, b);
        // 0x498DCA: `car+0x440 = a`, the prop's surface point at `y = 0` (0x498DA5…0x498DD5)
        self.last_contact = a;
        true
    }

    /// 0x498690: the car's footprint edges in x-z — corners `pos ± hz·fwd ± hx·right` in the order
    /// `+F−R`, `+F+R`, `−F−R`, `−F+R`, edges (2,0), (0,1), (1,3), (3,2) (`0x5D0B6C`) — against the
    /// box's slot 6 (0x486D70, [`crate::props::box_push_edge_test`] — *until 2026-09-27 the camera's
    /// slot 7 0x486FB0 with the normal's sign reversed*): two crossings take the
    /// crossing's point and normal at once; one takes the
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
            let (count, point, n) = crate::props::box_push_edge_test(corners, p[a], p[b]);
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
        // (the box then writes its own event word over the ground's, 0x498A44 — no ground event)
        let _ = self.body_ground_collide_mass(self.sim_mass, n, point, 0.1);
        // 0x498A4D: `car+0x440 = car+0x330` (`ebp`, 0x49874B), over the centroid `GroundImpulse` left
        self.last_contact = self.position;
        true
    }

    /// `WallSpin` 0x493F10 (car, j, n), read to the instruction 2026-09-26: `car+0x434 = 0`; nothing
    /// when `j ≥ 15` (`[0x5B243C]`) **and** `car+0xD60` (the body-frame forward speed) `≥ 20`
    /// (`[0x5B2470]`, 0x493F14…0x493F41); else the world yaw rate `car+0x38C` = `0.25 ·
    /// min(|j|, |n · frame_x|)` (`[0x5B2468]`; `frame_x` the physics frame's row `car+0x490`,
    /// 0x493F5D…0x493FAD) with the sign from `j` and `nz = n · frame_z` (`car+0x4A8`,
    /// 0x493F4C…0x493F59): `j < 0` → `nz < 0 ? −spin : +spin` (0x493FD0…0x493FDD); `j ≥ 0` →
    /// `(nz > 0 ? −spin : +spin) · 0.5` (0x493FDF…0x493FEC, `[0x5B23F0]`). *(Until 2026-09-26 the
    /// port turned by the sign of `n · right` and of the forward speed, without the halving —
    /// the "sign table MEDIUM".)* The rate is set outright, in turns/s like `car+0x38C`.
    fn wall_spin(&mut self, j: f32, n: Vec3) {
        if j >= 15.0 && self.v_local.z >= 20.0 {
            return;
        }
        let nx = n.dot(self.frame.row(0));
        let nz = n.dot(self.frame.row(2));
        let spin = 0.25 * j.abs().min(nx.abs());
        self.angular_velocity.y = if j < 0.0 {
            if nz < 0.0 { -spin } else { spin }
        } else {
            (if nz > 0.0 { -spin } else { spin }) * 0.5
        };
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
        self.wall_spin(vclose, n);
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
}
