//! Orbit camera used by the 3D view.

use glam::{Mat4, Quat, Vec2, Vec3};

use crate::geometry::Ray;

/// Perspective camera orbiting a target point.
///
/// The camera frame is stored as a quaternion `orientation`; the eye sits at
/// `target + orientation * (0, 0, distance)` and looks at the target with
/// `orientation * Y` as up vector. Rotations are applied in the camera's
/// local frame, which avoids gimbal lock (trackball behaviour).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    /// Camera frame orientation.
    pub orientation: Quat,
    /// Distance from target to eye.
    pub distance: f32,
    /// Orbit centre in model space.
    pub target: Vec3,
    /// Vertical field of view in radians.
    pub fov_y: f32,
}

impl OrbitCamera {
    /// Minimum orbit distance.
    pub const MIN_DISTANCE: f32 = 0.2;
    /// Maximum orbit distance.
    pub const MAX_DISTANCE: f32 = 10.0;
    /// Near clipping plane.
    pub const NEAR: f32 = 0.01;
    /// Far clipping plane.
    pub const FAR: f32 = 20.0;

    /// Default view: looking along +Z (from slice 0 towards the last slice)
    /// with image rows pointing down, i.e. the radiological axial view.
    pub fn default_orientation() -> Quat {
        Quat::from_rotation_x(std::f32::consts::PI)
    }

    /// Eye position.
    pub fn eye(&self) -> Vec3 {
        self.target + self.orientation * Vec3::new(0.0, 0.0, self.distance)
    }

    /// Up vector.
    pub fn up(&self) -> Vec3 {
        self.orientation * Vec3::Y
    }

    /// Right vector.
    pub fn right(&self) -> Vec3 {
        self.orientation * Vec3::X
    }

    /// Unit viewing direction (eye → target).
    pub fn view_dir(&self) -> Vec3 {
        self.orientation * Vec3::NEG_Z
    }

    /// World-to-camera matrix.
    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, self.up())
    }

    /// Projection matrix (right-handed, depth `[0, 1]` as used by wgpu).
    pub fn projection(&self, aspect: f32) -> Mat4 {
        glam::camera::rh::proj::directx::perspective(self.fov_y, aspect.max(1e-4), Self::NEAR, Self::FAR)
    }

    /// Combined projection × view.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    /// Primary ray through normalised device coordinates `ndc ∈ [-1, 1]²`
    /// (`+y` up).
    pub fn ray(&self, ndc: Vec2, aspect: f32) -> Ray {
        let inv = self.view_projection(aspect).inverse();
        let far = inv.project_point3(ndc.extend(1.0));
        let eye = self.eye();
        Ray::new(eye, far - eye)
    }

    /// Orbits by `dx`, `dy` radians around the camera's up and right axes.
    pub fn rotate(&mut self, dx: f32, dy: f32) {
        let q = self.orientation * Quat::from_rotation_y(-dx) * Quat::from_rotation_x(-dy);
        self.orientation = q.normalize();
    }

    /// Multiplies the distance by `exp(-amount)` (positive = zoom in).
    pub fn zoom(&mut self, amount: f32) {
        self.distance = (self.distance * (-amount).exp()).clamp(Self::MIN_DISTANCE, Self::MAX_DISTANCE);
    }

    /// Moves the target in the view plane by fractions of the visible height.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let h = 2.0 * self.distance * (self.fov_y * 0.5).tan();
        self.target += (-self.right() * dx + self.up() * dy) * h;
    }

    /// Direction the light travels, in model space: a head-light placed at
    /// the upper-left of the camera.
    pub fn light_dir(&self) -> Vec3 {
        (self.orientation * Vec3::new(0.5, -0.5, -1.0)).normalize()
    }

    /// Resets to the default view.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self { orientation: Self::default_orientation(), distance: 1.6, target: Vec3::ZERO, fov_y: 60f32.to_radians() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_looks_along_positive_z_with_rows_down() {
        let c = OrbitCamera::default();
        assert!((c.view_dir() - Vec3::Z).length() < 1e-6);
        assert!((c.up() - Vec3::NEG_Y).length() < 1e-6);
        assert!((c.right() - Vec3::X).length() < 1e-6);
    }

    #[test]
    fn centre_ray_hits_target() {
        let c = OrbitCamera::default();
        let r = c.ray(Vec2::ZERO, 1.5);
        assert!((r.direction - c.view_dir()).length() < 1e-4);
        // A point at the top of the screen projects "up" in the camera frame.
        let top = c.ray(Vec2::new(0.0, 0.9), 1.0);
        assert!(top.direction.dot(c.up()) > 0.0);
    }

    #[test]
    fn rotation_preserves_distance() {
        let mut c = OrbitCamera::default();
        for _ in 0..100 {
            c.rotate(0.13, -0.07);
        }
        assert!(((c.eye() - c.target).length() - c.distance).abs() < 1e-4);
        assert!((c.orientation.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut c = OrbitCamera::default();
        c.zoom(100.0);
        assert_eq!(c.distance, OrbitCamera::MIN_DISTANCE);
        c.zoom(-100.0);
        assert_eq!(c.distance, OrbitCamera::MAX_DISTANCE);
    }

    #[test]
    fn pan_moves_target_in_view_plane() {
        let mut c = OrbitCamera::default();
        c.pan(0.1, 0.0);
        assert!(c.target.dot(c.view_dir()).abs() < 1e-6);
        assert!(c.target.length() > 0.0);
        c.reset();
        assert_eq!(c, OrbitCamera::default());
    }

    #[test]
    fn light_comes_from_the_viewer_side() {
        let c = OrbitCamera::default();
        assert!(c.light_dir().dot(c.view_dir()) > 0.0);
    }
}
