use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { position: Vec3::new(0.0, 5.0, 12.0), target: Vec3::ZERO, up: Vec3::Y, fov_y_radians: 60f32.to_radians(), near: 0.1, far: 5000.0 }
    }
}

impl Camera {
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.target, self.up)
    }
    pub fn projection(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y_radians, aspect, self.near, self.far)
    }
    /// The game world is left-handed (D3D: +x right, +y up, +z forward — CRP winding is clockwise
    /// seen from +y); rendering it through a right-handed look-at would mirror the scene, so the
    /// clip-space x is flipped here and the pipeline's front face is set accordingly.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.view_projection_mirrored(aspect, false)
    }

    /// `mirror` undoes the left-handed clip flip, i.e. draws the world mirrored (race option `MIRROR`).
    pub fn view_projection_mirrored(&self, aspect: f32, mirror: bool) -> Mat4 {
        Mat4::from_scale(Vec3::new(if mirror { 1.0 } else { -1.0 }, 1.0, 1.0)) * self.projection(aspect) * self.view()
    }
}
