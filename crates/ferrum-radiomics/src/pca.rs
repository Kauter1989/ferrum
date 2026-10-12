//! Principal axes of a segment: the lengths of its three axes, how much
//! longer one is than another, and which way they point.
//!
//! The axes come from the covariance of the voxel centres in patient (LPS)
//! millimetres: with eigenvalues `λ1 ≥ λ2 ≥ λ3`, the major, minor and least
//! axis lengths are `4 √λ` (the IBSI definition; for a solid ellipsoid with
//! semi-axis `a` the length is `4 a / √5`). The covariance is the
//! population covariance (divisor `N`). It is computed on voxel indices and
//! then carried to millimetres, so it is exact for any spacing and
//! direction matrix.

use ferrum_domain::{MaskRegion, Volume};
use glam::DMat3;

/// A symmetric 3 × 3 matrix is diagonal enough below this share of its norm.
const OFF_DIAGONAL: f64 = 1e-14;
/// Eigenvalues closer than this share of the largest are one axis length.
const SAME_LENGTH: f64 = 1e-9;

/// Principal axes of a segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axes {
    /// Eigenvalues of the covariance, largest first, in mm².
    pub eigenvalues: [f64; 3],
    /// Major, minor and least axis length in mm.
    pub lengths_mm: [f64; 3],
    /// Unit vectors of the major, minor and least axis in LPS, with the
    /// sign chosen so that the largest component is positive; `None` where
    /// the direction is not unique (two axes are equally long, or the
    /// segment has no extent along them).
    pub directions: [Option<[f64; 3]>; 3],
}

impl Axes {
    /// `√(λ2 / λ1)`: how much shorter the minor axis is; `None` for a
    /// segment with no extent at all.
    pub fn elongation(&self) -> Option<f64> {
        self.ratio(1)
    }

    /// `√(λ3 / λ1)`: how much shorter the least axis is; `None` for a
    /// segment with no extent at all.
    pub fn flatness(&self) -> Option<f64> {
        self.ratio(2)
    }

    fn ratio(&self, axis: usize) -> Option<f64> {
        (self.eigenvalues[0] > 0.0).then(|| (self.eigenvalues[axis] / self.eigenvalues[0]).max(0.0).sqrt())
    }
}

/// Eigenvalues and eigenvectors (as columns) of a symmetric 3 × 3 matrix by
/// cyclic Jacobi rotations; the order of the operations is fixed, so the
/// result is the same on every machine.
fn jacobi(mut a: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let norm: f64 = a.iter().flatten().map(|x| x * x).sum::<f64>().sqrt();
    for _ in 0..64 {
        let off = a[0][1].powi(2) + a[0][2].powi(2) + a[1][2].powi(2);
        if off.sqrt() <= OFF_DIAGONAL * norm {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q] == 0.0 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let (c, s) = (1.0 / (t * t + 1.0).sqrt(), t / (t * t + 1.0).sqrt());
            for row in &mut a {
                let (akp, akq) = (row[p], row[q]);
                row[p] = c * akp - s * akq;
                row[q] = s * akp + c * akq;
            }
            let (row_p, row_q) = (a[p], a[q]);
            for (k, (apk, aqk)) in row_p.iter().zip(row_q.iter()).enumerate() {
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for row in &mut v {
                let (vp, vq) = (row[p], row[q]);
                row[p] = c * vp - s * vq;
                row[q] = s * vp + c * vq;
            }
        }
    }
    ([a[0][0], a[1][1], a[2][2]], v)
}

/// The covariance of the voxel indices of `region` (divisor `N`).
fn index_covariance(region: &MaskRegion) -> [[f64; 3]; 3] {
    let s = region.size();
    let (nx, ny) = (s.x as usize, s.y as usize);
    let at = |n: usize| [(n % nx) as f64, ((n / nx) % ny) as f64, (n / (nx * ny)) as f64];
    let (mut count, mut sum) = (0.0, [0.0; 3]);
    for n in region.inside.iter().enumerate().filter(|(_, v)| **v).map(|(n, _)| n) {
        let p = at(n);
        count += 1.0;
        (0..3).for_each(|a| sum[a] += p[a]);
    }
    let mean = sum.map(|x| x / count);
    let mut cov = [[0.0; 3]; 3];
    for n in region.inside.iter().enumerate().filter(|(_, v)| **v).map(|(n, _)| n) {
        let p = at(n);
        for r in 0..3 {
            for c in 0..3 {
                cov[r][c] += (p[r] - mean[r]) * (p[c] - mean[c]);
            }
        }
    }
    cov.map(|row| row.map(|x| x / count))
}

/// The unit vector with its largest component positive.
fn sign_normalised(v: [f64; 3]) -> [f64; 3] {
    let mut at = 0;
    for i in 1..3 {
        if v[i].abs() > v[at].abs() + 1e-12 {
            at = i;
        }
    }
    if v[at] < 0.0 {
        v.map(|x| -x)
    } else {
        v
    }
}

/// Principal axes of `region` (non-empty) on `volume`.
pub fn axes(region: &MaskRegion, volume: &Volume) -> Axes {
    let g = volume.geometry();
    let spacing = volume.spacing().as_dvec3();
    // millimetres in patient space = direction * diag(spacing) * index
    let m = DMat3::from_cols(
        g.direction.x_axis.as_dvec3() * spacing.x,
        g.direction.y_axis.as_dvec3() * spacing.y,
        g.direction.z_axis.as_dvec3() * spacing.z,
    );
    let c = index_covariance(region);
    let c = DMat3::from_cols_array(&[c[0][0], c[1][0], c[2][0], c[0][1], c[1][1], c[2][1], c[0][2], c[1][2], c[2][2]]);
    let p = m * c * m.transpose();
    let cols = p.to_cols_array_2d();
    let (values, vectors) = jacobi([
        [cols[0][0], cols[1][0], cols[2][0]],
        [cols[0][1], cols[1][1], cols[2][1]],
        [cols[0][2], cols[1][2], cols[2][2]],
    ]);
    let mut order = [0usize, 1, 2];
    order.sort_by(|&a, &b| values[b].total_cmp(&values[a]));
    let eigenvalues = order.map(|i| values[i].max(0.0));
    let largest = eigenvalues[0];
    let directions = std::array::from_fn(|r| {
        let apart = |other: usize| (eigenvalues[r] - eigenvalues[other]).abs() > SAME_LENGTH * largest;
        let unique = largest > 0.0 && (0..3).filter(|&o| o != r).all(apart);
        let i = order[r];
        unique.then(|| sign_normalised([vectors[0][i], vectors[1][i], vectors[2][i]]))
    });
    Axes { eigenvalues, lengths_mm: eigenvalues.map(|l| 4.0 * l.sqrt()), directions }
}

#[cfg(test)]
mod tests {
    use ferrum_domain::{Dims3, Geometry, Volume};
    use glam::{Mat3, Vec3};

    use super::*;

    fn volume(dims: [u32; 3], spacing: [f32; 3]) -> Volume {
        let d = Dims3::new(dims[0], dims[1], dims[2]);
        Volume::from_physical(d, Vec3::from(spacing), &vec![0.0; d.voxel_count()]).unwrap()
    }

    fn region(dims: [u32; 3], inside: impl Fn(f64, f64, f64) -> bool) -> MaskRegion {
        let d = Dims3::new(dims[0], dims[1], dims[2]);
        MaskRegion::from_grid(d, |n| {
            let n = n as u32;
            inside(f64::from(n % d.x), f64::from((n / d.x) % d.y), f64::from(n / (d.x * d.y)))
        })
        .unwrap()
    }

    /// A solid ellipsoid with semi-axes (mm) along the grid axes.
    fn ellipsoid(semi_mm: [f64; 3], spacing: [f32; 3]) -> (MaskRegion, Volume) {
        let n = semi_mm.iter().zip(spacing).map(|(a, s)| (2.0 * a / f64::from(s)) as u32 + 5).collect::<Vec<_>>();
        let dims = [n[0], n[1], n[2]];
        let centre = dims.map(|d| f64::from(d - 1) / 2.0);
        let r = region(dims, |i, j, k| {
            let p = [i, j, k];
            (0..3).map(|a| ((p[a] - centre[a]) * f64::from(spacing[a]) / semi_mm[a]).powi(2)).sum::<f64>() <= 1.0
        });
        (r, volume(dims, spacing))
    }

    fn close(a: f64, b: f64, tol: f64, what: &str) {
        assert!((a - b).abs() <= tol * b.abs().max(1.0), "{what}: {a} != {b}");
    }

    // covers 18.6-a, 18.6-b
    #[test]
    fn an_ellipsoid_has_the_axes_of_its_semi_axes() {
        // semi-axes 5 (i), 12 (j), 8 (k) mm: the axis length is 4 a / sqrt(5)
        let (r, v) = ellipsoid([5.0, 12.0, 8.0], [1.0, 1.0, 1.0]);
        let a = axes(&r, &v);
        let expected = |semi: f64| 4.0 * semi / 5.0_f64.sqrt();
        close(a.lengths_mm[0], expected(12.0), 0.03, "major");
        close(a.lengths_mm[1], expected(8.0), 0.03, "minor");
        close(a.lengths_mm[2], expected(5.0), 0.03, "least");
        close(a.elongation().unwrap(), 8.0 / 12.0, 0.03, "elongation");
        close(a.flatness().unwrap(), 5.0 / 12.0, 0.03, "flatness");
        // the same object at anisotropic spacing: lengths are in millimetres
        let (r, v) = ellipsoid([5.0, 12.0, 8.0], [1.0, 1.0, 2.0]);
        let b = axes(&r, &v);
        close(b.lengths_mm[0], expected(12.0), 0.04, "major, 2 mm slices");
        close(b.lengths_mm[2], expected(5.0), 0.04, "least, 2 mm slices");
    }

    // covers 18.6-d
    #[test]
    fn directions_are_unit_vectors_in_lps_with_a_fixed_sign() {
        let (r, v) = ellipsoid([5.0, 12.0, 8.0], [1.0, 1.0, 1.0]);
        let a = axes(&r, &v);
        let expected = [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]];
        for (got, want) in a.directions.iter().zip(expected) {
            let got = got.expect("the three axes differ in length");
            assert!((0..3).all(|i| (got[i] - want[i]).abs() < 1e-6), "{got:?} != {want:?}");
        }
        // a grid whose j axis points to -x (and i to +y): the major axis lies along x in the patient
        let turned = v
            .clone()
            .with_geometry(Geometry { origin: Vec3::ZERO, direction: Mat3::from_cols(Vec3::Y, -Vec3::X, Vec3::Z) });
        let major = axes(&r, &turned).directions[0].unwrap();
        assert!(
            (major[0] - 1.0).abs() < 1e-6 && major[1].abs() < 1e-6 && major[2].abs() < 1e-6,
            "{major:?}: -x flips to +x"
        );
        // the same input gives the same bits
        assert_eq!(axes(&r, &v), axes(&r, &v));
    }

    // covers 18.6-c
    #[test]
    fn a_line_a_plane_and_a_point_are_undefined_where_they_have_no_extent_and_zero_where_they_are_flat() {
        let v = volume([12, 12, 12], [1.0, 1.0, 1.0]);
        // a line of 9 voxels along i: variance (9^2 - 1) / 12
        let line = axes(&region([12, 12, 12], |i, j, k| j == 3.0 && k == 4.0 && i < 9.0), &v);
        close(line.lengths_mm[0], 4.0 * (80.0_f64 / 12.0).sqrt(), 1e-9, "line");
        assert_eq!((line.lengths_mm[1], line.lengths_mm[2]), (0.0, 0.0));
        assert_eq!((line.elongation(), line.flatness()), (Some(0.0), Some(0.0)));
        assert!(line.directions[0].is_some() && line.directions[1].is_none() && line.directions[2].is_none());
        // a square plate in the i-j plane: no extent along k, the other two axes are equally long
        let plate = axes(&region([12, 12, 12], |i, j, k| k == 2.0 && i < 8.0 && j < 8.0), &v);
        assert_eq!(plate.lengths_mm[2], 0.0);
        assert_eq!(plate.flatness(), Some(0.0));
        assert!(plate.directions[0].is_none() && plate.directions[1].is_none(), "major and minor are equally long");
        let least = plate.directions[2].expect("the plate's normal is unique");
        assert!((least[2] - 1.0).abs() < 1e-9, "{least:?}");
        // a single voxel has no extent at all
        let point = axes(&region([12, 12, 12], |i, j, k| (i, j, k) == (3.0, 4.0, 5.0)), &v);
        assert_eq!(point.lengths_mm, [0.0; 3]);
        assert_eq!((point.elongation(), point.flatness()), (None, None));
        assert!(point.directions.iter().all(Option::is_none));
    }

    // covers 18.6-d
    #[test]
    fn the_sign_of_a_direction_is_fixed_by_its_largest_component() {
        assert_eq!(sign_normalised([-0.1, 0.9, 0.0]), [-0.1, 0.9, 0.0], "the largest component is positive already");
        assert_eq!(sign_normalised([0.1, -0.9, 0.0]), [-0.1, 0.9, 0.0], "the largest component is negative: flip");
        assert_eq!(sign_normalised([-0.3, 0.3, 0.0]), [0.3, -0.3, 0.0], "a tie is decided by the first axis");
        assert_eq!(sign_normalised([0.3, -0.3, 0.0]), [0.3, -0.3, 0.0]);
        assert_eq!(sign_normalised([0.0, 0.0, -1.0]), [0.0, 0.0, 1.0]);
    }

    // covers 18.6-d
    #[test]
    fn an_object_turned_by_45_degrees_gets_a_fixed_direction_whichever_way_jacobi_points() {
        // the grid is turned about the patient's z axis, so the 12 mm axis (grid j) lies along
        // (-sin 45, cos 45, 0): both signs are the same axis, the rule picks x positive
        let (r, v) = ellipsoid([5.0, 12.0, 8.0], [1.0, 1.0, 1.0]);
        let (sin, cos) = (std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);
        let direction = Mat3::from_cols(Vec3::new(cos, sin, 0.0), Vec3::new(-sin, cos, 0.0), Vec3::Z);
        let turned = v.with_geometry(Geometry { origin: Vec3::ZERO, direction });
        let major = axes(&r, &turned).directions[0].expect("a unique major axis");
        let h = std::f64::consts::FRAC_1_SQRT_2;
        assert!((major[0] - h).abs() < 1e-6 && (major[1] + h).abs() < 1e-6 && major[2].abs() < 1e-6, "{major:?}");
    }

    #[test]
    fn jacobi_diagonalises_a_known_matrix() {
        // eigenvalues of [[2,1,0],[1,2,0],[0,0,5]] are 1, 3 and 5
        let (values, vectors) = jacobi([[2.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 0.0, 5.0]]);
        let mut sorted = values;
        sorted.sort_by(f64::total_cmp);
        assert!((sorted[0] - 1.0).abs() < 1e-12 && (sorted[1] - 3.0).abs() < 1e-12 && (sorted[2] - 5.0).abs() < 1e-12);
        // columns are orthonormal
        for a in 0..3 {
            for b in 0..3 {
                let dot: f64 = (0..3).map(|k| vectors[k][a] * vectors[k][b]).sum();
                assert!((dot - f64::from(u8::from(a == b))).abs() < 1e-12);
            }
        }
    }
}
