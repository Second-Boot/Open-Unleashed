//! Start positions from `<track>_fstart.scn` / `_bstart.scn`.
//!
//! Status: HYPOTHESIS. The start files contain TRIGGER elements (each with a position, a 3×3
//! orientation and a 17-token parameter line) plus FNSH GEOM props. The first trigger's pose is
//! used as the player's start; the grid layout behind it is not yet recovered.

use glam::{Mat3, Quat, Vec3};
use nfs_formats::scn::{ElementKind, Scene};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StartPose {
    pub position: Vec3,
    pub rotation: Quat,
}

impl StartPose {
    /// Forward direction of the pose (local +Z, HYPOTHESIS: orientation rows are local axes).
    pub fn forward(&self) -> Vec3 {
        self.rotation * Vec3::Z
    }
}

pub fn start_poses(scene: &Scene) -> Vec<StartPose> {
    scene
        .of_kind(&ElementKind::Trigger)
        .map(|e| {
            let r = e.orientation.unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
            let m = Mat3::from_cols(Vec3::from(r[0]), Vec3::from(r[1]), Vec3::from(r[2]));
            StartPose { position: Vec3::from(e.position), rotation: Quat::from_mat3(&m).normalize() }
        })
        .collect()
}
