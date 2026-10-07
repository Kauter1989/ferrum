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

/// Largest number of voxels whose opening is computed exactly; larger
/// regions are measured on blocks.
const EXACT_VOXELS: usize = 3_000_000;

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
/// The mean is taken over the stored values (the physical value is an
/// affine function of them), which is several times faster than reading
/// physical values one by one.
fn smoothed(volume: &Volume, radius: [i64; 3], i: i64, j: i64, k: i64) -> Option<f32> {
    let d = volume.dims();
    if !d.contains(i, j, k) {
        return None;
    }
    if radius == [0; 3] {
        return volume.physical(i as u32, j as u32, k as u32);
    }
    let data = volume.data();
    let (x0, x1) = ((i - radius[0]).max(0) as usize, (i + radius[0]).min(i64::from(d.x) - 1) as usize);
    let (mut sum, mut n) = (0u64, 0u64);
    for z in (k - radius[2]).max(0)..=(k + radius[2]).min(i64::from(d.z) - 1) {
        for y in (j - radius[1]).max(0)..=(j + radius[1]).min(i64::from(d.y) - 1) {
            let row = d.index(0, y as u32, z as u32);
            sum += data[row + x0..=row + x1].iter().map(|&v| u64::from(v)).sum::<u64>();
            n += (x1 - x0 + 1) as u64;
        }
    }
    Some(volume.range().from_storage_f64(sum as f64 / n as f64))
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
/// voxel is tested once (the verdict is kept, also for voxels met
/// diagonally before a face neighbour reaches them). The seed is part of
/// the region whatever its own value (the person chose it; the reference
/// value is only close to it). `None` when the region exceeds
/// `max_voxels`.
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
    /// Smoothed value in range, met only diagonally so far.
    const PENDING: u8 = 4;
    let (nx, ny, nz) = (d.x as usize, d.y as usize, d.z as usize);
    let plane = nx * ny;
    // the 26 neighbours as flat offsets; `true` for the 6 face neighbours
    let offsets: Vec<(isize, [i8; 3], bool)> = (-1i8..=1)
        .flat_map(|dz| (-1i8..=1).flat_map(move |dy| (-1i8..=1).map(move |dx| [dx, dy, dz])))
        .filter(|o| *o != [0, 0, 0])
        .map(|o| {
            let delta = o[0] as isize + o[1] as isize * nx as isize + o[2] as isize * plane as isize;
            (delta, o, o.iter().map(|c| c.unsigned_abs()).sum::<u8>() == 1)
        })
        .collect();
    let mut state = vec![UNSEEN; d.voxel_count()];
    let seed_idx = d.index(seed.x, seed.y, seed.z);
    state[seed_idx] = INSIDE;
    let mut queue: VecDeque<u32> = VecDeque::from([seed_idx as u32]);
    let (mut lo, mut hi, mut count) = (seed, seed, 0u64);
    while let Some(idx) = queue.pop_front() {
        let idx = idx as usize;
        let (i, j, k) = (idx % nx, (idx / nx) % ny, idx / plane);
        count += 1;
        if count > max_voxels {
            return None;
        }
        let v = UVec3::new(i as u32, j as u32, k as u32);
        lo = lo.min(v);
        hi = hi.max(v);
        let inner = i > 0 && j > 0 && k > 0 && i + 1 < nx && j + 1 < ny && k + 1 < nz;
        for &(delta, o, face) in &offsets {
            if !inner {
                let (ni, nj, nk) = (i as i64 + i64::from(o[0]), j as i64 + i64::from(o[1]), k as i64 + i64::from(o[2]));
                if !d.contains(ni, nj, nk) {
                    continue;
                }
            }
            let n = (idx as isize + delta) as usize;
            match state[n] {
                UNSEEN => {}
                PENDING if face => {
                    state[n] = INSIDE;
                    queue.push_back(n as u32);
                    continue;
                }
                _ => continue,
            }
            let (ni, nj, nk) = (n % nx, (n / nx) % ny, n / plane);
            state[n] = match accept(ni as u32, nj as u32, nk as u32) {
                Accept::Core if face => {
                    queue.push_back(n as u32);
                    INSIDE
                }
                // reached diagonally: growing continues only across faces
                Accept::Core => PENDING,
                Accept::Edge => {
                    count += 1;
                    if count > max_voxels {
                        return None;
                    }
                    let u = UVec3::new(ni as u32, nj as u32, nk as u32);
                    lo = lo.min(u);
                    hi = hi.max(u);
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
            let row = d.index(bx.min.x, j, k);
            mask.extend(
                state[row..row + (bx.max.x - bx.min.x) as usize].iter().map(|&s| u8::from(s == INSIDE || s == EDGE)),
            );
        }
    }
    Some((bx, mask))
}

/// Block sizes (in voxels per axis) for the opening of radius `r` mm of a
/// box of `size`: `1` while the box has at most [`EXACT_VOXELS`] voxels,
/// otherwise the smallest blocks, grown along the finest axis first, that
/// bring it under that count, never more than `r / 2` mm across.
fn block_factors(size: [usize; 3], w: [f32; 3], r: f32) -> [usize; 3] {
    let mut f = [1usize; 3];
    let count = |f: &[usize; 3]| (0..3).map(|a| size[a].div_ceil(f[a])).product::<usize>();
    while count(&f) > EXACT_VOXELS {
        let grow = (0..3)
            .filter(|&a| (f[a] + 1) as f32 * w[a] <= r / 2.0 && f[a] < size[a])
            .min_by(|&a, &b| (f[a] as f32 * w[a]).total_cmp(&(f[b] as f32 * w[b])));
        match grow {
            Some(a) => f[a] += 1,
            None => break,
        }
    }
    f
}

/// Side of the blocks of the coarse exterior search of [`enclosed`].
const BLOCK: usize = 4;

/// A box cut into blocks, with what each block holds and which blocks are
/// connected to the outside through empty blocks.
struct Blocks {
    size: [usize; 3],
    f: [usize; 3],
    cd: [usize; 3],
    exterior: Vec<bool>,
    has_background: Vec<bool>,
}

impl Blocks {
    fn new(data: &[u8], size: [usize; 3]) -> Self {
        let [nx, ny, nz] = size;
        let f = [BLOCK.min(nx), BLOCK.min(ny), if nz == 1 { 1 } else { BLOCK.min(nz) }];
        let cd = [nx.div_ceil(f[0]), ny.div_ceil(f[1]), nz.div_ceil(f[2])];
        let mut b = Self { size, f, cd, exterior: vec![false; cd[0] * cd[1] * cd[2]], has_background: Vec::new() };
        let mut has_set = vec![false; b.exterior.len()];
        b.has_background = vec![false; b.exterior.len()];
        for k in 0..nz {
            for j in 0..ny {
                let row = (j + ny * k) * nx;
                for (i, &v) in data[row..row + nx].iter().enumerate() {
                    let c = b.block(i, j, k);
                    has_set[c] |= v == 1;
                    b.has_background[c] |= v == 0;
                }
            }
        }
        b.flood_exterior(&has_set);
        b
    }

    fn block(&self, i: usize, j: usize, k: usize) -> usize {
        (i / self.f[0]) + self.cd[0] * ((j / self.f[1]) + self.cd[1] * (k / self.f[2]))
    }

    /// Marks the blocks without a set voxel that connect to the border.
    fn flood_exterior(&mut self, has_set: &[bool]) {
        let cd = self.cd;
        let on_border = |i: usize, j: usize, k: usize| {
            i == 0 || j == 0 || i + 1 == cd[0] || j + 1 == cd[1] || (cd[2] > 1 && (k == 0 || k + 1 == cd[2]))
        };
        let mut stack: Vec<usize> = (0..has_set.len())
            .filter(|&c| !has_set[c] && on_border(c % cd[0], (c / cd[0]) % cd[1], c / (cd[0] * cd[1])))
            .collect();
        for &c in &stack {
            self.exterior[c] = true;
        }
        let plane = cd[0] * cd[1];
        while let Some(c) = stack.pop() {
            let (i, j, k) = (c % cd[0], (c / cd[0]) % cd[1], c / plane);
            let around = [
                (i > 0).then(|| c - 1),
                (i + 1 < cd[0]).then(|| c + 1),
                (j > 0).then(|| c - cd[0]),
                (j + 1 < cd[1]).then(|| c + cd[0]),
                (k > 0).then(|| c - plane),
                (k + 1 < cd[2]).then(|| c + plane),
            ];
            for n in around.into_iter().flatten() {
                if !has_set[n] && !self.exterior[n] {
                    self.exterior[n] = true;
                    stack.push(n);
                }
            }
        }
    }

    /// Blocks that can hold cavities: not exterior, with some background.
    fn candidates(&self) -> Vec<usize> {
        (0..self.exterior.len()).filter(|&c| !self.exterior[c] && self.has_background[c]).collect()
    }

    /// Calls `f` with the flat index of every voxel of block `c`.
    fn for_voxels(&self, c: usize, mut f: impl FnMut(usize, usize, usize, usize)) {
        let [nx, ny, nz] = self.size;
        let (ci, cj, ck) = (c % self.cd[0], (c / self.cd[0]) % self.cd[1], c / (self.cd[0] * self.cd[1]));
        for k in ck * self.f[2]..((ck + 1) * self.f[2]).min(nz) {
            for j in cj * self.f[1]..((cj + 1) * self.f[1]).min(ny) {
                for i in ci * self.f[0]..((ci + 1) * self.f[0]).min(nx) {
                    f(i + nx * (j + ny * k), i, j, k);
                }
            }
        }
    }

    /// Background voxels that touch the border of the box or an exterior
    /// block, in the candidate blocks.
    fn seeds(&self, data: &[u8], candidates: &[usize]) -> Vec<u32> {
        let [nx, ny, nz] = self.size;
        let ext = |i: usize, j: usize, k: usize| self.exterior[self.block(i, j, k)];
        let mut seeds = Vec::new();
        for &c in candidates {
            self.for_voxels(c, |idx, i, j, k| {
                if data[idx] != 0 {
                    return;
                }
                let border = i == 0 || j == 0 || i + 1 == nx || j + 1 == ny || (nz > 1 && (k == 0 || k + 1 == nz));
                let touches = (i > 0 && ext(i - 1, j, k))
                    || (i + 1 < nx && ext(i + 1, j, k))
                    || (j > 0 && ext(i, j - 1, k))
                    || (j + 1 < ny && ext(i, j + 1, k))
                    || (k > 0 && ext(i, j, k - 1))
                    || (k + 1 < nz && ext(i, j, k + 1));
                if border || touches {
                    seeds.push(idx as u32);
                }
            });
        }
        seeds
    }
}

/// Indices of the background voxels (`0`) of a box of `size` (x fastest; a
/// flat slice has `size[2] == 1`) that cannot reach the border of the box
/// through face neighbours: the cavities of the mask.
///
/// Two levels keep this proportional to the surface of the mask instead of
/// the volume of the box: blocks without any set voxel are flooded from the
/// border first (cheap), then only the voxels of blocks that hold
/// background and are not outside are searched, starting where they touch
/// the exterior.
fn enclosed(data: &[u8], size: [usize; 3]) -> Vec<usize> {
    let [nx, ny, nz] = size;
    let blocks = Blocks::new(data, size);
    let candidates = blocks.candidates();
    let mut seen = vec![false; data.len()];
    let mut stack = blocks.seeds(data, &candidates);
    for &idx in &stack {
        seen[idx as usize] = true;
    }
    let plane = nx * ny;
    while let Some(idx) = stack.pop() {
        let idx = idx as usize;
        let (i, j, k) = (idx % nx, (idx / nx) % ny, idx / plane);
        let around = [
            (i > 0).then(|| idx - 1),
            (i + 1 < nx).then(|| idx + 1),
            (j > 0).then(|| idx - nx),
            (j + 1 < ny).then(|| idx + nx),
            (k > 0).then(|| idx - plane),
            (k + 1 < nz).then(|| idx + plane),
        ];
        for n in around.into_iter().flatten() {
            if !seen[n] && data[n] == 0 {
                seen[n] = true;
                stack.push(n as u32);
            }
        }
    }
    let mut out = Vec::new();
    for &c in &candidates {
        blocks.for_voxels(c, |idx, _, _, _| {
            if data[idx] == 0 && !seen[idx] {
                out.push(idx);
            }
        });
    }
    out
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
    /// component holding the seed survives (the nearest one if the seed
    /// itself is not in the core). A structure with no core (a small
    /// lesion) is kept as is.
    ///
    /// Small regions are exact. A region of more than [`EXACT_VOXELS`]
    /// voxels is measured on blocks of at most `r / 2` (majority vote),
    /// which makes the cut position of a bridge uncertain by that much and
    /// large organs on fine grids tens of times cheaper.
    fn open(&mut self, r: f32, spacing: Vec3, seed: UVec3) {
        let (nx, ny, nz) = self.size();
        let w = [spacing.x, spacing.y, spacing.z];
        let f = block_factors([nx, ny, nz], w, r);
        let cd = [nx.div_ceil(f[0]), ny.div_ceil(f[1]), nz.div_ceil(f[2])];
        let block = |i: usize, j: usize, k: usize| (i / f[0]) + cd[0] * ((j / f[1]) + cd[1] * (k / f[2]));
        let mut votes = vec![0u32; cd[0] * cd[1] * cd[2]];
        for k in 0..nz {
            for j in 0..ny {
                let row = self.at(0, j, k);
                for (i, &v) in self.data[row..row + nx].iter().enumerate() {
                    votes[block(i, j, k)] += u32::from(v);
                }
            }
        }
        let size_in = |a: usize, n: usize, c: usize| (n - c * f[a]).min(f[a]);
        let coarse: Vec<u8> = (0..votes.len())
            .map(|b| {
                let (ci, cj, ck) = (b % cd[0], (b / cd[0]) % cd[1], b / (cd[0] * cd[1]));
                let full = size_in(0, nx, ci) * size_in(1, ny, cj) * size_in(2, nz, ck);
                u8::from(votes[b] as usize * 2 > full)
            })
            .collect();
        let coarse =
            Mask::new(VoxelBox::new(UVec3::ZERO, UVec3::new(cd[0] as u32, cd[1] as u32, cd[2] as u32)), coarse);
        let cw = Vec3::new(w[0] * f[0] as f32, w[1] * f[1] as f32, w[2] * f[2] as f32);
        let s = seed - self.bx.min;
        let seed_c = UVec3::new(s.x / f[0] as u32, s.y / f[1] as u32, s.z / f[2] as u32);
        let Some(near) = coarse.near_core(r, cw, seed_c) else {
            return;
        };
        let r2 = f64::from(r) * f64::from(r);
        for k in 0..nz {
            for j in 0..ny {
                let row = self.at(0, j, k);
                for (i, v) in self.data[row..row + nx].iter_mut().enumerate() {
                    *v &= u8::from(near[block(i, j, k)] <= r2);
                }
            }
        }
    }

    /// Squared distances (mm²) of every voxel to the opening's core: the
    /// part of the mask at least `r` mm from its outside, restricted to the
    /// component nearest to `seed`. `None` when the mask has no core (the
    /// region is too thin for an opening of `r`).
    fn near_core(&self, r: f32, spacing: Vec3, seed: UVec3) -> Option<Vec<f64>> {
        let (nx, ny, nz) = self.size();
        let r2 = f64::from(r) * f64::from(r);
        // the margin of one voxel makes the box border count as outside
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
        let mut core = Mask::new(self.bx, vec![0u8; self.data.len()]);
        let mut any = false;
        for k in 0..nz {
            for j in 0..ny {
                for i in 0..nx {
                    let idx = self.at(i, j, k);
                    let inside = self.data[idx] == 1 && d2[(i + 1) + px * ((j + 1) + py * (k + 1))] >= r2;
                    core.data[idx] = u8::from(inside);
                    any |= inside;
                }
            }
        }
        if !any {
            return None;
        }
        let nearest = core.component(self.at(seed.x as usize, seed.y as usize, seed.z as usize), spacing)?;
        Some(squared_edt(&nearest.iter().map(|&c| c == 1).collect::<Vec<_>>(), self.bx.size(), spacing))
    }

    /// The connected component of the mask closest to `seed_idx` (the one
    /// containing it when it is set; the larger one on a tie), measured in
    /// millimetres; `None` if the mask is empty.
    fn component(&self, seed_idx: usize, spacing: Vec3) -> Option<Vec<u8>> {
        let (nx, ny, _) = self.size();
        let at = |idx: usize| [idx % nx, (idx / nx) % ny, idx / (nx * ny)];
        let (s, w) = (at(seed_idx), [spacing.x, spacing.y, spacing.z]);
        let dist2 = |idx: usize| {
            let p = at(idx);
            (0..3).map(|a| (p[a].abs_diff(s[a]) as f32 * w[a]).powi(2)).sum::<f32>()
        };
        let mut seen = vec![false; self.data.len()];
        let mut best: Option<(f32, Vec<usize>)> = None;
        for start in (0..self.data.len()).filter(|&i| self.data[i] == 1) {
            if seen[start] {
                continue;
            }
            let mut comp = vec![start];
            seen[start] = true;
            let (mut head, mut nearest) = (0, f32::INFINITY);
            while head < comp.len() {
                let v = comp[head];
                head += 1;
                nearest = nearest.min(dist2(v));
                for n in self.around(v) {
                    if !seen[n] && self.data[n] == 1 {
                        seen[n] = true;
                        comp.push(n);
                    }
                }
            }
            let better = best.as_ref().is_none_or(|(d, c)| nearest < *d || (nearest == *d && comp.len() > c.len()));
            if better {
                best = Some((nearest, comp));
            }
        }
        let (_, best) = best?;
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
        let (nx, ny, nz) = self.size();
        let min = self.bx.min;
        let free_at = |idx: usize| {
            free(min.x + (idx % nx) as u32, min.y + ((idx / nx) % ny) as u32, min.z + (idx / (nx * ny)) as u32)
        };
        // cavities of every slice along each axis (a cavity closed in 3D is closed in
        // every slice through it, so this covers the 3D case too)
        let (strides, dims) = ([1, nx, nx * ny], [nx, ny, nz]);
        for axis in 0..3 {
            let (a, b, c) = ((axis + 1) % 3, (axis + 2) % 3, axis);
            let mut plane = vec![0u8; dims[a] * dims[b]];
            for r in 0..dims[c] {
                for q in 0..dims[b] {
                    for p in 0..dims[a] {
                        plane[p + dims[a] * q] = self.data[p * strides[a] + q * strides[b] + r * strides[c]];
                    }
                }
                for flat in enclosed(&plane, [dims[a], dims[b], 1]) {
                    let (p, q) = (flat % dims[a], flat / dims[a]);
                    let idx = p * strides[a] + q * strides[b] + r * strides[c];
                    if free_at(idx) {
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

    // covers 16.6-k
    #[test]
    fn the_core_nearest_to_the_click_survives_the_opening() {
        // a small organ A, a long 1-voxel bridge and a larger organ B; the click is on the
        // surface of A, where there is no core
        let d = Dims3::new(60, 20, 20);
        let mut data = vec![0.0f32; d.voxel_count()];
        for k in 0..20u32 {
            for j in 0..20u32 {
                for i in 0..60u32 {
                    let a = (2..12).contains(&i) && (5..15).contains(&j) && (5..15).contains(&k);
                    let bridge = (12..40).contains(&i) && j == 10 && k == 10;
                    let b = (40..58).contains(&i) && (1..19).contains(&j) && (1..19).contains(&k);
                    data[d.index(i, j, k)] = if a || bridge || b { 60.0 } else { 0.0 };
                }
            }
        }
        let v = Volume::from_physical(d, Vec3::ONE, &data).unwrap();
        let set = SegmentationSet::new(d);
        let p = RegionParams { smoothing_mm: 0.0, opening_mm: 1.5, fill_holes: false, ..params() };
        let (bx, _) = grow_region_robust(&v, &set, UVec3::new(2, 10, 10), &p).unwrap();
        assert!(bx.max.x <= 14 && bx.min.x == 2, "organ A, not the larger B: {bx:?}");
    }

    // covers 16.6-b
    #[test]
    fn the_two_level_cavity_search_equals_a_plain_flood() {
        fn naive(data: &[u8], size: [usize; 3]) -> Vec<usize> {
            let [nx, ny, nz] = size;
            let mut seen = vec![false; data.len()];
            let mut stack = Vec::new();
            for idx in 0..data.len() {
                let (i, j, k) = (idx % nx, (idx / nx) % ny, idx / (nx * ny));
                let border = i == 0 || j == 0 || i + 1 == nx || j + 1 == ny || (nz > 1 && (k == 0 || k + 1 == nz));
                if data[idx] == 0 && border {
                    seen[idx] = true;
                    stack.push(idx);
                }
            }
            while let Some(idx) = stack.pop() {
                let (i, j, k) = (idx % nx, (idx / nx) % ny, idx / (nx * ny));
                let mut around = Vec::new();
                if i > 0 {
                    around.push(idx - 1);
                }
                if i + 1 < nx {
                    around.push(idx + 1);
                }
                if j > 0 {
                    around.push(idx - nx);
                }
                if j + 1 < ny {
                    around.push(idx + nx);
                }
                if k > 0 {
                    around.push(idx - nx * ny);
                }
                if k + 1 < nz {
                    around.push(idx + nx * ny);
                }
                for n in around {
                    if data[n] == 0 && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
            (0..data.len()).filter(|&i| data[i] == 0 && !seen[i]).collect()
        }
        // a hollow cube: its large cavity is made of empty blocks that are not exterior
        let hollow: Vec<u8> = (0..30 * 30 * 30)
            .map(|n| {
                let (i, j, k) = (n % 30, (n / 30) % 30, n / 900);
                let cube = (3..27).contains(&i) && (3..27).contains(&j) && (3..27).contains(&k);
                let cavity = (9..21).contains(&i) && (9..21).contains(&j) && (9..21).contains(&k);
                u8::from(cube && !cavity)
            })
            .collect();
        let mut fast = enclosed(&hollow, [30, 30, 30]);
        fast.sort_unstable();
        assert_eq!(fast, naive(&hollow, [30, 30, 30]));
        assert_eq!(fast.len(), 12 * 12 * 12);
        let mut h = 7u32;
        for size in [[13, 11, 9], [17, 6, 1], [5, 5, 5], [30, 30, 3]] {
            for density in [50u32, 70, 85] {
                let data: Vec<u8> = (0..size[0] * size[1] * size[2])
                    .map(|_| {
                        h = h.wrapping_mul(1664525).wrapping_add(1013904223);
                        u8::from((h >> 16) % 100 < density)
                    })
                    .collect();
                let mut fast = enclosed(&data, size);
                fast.sort_unstable();
                assert_eq!(fast, naive(&data, size), "{size:?} density {density}");
            }
        }
    }

    // covers 16.6-c
    #[test]
    fn small_regions_are_opened_exactly_and_large_ones_on_blocks() {
        assert_eq!(block_factors([30, 30, 30], [1.0; 3], 5.0), [1, 1, 1]);
        assert_eq!(block_factors([200, 200, 70], [1.4, 1.4, 5.0], 5.0), [1, 1, 1], "2.8 M voxels");
        let f = block_factors([512, 512, 519], [0.62, 0.62, 0.8], 5.0);
        assert_eq!(f, [4, 4, 3], "blocks of 2.5 mm at most");
        let count: usize = [512usize, 512, 519].iter().zip(f).map(|(n, f)| n.div_ceil(f)).product();
        assert!(count <= EXACT_VOXELS);
        // blocks never exceed r / 2, even when that leaves the box too big
        assert_eq!(block_factors([2000, 2000, 2000], [1.0; 3], 2.0), [1, 1, 1]);
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
