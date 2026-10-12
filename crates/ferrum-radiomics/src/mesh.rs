//! Surface mesh of a segment by marching cubes: area and enclosed volume.
//!
//! The mask is padded by one voxel and every cube between eight voxel
//! centres is cut at the midpoints of the edges whose ends differ
//! (the iso-level 0.5 of a binary mask). The surface inside a cube is not
//! read from a memorised table: for each of the 256 corner patterns it is
//! built from the face loops.
//!
//! - On each face of the cube the cut edges are joined in pairs. A face
//!   with all four edges cut (opposite corners inside) is resolved by
//!   separating the inside corners, and a cube whose only inside corners
//!   are opposite ends of its diagonal is two separate pieces. So voxels
//!   that touch only at an edge or a corner are separate pieces of the
//!   surface (`ferrum_domain::analysis::components` calls them one object
//!   and the Euler characteristic of the 26-connected foreground is
//!   computed separately). The rule depends on the corner values only, so
//!   the two cubes that share a face agree and the mesh is closed.
//! - Every cut edge then has exactly two partners, so the segments form
//!   closed loops. A loop is a polygon, drawn as a fan around its centroid
//!   and oriented away from the inside.
//!
//! Distances are in millimetres from the first voxel of the segment's box.

use std::sync::OnceLock;

use ferrum_domain::MaskRegion;
use glam::Vec3;

/// A triangle mesh in millimetres; every triangle lists its three corners.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mesh {
    /// Triangles, wound so that the normal points out of the segment.
    pub triangles: Vec<[[f64; 3]; 3]>,
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Mesh {
    /// Surface area in square millimetres.
    pub fn area_mm2(&self) -> f64 {
        self.triangles
            .iter()
            .map(|[a, b, c]| {
                let n = cross(sub(*b, *a), sub(*c, *a));
                0.5 * dot(n, n).sqrt()
            })
            .sum()
    }

    /// Enclosed volume in cubic millimetres: the sum of the signed volumes
    /// of the tetrahedra between the triangles and the origin. Exact for a
    /// closed, outward-wound mesh whatever the origin.
    pub fn volume_mm3(&self) -> f64 {
        self.triangles.iter().map(|[a, b, c]| dot(*a, cross(*b, *c)) / 6.0).sum()
    }
}

/// Corner `c` of a cube: bit 0 is `x`, bit 1 is `y`, bit 2 is `z`.
fn corner(c: usize) -> [f64; 3] {
    [(c & 1) as f64, ((c >> 1) & 1) as f64, ((c >> 2) & 1) as f64]
}

/// The 12 edges of a cube as corner pairs: four along `x`, four along `y`,
/// four along `z`.
const EDGES: [(usize, usize); 12] =
    [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];

fn edge_between(a: usize, b: usize) -> Option<usize> {
    EDGES.iter().position(|&(p, q)| (p, q) == (a, b) || (p, q) == (b, a))
}

/// The six faces as cycles of corners; the edge `j` joins corner `j` and
/// corner `j + 1`.
fn face_cycles() -> Vec<([usize; 4], [usize; 4])> {
    let mut faces = Vec::new();
    for axis in 0..3 {
        for side in 0..2 {
            let on_face: Vec<usize> = (0..8).filter(|c| (c >> axis) & 1 == side).collect();
            let mut cycle = vec![on_face[0]];
            while cycle.len() < 4 {
                let last = cycle[cycle.len() - 1];
                let next = on_face.iter().copied().find(|c| !cycle.contains(c) && edge_between(last, *c).is_some());
                cycle.extend(next);
            }
            let edges = std::array::from_fn(|j| edge_between(cycle[j], cycle[(j + 1) % 4]).unwrap_or(0));
            faces.push((std::array::from_fn(|j| cycle[j]), edges));
        }
    }
    faces
}

/// Joins the cut edges of every face: `adjacent[e]` are the edges linked to `e`.
fn link_cut_edges(inside: &dyn Fn(usize) -> bool) -> [Vec<usize>; 12] {
    let mut adjacent: [Vec<usize>; 12] = Default::default();
    let mut link = |a: usize, b: usize| {
        adjacent[a].push(b);
        adjacent[b].push(a);
    };
    for (corners, edges) in face_cycles() {
        let cut: Vec<usize> = (0..4).filter(|&j| inside(corners[j]) != inside(corners[(j + 1) % 4])).collect();
        match cut.len() {
            2 => link(edges[cut[0]], edges[cut[1]]),
            // opposite corners inside: each inside corner is cut off on its own
            4 => (0..4).filter(|&j| inside(corners[j])).for_each(|j| link(edges[(j + 3) % 4], edges[j])),
            _ => {}
        }
    }
    adjacent
}

/// The closed loops of cut edges, each as a list of edge indices.
fn trace_loops(adjacent: &[Vec<usize>; 12]) -> Vec<Vec<usize>> {
    let mut seen = [false; 12];
    let mut loops = Vec::new();
    for start in 0..12 {
        if seen[start] || adjacent[start].is_empty() {
            continue;
        }
        let (mut prev, mut at) = (usize::MAX, start);
        let mut cycle = Vec::new();
        while !seen[at] {
            seen[at] = true;
            cycle.push(at);
            let next = adjacent[at].iter().copied().find(|&n| n != prev && !seen[n]);
            match next {
                Some(n) => (prev, at) = (at, n),
                None => break,
            }
        }
        loops.push(cycle);
    }
    loops
}

/// Midpoint of edge `e` and the direction from its inside end to its
/// outside end.
fn edge_point(e: usize, inside: &dyn Fn(usize) -> bool) -> ([f64; 3], [f64; 3]) {
    let (a, b) = EDGES[e];
    let (p, q) = (corner(a), corner(b));
    let (from, to) = if inside(a) { (p, q) } else { (q, p) };
    ([(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0, (p[2] + q[2]) / 2.0], sub(to, from))
}

/// The surface of one corner pattern: polygons in cube coordinates, wound
/// so that their normals point out of the inside corners.
fn pattern_polygons(pattern: u8) -> Vec<Vec<[f64; 3]>> {
    let inside = move |c: usize| (pattern >> c) & 1 == 1;
    let adjacent = link_cut_edges(&inside);
    trace_loops(&adjacent)
        .into_iter()
        .map(|cycle| {
            let mut points: Vec<[f64; 3]> = Vec::new();
            let mut outward = [0.0; 3];
            for e in cycle {
                let (point, direction) = edge_point(e, &inside);
                points.push(point);
                outward = [outward[0] + direction[0], outward[1] + direction[1], outward[2] + direction[2]];
            }
            let mut normal = [0.0; 3];
            for (i, p) in points.iter().enumerate() {
                let c = cross(*p, points[(i + 1) % points.len()]);
                normal = [normal[0] + c[0], normal[1] + c[1], normal[2] + c[2]];
            }
            if dot(normal, outward) < 0.0 {
                points.reverse();
            }
            points
        })
        .collect()
}

fn patterns() -> &'static Vec<Vec<Vec<[f64; 3]>>> {
    static TABLE: OnceLock<Vec<Vec<Vec<[f64; 3]>>>> = OnceLock::new();
    TABLE.get_or_init(|| (0..=255u8).map(pattern_polygons).collect())
}

/// The mask of `region` padded by one voxel on every side, `i` fastest.
struct Padded {
    size: [usize; 3],
    cells: Vec<bool>,
}

impl Padded {
    fn new(region: &MaskRegion) -> Self {
        let s = region.size();
        let size = [s.x as usize + 2, s.y as usize + 2, s.z as usize + 2];
        let mut cells = vec![false; size[0] * size[1] * size[2]];
        let (nx, ny) = (s.x as usize, s.y as usize);
        for (n, v) in region.inside.iter().enumerate() {
            if *v {
                let (i, j, k) = (n % nx, (n / nx) % ny, n / (nx * ny));
                cells[(i + 1) + size[0] * ((j + 1) + size[1] * (k + 1))] = true;
            }
        }
        Self { size, cells }
    }

    fn at(&self, i: usize, j: usize, k: usize) -> bool {
        self.cells[i + self.size[0] * (j + self.size[1] * k)]
    }

    /// The corner pattern of the cube whose lowest corner is `(i, j, k)`.
    fn pattern(&self, i: usize, j: usize, k: usize) -> u8 {
        (0..8usize).fold(0u8, |acc, c| {
            let v = self.at(i + (c & 1), j + ((c >> 1) & 1), k + ((c >> 2) & 1));
            acc | (u8::from(v) << c)
        })
    }
}

/// The surface mesh of `region` (voxels of `spacing` mm) with the iso-level
/// at half the distance between a voxel inside and one outside.
pub fn marching_cubes(region: &MaskRegion, spacing: Vec3) -> Mesh {
    let padded = Padded::new(region);
    let table = patterns();
    let s = [f64::from(spacing.x), f64::from(spacing.y), f64::from(spacing.z)];
    let mut triangles = Vec::new();
    for k in 0..padded.size[2] - 1 {
        for j in 0..padded.size[1] - 1 {
            for i in 0..padded.size[0] - 1 {
                let pattern = padded.pattern(i, j, k);
                if pattern == 0 || pattern == 255 {
                    continue;
                }
                // padded index p is voxel p - 1; the cube origin is voxel (i - 1, j - 1, k - 1)
                let origin = [i as f64 - 1.0, j as f64 - 1.0, k as f64 - 1.0];
                for polygon in &table[usize::from(pattern)] {
                    fan(polygon, origin, s, &mut triangles);
                }
            }
        }
    }
    Mesh { triangles }
}

/// Appends the fan of `polygon` around its centroid.
fn fan(polygon: &[[f64; 3]], origin: [f64; 3], spacing: [f64; 3], out: &mut Vec<[[f64; 3]; 3]>) {
    let place = |p: [f64; 3]| {
        [(origin[0] + p[0]) * spacing[0], (origin[1] + p[1]) * spacing[1], (origin[2] + p[2]) * spacing[2]]
    };
    let n = polygon.len() as f64;
    let sum = polygon.iter().fold([0.0; 3], |a, p| [a[0] + p[0], a[1] + p[1], a[2] + p[2]]);
    let centre = place([sum[0] / n, sum[1] / n, sum[2] / n]);
    for (i, p) in polygon.iter().enumerate() {
        out.push([centre, place(*p), place(polygon[(i + 1) % polygon.len()])]);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ferrum_domain::Dims3;

    use super::*;

    fn region(dims: [u32; 3], inside: impl Fn(u32, u32, u32) -> bool) -> MaskRegion {
        let d = Dims3::new(dims[0], dims[1], dims[2]);
        MaskRegion::from_grid(d, |n| {
            let n = n as u32;
            inside(n % d.x, (n / d.x) % d.y, n / (d.x * d.y))
        })
        .unwrap()
    }

    /// Every directed edge of the mesh must be matched by exactly one edge
    /// in the opposite direction: a closed, consistently wound surface.
    /// Returns (vertices, undirected edges, triangles).
    fn closed_and_oriented(mesh: &Mesh) -> (usize, usize, usize) {
        let key = |p: [f64; 3]| [(p[0] * 8.0).round() as i64, (p[1] * 8.0).round() as i64, (p[2] * 8.0).round() as i64];
        let mut directed: HashMap<([i64; 3], [i64; 3]), u32> = HashMap::new();
        let mut vertices = std::collections::HashSet::new();
        for t in &mesh.triangles {
            for i in 0..3 {
                let (a, b) = (key(t[i]), key(t[(i + 1) % 3]));
                vertices.insert(a);
                *directed.entry((a, b)).or_default() += 1;
            }
        }
        for (&(a, b), &n) in &directed {
            assert_eq!(n, 1, "edge {a:?} -> {b:?} is used {n} times in this direction");
            assert_eq!(
                directed.get(&(b, a)),
                Some(&1),
                "edge {a:?} -> {b:?} has no partner: the surface is open or mis-wound"
            );
        }
        (vertices.len(), directed.len() / 2, mesh.triangles.len())
    }

    // covers 18.5-a
    #[test]
    fn every_corner_pattern_has_closed_loops_through_each_cut_edge_once() {
        for pattern in 0..=255u8 {
            let inside = |c: usize| (pattern >> c) & 1 == 1;
            let cut = EDGES.iter().filter(|&&(a, b)| inside(a) != inside(b)).count();
            let polygons = pattern_polygons(pattern);
            let vertices: usize = polygons.iter().map(Vec::len).sum();
            assert_eq!(vertices, cut, "pattern {pattern:08b}: each cut edge belongs to exactly one loop");
            assert!(polygons.iter().all(|p| p.len() >= 3), "pattern {pattern:08b}");
        }
        assert!(pattern_polygons(0).is_empty() && pattern_polygons(255).is_empty());
    }

    // covers 18.5-a
    #[test]
    fn a_single_voxel_is_an_octahedron_with_exact_area_and_volume() {
        let mesh = marching_cubes(&region([1, 1, 1], |_, _, _| true), Vec3::ONE);
        assert_eq!(mesh.triangles.len(), 8 * 3, "eight triangles, each fanned around its centroid");
        // octahedron with its vertices half a voxel from the centre: area sqrt(3), volume 1/6
        assert!((mesh.area_mm2() - 3.0_f64.sqrt()).abs() < 1e-12, "{}", mesh.area_mm2());
        assert!((mesh.volume_mm3() - 1.0 / 6.0).abs() < 1e-12, "{}", mesh.volume_mm3());
        closed_and_oriented(&mesh);
    }

    // covers 18.5-c
    #[test]
    fn spacing_scales_the_mesh_exactly() {
        // semi-axes 0.5, 0.5, 1.0 mm: area 3, volume (4/3) * 0.5 * 0.5 * 1.0
        let mesh = marching_cubes(&region([1, 1, 1], |_, _, _| true), Vec3::new(1.0, 1.0, 2.0));
        assert!((mesh.area_mm2() - 3.0).abs() < 1e-12, "{}", mesh.area_mm2());
        assert!((mesh.volume_mm3() - 1.0 / 3.0).abs() < 1e-12);
        // a bigger object: volume is exactly proportional to the voxel volume
        let blob = region([9, 9, 9], |i, j, k| (i + 2 * j + 3 * k) % 5 < 3);
        let a = marching_cubes(&blob, Vec3::ONE).volume_mm3();
        let b = marching_cubes(&blob, Vec3::new(0.5, 2.0, 3.0)).volume_mm3();
        assert!((b - a * 3.0).abs() < 1e-9 * a.max(1.0), "{a} {b}");
    }

    // covers 18.5-a
    #[test]
    fn the_surface_is_closed_and_wound_outwards_for_every_kind_of_cube() {
        // dense pseudo-random masks contain every pattern, including the ambiguous faces
        for (seed, density) in [(1u64, 2u64), (7, 3), (13, 4)] {
            let blob = region([10, 10, 10], |i, j, k| {
                let h = (u64::from(i) * 73856093)
                    ^ (u64::from(j) * 19349663)
                    ^ (u64::from(k) * 83492791)
                    ^ (seed * 2654435761);
                (h >> 7) % density == 0
            });
            let mesh = marching_cubes(&blob, Vec3::ONE);
            assert!(!mesh.triangles.is_empty());
            closed_and_oriented(&mesh);
            assert!(mesh.volume_mm3() > 0.0, "outward winding gives a positive volume");
        }
        // two voxels touching at an edge and at a corner (ambiguous cubes on purpose)
        for cells in [[(0, 0, 0), (1, 1, 0)], [(0, 0, 0), (1, 1, 1)]] {
            let m = region([2, 2, 2], |i, j, k| cells.contains(&(i, j, k)));
            let mesh = marching_cubes(&m, Vec3::ONE);
            closed_and_oriented(&mesh);
            assert!(mesh.volume_mm3() > 0.0);
        }
    }

    // covers 18.5-a
    #[test]
    fn voxels_that_touch_only_at_an_edge_or_a_corner_are_separate_closed_pieces() {
        for cells in [[(0, 0, 0), (1, 1, 0)], [(0, 0, 0), (1, 1, 1)]] {
            let m = region([2, 2, 2], |i, j, k| cells.contains(&(i, j, k)));
            let (v, e, f) = closed_and_oriented(&marching_cubes(&m, Vec3::ONE));
            assert_eq!(v as i64 - e as i64 + f as i64, 4, "two spheres, V - E + F = 4: {cells:?}");
        }
        // face neighbours are one piece
        let m = region([2, 1, 1], |_, _, _| true);
        let (v, e, f) = closed_and_oriented(&marching_cubes(&m, Vec3::ONE));
        assert_eq!(v as i64 - e as i64 + f as i64, 2);
    }

    // covers 18.5-a, 18.5-c
    #[test]
    fn the_area_of_a_ball_exceeds_the_analytic_value_by_the_known_margin_at_every_resolution() {
        // A mesh of a binary mask is rougher than the smooth sphere it came from, and finer
        // voxels do not remove that: measured +7.4 % at 1 mm and +6.6 % at 0.5 mm (r = 10 mm).
        let ratios = |radius_voxels: f64, spacing: f32| {
            let n = (radius_voxels * 2.0) as u32 + 5;
            let c = f64::from(n - 1) / 2.0;
            let ball = region([n, n, n], |i, j, k| {
                (f64::from(i) - c).powi(2) + (f64::from(j) - c).powi(2) + (f64::from(k) - c).powi(2)
                    <= radius_voxels.powi(2)
            });
            let r_mm = radius_voxels * f64::from(spacing);
            let m = marching_cubes(&ball, Vec3::splat(spacing));
            let pi = std::f64::consts::PI;
            (m.area_mm2() / (4.0 * pi * r_mm * r_mm), m.volume_mm3() / (4.0 / 3.0 * pi * r_mm.powi(3)))
        };
        let (coarse_area, coarse_volume) = ratios(10.0, 1.0);
        let (fine_area, fine_volume) = ratios(20.0, 0.5);
        assert!((1.05..1.09).contains(&coarse_area), "area at 1 mm: {coarse_area}");
        assert!((1.05..1.09).contains(&fine_area), "area at 0.5 mm: {fine_area}");
        assert!(
            (coarse_area - fine_area).abs() < 0.02,
            "the same object at two resolutions: {coarse_area} {fine_area}"
        );
        assert!(
            (coarse_volume - 1.0).abs() < 0.015 && (fine_volume - 1.0).abs() < 0.015,
            "{coarse_volume} {fine_volume}"
        );
    }
}
