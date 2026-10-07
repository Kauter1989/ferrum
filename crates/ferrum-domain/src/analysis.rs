//! Measurements and deterministic edits of binary masks: connected
//! components, shape (extent, largest slices, axial long and short axis),
//! agreement of two masks (Dice, Hausdorff distance) and clean-up edits.
//!
//! Everything works on a [`MaskRegion`]: a mask cropped to its bounding box,
//! so the cost follows the size of the object, not of the volume. Distances
//! are in millimetres between voxel centres; the grid's direction matrix is
//! orthonormal, so voxel offsets scaled by the spacing are patient
//! distances.

use std::collections::VecDeque;

use glam::{DVec3, UVec3, Vec3};

use crate::geometry::Dims3;
use crate::segmentation::{LabelMap, VoxelBox};

/// A binary mask cropped to a box of the volume grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskRegion {
    /// The box in grid coordinates.
    pub bx: VoxelBox,
    /// One flag per voxel of the box, `i` fastest.
    pub inside: Vec<bool>,
}

/// One connected component of a mask.
#[derive(Debug, Clone, PartialEq)]
pub struct Component {
    /// Number of voxels.
    pub voxels: u64,
    /// Bounding box in grid coordinates.
    pub bounds: VoxelBox,
    /// Mean voxel index (grid coordinates).
    pub centroid: DVec3,
}

/// The slice of one plane with the most voxels of a mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LargestSlice {
    /// 0-based slice index along the plane's normal axis.
    pub index: u32,
    /// Voxels of the mask on that slice.
    pub voxels: u64,
}

/// A distance between two voxel centres on one slice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Diameter {
    /// Length in millimetres.
    pub mm: f64,
    /// First end (voxel index).
    pub from: UVec3,
    /// Second end (voxel index).
    pub to: UVec3,
}

/// Shape measurements of a mask.
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    /// Number of voxels.
    pub voxels: u64,
    /// Tight bounding box.
    pub bounds: VoxelBox,
    /// Mean voxel index.
    pub centroid: DVec3,
    /// Extent of the bounding box along `i`, `j`, `k` in millimetres
    /// (voxel count × spacing).
    pub extent_mm: [f64; 3],
    /// Largest slice along `k` (axial), `j` (coronal) and `i` (sagittal).
    pub largest_slices: [LargestSlice; 3],
    /// Longest distance between two voxel centres on the largest axial
    /// slice.
    pub long_axis: Option<Diameter>,
    /// Longest extent perpendicular to [`Shape::long_axis`] on that slice.
    pub short_axis: Option<Diameter>,
    /// The mask touches the border of the volume grid.
    pub touches_border: bool,
    /// Number of 26-connected components.
    pub components: usize,
    /// Share of the voxels in the largest component (`0..=1`).
    pub largest_component_share: f64,
}

/// Agreement of two masks.
#[derive(Debug, Clone, PartialEq)]
pub struct Agreement {
    /// Voxels of the first mask.
    pub voxels_a: u64,
    /// Voxels of the second mask.
    pub voxels_b: u64,
    /// Voxels in both.
    pub intersection: u64,
    /// `2|A∩B| / (|A| + |B|)`.
    pub dice: f64,
    /// `|A∩B| / |A∪B|`.
    pub jaccard: f64,
    /// 95th percentile of the surface distances (the larger of the two
    /// directions), in mm; `None` if a mask is empty.
    pub hd95_mm: Option<f64>,
    /// Largest surface distance, in mm.
    pub hausdorff_mm: Option<f64>,
    /// Distance between the centroids, in mm.
    pub centroid_distance_mm: Option<f64>,
}

const NEIGHBOURS_6: [[i32; 3]; 6] = [[-1, 0, 0], [1, 0, 0], [0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1]];

fn neighbours_26() -> impl Iterator<Item = [i32; 3]> {
    (-1..=1).flat_map(|k| (-1..=1).flat_map(move |j| (-1..=1).map(move |i| [i, j, k]))).filter(|d| *d != [0, 0, 0])
}

impl MaskRegion {
    /// The voxels labelled `label`, cropped to their bounding box; `None` if
    /// there are none.
    pub fn of_label(labels: &LabelMap, label: u8) -> Option<Self> {
        Self::from_grid(labels.dims(), |idx| labels.data()[idx] == label)
    }

    /// The voxels of a grid of `dims` for which `inside(linear index)` holds,
    /// cropped to their bounding box; `None` if there are none.
    pub fn from_grid(dims: Dims3, inside: impl Fn(usize) -> bool) -> Option<Self> {
        let mut bounds: Option<(UVec3, UVec3)> = None;
        for k in 0..dims.z {
            for j in 0..dims.y {
                let row = dims.index(0, j, k);
                for i in 0..dims.x {
                    if inside(row + i as usize) {
                        let p = UVec3::new(i, j, k);
                        bounds = Some(bounds.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))));
                    }
                }
            }
        }
        let (lo, hi) = bounds?;
        let bx = VoxelBox::new(lo, hi + UVec3::ONE);
        let mut out = Self::empty(bx);
        let mut n = 0;
        for k in bx.min.z..bx.max.z {
            for j in bx.min.y..bx.max.y {
                for i in bx.min.x..bx.max.x {
                    out.inside[n] = inside(dims.index(i, j, k));
                    n += 1;
                }
            }
        }
        Some(out)
    }

    /// A mask over `bx` from `mask` (`i` fastest, non-zero = inside),
    /// cropped to its bounding box; `None` if it is empty.
    pub fn from_box_mask(bx: VoxelBox, mask: &[u8]) -> Option<Self> {
        let full = Self { bx, inside: mask.iter().map(|v| *v != 0).collect() };
        full.cropped()
    }

    /// An empty mask over `bx`.
    pub fn empty(bx: VoxelBox) -> Self {
        Self { bx, inside: vec![false; bx.voxel_count()] }
    }

    /// Size of the box.
    pub fn size(&self) -> UVec3 {
        self.bx.size()
    }

    fn local_index(&self, l: UVec3) -> usize {
        let s = self.size();
        l.x as usize + s.x as usize * (l.y as usize + s.y as usize * l.z as usize)
    }

    fn local_of(&self, n: usize) -> UVec3 {
        let s = self.size();
        let (sx, sy) = (s.x as usize, s.y as usize);
        UVec3::new((n % sx) as u32, ((n / sx) % sy) as u32, (n / (sx * sy)) as u32)
    }

    /// `true` if the grid voxel `p` is inside the mask.
    pub fn contains(&self, p: UVec3) -> bool {
        p.cmpge(self.bx.min).all() && p.cmplt(self.bx.max).all() && self.inside[self.local_index(p - self.bx.min)]
    }

    /// Number of voxels inside.
    pub fn count(&self) -> u64 {
        self.inside.iter().filter(|v| **v).count() as u64
    }

    /// The mask as `u8` values over [`MaskRegion::bx`], ready for
    /// [`crate::SegmentationSet::apply_mask`].
    pub fn to_u8(&self) -> Vec<u8> {
        self.inside.iter().map(|v| u8::from(*v)).collect()
    }

    /// The same mask over the larger box `bx` (which must contain this box).
    pub fn expanded(&self, bx: VoxelBox) -> Self {
        let mut out = Self::empty(bx);
        for (n, v) in self.inside.iter().enumerate() {
            if *v {
                let g = self.bx.min + self.local_of(n);
                let at = out.local_index(g - bx.min);
                out.inside[at] = true;
            }
        }
        out
    }

    /// The mask cropped to its bounding box; `None` if it is empty.
    pub fn cropped(&self) -> Option<Self> {
        let mut bounds: Option<(UVec3, UVec3)> = None;
        for (n, v) in self.inside.iter().enumerate() {
            if *v {
                let p = self.local_of(n);
                bounds = Some(bounds.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))));
            }
        }
        let (lo, hi) = bounds?;
        let bx = VoxelBox::new(self.bx.min + lo, self.bx.min + hi + UVec3::ONE);
        let mut out = Self::empty(bx);
        for n in 0..out.inside.len() {
            let g = bx.min + out.local_of(n);
            out.inside[n] = self.inside[self.local_index(g - self.bx.min)];
        }
        Some(out)
    }

    /// The part of the mask inside `bx`; `None` if nothing remains.
    pub fn restricted(&self, bx: VoxelBox) -> Option<Self> {
        let mut out = self.clone();
        for n in 0..out.inside.len() {
            let g = out.bx.min + out.local_of(n);
            if !(g.cmpge(bx.min).all() && g.cmplt(bx.max).all()) {
                out.inside[n] = false;
            }
        }
        out.cropped()
    }

    /// Mean voxel index of the voxels inside.
    pub fn centroid(&self) -> DVec3 {
        let (mut sum, mut n) = (DVec3::ZERO, 0u64);
        for (i, v) in self.inside.iter().enumerate() {
            if *v {
                sum += (self.bx.min + self.local_of(i)).as_dvec3();
                n += 1;
            }
        }
        if n == 0 {
            sum
        } else {
            sum / n as f64
        }
    }

    fn neighbour(&self, l: UVec3, d: [i32; 3]) -> Option<UVec3> {
        let s = self.size();
        let n = [l.x as i64 + i64::from(d[0]), l.y as i64 + i64::from(d[1]), l.z as i64 + i64::from(d[2])];
        let fits = n.iter().zip([s.x, s.y, s.z]).all(|(v, m)| *v >= 0 && *v < i64::from(m));
        fits.then(|| UVec3::new(n[0] as u32, n[1] as u32, n[2] as u32))
    }

    /// `true` if the voxel `n` (local index) is inside and has a 6-neighbour
    /// outside the mask.
    fn is_surface(&self, n: usize) -> bool {
        if !self.inside[n] {
            return false;
        }
        let l = self.local_of(n);
        NEIGHBOURS_6.iter().any(|d| self.neighbour(l, *d).is_none_or(|m| !self.inside[self.local_index(m)]))
    }
}

/// 26-connected components of `r`, largest first, and the component
/// number of every voxel of the box (`0` outside, `n + 1` for the `n`-th
/// component of the returned list).
pub fn components(r: &MaskRegion) -> (Vec<Component>, Vec<u32>) {
    let mut ids = vec![0u32; r.inside.len()];
    let mut found: Vec<Component> = Vec::new();
    for start in 0..r.inside.len() {
        if r.inside[start] && ids[start] == 0 {
            let id = found.len() as u32 + 1;
            found.push(flood(r, start, id, &mut ids));
        }
    }
    let mut order: Vec<usize> = (0..found.len()).collect();
    order.sort_by(|a, b| found[*b].voxels.cmp(&found[*a].voxels).then(a.cmp(b)));
    let mut renumber = vec![0u32; found.len() + 1];
    for (rank, old) in order.iter().enumerate() {
        renumber[old + 1] = rank as u32 + 1;
    }
    for id in &mut ids {
        *id = renumber[*id as usize];
    }
    let sorted = order.into_iter().map(|i| found[i].clone()).collect();
    (sorted, ids)
}

fn flood(r: &MaskRegion, start: usize, id: u32, ids: &mut [u32]) -> Component {
    let mut queue = VecDeque::from([start]);
    ids[start] = id;
    let first = r.local_of(start);
    let (mut lo, mut hi, mut sum, mut n) = (first, first, DVec3::ZERO, 0u64);
    while let Some(c) = queue.pop_front() {
        let l = r.local_of(c);
        lo = lo.min(l);
        hi = hi.max(l);
        sum += l.as_dvec3();
        n += 1;
        for d in neighbours_26() {
            if let Some(m) = r.neighbour(l, d) {
                let at = r.local_index(m);
                if r.inside[at] && ids[at] == 0 {
                    ids[at] = id;
                    queue.push_back(at);
                }
            }
        }
    }
    let base = r.bx.min;
    Component {
        voxels: n,
        bounds: VoxelBox::new(base + lo, base + hi + UVec3::ONE),
        centroid: base.as_dvec3() + sum / n as f64,
    }
}

/// The components of `r` as separate masks, largest first, keeping those
/// with at least `min_voxels` voxels.
pub fn split_components(r: &MaskRegion, min_voxels: u64) -> Vec<(Component, MaskRegion)> {
    let (list, ids) = components(r);
    list.into_iter()
        .enumerate()
        .filter(|(_, c)| c.voxels >= min_voxels)
        .filter_map(|(n, c)| {
            let id = n as u32 + 1;
            let mask = MaskRegion { bx: r.bx, inside: ids.iter().map(|v| *v == id).collect() };
            mask.cropped().map(|m| (c, m))
        })
        .collect()
}

/// Only the largest component of `r`.
pub fn keep_largest(r: &MaskRegion) -> MaskRegion {
    let (_, ids) = components(r);
    MaskRegion { bx: r.bx, inside: ids.iter().map(|v| *v == 1).collect() }
}

/// `r` with its enclosed holes filled: voxels outside the mask that are not
/// 6-connected to the border of the box. `fillable(grid voxel)` says
/// whether a hole voxel may be filled (e.g. it is background).
pub fn fill_holes(r: &MaskRegion, fillable: impl Fn(UVec3) -> bool) -> MaskRegion {
    let s = r.size();
    let mut outside = vec![false; r.inside.len()];
    let mut queue = VecDeque::new();
    for (n, (inside, out)) in r.inside.iter().zip(outside.iter_mut()).enumerate() {
        let l = r.local_of(n);
        let on_border = l.x == 0 || l.y == 0 || l.z == 0 || l.x + 1 == s.x || l.y + 1 == s.y || l.z + 1 == s.z;
        if on_border && !inside {
            *out = true;
            queue.push_back(n);
        }
    }
    while let Some(c) = queue.pop_front() {
        let l = r.local_of(c);
        for d in NEIGHBOURS_6 {
            if let Some(m) = r.neighbour(l, d) {
                let at = r.local_index(m);
                if !r.inside[at] && !outside[at] {
                    outside[at] = true;
                    queue.push_back(at);
                }
            }
        }
    }
    let inside =
        (0..r.inside.len()).map(|n| r.inside[n] || (!outside[n] && fillable(r.bx.min + r.local_of(n)))).collect();
    MaskRegion { bx: r.bx, inside }
}

fn per_slice_counts(r: &MaskRegion) -> [Vec<u64>; 3] {
    let s = r.size();
    let mut counts = [vec![0u64; s.x as usize], vec![0u64; s.y as usize], vec![0u64; s.z as usize]];
    for (n, v) in r.inside.iter().enumerate() {
        if *v {
            let l = r.local_of(n);
            counts[0][l.x as usize] += 1;
            counts[1][l.y as usize] += 1;
            counts[2][l.z as usize] += 1;
        }
    }
    counts
}

fn largest(counts: &[u64], offset: u32) -> LargestSlice {
    let mut best = LargestSlice { index: offset, voxels: 0 };
    for (n, c) in counts.iter().enumerate() {
        if *c > best.voxels {
            best = LargestSlice { index: offset + n as u32, voxels: *c };
        }
    }
    best
}

/// Shape measurements of `r` on a grid of `dims` with `spacing` (mm).
pub fn shape(r: &MaskRegion, dims: Dims3, spacing: Vec3) -> Shape {
    let sp = spacing.as_dvec3();
    let size = r.size().as_dvec3();
    let counts = per_slice_counts(r);
    // indices: [sagittal (i), coronal (j), axial (k)] → reported as axial, coronal, sagittal
    let largest_slices =
        [largest(&counts[2], r.bx.min.z), largest(&counts[1], r.bx.min.y), largest(&counts[0], r.bx.min.x)];
    let (long_axis, short_axis) = axial_axes(r, largest_slices[0].index, spacing);
    let (list, _) = components(r);
    let voxels = r.count();
    let touches_border = touches_low(r) || touches_high(r, dims);
    Shape {
        voxels,
        bounds: r.bx,
        centroid: r.centroid(),
        extent_mm: [size.x * sp.x, size.y * sp.y, size.z * sp.z],
        largest_slices,
        long_axis,
        short_axis,
        touches_border,
        components: list.len(),
        largest_component_share: list.first().map_or(0.0, |c| c.voxels as f64 / voxels.max(1) as f64),
    }
}

/// The mask has a voxel on a low face of the grid (index 0 on some axis).
fn touches_low(r: &MaskRegion) -> bool {
    r.bx.min.cmpeq(UVec3::ZERO).any()
        && r.inside.iter().enumerate().any(|(n, v)| {
            let g = r.bx.min + r.local_of(n);
            *v && (g.x == 0 || g.y == 0 || g.z == 0)
        })
}

/// The mask has a voxel on a high face of the grid.
fn touches_high(r: &MaskRegion, dims: Dims3) -> bool {
    if !(r.bx.max.x == dims.x || r.bx.max.y == dims.y || r.bx.max.z == dims.z) {
        return false;
    }
    r.inside.iter().enumerate().any(|(n, v)| {
        let g = r.bx.min + r.local_of(n);
        *v && (g.x + 1 == dims.x || g.y + 1 == dims.y || g.z + 1 == dims.z)
    })
}

/// Pixel centres of the mask on axial slice `k`, in mm in the slice plane,
/// with their voxel indices.
fn slice_points(r: &MaskRegion, k: u32, spacing: Vec3) -> Vec<([f64; 2], UVec3)> {
    let mut out = Vec::new();
    if k < r.bx.min.z || k >= r.bx.max.z {
        return out;
    }
    let s = r.size();
    let lz = k - r.bx.min.z;
    for j in 0..s.y {
        for i in 0..s.x {
            if r.inside[r.local_index(UVec3::new(i, j, lz))] {
                let g = r.bx.min + UVec3::new(i, j, lz);
                out.push(([f64::from(g.x) * f64::from(spacing.x), f64::from(g.y) * f64::from(spacing.y)], g));
            }
        }
    }
    out
}

/// Convex hull (monotone chain) of `pts`, as indices into `pts`.
fn hull(pts: &[([f64; 2], UVec3)]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    idx.sort_by(|a, b| pts[*a].0[0].total_cmp(&pts[*b].0[0]).then(pts[*a].0[1].total_cmp(&pts[*b].0[1])));
    if idx.len() < 3 {
        return idx;
    }
    let cross = |o: usize, a: usize, b: usize| {
        let (o, a, b) = (pts[o].0, pts[a].0, pts[b].0);
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut lower: Vec<usize> = Vec::new();
    for &p in &idx {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0 {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<usize> = Vec::new();
    for &p in idx.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0 {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Long axis (longest distance between pixel centres) and short axis
/// (longest extent perpendicular to it, within bands one pixel wide along
/// the long axis) on axial slice `k`.
fn axial_axes(r: &MaskRegion, k: u32, spacing: Vec3) -> (Option<Diameter>, Option<Diameter>) {
    let pts = slice_points(r, k, spacing);
    let Some(first) = pts.first() else { return (None, None) };
    let h = hull(&pts);
    let mut best = (0.0, first.1, first.1, first.0, first.0);
    for (n, a) in h.iter().enumerate() {
        for b in &h[n + 1..] {
            let d = dist(pts[*a].0, pts[*b].0);
            if d > best.0 {
                best = (d, pts[*a].1, pts[*b].1, pts[*a].0, pts[*b].0);
            }
        }
    }
    let long = Diameter { mm: best.0, from: best.1, to: best.2 };
    if best.0 <= 0.0 {
        return (Some(long), Some(Diameter { mm: 0.0, from: first.1, to: first.1 }));
    }
    let u = [(best.4[0] - best.3[0]) / best.0, (best.4[1] - best.3[1]) / best.0];
    let nrm = [-u[1], u[0]];
    let band = f64::from(spacing.x.min(spacing.y));
    let mut bands: std::collections::HashMap<i64, (f64, usize, f64, usize)> = std::collections::HashMap::new();
    for (n, (p, _)) in pts.iter().enumerate() {
        let s = ((p[0] - best.3[0]) * u[0] + (p[1] - best.3[1]) * u[1]) / band;
        let t = (p[0] - best.3[0]) * nrm[0] + (p[1] - best.3[1]) * nrm[1];
        let e = bands.entry(s.round() as i64).or_insert((t, n, t, n));
        if t < e.0 {
            (e.0, e.1) = (t, n);
        }
        if t > e.2 {
            (e.2, e.3) = (t, n);
        }
    }
    let short = bands
        .values()
        .map(|(lo, a, hi, b)| (hi - lo, *a, *b))
        .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)))
        .map(|(_, a, b)| Diameter { mm: dist(pts[a].0, pts[b].0), from: pts[a].1, to: pts[b].1 });
    (Some(long), short)
}

/// Agreement of `a` and `b` (either may be empty) with `spacing` (mm).
pub fn compare(a: Option<&MaskRegion>, b: Option<&MaskRegion>, spacing: Vec3) -> Agreement {
    let (na, nb) = (a.map_or(0, MaskRegion::count), b.map_or(0, MaskRegion::count));
    let intersection = match (a, b) {
        (Some(a), Some(b)) => {
            a.inside.iter().enumerate().filter(|(n, v)| **v && b.contains(a.bx.min + a.local_of(*n))).count() as u64
        }
        _ => 0,
    };
    let union = na + nb - intersection;
    let dice = if na + nb == 0 { 1.0 } else { 2.0 * intersection as f64 / (na + nb) as f64 };
    let jaccard = if union == 0 { 1.0 } else { intersection as f64 / union as f64 };
    let mut out = Agreement {
        voxels_a: na,
        voxels_b: nb,
        intersection,
        dice,
        jaccard,
        hd95_mm: None,
        hausdorff_mm: None,
        centroid_distance_mm: None,
    };
    if let (Some(a), Some(b)) = (a, b) {
        let sp = spacing.as_dvec3();
        out.centroid_distance_mm = Some(((a.centroid() - b.centroid()) * sp).length());
        let (ab, ba) = (surface_distances(a, b, spacing), surface_distances(b, a, spacing));
        out.hd95_mm = Some(percentile(&ab, 0.95).max(percentile(&ba, 0.95)));
        out.hausdorff_mm = Some(percentile(&ab, 1.0).max(percentile(&ba, 1.0)));
    }
    out
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

/// Distances (mm, sorted) from every surface voxel of `from` to the nearest
/// surface voxel of `to`, via an exact Euclidean distance transform over
/// the union of both boxes.
fn surface_distances(from: &MaskRegion, to: &MaskRegion, spacing: Vec3) -> Vec<f64> {
    let bx = from.bx.union(to.bx);
    let target = to.expanded(bx);
    let seeds: Vec<bool> = (0..target.inside.len()).map(|n| target.is_surface(n)).collect();
    let d2 = squared_edt(&seeds, bx.size(), spacing);
    let mut out: Vec<f64> = (0..from.inside.len())
        .filter(|n| from.is_surface(*n))
        .map(|n| {
            let g = from.bx.min + from.local_of(n) - bx.min;
            let s = bx.size();
            d2[g.x as usize + s.x as usize * (g.y as usize + s.y as usize * g.z as usize)].sqrt()
        })
        .collect();
    out.sort_by(f64::total_cmp);
    out
}

/// Squared Euclidean distance (mm²) of every voxel of a box of `size` to
/// the nearest seed, with anisotropic `spacing` (Felzenszwalb–Huttenlocher,
/// separable).
pub(crate) fn squared_edt(seeds: &[bool], size: UVec3, spacing: Vec3) -> Vec<f64> {
    let mut f: Vec<f64> = seeds.iter().map(|s| if *s { 0.0 } else { f64::INFINITY }).collect();
    let (sx, sy, sz) = (size.x as usize, size.y as usize, size.z as usize);
    let lines: [(usize, usize, usize, f64); 3] = [
        (sx, 1, sy * sz, f64::from(spacing.x)),
        (sy, sx, sx * sz, f64::from(spacing.y)),
        (sz, sx * sy, sx * sy, f64::from(spacing.z)),
    ];
    for (axis, (len, stride, count, w)) in lines.into_iter().enumerate() {
        let mut line = vec![0.0; len];
        for n in 0..count {
            let base = match axis {
                0 => n * sx,
                1 => (n % sx) + (n / sx) * sx * sy,
                _ => n,
            };
            for (t, v) in line.iter_mut().enumerate() {
                *v = f[base + t * stride];
            }
            for (t, v) in edt_1d(&line, w).into_iter().enumerate() {
                f[base + t * stride] = v;
            }
        }
    }
    f
}

/// 1D squared distance transform of `f` with sample spacing `w`.
fn edt_1d(f: &[f64], w: f64) -> Vec<f64> {
    let n = f.len();
    let finite: Vec<usize> = (0..n).filter(|q| f[*q].is_finite()).collect();
    if finite.is_empty() {
        return vec![f64::INFINITY; n];
    }
    let pos = |q: usize| q as f64 * w;
    let meet = |p: usize, q: usize| ((f[q] + pos(q) * pos(q)) - (f[p] + pos(p) * pos(p))) / (2.0 * (pos(q) - pos(p)));
    let mut v: Vec<usize> = Vec::with_capacity(finite.len());
    let mut z: Vec<f64> = Vec::with_capacity(finite.len() + 1);
    for &q in &finite {
        while let Some(&last) = v.last() {
            if meet(last, q) <= *z.last().unwrap_or(&f64::NEG_INFINITY) {
                v.pop();
                z.pop();
            } else {
                break;
            }
        }
        z.push(v.last().map_or(f64::NEG_INFINITY, |&last| meet(last, q)));
        v.push(q);
    }
    z.push(f64::INFINITY);
    let mut out = vec![0.0; n];
    let mut k = 0;
    for (q, o) in out.iter_mut().enumerate() {
        while z[k + 1] < pos(q) {
            k += 1;
        }
        let d = pos(q) - pos(v[k]);
        *o = d * d + f[v[k]];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ball(dims: Dims3, c: [f32; 3], r: f32) -> MaskRegion {
        MaskRegion::from_grid(dims, |n| {
            let (i, j, k) = (n as u32 % dims.x, (n as u32 / dims.x) % dims.y, n as u32 / (dims.x * dims.y));
            (i as f32 - c[0]).powi(2) + (j as f32 - c[1]).powi(2) + (k as f32 - c[2]).powi(2) <= r * r
        })
        .unwrap()
    }

    #[test]
    fn regions_crop_and_expand() {
        let dims = Dims3::new(10, 10, 10);
        let r = ball(dims, [5.0, 5.0, 5.0], 2.0);
        assert_eq!(r.bx, VoxelBox::new(UVec3::splat(3), UVec3::splat(8)));
        assert_eq!(r.count(), 33);
        assert!(r.contains(UVec3::splat(5)) && !r.contains(UVec3::splat(3)) && !r.contains(UVec3::ZERO));
        let big = r.expanded(VoxelBox::full(dims));
        assert_eq!(big.cropped().unwrap(), r);
        assert_eq!(MaskRegion::from_box_mask(big.bx, &big.to_u8()).unwrap(), r);
        assert!((r.centroid() - DVec3::splat(5.0)).length() < 1e-9);
        assert!(r.restricted(VoxelBox::new(UVec3::ZERO, UVec3::splat(2))).is_none());
        assert_eq!(r.restricted(VoxelBox::new(UVec3::ZERO, UVec3::new(10, 10, 5))).unwrap().bx.max.z, 5);
        assert!(MaskRegion::from_box_mask(r.bx, &vec![0; r.bx.voxel_count()]).is_none());
        let labels = LabelMap::from_data(dims, big.to_u8().iter().map(|v| v * 7).collect()).unwrap();
        assert_eq!(MaskRegion::of_label(&labels, 7).unwrap(), r);
        assert!(MaskRegion::of_label(&labels, 3).is_none());
    }

    #[test]
    fn components_are_sorted_and_split() {
        let dims = Dims3::new(20, 10, 10);
        let a = ball(dims, [4.0, 5.0, 5.0], 2.0).expanded(VoxelBox::full(dims));
        let b = ball(dims, [14.0, 5.0, 5.0], 3.0).expanded(VoxelBox::full(dims));
        let both = MaskRegion {
            bx: VoxelBox::full(dims),
            inside: a.inside.iter().zip(&b.inside).map(|(x, y)| *x || *y).collect(),
        }
        .cropped()
        .unwrap();
        let (list, ids) = components(&both);
        assert_eq!(list.len(), 2);
        assert!(list[0].voxels > list[1].voxels);
        assert!((list[0].centroid.x - 14.0).abs() < 1e-9);
        assert_eq!(ids.iter().filter(|v| **v == 2).count() as u64, list[1].voxels);
        let parts = split_components(&both, 40);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].1.count(), list[0].voxels);
        assert_eq!(keep_largest(&both).count(), list[0].voxels);
        // diagonal neighbours belong together (26-connectivity)
        let diag = MaskRegion {
            bx: VoxelBox::new(UVec3::ZERO, UVec3::splat(2)),
            inside: vec![true, false, false, false, false, false, false, true],
        };
        assert_eq!(components(&diag).0.len(), 1);
    }

    #[test]
    fn holes_are_filled_where_allowed() {
        let dims = Dims3::new(9, 9, 9);
        let solid = ball(dims, [4.0, 4.0, 4.0], 3.5);
        let mut hollow = solid.clone();
        let centre = hollow.local_index(UVec3::splat(4) - hollow.bx.min);
        hollow.inside[centre] = false;
        assert_eq!(fill_holes(&hollow, |_| true), solid);
        assert_eq!(fill_holes(&hollow, |_| false), hollow);
    }

    #[test]
    fn shape_of_a_box() {
        let dims = Dims3::new(30, 30, 10);
        // 10 × 4 voxels on slices 2..5 at 0.5 mm in-plane, 2 mm slices
        let r = MaskRegion::from_grid(dims, |n| {
            let (i, j, k) = (n as u32 % 30, (n as u32 / 30) % 30, n as u32 / 900);
            (5..15).contains(&i) && (10..14).contains(&j) && (2..5).contains(&k)
        })
        .unwrap();
        let s = shape(&r, dims, Vec3::new(0.5, 0.5, 2.0));
        assert_eq!(s.voxels, 120);
        assert_eq!(s.extent_mm, [5.0, 2.0, 6.0]);
        assert_eq!(s.largest_slices[0], LargestSlice { index: 2, voxels: 40 });
        assert_eq!(s.largest_slices[2].voxels, 12);
        let long = s.long_axis.unwrap();
        assert!((long.mm - (4.5f64.powi(2) + 1.5f64.powi(2)).sqrt()).abs() < 1e-9, "{long:?}");
        let short = s.short_axis.unwrap();
        assert!(short.mm > 1.0 && short.mm < 3.0, "{short:?}");
        assert!(!s.touches_border);
        assert_eq!((s.components, s.largest_component_share), (1, 1.0));
        let edge = MaskRegion::from_grid(dims, |n| n % 900 == 0).unwrap();
        assert!(shape(&edge, dims, Vec3::ONE).touches_border);
        let far = MaskRegion::from_grid(dims, |n| n == dims.voxel_count() - 1).unwrap();
        let s = shape(&far, dims, Vec3::ONE);
        assert!(s.touches_border);
        assert_eq!((s.long_axis.unwrap().mm, s.short_axis.unwrap().mm), (0.0, 0.0));
    }

    #[test]
    fn agreement_of_shifted_balls() {
        let dims = Dims3::new(30, 30, 30);
        let a = ball(dims, [15.0, 15.0, 15.0], 6.0);
        let same = compare(Some(&a), Some(&a), Vec3::ONE);
        assert_eq!(
            (same.dice, same.jaccard, same.hd95_mm, same.centroid_distance_mm),
            (1.0, 1.0, Some(0.0), Some(0.0))
        );
        let b = ball(dims, [17.0, 15.0, 15.0], 6.0);
        let shifted = compare(Some(&a), Some(&b), Vec3::new(1.0, 1.0, 1.0));
        assert!(shifted.dice > 0.7 && shifted.dice < 0.95, "{shifted:?}");
        assert!((shifted.centroid_distance_mm.unwrap() - 2.0).abs() < 1e-9);
        assert!((shifted.hausdorff_mm.unwrap() - 2.0).abs() < 1e-9, "{shifted:?}");
        let scaled = compare(Some(&a), Some(&b), Vec3::new(0.5, 1.0, 1.0));
        assert!((scaled.hausdorff_mm.unwrap() - 1.0).abs() < 1e-9);
        let none = compare(Some(&a), None, Vec3::ONE);
        assert_eq!((none.dice, none.hd95_mm), (0.0, None));
        assert_eq!(compare(None, None, Vec3::ONE).dice, 1.0);
    }

    #[test]
    fn distance_transform_is_exact() {
        let f = [f64::INFINITY, 0.0, f64::INFINITY, f64::INFINITY, 0.0];
        assert_eq!(edt_1d(&f, 2.0), vec![4.0, 0.0, 4.0, 4.0, 0.0]);
        assert!(edt_1d(&[f64::INFINITY; 3], 1.0).iter().all(|v| v.is_infinite()));
        let mut seeds = vec![false; 27];
        seeds[0] = true;
        let d = squared_edt(&seeds, UVec3::splat(3), Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(d[26], 4.0 + 16.0 + 36.0);
        assert_eq!(d[1 + 3], 1.0 + 4.0);
        // a line of one column: the second axis must not be mistaken for the first
        let d = squared_edt(&[true, false, false], UVec3::new(1, 3, 1), Vec3::ONE);
        assert_eq!(d, vec![0.0, 1.0, 4.0]);
    }
}
