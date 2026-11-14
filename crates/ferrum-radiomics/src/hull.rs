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

use std::collections::{HashMap, HashSet};

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

/// Hull under construction (Quickhull): every face lists the points that
/// are outside it and not yet inside the hull. A point is tested again
/// only when its face goes, so the cost follows the points near the
/// surface, not the product of all points and all faces.
struct Builder<'a> {
    points: &'a [P],
    faces: Vec<Option<Face>>,
    outside: Vec<Vec<usize>>,
    /// The face that owns each directed edge (a, b) of its winding.
    owner: HashMap<(usize, usize), usize>,
    eps: f64,
}

impl Builder<'_> {
    fn add_face(&mut self, face: Face) -> usize {
        let id = self.faces.len();
        for j in 0..3 {
            self.owner.insert((face.v[j], face.v[(j + 1) % 3]), id);
        }
        self.faces.push(Some(face));
        self.outside.push(Vec::new());
        id
    }

    fn remove_face(&mut self, id: usize) {
        if let Some(face) = self.faces[id].take() {
            for j in 0..3 {
                self.owner.remove(&(face.v[j], face.v[(j + 1) % 3]));
            }
        }
    }

    /// Lists point `p` as outside the first of `candidates` that sees it;
    /// a point no candidate sees is inside and is dropped.
    fn assign(&mut self, p: usize, candidates: &[usize]) {
        let at = self.points[p];
        let found = candidates.iter().copied().find(|id| self.faces[*id].is_some_and(|f| f.height(at) > self.eps));
        if let Some(id) = found {
            self.outside[id].push(p);
        }
    }

    /// The faces that see `apex`, found from face `start` across shared edges.
    fn visible_from(&self, start: usize, apex: P) -> Vec<usize> {
        let mut seen = HashSet::from([start]);
        let mut visible = vec![start];
        let mut next = 0;
        while next < visible.len() {
            let Some(face) = self.faces[visible[next]] else { break };
            next += 1;
            for j in 0..3 {
                let twin = self.owner.get(&(face.v[(j + 1) % 3], face.v[j])).copied();
                if let Some(t) = twin.filter(|t| !seen.contains(t)) {
                    if self.faces[t].is_some_and(|f| f.height(apex) > self.eps) {
                        seen.insert(t);
                        visible.push(t);
                    }
                }
            }
        }
        visible
    }

    /// Replaces the faces that see the furthest outside point of `id` by a
    /// cone from that point to the edge of the gap; returns the new faces.
    fn grow(&mut self, id: usize) -> Vec<usize> {
        let face = self.faces[id].unwrap_or_else(|| unreachable!("a listed face is alive"));
        let apex = self.outside[id]
            .iter()
            .copied()
            .max_by(|a, b| face.height(self.points[*a]).total_cmp(&face.height(self.points[*b])))
            .unwrap_or_else(|| unreachable!("a listed face has points"));
        let visible = self.visible_from(id, self.points[apex]);
        let gone: HashSet<usize> = visible.iter().copied().collect();
        let mut horizon = Vec::new();
        for vf in &visible {
            let Some(f) = self.faces[*vf] else { continue };
            for j in 0..3 {
                let (a, b) = (f.v[j], f.v[(j + 1) % 3]);
                if self.owner.get(&(b, a)).is_none_or(|t| !gone.contains(t)) {
                    horizon.push((a, b));
                }
            }
        }
        let pool: Vec<usize> = visible.iter().flat_map(|vf| std::mem::take(&mut self.outside[*vf])).collect();
        for vf in &visible {
            self.remove_face(*vf);
        }
        let new: Vec<usize> =
            horizon.iter().map(|(a, b)| self.add_face(Face::through(self.points, [*a, *b, apex]))).collect();
        for p in pool.into_iter().filter(|p| *p != apex) {
            self.assign(p, &new);
        }
        new
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
    let start = [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]].map(|f| Face::new(points, f.map(|j| s[j]), centre));
    let mut builder = Builder { points, faces: Vec::new(), outside: Vec::new(), owner: HashMap::new(), eps };
    let ids: Vec<usize> = start.into_iter().map(|f| builder.add_face(f)).collect();
    for p in (0..points.len()).filter(|p| !s.contains(p)) {
        builder.assign(p, &ids);
    }
    let mut pending: Vec<usize> = ids;
    while let Some(id) = pending.pop() {
        if builder.faces[id].is_some() && !builder.outside[id].is_empty() {
            let new = builder.grow(id);
            pending.extend(new.into_iter().filter(|n| !builder.outside[*n].is_empty()));
        }
    }
    let faces = sorted(builder.faces.iter().flatten().map(|f| f.v).collect());
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

    /// The first version of the hull, kept as a reference: every point is tested against every
    /// face, which is simple and right but far too slow for the point clouds of real masks
    /// (16 s for a blobby mask of 100 000 voxels).
    fn reference_hull(points: &[P]) -> Option<(f64, f64, Vec<P>)> {
        if points.len() < 4 {
            return None;
        }
        let (lo, hi) = points.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), p| {
            (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn(|i| hi[i].max(p[i])))
        });
        let eps = 1e-9 * norm(sub(hi, lo));
        let s = simplex(points, eps)?;
        let sum = s.iter().fold([0.0; 3], |a, i| [a[0] + points[*i][0], a[1] + points[*i][1], a[2] + points[*i][2]]);
        let centre = [sum[0] / 4.0, sum[1] / 4.0, sum[2] / 4.0];
        let mut faces: Vec<Face> =
            [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]].map(|f| Face::new(points, f.map(|j| s[j]), centre)).to_vec();
        for p in 0..points.len() {
            let at = points[p];
            let (seen, kept): (Vec<Face>, Vec<Face>) = faces.iter().copied().partition(|f| f.height(at) > eps);
            if seen.is_empty() {
                continue;
            }
            let edges: HashSet<(usize, usize)> =
                seen.iter().flat_map(|f| (0..3).map(|j| (f.v[j], f.v[(j + 1) % 3]))).collect();
            faces = kept;
            for (a, b) in edges.iter().filter(|(a, b)| !edges.contains(&(*b, *a))) {
                faces.push(Face::through(points, [*a, *b, p]));
            }
        }
        let hull = Hull { points: points.to_vec(), faces: faces.iter().map(|f| f.v).collect() };
        Some((hull.volume(), hull.area(), hull.vertices()))
    }

    // covers 18.7-a, 18.7-c
    #[test]
    fn the_fast_hull_equals_the_reference_on_lattice_clouds_with_many_coplanar_points() {
        use ferrum_domain::{Dims3, MaskRegion};

        use crate::mesh::marching_cubes;
        for (seed, keep) in [(1u64, 0.55), (2, 0.62), (3, 0.7)] {
            let mut s = seed;
            // blobby masks: value noise on a coarse lattice, kept where high
            let coarse: Vec<f64> = (0..10 * 10 * 10).map(|_| random(&mut s)).collect();
            let at =
                |x: usize, y: usize, z: usize| coarse[(x / 4).min(9) + 10 * ((y / 4).min(9) + 10 * (z / 4).min(9))];
            let jitter: Vec<f64> = (0..36 * 36 * 24).map(|_| random(&mut s)).collect();
            let r = MaskRegion::from_grid(Dims3 { x: 36, y: 36, z: 24 }, |n| {
                let (i, j, k) = (n % 36, (n / 36) % 36, n / (36 * 36));
                at(i, j, k) + 0.3 * jitter[n] > keep + 0.3
            })
            .unwrap();
            let mesh = marching_cubes(&r, glam::Vec3::new(0.8, 0.8, 2.5));
            let mut points: Vec<P> = mesh.triangles.iter().flatten().copied().collect();
            points
                .sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])).then(a[2].total_cmp(&b[2])));
            points.dedup();
            assert!(points.len() > 2000, "{} points: the cloud should be big enough to matter", points.len());
            let fast = convex_hull(&points).unwrap();
            let (volume, area, corners) = reference_hull(&points).unwrap();
            assert!((fast.volume() - volume).abs() < 1e-6 * volume, "seed {seed}: {} != {volume}", fast.volume());
            assert!((fast.area() - area).abs() < 1e-6 * area, "seed {seed}: {} != {area}", fast.area());
            // points on a facet may or may not be kept as corners: what matters is that the farthest
            // pair, which the 3D diameter takes from the corners, is the same
            let widest = |v: &[P]| {
                v.iter()
                    .enumerate()
                    .flat_map(|(n, a)| v[n + 1..].iter().map(move |b| norm(sub(*a, *b))))
                    .fold(0.0, f64::max)
            };
            assert!((widest(&fast.vertices()) - widest(&corners)).abs() < 1e-9);
            assert!(fast.vertices().len() <= corners.len());
            // and it is a hull: closed, outward, every point inside
            let mut edges = HashSet::new();
            for f in &fast.faces {
                for j in 0..3 {
                    assert!(edges.insert((f[j], f[(j + 1) % 3])));
                }
                let face = Face::through(&fast.points, *f);
                assert!(points.iter().all(|p| face.height(*p) < 1e-7));
            }
            assert!(edges.iter().all(|(a, b)| edges.contains(&(*b, *a))));
        }
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
