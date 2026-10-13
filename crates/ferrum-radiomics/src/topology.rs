//! Connectivity of a segment: pieces, cavities, Euler number, warnings.
//!
//! Pieces are the 26-connected components and cavities are the
//! 6-connected pieces of background that do not reach the border of the
//! box, as the pair of connectivities of a digital object and its
//! background requires. The pieces and the cavity mask come from
//! `ferrum_domain::analysis` (one implementation); only the grouping of
//! the cavity voxels is done here.

use ferrum_domain::analysis::{components, fill_holes};
use ferrum_domain::{Dims3, MaskRegion};

/// A segment warns `fragmented` when its largest piece holds less than
/// this share of its voxels.
pub const FRAGMENTED_BELOW: f64 = 0.95;

/// Connectivity measures of a segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Topology {
    /// 26-connected pieces.
    pub components: u64,
    /// Share of the voxels in the largest piece, `0..=1`.
    pub largest_share: f64,
    /// Cavities: background enclosed by the segment (6-connected).
    pub holes: u64,
    /// Voxels in the cavities.
    pub hole_voxels: u64,
    /// Euler characteristic of the segment as a union of closed voxels
    /// (pieces − tunnels + cavities).
    pub euler: i64,
    /// Sides of the image (of six) the segment touches.
    pub border_sides: u32,
}

/// The connectivity of `region` (non-empty and tight) inside an image of `dims`.
pub fn topology(region: &MaskRegion, dims: Dims3) -> Topology {
    let (pieces, _) = components(region);
    let total: u64 = pieces.iter().map(|c| c.voxels).sum();
    let (holes, hole_voxels) = cavities(region);
    let (lo, hi) = (region.bx.min.to_array(), region.bx.max.to_array());
    let d = [dims.x, dims.y, dims.z];
    Topology {
        components: pieces.len() as u64,
        largest_share: pieces.first().map_or(0.0, |c| c.voxels as f64 / total.max(1) as f64),
        holes,
        hole_voxels,
        euler: euler_number(region),
        border_sides: (0..3).map(|a| u32::from(lo[a] == 0) + u32::from(hi[a] == d[a])).sum(),
    }
}

/// Number of cavities and of their voxels.
fn cavities(region: &MaskRegion) -> (u64, u64) {
    let filled = fill_holes(region, |_| true);
    let hole: Vec<bool> = filled.inside.iter().zip(&region.inside).map(|(f, r)| *f && !*r).collect();
    let s = region.size().to_array().map(|v| v as i64);
    let mut seen = vec![false; hole.len()];
    let (mut count, mut voxels) = (0u64, 0u64);
    for start in 0..hole.len() {
        if !hole[start] || seen[start] {
            continue;
        }
        count += 1;
        seen[start] = true;
        let mut stack = vec![start];
        while let Some(n) = stack.pop() {
            voxels += 1;
            let n = n as i64;
            let at = [n % s[0], (n / s[0]) % s[1], n / (s[0] * s[1])];
            for (axis, step) in [(0, 1), (0, -1), (1, 1), (1, -1), (2, 1), (2, -1)] {
                let mut to = at;
                to[axis] += step;
                if (0..3).all(|a| to[a] >= 0 && to[a] < s[a]) {
                    let m = (to[0] + s[0] * (to[1] + s[1] * to[2])) as usize;
                    if hole[m] && !seen[m] {
                        seen[m] = true;
                        stack.push(m);
                    }
                }
            }
        }
    }
    (count, voxels)
}

/// `V − E + F − C` of the cubical complex of the closed voxels: every
/// vertex, edge and face counts once however many voxels share it.
fn euler_number(region: &MaskRegion) -> i64 {
    let s = region.size().to_array().map(|v| v as usize);
    let lattice = (s[0] + 1) * (s[1] + 1) * (s[2] + 1);
    let at = |i: usize, j: usize, k: usize| i + (s[0] + 1) * (j + (s[1] + 1) * k);
    // vertices, edges along x, y, z and faces with normal x, y, z
    let mut cells = vec![vec![false; lattice]; 7];
    let mut cubes = 0i64;
    for (n, inside) in region.inside.iter().enumerate() {
        if !*inside {
            continue;
        }
        cubes += 1;
        let (i, j, k) = (n % s[0], (n / s[0]) % s[1], n / (s[0] * s[1]));
        for (a, b) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
            cells[1][at(i, j + a, k + b)] = true;
            cells[2][at(i + a, j, k + b)] = true;
            cells[3][at(i + a, j + b, k)] = true;
        }
        for c in 0..2 {
            cells[4][at(i + c, j, k)] = true;
            cells[5][at(i, j + c, k)] = true;
            cells[6][at(i, j, k + c)] = true;
            for (a, b) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                cells[0][at(i + c, j + a, k + b)] = true;
            }
        }
    }
    let count =
        |range: std::ops::Range<usize>| -> i64 { range.map(|t| cells[t].iter().filter(|v| **v).count() as i64).sum() };
    count(0..1) - count(1..4) + count(4..7) - cubes
}

/// The warnings for `t`, each starting with its stable code.
pub fn warnings(t: &Topology) -> Vec<String> {
    let mut out = Vec::new();
    if t.largest_share < FRAGMENTED_BELOW {
        out.push(format!(
            "fragmented: {} pieces, the largest holds {:.1} % of the voxels (warns below {:.0} %)",
            t.components,
            100.0 * t.largest_share,
            100.0 * FRAGMENTED_BELOW
        ));
    }
    if t.holes > 0 {
        out.push(format!("has_holes: {} cavities of {} voxels enclosed by the segment", t.holes, t.hole_voxels));
    }
    if t.border_sides > 0 {
        out.push(format!(
            "touches_border: the segment reaches {} side(s) of the image and may be cut off",
            t.border_sides
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use glam::UVec3;

    use super::*;

    const DIMS: Dims3 = Dims3 { x: 12, y: 12, z: 12 };

    fn region(inside: impl Fn(UVec3) -> bool) -> MaskRegion {
        MaskRegion::from_grid(DIMS, |n| {
            let n = n as u32;
            inside(UVec3::new(n % 12, (n / 12) % 12, n / 144))
        })
        .unwrap()
    }

    fn in_box(p: UVec3, lo: [u32; 3], hi: [u32; 3]) -> bool {
        p.cmpge(UVec3::from(lo)).all() && p.cmplt(UVec3::from(hi)).all()
    }

    // covers 18.8-a, 18.8-c
    #[test]
    fn a_box_is_one_piece_without_holes_and_has_euler_number_one() {
        let t = topology(&region(|p| in_box(p, [2, 3, 4], [6, 7, 8])), DIMS);
        assert_eq!((t.components, t.holes, t.hole_voxels, t.euler, t.border_sides), (1, 0, 0, 1, 0));
        assert_eq!(t.largest_share, 1.0);
        assert!(warnings(&t).is_empty());
    }

    // covers 18.8-b, 18.8-c
    #[test]
    fn a_box_with_a_cavity_has_one_hole_of_eight_voxels_and_euler_number_two() {
        let r = region(|p| in_box(p, [2, 2, 2], [8, 8, 8]) && !in_box(p, [4, 4, 4], [6, 6, 6]));
        let t = topology(&r, DIMS);
        assert_eq!((t.components, t.holes, t.hole_voxels, t.euler), (1, 1, 8, 2));
        assert!(warnings(&t)[0].starts_with("has_holes: 1 cavities of 8 voxels"));
    }

    // covers 18.8-b
    #[test]
    fn separate_cavities_are_counted_and_a_tunnel_is_not_a_cavity() {
        let two = region(|p| {
            in_box(p, [1, 1, 1], [11, 6, 6]) && !in_box(p, [3, 3, 3], [4, 4, 4]) && !in_box(p, [6, 3, 3], [8, 5, 4])
        });
        let t = topology(&two, DIMS);
        assert_eq!((t.holes, t.hole_voxels, t.euler), (2, 5, 3));
        // a ring of voxels in one slice: the middle is open above and below
        let ring = region(|p| in_box(p, [3, 3, 5], [6, 6, 6]) && p != UVec3::new(4, 4, 5));
        let t = topology(&ring, DIMS);
        assert_eq!((t.components, t.holes, t.euler), (1, 0, 0));
    }

    // covers 18.8-a, 18.8-c, 18.8-d
    #[test]
    fn pieces_touching_at_an_edge_or_a_corner_are_one_piece_and_far_pieces_are_not() {
        let corner = region(|p| p == UVec3::new(2, 2, 2) || p == UVec3::new(3, 3, 3));
        let t = topology(&corner, DIMS);
        assert_eq!((t.components, t.euler), (1, 1), "closed voxels that share a vertex are one piece");
        let edge = region(|p| p == UVec3::new(2, 2, 2) || p == UVec3::new(3, 3, 2));
        assert_eq!(topology(&edge, DIMS).components, 1);
        // 27 voxels and 3 far away: the largest piece holds 90 %
        let r = region(|p| in_box(p, [1, 1, 1], [4, 4, 4]) || in_box(p, [8, 8, 8], [11, 9, 9]));
        let t = topology(&r, DIMS);
        assert_eq!((t.components, t.euler), (2, 2));
        assert!((t.largest_share - 0.9).abs() < 1e-12);
        let w = warnings(&t);
        assert!(w.len() == 1 && w[0].starts_with("fragmented: 2 pieces, the largest holds 90.0 %"), "{w:?}");
        // 100 + 3 voxels: the largest piece holds 97 %, above the threshold
        let r = region(|p| in_box(p, [1, 1, 1], [11, 11, 2]) || in_box(p, [1, 1, 5], [4, 2, 6]));
        assert!(warnings(&topology(&r, DIMS)).iter().all(|w| !w.starts_with("fragmented")));
    }

    // covers 18.8-d
    #[test]
    fn a_segment_at_the_edge_of_the_image_touches_the_border() {
        let t = topology(&region(|p| in_box(p, [0, 3, 3], [4, 6, 12])), DIMS);
        assert_eq!(t.border_sides, 2);
        assert!(warnings(&t)[0].starts_with("touches_border: the segment reaches 2 side(s)"));
    }

    // covers 18.8-d
    #[test]
    fn warnings_leave_the_mask_alone() {
        let r = region(|p| in_box(p, [2, 2, 2], [8, 8, 8]) && !in_box(p, [4, 4, 4], [6, 6, 6]));
        let before = r.clone();
        let _ = warnings(&topology(&r, DIMS));
        assert_eq!(r, before);
    }
}
