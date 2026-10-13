//! Three-dimensional convex hull of a point set, and the farthest pair.
//!
//! An incremental hull: it starts from a tetrahedron of well-separated
//! points, then takes the points that lie outside the hull one by one,
//! removes the faces the point sees and closes the gap with new faces
//! from the point to the horizon. The six axis extremes go in first, so
//! the points inside the hull are rejected against a handful of faces
//! and the cost follows the surface, not the number of points.
//!
//! A point closer to a face than `1e-9` of the size of the set counts as
//! on the face. The points are in a fixed order, so the result is the
//! same on every machine.

use std::collections::HashSet;

type P = [f64; 3];

fn sub(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: P, b: P) -> P {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: P, b: P) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: P) -> f64 {
    dot(a, a).sqrt()
}

/// A triangle of the hull with its outward unit normal.
#[derive(Debug, Clone, Copy)]
struct Face {
    v: [usize; 3],
    normal: P,
    offset: f64,
}

impl Face {
    /// The face through `v`, wound so that `inside` is behind it.
    fn new(points: &[P], v: [usize; 3], inside: P) -> Self {
        let face = Self::through(points, v);
        if face.height(inside) > 0.0 {
            Self::through(points, [v[0], v[2], v[1]])
        } else {
            face
        }
    }

    /// The face through `v` in the order given; a flat triangle gets a zero
    /// normal, so no point ever sees it.
    fn through(points: &[P], v: [usize; 3]) -> Self {
        let n = cross(sub(points[v[1]], points[v[0]]), sub(points[v[2]], points[v[0]]));
        let len = norm(n);
        if len > 0.0 {
            let normal = [n[0] / len, n[1] / len, n[2] / len];
            Self { v, normal, offset: dot(normal, points[v[0]]) }
        } else {
            Self { v, normal: [0.0; 3], offset: 0.0 }
        }
    }

    /// Signed distance of `p` above the face (positive outside).
    fn height(&self, p: P) -> f64 {
        dot(self.normal, p) - self.offset
    }
}

/// The convex hull of a point set: its points and outward-wound triangles.
#[derive(Debug, Clone, PartialEq)]
pub struct Hull {
    /// The points the faces index.
    pub points: Vec<[f64; 3]>,
    /// Triangles (indices into `points`), counter-clockwise seen from outside.
    pub faces: Vec<[usize; 3]>,
}

impl Hull {
    /// Enclosed volume.
    pub fn volume(&self) -> f64 {
        self.faces.iter().map(|f| dot(self.points[f[0]], cross(self.points[f[1]], self.points[f[2]])) / 6.0).sum()
    }

    /// Surface area.
    pub fn area(&self) -> f64 {
        self.faces
            .iter()
            .map(|f| {
                0.5 * norm(cross(sub(self.points[f[1]], self.points[f[0]]), sub(self.points[f[2]], self.points[f[0]])))
            })
            .sum()
    }

    /// The points that are corners of a face, in the order of `points`.
    pub fn vertices(&self) -> Vec<[f64; 3]> {
        let used: HashSet<usize> = self.faces.iter().flatten().copied().collect();
        (0..self.points.len()).filter(|i| used.contains(i)).map(|i| self.points[i]).collect()
    }
}

/// Farthest points of an axis: the minimum and the maximum along each.
fn extremes(points: &[P]) -> Vec<usize> {
    let mut picked = Vec::with_capacity(6);
    for axis in 0..3 {
        let by = |a: &usize, b: &usize| points[*a][axis].total_cmp(&points[*b][axis]);
        let (lo, hi) = (0..points.len()).fold((0, 0), |(lo, hi), i| {
            (if by(&i, &lo).is_lt() { i } else { lo }, if by(&i, &hi).is_gt() { i } else { hi })
        });
        for i in [lo, hi] {
            if !picked.contains(&i) {
                picked.push(i);
            }
        }
    }
    picked
}

/// Four points that span a tetrahedron, or `None` if all points are on a
/// plane (within `eps`).
fn simplex(points: &[P], eps: f64) -> Option<[usize; 4]> {
    let far = |score: &dyn Fn(usize) -> f64| {
        (0..points.len()).fold((0, 0.0), |(best, top), i| {
            let s = score(i);
            if s > top {
                (i, s)
            } else {
                (best, top)
            }
        })
    };
    let a = extremes(points)[0];
    let (b, d_ab) = far(&|i| norm(sub(points[i], points[a])));
    if d_ab <= eps {
        return None;
    }
    let ab = sub(points[b], points[a]);
    let (c, d_c) = far(&|i| norm(cross(ab, sub(points[i], points[a]))) / d_ab);
    if d_c <= eps {
        return None;
    }
    let n = cross(ab, sub(points[c], points[a]));
    let len = norm(n);
    let (d, d_d) = far(&|i| (dot(n, sub(points[i], points[a])) / len).abs());
    (d_d > eps).then_some([a, b, c, d])
}

/// Hull under construction.
struct Builder<'a> {
    points: &'a [P],
    faces: Vec<Face>,
    eps: f64,
}

impl Builder<'_> {
    /// Adds point `p` (index into `points`): nothing if it is inside.
    fn add(&mut self, p: usize) {
        let at = self.points[p];
        let eps = self.eps;
        let (seen, kept): (Vec<Face>, Vec<Face>) = self.faces.iter().copied().partition(|f| f.height(at) > eps);
        if seen.is_empty() {
            return;
        }
        let edges: HashSet<(usize, usize)> =
            seen.iter().flat_map(|f| (0..3).map(|j| (f.v[j], f.v[(j + 1) % 3]))).collect();
        self.faces = kept;
        for (a, b) in edges.iter().filter(|(a, b)| !edges.contains(&(*b, *a))) {
            self.faces.push(Face::through(self.points, [*a, *b, p]));
        }
    }
}

/// Face order must not depend on the hasher: sort the new faces.
fn sorted(mut faces: Vec<[usize; 3]>) -> Vec<[usize; 3]> {
    faces.sort_unstable();
    faces
}

/// The convex hull of `points`; `None` if there are fewer than four
/// points or they all lie on a plane.
pub fn convex_hull(points: &[[f64; 3]]) -> Option<Hull> {
    if points.len() < 4 {
        return None;
    }
    let (lo, hi) = points.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), p| {
        (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
    });
    let eps = 1e-9 * norm(sub(hi, lo));
    let s = simplex(points, eps)?;
    let centre = {
        let sum = s.iter().fold([0.0; 3], |a, i| [a[0] + points[*i][0], a[1] + points[*i][1], a[2] + points[*i][2]]);
        [sum[0] / 4.0, sum[1] / 4.0, sum[2] / 4.0]
    };
    let faces = [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]].map(|f| Face::new(points, f.map(|j| s[j]), centre));
    let mut builder = Builder { points, faces: faces.to_vec(), eps };
    for i in extremes(points).into_iter().chain(0..points.len()) {
        builder.add(i);
    }
    let faces = sorted(builder.faces.iter().map(|f| f.v).collect());
    Some(Hull { points: points.to_vec(), faces })
}

/// Largest distance between two of `points`.
///
/// The ends of the farthest pair are corners of the hull, so the pairs are
/// searched among the hull vertices; for a set on a plane or a line there
/// is no 3D hull and all points are searched.
pub fn farthest_pair(points: &[[f64; 3]]) -> f64 {
    let corners = convex_hull(points).map_or_else(|| points.to_vec(), |h| h.vertices());
    let mut best = 0.0f64;
    for (n, a) in corners.iter().enumerate() {
        for b in &corners[n + 1..] {
            best = best.max(norm(sub(*a, *b)));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random numbers in `0..1` (an LCG).
    fn random(seed: &mut u64) -> f64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64
    }

    fn cloud(n: usize, seed: u64) -> Vec<P> {
        let mut s = seed;
        (0..n).map(|_| [random(&mut s) * 10.0, random(&mut s) * 7.0, random(&mut s) * 4.0]).collect()
    }

    fn corners_of_box(size: P) -> Vec<P> {
        (0..8)
            .map(|c| [(c & 1) as f64 * size[0], ((c >> 1) & 1) as f64 * size[1], ((c >> 2) & 1) as f64 * size[2]])
            .collect()
    }

    /// Reference by brute force: a triple of points is a face of the hull
    /// when no point is above its plane; each face is counted once.
    fn brute_force_volume(points: &[P]) -> f64 {
        let n = points.len();
        let centre = points
            .iter()
            .fold([0.0; 3], |a, p| [a[0] + p[0] / n as f64, a[1] + p[1] / n as f64, a[2] + p[2] / n as f64]);
        let mut volume = 0.0;
        for a in 0..n {
            for b in a + 1..n {
                for c in b + 1..n {
                    let nrm = cross(sub(points[b], points[a]), sub(points[c], points[a]));
                    let len = norm(nrm);
                    if len < 1e-9 {
                        continue;
                    }
                    let side = |p: P| dot(nrm, sub(p, points[a])) / len;
                    let above = points.iter().any(|p| side(*p) > 1e-9);
                    let below = points.iter().any(|p| side(*p) < -1e-9);
                    if above && below {
                        continue;
                    }
                    // a face of the hull: every point is on one side. Tetrahedron with the centre.
                    volume += (dot(sub(points[a], centre), cross(sub(points[b], centre), sub(points[c], centre)))
                        / 6.0)
                        .abs();
                }
            }
        }
        volume
    }

    // covers 18.7-a
    #[test]
    fn volume_and_area_of_a_box() {
        let hull = convex_hull(&corners_of_box([2.0, 3.0, 4.0])).unwrap();
        assert!((hull.volume() - 24.0).abs() < 1e-9, "{}", hull.volume());
        assert!((hull.area() - 2.0 * (6.0 + 8.0 + 12.0)).abs() < 1e-9, "{}", hull.area());
        assert_eq!(hull.faces.len(), 12);
        assert_eq!(hull.vertices().len(), 8);
    }

    // covers 18.7-a, 18.7-c
    #[test]
    fn every_point_is_inside_or_on_the_hull_and_the_surface_is_closed() {
        for seed in 1..6 {
            let points = cloud(300, seed);
            let hull = convex_hull(&points).unwrap();
            let mut seen = HashSet::new();
            for f in &hull.faces {
                for j in 0..3 {
                    assert!(seen.insert((f[j], f[(j + 1) % 3])), "an edge is used twice in one direction");
                }
                let face = Face::through(&hull.points, *f);
                assert!(points.iter().all(|p| face.height(*p) < 1e-7), "a point lies outside a face");
            }
            assert!(seen.iter().all(|(a, b)| seen.contains(&(*b, *a))), "an edge has no partner: the surface is open");
            // Euler: V - E + F = 2 with E = 3F/2
            let v = hull.vertices().len() as i64;
            let f = hull.faces.len() as i64;
            assert_eq!(v - 3 * f / 2 + f, 2);
            assert!(hull.volume() > 0.0);
        }
    }

    // covers 18.7-a
    #[test]
    fn volume_equals_the_brute_force_hull_of_a_small_cloud() {
        for seed in 1..5 {
            let points = cloud(24, seed);
            let hull = convex_hull(&points).unwrap();
            let expected = brute_force_volume(&points);
            assert!((hull.volume() - expected).abs() < 1e-9 * expected.max(1.0), "{} != {expected}", hull.volume());
        }
    }

    // covers 18.7-b
    #[test]
    fn the_hull_of_an_l_shaped_mesh_is_known_by_hand() {
        use ferrum_domain::{Dims3, MaskRegion};

        use crate::mesh::marching_cubes;
        // Five voxels of 1 mm in one slice: an L with arms of three.
        // The corners of the hull are, on the middle plane, the octagon D
        // (1,.5) (3,.5) (3.5,1) (3,1.5) (1.5,3) (1,3.5) (.5,3) (.5,1), area 5.5,
        // and, half a voxel above and below, the triangle T of the centres
        // (1,1) (3,1) (1,3), area 2. The section at height t is (1-t)D + tT,
        // so the volume is (A_D + V(D,T) + A_T) / 3 with the mixed area
        // V(D,T) = (h_D(0,-1)*2 + h_D(-1,0)*2 + h_D(1,1)/sqrt2 * 2 sqrt2) / 2
        //        = (-0.5*2 - 0.5*2 + 4.5*2) / 2 = 3.5: (5.5 + 3.5 + 2) / 3 = 11/3.
        let cells = [(1, 1, 1), (2, 1, 1), (1, 2, 1), (1, 3, 1), (3, 1, 1)];
        let r = MaskRegion::from_grid(Dims3 { x: 5, y: 5, z: 3 }, |n| {
            cells.contains(&(n as u32 % 5, (n as u32 / 5) % 5, n as u32 / 25))
        })
        .unwrap();
        let mesh = marching_cubes(&r, glam::Vec3::ONE);
        let mut points: Vec<P> = mesh.triangles.iter().flatten().copied().collect();
        points.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])).then(a[2].total_cmp(&b[2])));
        points.dedup();
        let hull = convex_hull(&points).unwrap();
        assert!((hull.volume() - 11.0 / 3.0).abs() < 1e-9, "{}", hull.volume());
        assert!(hull.volume() > mesh.volume_mm3() * 1.2, "an L is dented: {} vs {}", hull.volume(), mesh.volume_mm3());
    }

    // covers 18.7-a
    #[test]
    fn interior_and_repeated_points_change_nothing() {
        let mut points = corners_of_box([2.0, 2.0, 2.0]);
        let plain = convex_hull(&points).unwrap().volume();
        points.extend(cloud(200, 7).iter().map(|p| [p[0] * 0.1 + 0.5, p[1] * 0.1 + 0.5, p[2] * 0.1 + 0.5]));
        points.extend(corners_of_box([2.0, 2.0, 2.0]));
        points.push([1.0, 2.0, 1.0]);
        let hull = convex_hull(&points).unwrap();
        assert!((hull.volume() - plain).abs() < 1e-9);
        assert_eq!(hull.vertices().len(), 8);
    }

    // covers 18.7-a
    #[test]
    fn flat_or_tiny_input_has_no_hull() {
        assert!(convex_hull(&[]).is_none());
        assert!(convex_hull(&[[0.0; 3]; 3]).is_none());
        let flat: Vec<P> = (0..30).map(|i| [f64::from(i % 6), f64::from(i / 6), 2.0]).collect();
        assert!(convex_hull(&flat).is_none());
        let line: Vec<P> = (0..9).map(|i| [f64::from(i), 2.0 * f64::from(i), 0.0]).collect();
        assert!(convex_hull(&line).is_none());
    }

    // covers 18.2-c
    #[test]
    fn farthest_pair_equals_brute_force_and_handles_flat_sets() {
        for seed in 1..5 {
            let points = cloud(200, seed);
            let mut expected = 0.0f64;
            for (n, a) in points.iter().enumerate() {
                for b in &points[n + 1..] {
                    expected = expected.max(norm(sub(*a, *b)));
                }
            }
            assert!((farthest_pair(&points) - expected).abs() < 1e-12);
        }
        assert!((farthest_pair(&corners_of_box([3.0, 4.0, 12.0])) - 13.0).abs() < 1e-12);
        let flat = [[0.0, 0.0, 1.0], [3.0, 4.0, 1.0], [1.0, 1.0, 1.0], [0.0, 4.0, 1.0], [3.0, 0.0, 1.0]];
        assert!((farthest_pair(&flat) - 5.0).abs() < 1e-12);
        assert_eq!(farthest_pair(&[[1.0, 1.0, 1.0]]), 0.0);
        assert_eq!(farthest_pair(&[]), 0.0);
    }

    // covers 18.7-c
    #[test]
    fn cost_follows_the_surface() {
        // a ball of many points inside a small hull: only the outer shell
        // survives as vertices
        let mut points = corners_of_box([10.0, 10.0, 10.0]);
        points.extend(cloud(3000, 11).iter().map(|p| [p[0] * 0.9 + 0.5, p[1] * 1.2 + 0.5, p[2] * 2.0 + 0.5]));
        let hull = convex_hull(&points).unwrap();
        assert_eq!(hull.vertices().len(), 8);
        assert_eq!(hull.faces.len(), 12);
    }
}
