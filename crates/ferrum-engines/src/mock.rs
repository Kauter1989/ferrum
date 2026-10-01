//! In-process engine without a model, for tests and demos.
//!
//! [`MockEngine`] implements the whole interactive part of the protocol
//! with deterministic region growing, so the AI tools can be exercised
//! without a GPU or a Python stack:
//!
//! * point — 6-connected region growing from the voxel, within an
//!   intensity tolerance and a radius; negative points remove the connected
//!   part of the mask they hit;
//! * box — region growing from the box centre, limited to the box;
//!   negative boxes clear the mask inside;
//! * scribble / lasso — the given mask is added (or removed).
//!
//! Undo is supported.

use std::collections::VecDeque;

use ferrum_domain::{
    Dims3, EngineCapabilities, EngineError, EngineInfo, InteractiveSession, Prompt, PromptKind, PromptResult,
    SegmentationEngine, Volume, VoxelBox, ENGINE_PROTOCOL,
};
use glam::UVec3;

/// Tuning of the mock region growing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MockParams {
    /// Intensity tolerance as a fraction of the volume's intensity range.
    pub tolerance: f32,
    /// Maximum growth distance from the seed in voxels (per axis).
    pub max_radius: u32,
}

impl Default for MockParams {
    fn default() -> Self {
        Self { tolerance: 0.08, max_radius: 64 }
    }
}

/// A deterministic engine without a model.
#[derive(Debug, Clone, Default)]
pub struct MockEngine {
    /// Region growing parameters.
    pub params: MockParams,
}

impl MockEngine {
    /// Engine description.
    pub fn describe() -> EngineInfo {
        EngineInfo {
            protocol: ENGINE_PROTOCOL.into(),
            name: "FERRUM mock engine".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            vendor: "FERRUM".into(),
            device: "cpu".into(),
            capabilities: EngineCapabilities {
                interactive: true,
                automatic: false,
                prompts: PromptKind::ALL.to_vec(),
                planar_boxes_only: false,
                undo: true,
            },
            modalities: Vec::new(),
            labels: Vec::new(),
            research_only: false,
            license: "MIT — region growing, no model (tests and demos)".into(),
            max_voxels: 0,
            session_ttl_s: 0,
        }
    }
}

impl SegmentationEngine for MockEngine {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        Ok(Self::describe())
    }

    fn open_session(&self, volume: &Volume, _modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        Ok(Box::new(MockSession::new(volume.clone(), self.params)))
    }
}

/// Voxels changed by one prompt (indices and previous values).
#[derive(Debug, Clone, Default)]
struct Change {
    indices: Vec<u32>,
    previous: Vec<u8>,
}

/// Session of the [`MockEngine`].
#[derive(Debug)]
pub struct MockSession {
    volume: Volume,
    params: MockParams,
    mask: Vec<u8>,
    count: usize,
    history: Vec<Change>,
    revision: u64,
}

impl MockSession {
    /// Creates a session on `volume`.
    pub fn new(volume: Volume, params: MockParams) -> Self {
        let n = volume.dims().voxel_count();
        Self { volume, params, mask: vec![0; n], count: 0, history: Vec::new(), revision: 0 }
    }

    fn dims(&self) -> Dims3 {
        self.volume.dims()
    }

    /// Voxels connected to `seed` (6-neighbourhood) inside `limit` that
    /// satisfy `accept`.
    fn grow(&self, seed: UVec3, limit: VoxelBox, accept: impl Fn(usize) -> bool) -> Vec<usize> {
        let d = self.dims();
        let mut seen = vec![false; limit.voxel_count()];
        let local = |p: UVec3| {
            let q = p - limit.min;
            let s = limit.size();
            (q.x + s.x * (q.y + s.y * q.z)) as usize
        };
        let mut out = Vec::new();
        let mut queue = VecDeque::from([seed]);
        seen[local(seed)] = true;
        while let Some(p) = queue.pop_front() {
            let idx = d.index(p.x, p.y, p.z);
            if !accept(idx) {
                continue;
            }
            out.push(idx);
            for (axis, step) in [(0usize, -1i64), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)] {
                let c = i64::from(p[axis]) + step;
                if c < i64::from(limit.min[axis]) || c >= i64::from(limit.max[axis]) {
                    continue;
                }
                let mut n = p;
                n[axis] = c as u32;
                let l = local(n);
                if !seen[l] {
                    seen[l] = true;
                    queue.push_back(n);
                }
            }
        }
        out
    }

    fn similar_to(&self, seed: UVec3) -> impl Fn(usize) -> bool + '_ {
        let data = self.volume.data();
        let s = f32::from(data[self.dims().index(seed.x, seed.y, seed.z)]);
        let tol = self.params.tolerance * f32::from(u16::MAX);
        move |idx| (f32::from(data[idx]) - s).abs() <= tol
    }

    fn around(&self, seed: UVec3) -> VoxelBox {
        let r = UVec3::splat(self.params.max_radius);
        VoxelBox::new(seed.saturating_sub(r), (seed + r + UVec3::ONE).min(self.dims().as_uvec3()))
    }

    fn set(&mut self, indices: impl IntoIterator<Item = usize>, value: u8, change: &mut Change) {
        for idx in indices {
            let old = self.mask[idx];
            if old != value {
                change.indices.push(idx as u32);
                change.previous.push(old);
                self.mask[idx] = value;
                if value != 0 {
                    self.count += 1;
                } else {
                    self.count -= 1;
                }
            }
        }
    }

    fn box_indices(&self, bx: VoxelBox) -> impl Iterator<Item = (usize, usize)> + '_ {
        let d = self.dims();
        (bx.min.z..bx.max.z)
            .flat_map(move |k| (bx.min.y..bx.max.y).flat_map(move |j| (bx.min.x..bx.max.x).map(move |i| (i, j, k))))
            .enumerate()
            .map(move |(n, (i, j, k))| (n, d.index(i, j, k)))
    }

    fn apply(&mut self, prompt: &Prompt) -> Change {
        let mut change = Change::default();
        match prompt {
            Prompt::Point { positive: true, voxel } => {
                let region = self.grow(*voxel, self.around(*voxel), self.similar_to(*voxel));
                self.set(region, 1, &mut change);
            }
            Prompt::Point { positive: false, voxel } => {
                let mask = &self.mask;
                let region = self.grow(*voxel, VoxelBox::full(self.dims()), |idx| mask[idx] != 0);
                self.set(region, 0, &mut change);
            }
            Prompt::Box { positive: true, bx } => {
                let centre = (bx.min + bx.max.saturating_sub(UVec3::ONE)) / 2;
                let region = self.grow(centre, *bx, self.similar_to(centre));
                self.set(region, 1, &mut change);
            }
            Prompt::Box { positive: false, bx } => {
                let all: Vec<usize> = self.box_indices(*bx).map(|(_, idx)| idx).collect();
                self.set(all, 0, &mut change);
            }
            Prompt::Scribble { positive, bx, mask } | Prompt::Lasso { positive, bx, mask } => {
                let marked: Vec<usize> =
                    self.box_indices(*bx).filter(|(n, _)| mask[*n] != 0).map(|(_, idx)| idx).collect();
                self.set(marked, u8::from(*positive), &mut change);
            }
        }
        change
    }

    fn result(&mut self, change: &Change) -> PromptResult {
        let d = self.dims();
        let changed = change.indices.iter().fold(None::<VoxelBox>, |acc, &idx| {
            let idx = idx as usize;
            let p = UVec3::new(
                (idx % d.x as usize) as u32,
                ((idx / d.x as usize) % d.y as usize) as u32,
                (idx / d.slice_len()) as u32,
            );
            let one = VoxelBox::new(p, p + UVec3::ONE);
            Some(acc.map_or(one, |b| b.union(one)))
        });
        if changed.is_some() {
            self.revision += 1;
        }
        PromptResult { revision: self.revision, changed, empty: self.count == 0 }
    }
}

impl InteractiveSession for MockSession {
    fn prompt(&mut self, prompt: &Prompt) -> Result<PromptResult, EngineError> {
        prompt.validate(self.dims())?;
        let change = self.apply(prompt);
        let result = self.result(&change);
        self.history.push(change);
        Ok(result)
    }

    fn mask(&mut self, bx: VoxelBox) -> Result<Vec<u8>, EngineError> {
        if !bx.fits(self.dims()) {
            return Err(EngineError::BadRequest(format!("box {}..{} is empty or outside the volume", bx.min, bx.max)));
        }
        Ok(self.box_indices(bx).map(|(_, idx)| self.mask[idx]).collect())
    }

    fn undo(&mut self) -> Result<PromptResult, EngineError> {
        let Some(change) = self.history.pop() else {
            return Ok(PromptResult { revision: self.revision, changed: None, empty: self.count == 0 });
        };
        for (&idx, &prev) in change.indices.iter().zip(&change.previous).rev() {
            let idx = idx as usize;
            match (self.mask[idx] != 0, prev != 0) {
                (true, false) => self.count -= 1,
                (false, true) => self.count += 1,
                _ => {}
            }
            self.mask[idx] = prev;
        }
        Ok(self.result(&change))
    }

    fn reset(&mut self) -> Result<(), EngineError> {
        self.mask.fill(0);
        self.count = 0;
        self.history.clear();
        self.revision += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// Two bright cubes (value 1000) on a dark background, not touching.
    fn cubes() -> Volume {
        let dims = Dims3::new(20, 12, 8);
        let mut vals = vec![0.0f32; dims.voxel_count()];
        for k in 2..6 {
            for j in 2..10 {
                for i in 2..8 {
                    vals[dims.index(i, j, k)] = 1000.0;
                    vals[dims.index(i + 10, j, k)] = 1000.0;
                }
            }
        }
        Volume::from_physical(dims, Vec3::ONE, &vals).unwrap()
    }

    fn session() -> Box<dyn InteractiveSession> {
        MockEngine::default().open_session(&cubes(), "CT").unwrap()
    }

    #[test]
    fn describes_itself() {
        let info = MockEngine::default().info().unwrap();
        assert_eq!(info.protocol, ENGINE_PROTOCOL);
        assert!(PromptKind::ALL.iter().all(|k| info.supports(*k)));
        assert!(info.capabilities.undo && !info.research_only);
    }

    #[test]
    fn point_grows_one_cube_and_negative_point_removes_it() {
        let mut s = session();
        let r = s.prompt(&Prompt::Point { positive: true, voxel: UVec3::new(4, 5, 3) }).unwrap();
        assert_eq!(r.revision, 1);
        assert_eq!(r.changed, Some(VoxelBox::new(UVec3::new(2, 2, 2), UVec3::new(8, 10, 6))));
        assert!(!r.empty);
        let m = s.mask(VoxelBox::full(Dims3::new(20, 12, 8))).unwrap();
        assert_eq!(m.iter().filter(|&&v| v == 1).count(), 6 * 8 * 4);
        let r = s.prompt(&Prompt::Point { positive: true, voxel: UVec3::new(14, 5, 3) }).unwrap();
        assert_eq!(r.changed.unwrap().min.x, 12);
        let r = s.prompt(&Prompt::Point { positive: false, voxel: UVec3::new(4, 5, 3) }).unwrap();
        assert_eq!(r.changed, Some(VoxelBox::new(UVec3::new(2, 2, 2), UVec3::new(8, 10, 6))));
        assert!(!r.empty);
        // a negative point on background changes nothing
        let r = s.prompt(&Prompt::Point { positive: false, voxel: UVec3::new(0, 0, 0) }).unwrap();
        assert_eq!((r.changed, r.revision), (None, 3));
    }

    #[test]
    fn boxes_scribbles_undo_and_reset() {
        let mut s = session();
        let left = VoxelBox::new(UVec3::new(0, 0, 3), UVec3::new(10, 12, 4));
        let r = s.prompt(&Prompt::Box { positive: true, bx: left }).unwrap();
        // the centre (4, 5, 3) is inside the left cube: one plane of it
        assert_eq!(r.changed, Some(VoxelBox::new(UVec3::new(2, 2, 3), UVec3::new(8, 10, 4))));
        let corner = VoxelBox::new(UVec3::new(0, 0, 0), UVec3::new(2, 1, 1));
        let r = s.prompt(&Prompt::Scribble { positive: true, bx: corner, mask: vec![1, 0] }).unwrap();
        assert_eq!(r.changed, Some(VoxelBox::new(UVec3::ZERO, UVec3::ONE)));
        let r = s.prompt(&Prompt::Lasso { positive: false, bx: corner, mask: vec![1, 1] }).unwrap();
        assert_eq!(r.changed, Some(VoxelBox::new(UVec3::ZERO, UVec3::ONE)));
        let r = s.prompt(&Prompt::Box { positive: false, bx: left }).unwrap();
        assert!(r.empty);
        let r = s.undo().unwrap();
        assert!(!r.empty);
        assert_eq!(r.changed, Some(VoxelBox::new(UVec3::new(2, 2, 3), UVec3::new(8, 10, 4))));
        s.undo().unwrap();
        s.undo().unwrap();
        let r = s.undo().unwrap();
        assert!(r.empty);
        let r = s.undo().unwrap();
        assert_eq!(r.changed, None, "nothing left to undo");
        s.prompt(&Prompt::Point { positive: true, voxel: UVec3::new(4, 5, 3) }).unwrap();
        s.reset().unwrap();
        assert!(s.mask(VoxelBox::full(Dims3::new(20, 12, 8))).unwrap().iter().all(|&v| v == 0));
    }

    #[test]
    fn growth_is_limited_and_input_is_validated() {
        let engine = MockEngine { params: MockParams { tolerance: 0.08, max_radius: 1 } };
        let mut s = engine.open_session(&cubes(), "").unwrap();
        let r = s.prompt(&Prompt::Point { positive: true, voxel: UVec3::new(4, 5, 3) }).unwrap();
        assert_eq!(r.changed, Some(VoxelBox::new(UVec3::new(3, 4, 2), UVec3::new(6, 7, 5))));
        assert!(matches!(
            s.prompt(&Prompt::Point { positive: true, voxel: UVec3::new(20, 0, 0) }),
            Err(EngineError::BadRequest(_))
        ));
        assert!(s.mask(VoxelBox::new(UVec3::ZERO, UVec3::new(21, 1, 1))).is_err());
    }
}
