use glam::{Mat4, Vec3, Vec4};

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
    /// The right-handed look-at, written out as glam 0.30's `Mat4::look_at_rh` computed it (deprecated
    /// in glam 0.33, whose replacement is the same here), so the matrices — and every still — stay
    /// bit-identical across the glam update.
    pub fn view(&self) -> Mat4 {
        let f = (self.target - self.position).normalize();
        let s = f.cross(self.up).normalize();
        let u = s.cross(f);
        Mat4::from_cols(
            Vec4::new(s.x, u.x, -f.x, 0.0),
            Vec4::new(s.y, u.y, -f.y, 0.0),
            Vec4::new(s.z, u.z, -f.z, 0.0),
            Vec4::new(-self.position.dot(s), -self.position.dot(u), self.position.dot(f), 1.0),
        )
    }
    /// The right-handed `[0, 1]`-depth perspective of glam 0.30's `Mat4::perspective_rh`, written out
    /// with its own operations (`r = far / (near − far)`): glam 0.33's replacement
    /// (`camera::rh::proj::directx::perspective`) computes `−far · (1 / (far − near))`, which can
    /// differ in the last bit.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let (sin_fov, cos_fov) = (0.5 * self.fov_y_radians).sin_cos();
        let h = cos_fov / sin_fov;
        let w = h / aspect;
        let r = self.far / (self.near - self.far);
        Mat4::from_cols(Vec4::new(w, 0.0, 0.0, 0.0), Vec4::new(0.0, h, 0.0, 0.0), Vec4::new(0.0, 0.0, r, -1.0), Vec4::new(0.0, 0.0, r * self.near, 0.0))
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
