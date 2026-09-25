//! Vehicle simulation and collision.
//!
//! `vehicle` reproduces the original car model recovered in `RE/notes/physics.md` (32 Hz force
//! step, 64 Hz integration, angles in turns, torque curve at 500 rpm spacing, converted gear
//! ratios, drag/brake/load-transfer/tyre/friction-circle laws). `collision` is a native
//! triangle grid used for the ground query the original performs through its own track structures.
pub mod collision;
pub mod props;
pub mod smackable;
pub mod specs;
pub mod vehicle;
pub mod walls;
