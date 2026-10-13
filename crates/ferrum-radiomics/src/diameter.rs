//! Maximum 3D diameter: the largest distance between two voxel centres.
//!
//! The distance from a fixed point along a line of voxels is largest at one
//! end of the line, so the farthest pair is searched among the end voxels
//! of the lines along the longest side of the box, and then among the
//! corners of their hull ([`farthest_pair`]).

use ferrum_domain::MaskRegion;
use glam::Vec3;

use crate::hull::farthest_pair;

/// Voxel centres, in mm from the first voxel of the box, of the first and
/// last voxel of every line of voxels along the longest side of the box.
fn line_ends(region: &MaskRegion, spacing: Vec3) -> Vec<[f64; 3]> {
    let size = region.size().to_array().map(|s| s as usize);
    let axis = (0..3).max_by_key(|a| (size[*a], 2 - *a)).unwrap_or(0);
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let mut ends = vec![(usize::MAX, 0usize); size[u] * size[v]];
    for (n, inside) in region.inside.iter().enumerate() {
        if *inside {
            let at = [n % size[0], (n / size[0]) % size[1], n / (size[0] * size[1])];
            let line = &mut ends[at[u] + size[u] * at[v]];
            line.0 = line.0.min(at[axis]);
            line.1 = line.1.max(at[axis]);
        }
    }
    let mm = |line: usize, along: usize| {
        let mut at = [0usize; 3];
        at[axis] = along;
        at[u] = line % size[u];
        at[v] = line / size[u];
        [0, 1, 2].map(|a| at[a] as f64 * f64::from(spacing[a]))
    };
    let mut points = Vec::new();
    for (line, (first, last)) in ends.iter().enumerate().filter(|(_, e)| e.0 != usize::MAX) {
        points.push(mm(line, *first));
        if last != first {
            points.push(mm(line, *last));
        }
    }
    points
}

/// The largest distance between two voxel centres of `region` in mm; zero
/// for a single voxel. `region` is non-empty.
pub fn max_diameter_mm(region: &MaskRegion, spacing: Vec3) -> f64 {
    farthest_pair(&line_ends(region, spacing))
}

#[cfg(test)]
mod tests {
    use ferrum_domain::Dims3;
    use glam::UVec3;

    use super::*;

    fn region(dims: [u32; 3], inside: impl Fn(UVec3) -> bool) -> MaskRegion {
        let d = Dims3 { x: dims[0], y: dims[1], z: dims[2] };
        MaskRegion::from_grid(d, |n| {
            let n = n as u32;
            inside(UVec3::new(n % dims[0], (n / dims[0]) % dims[1], n / (dims[0] * dims[1])))
        })
        .unwrap()
        .cropped()
        .unwrap()
    }

    fn brute_force(region: &MaskRegion, spacing: Vec3) -> f64 {
        let s = region.size();
        let centres: Vec<[f64; 3]> = region
            .inside
            .iter()
            .enumerate()
            .filter(|(_, v)| **v)
            .map(|(n, _)| {
                let n = n as u32;
                let l = [n % s.x, (n / s.x) % s.y, n / (s.x * s.y)];
                [0, 1, 2].map(|a| f64::from(l[a]) * f64::from(spacing[a]))
            })
            .collect();
        let mut best = 0.0f64;
        for (n, a) in centres.iter().enumerate() {
            for b in &centres[n + 1..] {
                best = best.max(((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt());
            }
        }
        best
    }

    // covers 18.2-c, 18.2-e
    #[test]
    fn the_diameter_of_a_box_is_its_space_diagonal() {
        let spacing = Vec3::new(0.7, 0.7, 2.5);
        let r = region([10, 10, 10], |p| p.cmpge(UVec3::new(1, 2, 3)).all() && p.cmplt(UVec3::new(5, 7, 9)).all());
        // 4 x 5 x 6 voxels: the centres span 3, 4 and 5 steps (the spacing is stored as f32)
        let expected = ((3.0f64 * 0.7).powi(2) + (4.0f64 * 0.7).powi(2) + (5.0f64 * 2.5).powi(2)).sqrt();
        assert!(
            (max_diameter_mm(&r, spacing) - expected).abs() < 1e-6,
            "{} != {expected}",
            max_diameter_mm(&r, spacing)
        );
    }

    // covers 18.2-c
    #[test]
    fn the_diameter_equals_a_brute_force_search_on_random_blobs() {
        for (seed, spacing) in [(3u64, Vec3::new(1.0, 1.0, 1.0)), (4, Vec3::new(0.6, 0.9, 3.0))] {
            let mut s = seed;
            let mut next = move || {
                s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (s >> 33) as u32
            };
            let flags: Vec<bool> = (0..14 * 12 * 9).map(|_| next() % 5 < 2).collect();
            let r = region([14, 12, 9], |p| flags[(p.x + 14 * (p.y + 12 * p.z)) as usize]);
            assert!((max_diameter_mm(&r, spacing) - brute_force(&r, spacing)).abs() < 1e-9, "seed {seed}");
        }
    }

    // covers 18.2-c, 18.2-e
    #[test]
    fn flat_thin_and_single_voxel_segments_have_a_diameter() {
        let spacing = Vec3::new(1.0, 2.0, 3.0);
        let one = region([5, 5, 5], |p| p == UVec3::new(2, 2, 2));
        assert_eq!(max_diameter_mm(&one, spacing), 0.0);
        let row = region([8, 5, 5], |p| p.y == 1 && p.z == 3 && p.x >= 1 && p.x < 7);
        assert!((max_diameter_mm(&row, spacing) - 5.0).abs() < 1e-12);
        // one i-slice: the longest side is along j or k
        let wall = region([4, 9, 6], |p| p.x == 1 && p.y >= 1 && p.y < 8 && p.z >= 2 && p.z < 5);
        assert!((max_diameter_mm(&wall, spacing) - brute_force(&wall, spacing)).abs() < 1e-9);
        let slice = region([9, 9, 3], |p| p.z == 1 && (p.x as i32 - 4).pow(2) + (p.y as i32 - 4).pow(2) <= 16);
        assert!((max_diameter_mm(&slice, spacing) - brute_force(&slice, spacing)).abs() < 1e-9);
    }
}
