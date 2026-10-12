//! Shape of a segment: the surface mesh and the principal axes together,
//! and the figures derived from them (IBSI definitions).

use std::f64::consts::PI;

use ferrum_domain::{MaskRegion, Volume};

use crate::mesh::marching_cubes;
use crate::pca::{axes, Axes};

/// Surface and axes of a segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape3d {
    /// Area of the surface mesh in mm².
    pub area_mm2: f64,
    /// Volume enclosed by the surface mesh in mm³.
    pub mesh_volume_mm3: f64,
    /// Principal axes.
    pub axes: Axes,
}

/// The shape of `region` (non-empty, inside `volume`).
pub fn shape3d(region: &MaskRegion, volume: &Volume) -> Shape3d {
    let mesh = marching_cubes(region, volume.spacing());
    Shape3d { area_mm2: mesh.area_mm2(), mesh_volume_mm3: mesh.volume_mm3(), axes: axes(region, volume) }
}

impl Shape3d {
    /// Mesh volume and area, when both are positive.
    fn volume_and_area(&self) -> Option<(f64, f64)> {
        (self.mesh_volume_mm3 > 0.0 && self.area_mm2 > 0.0).then_some((self.mesh_volume_mm3, self.area_mm2))
    }

    /// `A / V` in 1/mm.
    pub fn surface_to_volume(&self) -> Option<f64> {
        self.volume_and_area().map(|(v, a)| a / v)
    }

    /// `V / (√π · A^(3/2))`.
    pub fn compactness_1(&self) -> Option<f64> {
        self.volume_and_area().map(|(v, a)| v / (PI.sqrt() * a.powf(1.5)))
    }

    /// `36 π V² / A³`.
    pub fn compactness_2(&self) -> Option<f64> {
        self.volume_and_area().map(|(v, a)| 36.0 * PI * v * v / a.powi(3))
    }

    /// `(36 π V²)^(1/3) / A`.
    pub fn sphericity(&self) -> Option<f64> {
        self.volume_and_area().map(|(v, a)| (36.0 * PI * v * v).cbrt() / a)
    }

    /// `A / (4 π R²)` with `R = (3 V / 4 π)^(1/3)`.
    pub fn spherical_disproportion(&self) -> Option<f64> {
        self.volume_and_area().map(|(v, a)| {
            let r = (3.0 * v / (4.0 * PI)).cbrt();
            a / (4.0 * PI * r * r)
        })
    }
}
