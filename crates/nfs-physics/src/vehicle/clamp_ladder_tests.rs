use super::*;

#[test]
fn tyre_demand_clamp_follows_0x4a238e() {
    assert_eq!(tyre_demand_clamp(5.0, 3.0), 3.0);
    assert_eq!(tyre_demand_clamp(-5.0, 3.0), -3.0);
    assert_eq!(tyre_demand_clamp(1.0, 3.0), 1.0);
    // a negative limit: `d > L` takes `L`, everything else `−L` (no panic)
    assert_eq!(tyre_demand_clamp(0.5, -2.0), -2.0);
    assert_eq!(tyre_demand_clamp(-2.0, -2.0), 2.0);
    assert_eq!(tyre_demand_clamp(-7.0, -2.0), 2.0);
    assert_eq!(tyre_demand_clamp(f32::NAN, 3.0), -3.0);
}

#[test]
fn sign_clamp_follows_0x49a5a5() {
    assert_eq!(sign_clamp(0.3, 0.2), 0.2);
    assert_eq!(sign_clamp(-0.3, 0.2), -0.2);
    assert_eq!(sign_clamp(0.1, 0.2), 0.1);
    assert_eq!(sign_clamp(0.0, 0.2), 0.0);
    assert_eq!(sign_clamp(0.1, -0.2), -0.2);
    assert_eq!(sign_clamp(-0.1, -0.2), 0.2);
    assert_eq!(sign_clamp(0.0, -0.2), 0.2);
}
