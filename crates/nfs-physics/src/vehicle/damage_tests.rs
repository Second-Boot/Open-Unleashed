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
    assert!(v.brakes_wear(&specs) > 0.0);
    assert_eq!(v.part_damage(&[false; 10]), 0.0);
}
