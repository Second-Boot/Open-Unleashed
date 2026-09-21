//! How much tyre squeal the port produces in ordinary driving — the report was "a constant
//! wheelspin sound while driving". Prints, per phase, the share of ticks at which the squeal
//! producer (sfx.md §5.1, 0x40FBC0) would be at full volume and the mean front/rear slip sums.
use glam::{Mat3, Vec3};
use nfs_physics::collision::{Triangle, TriangleGrid};
use nfs_physics::specs::CarSpecs;
use nfs_physics::vehicle::{Controls, Vehicle};

fn main() {
    let s = 5000.0;
    let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
    let grid = TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 200.0);
    let p = std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
        .join("Games/NFS-Porsche-Unleashed/game_info/data/GameData/Simulation/CarData/993coupe36.sim");
    let specs = CarSpecs::from_sim(&nfs_formats::sim::CarSim::parse(&std::fs::read(p).unwrap()).unwrap(), false);
    let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
    let phases: &[(&str, Controls, f32)] = &[
        ("settle (brake)", Controls { brake: 254, ..Default::default() }, 1.5),
        ("full throttle 0-6 s", Controls { throttle: 254, ..Default::default() }, 6.0),
        ("half throttle cruise", Controls { throttle: 127, ..Default::default() }, 6.0),
        ("light throttle cruise", Controls { throttle: 60, ..Default::default() }, 6.0),
        ("coast", Controls::default(), 4.0),
        ("gentle right turn, half thr", Controls { throttle: 127, steer: 40, ..Default::default() }, 4.0),
    ];
    // time-resolved: a keyboard player's "driving" is full throttle, so watch it for 25 s
    if std::env::args().any(|a| a == "--timeline") {
        let full = Controls { throttle: 254, ..Default::default() };
        let settle = Controls { brake: 254, ..Default::default() };
        for t in 0..(1.5 * 64.0) as u32 { if t % 2 == 0 { v.step32(&settle, &specs, &grid); } v.step64(&specs); }
        for sec in 0..25 {
            let (mut loud, mut spin, mut n) = (0u32, 0u32, 0u32);
            for t in 0..64u32 {
                if t % 2 == 0 { v.step32(&full, &specs, &grid); }
                v.step64(&specs);
                if t % 32 == 0 { v.renormalise(); }
                let rear = v.wheels[2].slip + v.wheels[3].slip;
                loud += (((rear.min(10.0) * 0.5) * 127.0).min(127.0) >= 64.0) as u32;
                spin += v.rear_spin as u32;
                n += 1;
            }
            println!("t={sec:2}-{:2}s  {:5.1} km/h  gear {}  rpm {:5.0}  rear loud {:3.0}%  rear_spin {:3.0}%", sec + 1, v.speed() * 3.6, v.gear, v.rpm, 100.0 * loud as f32 / n as f32, 100.0 * spin as f32 / n as f32);
        }
        return;
    }
    for (name, c, secs) in phases {
        let (mut n, mut loud_f, mut loud_r, mut sf, mut sr, mut spin) = (0u32, 0u32, 0u32, 0.0f32, 0.0f32, 0u32);
        for t in 0..(secs * 64.0) as u32 {
            if t % 2 == 0 {
                v.step32(c, &specs, &grid);
            }
            v.step64(&specs);
            if t % 32 == 0 {
                v.renormalise();
            }
            let front = v.wheels[0].slip + v.wheels[1].slip;
            let rear = v.wheels[2].slip + v.wheels[3].slip;
            // sfx.md §5.3: level = min(vol * 127, 127); rear is halved; front is dropped when rear is 0
            let lf = if rear > 0.0 { (front.min(10.0) * 127.0).min(127.0) } else { 0.0 };
            let lr = ((rear.min(10.0) * 0.5) * 127.0).min(127.0);
            n += 1;
            loud_f += (lf >= 64.0) as u32;
            loud_r += (lr >= 64.0) as u32;
            sf += front;
            sr += rear;
            spin += v.rear_spin as u32;
        }
        println!(
            "{name:28} {:5.1} km/h gear {}  rear loud {:3.0}%  front loud {:3.0}%  mean slip F {:.2} R {:.2}  rear_spin {:3.0}%",
            v.speed() * 3.6, v.gear, 100.0 * loud_r as f32 / n as f32, 100.0 * loud_f as f32 / n as f32,
            sf / n as f32, sr / n as f32, 100.0 * spin as f32 / n as f32
        );
    }
}
