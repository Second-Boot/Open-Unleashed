use glam::{DMat4, DVec3, DVec4, Mat4, Vec3, Vec4};

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
}

/// [`Camera::default`]: a viewer's start (`tools/`), up and back from the origin, looking at it.
const DEFAULT_POSITION: Vec3 = Vec3::new(0.0, 5.0, 12.0);
const DEFAULT_FOV_Y_DEG: f32 = 60.0;
const DEFAULT_NEAR: f32 = 0.1;
const DEFAULT_FAR: f32 = 5000.0;

impl Default for Camera {
    fn default() -> Self {
        Self { position: DEFAULT_POSITION, target: Vec3::ZERO, up: Vec3::Y, fov_y_radians: DEFAULT_FOV_Y_DEG.to_radians(), near: DEFAULT_NEAR, far: DEFAULT_FAR }
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

// ---------------------------------------------------------------------------------------------
// The camera's matrices in f64 (the motion blur's reprojection, ENHANCEMENT ENH-TEMPORAL's motion vectors)

/// `prev VP * inverse(VP)` in f64 (the world's coordinates are in the thousands of metres, so the two
/// matrices' product is cancelled out in double precision), for the main view of `aspect` (`mirror`
/// = the `MIRROR` race option's flip, which both views share).
pub(crate) fn reprojection(cur: &Camera, prev: &Camera, aspect: f32, mirror: bool) -> Mat4 {
    let r = vp64(prev, aspect, mirror) * vp64(cur, aspect, mirror).inverse();
    Mat4::from_cols_array(&r.to_cols_array().map(|v| v as f32))
}

/// `Camera::view_projection_mirrored` in f64.
pub(crate) fn vp64(c: &Camera, aspect: f32, mirror: bool) -> DMat4 {
    vp64_at(c, aspect, mirror, false)
}

/// [`vp64`], or with `rotation_only` the same view's matrix with the eye at the origin (ENHANCEMENT
/// ENH-TEMPORAL: the sky moves with the camera's rotation alone).
pub(crate) fn vp64_at(c: &Camera, aspect: f32, mirror: bool, rotation_only: bool) -> DMat4 {
    let (pos, target, up) = (c.position.as_dvec3(), c.target.as_dvec3(), c.up.as_dvec3());
    let f = (target - pos).normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    let eye = if rotation_only { DVec3::ZERO } else { pos };
    let view = DMat4::from_cols(DVec4::new(s.x, u.x, -f.x, 0.0), DVec4::new(s.y, u.y, -f.y, 0.0), DVec4::new(s.z, u.z, -f.z, 0.0), DVec4::new(-eye.dot(s), -eye.dot(u), eye.dot(f), 1.0));
    let (near, far) = (c.near as f64, c.far as f64);
    let (sin_fov, cos_fov) = (0.5 * c.fov_y_radians as f64).sin_cos();
    let h = cos_fov / sin_fov;
    let w = h / aspect as f64;
    let r = far / (near - far);
    let proj = DMat4::from_cols(DVec4::new(w, 0.0, 0.0, 0.0), DVec4::new(0.0, h, 0.0, 0.0), DVec4::new(0.0, 0.0, r, -1.0), DVec4::new(0.0, 0.0, r * near, 0.0));
    DMat4::from_scale(DVec3::new(if mirror { 1.0 } else { -1.0 }, 1.0, 1.0)) * proj * view
}

/// PERF: the largest difference from the identity of a reprojection that cannot move any pixel. The shader returns the
/// pixel's own colour when the uv motion's length, `|(ndc - reprojected ndc) / 2|` (x by the aspect) times the shutter (<= 1),
/// is under 1e-5; an error of `e` in each matrix entry moves a reprojected NDC point (|x|, |y|, |z| <= 1, w = 1) by at most
/// about `8 e`, so 1e-7 keeps the length below 1e-5 at any aspect up to 10 with room for the shader's own f32 rounding.
const STILL_REPROJECTION: f32 = 1e-7;

/// PERF: the blur has nothing to do this frame: the camera has not moved since `prev` (the reprojection is the identity
/// within [`STILL_REPROJECTION`]: paused, the photo mode, a standstill). Every pixel would come out as the pass's input,
/// so the chain skips the pass and the passes after it take the input itself.
pub(crate) fn camera_still(cur: &Camera, prev: &Camera, aspect: f32, mirror: bool) -> bool {
    let m = reprojection(cur, prev, aspect, mirror).to_cols_array();
    m.iter().zip(Mat4::IDENTITY.to_cols_array()).all(|(a, b)| (a - b).abs() < STILL_REPROJECTION)
}
