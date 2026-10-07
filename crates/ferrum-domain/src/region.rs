//! Noise-robust region growing for whole organs (liver, lung) as well as
//! lesions.
//!
//! Plain [`grow_region`](crate::grow_region) compares every raw voxel with
//! the value of one seed voxel, so scanner noise decides the result: a
//! noisy seed misses the organ, noisy voxels leave holes and thin bridges
//! leak into neighbours. [`grow_region_robust`] fixes each of these:
//!
//! 1. the reference value is the **median of the smoothed values around the
//!    seed**, not one voxel;
//! 2. voxels are accepted by their **smoothed value** (box mean over
//!    `smoothing_mm` in each direction, in millimetres so that thick and
//!    thin slices are treated alike);
//! 3. an **opening** of `opening_mm` (a distance transform in millimetres,
//!    so anisotropic voxels are handled) removes bridges thinner than
//!    twice that between the organ and its neighbours;
//! 4. **holes** enclosed by the region (vessels in the liver, bronchi and
//!    vessels in the lung) are filled.

use std::collections::VecDeque;

use glam::{UVec3, Vec3};

use crate::analysis::squared_edt;
use crate::segmentation::{SegmentationSet, VoxelBox};
use crate::volume::Volume;

/// Parameters of [`grow_region_robust`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionParams {
    /// Accepted distance of a smoothed voxel value from the reference
    /// value, in the volume's units.
    pub tolerance: f32,
    /// Region size limit in voxels; a larger region has leaked.
    pub max_voxels: u64,
    /// Half-width of the box mean applied before comparing, in millimetres
    /// (`0` compares raw values; at most [`MAX_SMOOTHING_VOXELS`] voxels
    /// per axis).
    pub smoothing_mm: f32,
    /// Radius of the opening in millimetres (`0` keeps thin bridges).
    pub opening_mm: f32,
    /// Fill cavities enclosed by the region.
    pub fill_holes: bool,
}

/// Why no region was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionError {
    /// The seed is outside the grid, or the label map is on another grid.
    OutOfGrid,
    /// The seed already carries a label.
    Labelled,
    /// The region exceeded `max_voxels`.
    TooLarge,
}

/// How a voxel takes part in the growing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Accept {
    /// Not part of the region.
    No,
    /// Smoothed value in range: part of the region, and growing goes on
    /// from it.
    Core,
    /// Only the raw value is in range: part of the region (so smoothing
    /// does not eat the organ's boundary layer), but growing stops here,
    /// so noise cannot open a path into neighbouring tissue.
    Edge,
}

/// Largest smoothing radius per axis, in voxels.
pub const MAX_SMOOTHING_VOXELS: i64 = 6;
/// Radius of the seed neighbourhood whose median is the reference value.
const SEED_RADIUS: i64 = 1;

/// Grows the 6-connected region of unlabelled voxels around `seed` whose
/// smoothed value is within `params.tolerance` of the reference value (see
/// the module documentation).
///
/// Returns the region's bounding box and its mask over that box (`i`
/// fastest, `1` inside), ready for [`SegmentationSet::apply_mask`].
pub fn grow_region_robust(
    volume: &Volume,
    labels: &SegmentationSet,
    seed: UVec3,
    params: &RegionParams,
) -> Result<(VoxelBox, Vec<u8>), RegionError> {
    let d = volume.dims();
    if labels.dims() != d || !d.contains(i64::from(seed.x), i64::from(seed.y), i64::from(seed.z)) {
        return Err(RegionError::OutOfGrid);
    }
    if labels.labels().label(seed.x, seed.y, seed.z) != Some(0) {
        return Err(RegionError::Labelled);
    }
    let sp = volume.spacing();
    let axis_radius = |mm: f32, w: f32| ((mm / w.max(1e-6)).floor() as i64).clamp(0, MAX_SMOOTHING_VOXELS);
    let radius = [
        axis_radius(params.smoothing_mm, sp.x),
        axis_radius(params.smoothing_mm, sp.y),
        axis_radius(params.smoothing_mm, sp.z),
    ];
    let smooth = |i: i64, j: i64, k: i64| smoothed(volume, radius, i, j, k);
    let reference = seed_reference(&smooth, seed, d.x, d.y, d.z).ok_or(RegionError::OutOfGrid)?;
    let (lo, hi) = (reference - params.tolerance, reference + params.tolerance);
    let accept = |i: u32, j: u32, k: u32| {
        if labels.labels().label(i, j, k) != Some(0) {
            return Accept::No;
        }
        if smooth(i64::from(i), i64::from(j), i64::from(k)).is_some_and(|x| (lo..=hi).contains(&x)) {
            Accept::Core
        } else if volume.physical(i, j, k).is_some_and(|x| (lo..=hi).contains(&x)) {
            Accept::Edge
        } else {
            Accept::No
        }
    };
    let (bx, mask) = flood(d, seed, params.max_voxels, accept).ok_or(RegionError::TooLarge)?;
    let mut mask = Mask::new(bx, mask);
    if params.opening_mm > 0.0 {
        mask.open(params.opening_mm, sp, seed);
    }
    if params.fill_holes {
        mask.fill_holes(|i, j, k| labels.labels().label(i, j, k) == Some(0));
    }
    mask.into_result().ok_or(RegionError::OutOfGrid)
}

/// Box mean of the values within `radius` voxels (per axis) of `(i, j, k)`.
fn smoothed(volume: &Volume, radius: [i64; 3], i: i64, j: i64, k: i64) -> Option<f32> {
    let d = volume.dims();
    if radius == [0; 3] {
        return volume.physical(u32::try_from(i).ok()?, u32::try_from(j).ok()?, u32::try_from(k).ok()?);
    }
    let (mut sum, mut n) = (0.0f64, 0u32);
    for z in (k - radius[2]).max(0)..=(k + radius[2]).min(i64::from(d.z) - 1) {
        for y in (j - radius[1]).max(0)..=(j + radius[1]).min(i64::from(d.y) - 1) {
            for x in (i - radius[0]).max(0)..=(i + radius[0]).min(i64::from(d.x) - 1) {
                if let Some(v) = volume.physical(x as u32, y as u32, z as u32) {
                    sum += f64::from(v);
                    n += 1;
                }
            }
        }
    }
    (n > 0).then(|| (sum / f64::from(n)) as f32)
}

/// Median of the smoothed values in the cube around the seed.
fn seed_reference(
    smooth: &impl Fn(i64, i64, i64) -> Option<f32>,
    seed: UVec3,
    nx: u32,
    ny: u32,
    nz: u32,
) -> Option<f32> {
    let mut values = Vec::new();
    let (si, sj, sk) = (i64::from(seed.x), i64::from(seed.y), i64::from(seed.z));
    for k in (sk - SEED_RADIUS).max(0)..=(sk + SEED_RADIUS).min(i64::from(nz) - 1) {
        for j in (sj - SEED_RADIUS).max(0)..=(sj + SEED_RADIUS).min(i64::from(ny) - 1) {
            for i in (si - SEED_RADIUS).max(0)..=(si + SEED_RADIUS).min(i64::from(nx) - 1) {
                values.extend(smooth(i, j, k).filter(|v| v.is_finite()));
            }
        }
    }
    values.sort_by(f32::total_cmp);
    values.get(values.len() / 2).copied()
}

/// 6-connected flood fill from `seed` over voxels satisfying `accept`; each
/// voxel is tested once. The seed is part of the region whatever its own
/// value (the person chose it; the reference value is only close to it).
/// `None` when the region exceeds `max_voxels`.
fn flood(
    d: crate::geometry::Dims3,
    seed: UVec3,
    max_voxels: u64,
    accept: impl Fn(u32, u32, u32) -> Accept,
) -> Option<(VoxelBox, Vec<u8>)> {
    const UNSEEN: u8 = 0;
    const INSIDE: u8 = 1;
    const REJECTED: u8 = 2;
    const EDGE: u8 = 3;
    let mut state = vec![UNSEEN; d.voxel_count()];
    state[d.index(seed.x, seed.y, seed.z)] = INSIDE;
    let mut queue = VecDeque::from([seed]);
    let (mut lo, mut hi, mut count) = (seed, seed, 0u64);
    while let Some(v) = queue.pop_front() {
        count += 1;
        if count > max_voxels {
            return None;
        }
        lo = lo.min(v);
        hi = hi.max(v);
        for n in cube(d, v) {
            let idx = d.index(n.x, n.y, n.z);
            if state[idx] != UNSEEN {
                continue;
            }
            let face = n.x.abs_diff(v.x) + n.y.abs_diff(v.y) + n.z.abs_diff(v.z) == 1;
            state[idx] = match accept(n.x, n.y, n.z) {
                Accept::Core if face => {
                    queue.push_back(n);
                    INSIDE
                }
                // reached diagonally: growing continues only across faces
                Accept::Core => continue,
                Accept::Edge => {
                    count += 1;
                    if count > max_voxels {
                        return None;
                    }
                    lo = lo.min(n);
                    hi = hi.max(n);
                    EDGE
                }
                Accept::No => REJECTED,
            };
        }
    }
    let bx = VoxelBox::new(lo, hi + UVec3::ONE);
    let mut mask = Vec::with_capacity(bx.voxel_count());
    for k in bx.min.z..bx.max.z {
        for j in bx.min.y..bx.max.y {
            for i in bx.min.x..bx.max.x {
                mask.push(u8::from(matches!(state[d.index(i, j, k)], INSIDE | EDGE)));
            }
        }
    }
    Some((bx, mask))
}

/// The 26 voxels around `v` that lie inside the grid.
fn cube(d: crate::geometry::Dims3, v: UVec3) -> impl Iterator<Item = UVec3> {
    (-1i64..=1)
        .flat_map(|dz| (-1i64..=1).flat_map(move |dy| (-1i64..=1).map(move |dx| (dx, dy, dz))))
        .filter(|&delta| delta != (0, 0, 0))
        .filter_map(move |(dx, dy, dz)| {
            let (x, y, z) = (i64::from(v.x) + dx, i64::from(v.y) + dy, i64::from(v.z) + dz);
            d.contains(x, y, z).then(|| UVec3::new(x as u32, y as u32, z as u32))
        })
}

/// Binary mask over a voxel box, with the morphology the region growing
/// needs.
struct Mask {
    bx: VoxelBox,
    data: Vec<u8>,
}

impl Mask {
    fn new(bx: VoxelBox, data: Vec<u8>) -> Self {
        Self { bx, data }
    }

    fn size(&self) -> (usize, usize, usize) {
        let s = self.bx.size();
        (s.x as usize, s.y as usize, s.z as usize)
    }

    fn at(&self, i: usize, j: usize, k: usize) -> usize {
        let (nx, ny, _) = self.size();
        i + nx * (j + ny * k)
    }

    /// Face neighbours of `idx` inside the box.
    fn around(&self, idx: usize) -> impl Iterator<Item = usize> {
        let (nx, ny, nz) = self.size();
        let (i, j, k) = (idx % nx, (idx / nx) % ny, idx / (nx * ny));
        [
            i.checked_sub(1).map(|i| self.at(i, j, k)),
            (i + 1 < nx).then(|| self.at(i + 1, j, k)),
            j.checked_sub(1).map(|j| self.at(i, j, k)),
            (j + 1 < ny).then(|| self.at(i, j + 1, k)),
            k.checked_sub(1).map(|k| self.at(i, j, k)),
            (k + 1 < nz).then(|| self.at(i, j, k + 1)),
        ]
        .into_iter()
        .flatten()
    }

    /// Opening of radius `r` mm: the core is the part of the region at
    /// least `r` from its outside (exact anisotropic distance transform),
    /// then it is grown back by `r`, never beyond the original region. The
    /// component holding the seed survives (the largest one if the seed
    /// itself is not in the core). A structure with no core (a small
    /// lesion) is kept as is.
    fn open(&mut self, r: f32, spacing: Vec3, seed: UVec3) {
        let (nx, ny, nz) = self.size();
        let r2 = f64::from(r) * f64::from(r);
        // distance of every region voxel to the nearest outside voxel; the
        // margin of one voxel makes the box border count as outside
        let (px, py, pz) = (nx + 2, ny + 2, nz + 2);
        let mut outside = vec![true; px * py * pz];
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    outside[(i + 1) + px * ((j + 1) + py * (k + 1))] = self.data[self.at(i, j, k)] != 1;
                }
            }
        }
        let d2 = squared_edt(&outside, UVec3::new(px as u32, py as u32, pz as u32), spacing);
        let mut core = vec![0u8; self.data.len()];
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let idx = self.at(i, j, k);
                    core[idx] = u8::from(self.data[idx] == 1 && d2[(i + 1) + px * ((j + 1) + py * (k + 1))] >= r2);
                }
            }
        }
        let original = std::mem::replace(&mut self.data, core);
        let s = seed - self.bx.min;
        let Some(core) = self.component(self.at(s.x as usize, s.y as usize, s.z as usize)) else {
            self.data = original;
            return;
        };
        let near = squared_edt(&core.iter().map(|&c| c == 1).collect::<Vec<_>>(), self.bx.size(), spacing);
        self.data = (0..original.len()).map(|n| u8::from(original[n] == 1 && near[n] <= r2)).collect();
    }

    /// The connected component of the mask containing `seed_idx`, or the
    /// largest one when the seed is not set; `None` if the mask is empty.
    fn component(&self, seed_idx: usize) -> Option<Vec<u8>> {
        let mut seen = vec![false; self.data.len()];
        let mut best: Vec<usize> = Vec::new();
        let starts = std::iter::once(seed_idx).chain(0..self.data.len());
        for start in starts {
            if seen[start] || self.data[start] != 1 {
                continue;
            }
            let mut comp = vec![start];
            seen[start] = true;
            let mut head = 0;
            while head < comp.len() {
                let v = comp[head];
                head += 1;
                for n in self.around(v) {
                    if !seen[n] && self.data[n] == 1 {
                        seen[n] = true;
                        comp.push(n);
                    }
                }
            }
            if start == seed_idx && self.data[seed_idx] == 1 {
                best = comp;
                break;
            }
            if comp.len() > best.len() {
                best = comp;
            }
        }
        if best.is_empty() {
            return None;
        }
        let mut out = vec![0; self.data.len()];
        for v in best {
            out[v] = 1;
        }
        Some(out)
    }

    /// Fills cavities not connected to the outside of the box in 3D, and
    /// in each slice along every axis: a vessel that leaves the organ
    /// through its surface is open in 3D but a closed ring in the slices
    /// across it. Only voxels for which `free` holds (unlabelled) are added.
    fn fill_holes(&mut self, free: impl Fn(u32, u32, u32) -> bool) {
        self.fill_holes_3d(&free);
        for axis in 0..3 {
            self.fill_slices(axis, &free);
        }
    }

    /// Fills the enclosed holes of every slice perpendicular to `axis`.
    fn fill_slices(&mut self, axis: usize, free: &impl Fn(u32, u32, u32) -> bool) {
        let (nx, ny, nz) = self.size();
        let dims = [nx, ny, nz];
        let (a, b, c) = ((axis + 1) % 3, (axis + 2) % 3, axis);
        let (na, nb, nc) = (dims[a], dims[b], dims[c]);
        let at = |m: &Self, p: usize, q: usize, r: usize| {
            let mut ijk = [0usize; 3];
            (ijk[a], ijk[b], ijk[c]) = (p, q, r);
            m.at(ijk[0], ijk[1], ijk[2])
        };
        let (wa, wb) = (na + 2, nb + 2);
        for r in 0..nc {
            let mut outside = vec![false; wa * wb];
            let mut queue = VecDeque::from([(0usize, 0usize)]);
            outside[0] = true;
            while let Some((p, q)) = queue.pop_front() {
                let cand = [
                    p.checked_sub(1).map(|p| (p, q)),
                    (p + 1 < wa).then_some((p + 1, q)),
                    q.checked_sub(1).map(|q| (p, q)),
                    (q + 1 < wb).then_some((p, q + 1)),
                ];
                for (x, y) in cand.into_iter().flatten() {
                    let set =
                        (1..=na).contains(&x) && (1..=nb).contains(&y) && self.data[at(self, x - 1, y - 1, r)] == 1;
                    if !outside[x + wa * y] && !set {
                        outside[x + wa * y] = true;
                        queue.push_back((x, y));
                    }
                }
            }
            for q in 0..nb {
                for p in 0..na {
                    let idx = at(self, p, q, r);
                    let mut ijk = [0usize; 3];
                    (ijk[a], ijk[b], ijk[c]) = (p, q, r);
                    let (x, y, z) = (ijk[0] as u32, ijk[1] as u32, ijk[2] as u32);
                    if self.data[idx] == 0
                        && !outside[(p + 1) + wa * (q + 1)]
                        && free(self.bx.min.x + x, self.bx.min.y + y, self.bx.min.z + z)
                    {
                        self.data[idx] = 1;
                    }
                }
            }
        }
    }

    fn fill_holes_3d(&mut self, free: &impl Fn(u32, u32, u32) -> bool) {
        let (nx, ny, nz) = self.size();
        // flood the background from the outside, through a 1-voxel margin
        let (px, py, pz) = (nx + 2, ny + 2, nz + 2);
        let pidx = |i: usize, j: usize, k: usize| i + px * (j + py * k);
        let mut outside = vec![false; px * py * pz];
        let mut queue = VecDeque::from([(0usize, 0usize, 0usize)]);
        outside[0] = true;
        while let Some((i, j, k)) = queue.pop_front() {
            let cand = [
                i.checked_sub(1).map(|i| (i, j, k)),
                (i + 1 < px).then_some((i + 1, j, k)),
                j.checked_sub(1).map(|j| (i, j, k)),
                (j + 1 < py).then_some((i, j + 1, k)),
                k.checked_sub(1).map(|k| (i, j, k)),
                (k + 1 < pz).then_some((i, j, k + 1)),
            ];
            for (a, b, c) in cand.into_iter().flatten() {
                let inside_mask = (1..=nx).contains(&a)
                    && (1..=ny).contains(&b)
                    && (1..=nz).contains(&c)
                    && self.data[self.at(a - 1, b - 1, c - 1)] == 1;
                if !outside[pidx(a, b, c)] && !inside_mask {
                    outside[pidx(a, b, c)] = true;
                    queue.push_back((a, b, c));
                }
            }
        }
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let idx = self.at(i, j, k);
                    if self.data[idx] == 0
                        && !outside[pidx(i + 1, j + 1, k + 1)]
                        && free(self.bx.min.x + i as u32, self.bx.min.y + j as u32, self.bx.min.z + k as u32)
                    {
                        self.data[idx] = 1;
                    }
                }
            }
        }
    }

    /// Crops the mask to its bounding box; `None` when empty.
    fn into_result(self) -> Option<(VoxelBox, Vec<u8>)> {
        let (nx, ny, _) = self.size();
        let (mut lo, mut hi) = ([usize::MAX; 3], [0usize; 3]);
        for idx in (0..self.data.len()).filter(|&idx| self.data[idx] == 1) {
            let ijk = [idx % nx, (idx / nx) % ny, idx / (nx * ny)];
            for a in 0..3 {
                lo[a] = lo[a].min(ijk[a]);
                hi[a] = hi[a].max(ijk[a]);
            }
        }
        if lo[0] == usize::MAX {
            return None;
        }
        let min = self.bx.min + UVec3::new(lo[0] as u32, lo[1] as u32, lo[2] as u32);
        let max = self.bx.min + UVec3::new(hi[0] as u32 + 1, hi[1] as u32 + 1, hi[2] as u32 + 1);
        let bx = VoxelBox::new(min, max);
        let mut out = Vec::with_capacity(bx.voxel_count());
        for k in lo[2]..=hi[2] {
            for j in lo[1]..=hi[1] {
                for i in lo[0]..=hi[0] {
                    out.push(self.data[self.at(i, j, k)]);
                }
            }
        }
        Some((bx, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Dims3;
    use glam::Vec3;

    /// Deterministic noise in `[-amp, amp]`.
    fn noise(i: u32, j: u32, k: u32, amp: f32) -> f32 {
        let mut h = i.wrapping_mul(73856093) ^ j.wrapping_mul(19349663) ^ k.wrapping_mul(83492791);
        h = h.wrapping_mul(2654435761);
        h ^= h >> 15;
        ((h % 2001) as f32 / 1000.0 - 1.0) * amp
    }

    /// 40³ volume: a "liver" cube (60 HU, 12..28) with a bright vessel
    /// tube through it (180 HU), a 1-voxel bridge of liver values to a
    /// second blob, background 0 HU; Gaussian-like noise of ±`amp`.
    fn phantom(amp: f32) -> (Volume, SegmentationSet) {
        let d = Dims3::new(40, 40, 40);
        let mut data = Vec::new();
        for k in 0..40u32 {
            for j in 0..40u32 {
                for i in 0..40u32 {
                    let in_cube = (12..28).contains(&i) && (12..28).contains(&j) && (12..28).contains(&k);
                    let vessel = in_cube && (19..21).contains(&j) && (19..21).contains(&k);
                    let bridge = (28..34).contains(&i) && j == 14 && k == 14;
                    let blob = (34..39).contains(&i) && (11..17).contains(&j) && (11..17).contains(&k);
                    let base = if vessel {
                        180.0
                    } else if in_cube || bridge || blob {
                        60.0
                    } else {
                        0.0
                    };
                    data.push(base + noise(i, j, k, amp));
                }
            }
        }
        (Volume::from_physical(d, Vec3::ONE, &data).unwrap(), SegmentationSet::new(d))
    }

    fn params() -> RegionParams {
        RegionParams { tolerance: 25.0, max_voxels: u64::MAX, smoothing_mm: 1.0, opening_mm: 1.5, fill_holes: true }
    }

    fn count(r: &(VoxelBox, Vec<u8>)) -> usize {
        r.1.iter().filter(|v| **v == 1).count()
    }

    // covers 16.6-b
    #[test]
    fn noise_does_not_break_the_organ_and_vessels_are_filled() {
        let (v, set) = phantom(40.0);
        let r = grow_region_robust(&v, &set, UVec3::new(15, 15, 15), &params()).unwrap();
        // the cube is 16³ = 4096 voxels; allow a little erosion at the edge
        assert!((3500..=4300).contains(&count(&r)), "{}", count(&r));
        // the vessel centre is inside
        let (bx, mask) = &r;
        let rel = UVec3::new(20, 20, 20) - bx.min;
        let s = bx.size();
        assert_eq!(mask[(rel.x + s.x * (rel.y + s.y * rel.z)) as usize], 1, "vessel filled");
    }

    // covers 16.6-a
    #[test]
    fn a_noisy_seed_voxel_does_not_matter() {
        let (mut v, set) = phantom(10.0);
        // an outlier at the seed: raw growing would reject the seed itself
        let d = v.dims();
        let mut data: Vec<f32> = (0..d.voxel_count())
            .map(|idx| {
                let (i, j, k) = (idx as u32 % d.x, (idx as u32 / d.x) % d.y, idx as u32 / (d.x * d.y));
                v.physical(i, j, k).unwrap()
            })
            .collect();
        data[d.index(15, 15, 15)] = 200.0;
        v = Volume::from_physical(d, Vec3::ONE, &data).unwrap();
        assert!(crate::grow_region(&v, &set, UVec3::new(15, 15, 15), 35.0, 85.0, u64::MAX).is_none());
        let r = grow_region_robust(&v, &set, UVec3::new(15, 15, 15), &params()).unwrap();
        assert!(count(&r) > 3500, "{}", count(&r));
    }

    // covers 16.6-c
    #[test]
    fn the_opening_cuts_thin_bridges() {
        let (v, set) = phantom(0.0);
        let without = RegionParams { opening_mm: 0.0, smoothing_mm: 0.0, fill_holes: false, ..params() };
        let leaked = grow_region_robust(&v, &set, UVec3::new(15, 15, 15), &without).unwrap();
        assert!(leaked.0.max.x > 34, "leaks through the bridge into the blob");
        let cut =
            grow_region_robust(&v, &set, UVec3::new(15, 15, 15), &RegionParams { opening_mm: 1.5, ..without }).unwrap();
        assert!(cut.0.max.x <= 29, "bridge removed: {:?}", cut.0);
    }

    // covers 16.6-i
    #[test]
    fn the_opening_works_in_millimetres_on_thick_slices() {
        // 5 mm slices: a bridge 4 voxels wide in the plane but a single slice thick
        // is 4 mm × 5 mm; an opening counted in voxels would not see it as thin
        let d = Dims3::new(40, 40, 12);
        let mut data = vec![0.0f32; d.voxel_count()];
        for k in 0..12u32 {
            for j in 0..40u32 {
                for i in 0..40u32 {
                    let organ = (10..26).contains(&i) && (10..26).contains(&j) && (3..9).contains(&k);
                    let bridge = (26..36).contains(&i) && (16..20).contains(&j) && k == 5;
                    let blob = (36..40).contains(&i) && (10..26).contains(&j) && (3..9).contains(&k);
                    data[d.index(i, j, k)] = if organ || bridge || blob { 60.0 } else { 0.0 };
                }
            }
        }
        let v = Volume::from_physical(d, Vec3::new(1.0, 1.0, 5.0), &data).unwrap();
        let set = SegmentationSet::new(d);
        let p = RegionParams { smoothing_mm: 0.0, opening_mm: 0.0, fill_holes: false, ..params() };
        let seed = UVec3::new(18, 18, 5);
        let leaked = grow_region_robust(&v, &set, seed, &p).unwrap();
        assert_eq!(leaked.0.max.x, 40, "without the opening the blob is reached");
        let cut = grow_region_robust(&v, &set, seed, &RegionParams { opening_mm: 3.0, ..p }).unwrap();
        assert!(cut.0.max.x <= 28, "the bridge is cut: {:?}", cut.0);
        assert_eq!(cut.0.min, UVec3::new(10, 10, 3), "the organ keeps its full extent");
    }

    // covers 16.6-d
    #[test]
    fn small_lesions_survive_the_opening() {
        let (v, set) = phantom(0.0);
        // the blob is 6³ = 216 voxels, wider than the opening
        let r = grow_region_robust(&v, &set, UVec3::new(36, 14, 14), &RegionParams { opening_mm: 2.5, ..params() });
        assert!(r.is_ok());
        // a 3-voxel lesion erodes away and is kept unchanged
        let d = Dims3::new(9, 9, 9);
        let mut data = vec![0.0; 729];
        for (i, j, k) in (3..6).flat_map(|i| (3..6).flat_map(move |j| (3..6).map(move |k| (i, j, k)))) {
            data[d.index(i, j, k)] = 100.0;
        }
        let v = Volume::from_physical(d, Vec3::ONE, &data).unwrap();
        let p = RegionParams { smoothing_mm: 0.0, opening_mm: 3.0, ..params() };
        let r = grow_region_robust(&v, &SegmentationSet::new(d), UVec3::splat(4), &p).unwrap();
        assert_eq!(count(&r), 27);
    }

    // covers 16.6-h
    #[test]
    fn a_seed_on_an_edge_or_a_vessel_is_never_rejected() {
        let (v, set) = phantom(0.0);
        // on the vessel (180 HU) inside a 60 HU organ, with a tolerance that excludes the vessel
        // from the median of its neighbourhood: the seed itself stays in the region
        let p = RegionParams { tolerance: 5.0, smoothing_mm: 1.0, opening_mm: 0.0, fill_holes: false, ..params() };
        for seed in [UVec3::new(15, 19, 19), UVec3::new(12, 12, 12), UVec3::new(27, 27, 27)] {
            let (bx, mask) = grow_region_robust(&v, &set, seed, &p).unwrap();
            let rel = seed - bx.min;
            let s = bx.size();
            assert_eq!(mask[(rel.x + s.x * (rel.y + s.y * rel.z)) as usize], 1, "{seed:?}");
        }
        // too large is only about the limit
        let p = RegionParams { max_voxels: 1, ..p };
        assert_eq!(grow_region_robust(&v, &set, UVec3::splat(15), &p), Err(RegionError::TooLarge));
    }

    // covers 16.6-f
    #[test]
    fn errors_say_why() {
        let (v, mut set) = phantom(0.0);
        let p = params();
        assert_eq!(grow_region_robust(&v, &set, UVec3::splat(99), &p).unwrap_err(), RegionError::OutOfGrid);
        assert_eq!(
            grow_region_robust(&v, &set, UVec3::splat(15), &RegionParams { max_voxels: 10, ..p }).unwrap_err(),
            RegionError::TooLarge
        );
        let l = set.add_segment("x").unwrap();
        set.apply_mask(l, VoxelBox::new(UVec3::splat(15), UVec3::splat(16)), &[1], false).unwrap();
        assert_eq!(grow_region_robust(&v, &set, UVec3::splat(15), &p).unwrap_err(), RegionError::Labelled);
    }
}
