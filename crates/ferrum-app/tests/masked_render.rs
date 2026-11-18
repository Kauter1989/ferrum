//! The masked-render use case (Clean Pixel Data, Stage 31, criterion
//! 31.3-c): the view is drawn over a masked copy of the data, and the
//! original dataset is restored afterwards.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;

use ferrum_app::Viewer;
use ferrum_domain::{
    Dims3, LoadedSeries, ProgressSink, RepositoryError, SeriesDescriptor, SeriesMetadata, Volume, VolumeRepository,
    VoxelBox,
};
use glam::{UVec3, Vec3};

/// A repository with one gradient phantom.
struct One;

impl VolumeRepository for One {
    fn name(&self) -> &str {
        "fake"
    }
    fn scan(&self, _: &[PathBuf], _: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        Ok(vec![SeriesDescriptor {
            id: "s".into(),
            format: "fake".into(),
            description: "phantom".into(),
            modality: "CT".into(),
            dims: Dims3::new(8, 8, 4),
            sources: vec![],
            volume: None,
        }])
    }
    fn volumes(&self, s: &SeriesDescriptor, _: &dyn ProgressSink) -> Result<Vec<SeriesDescriptor>, RepositoryError> {
        Ok(vec![s.clone()])
    }
    fn load(&self, _: &SeriesDescriptor, _: &dyn ProgressSink) -> Result<LoadedSeries, RepositoryError> {
        let dims = Dims3::new(8, 8, 4);
        let values: Vec<f32> = (0..dims.voxel_count()).map(|i| (i % 100) as f32).collect();
        Ok(LoadedSeries {
            volume: Volume::from_physical(dims, Vec3::ONE, &values).unwrap(),
            metadata: SeriesMetadata::default(),
        })
    }
}

#[test]
fn render_masked_draws_over_a_masked_copy_and_restores_the_data() {
    // covers 31.3-c
    let mut v = Viewer::new(Arc::new(One));
    v.open_paths(vec![PathBuf::from("/data/s")]);
    v.wait_idle();
    let original = (*v.dataset().unwrap().volume).clone();
    let revision = v.dataset().unwrap().revision;
    let boxes = [VoxelBox::new(UVec3::new(1, 2, 0), UVec3::new(4, 5, 4))];
    let seen = v
        .render_masked(&boxes, 0, |viewer| {
            let d = viewer.dataset().unwrap();
            assert_ne!(d.revision, revision, "the masked copy is a fresh revision, so the GPU sink uploads it");
            (*d.volume).clone()
        })
        .unwrap();
    // inside the closure the data was the masked copy
    assert_eq!(seen, original.masked_copy(&boxes, 0));
    assert_ne!(seen, original);
    // afterwards the original dataset is back, unchanged
    let d = v.dataset().unwrap();
    assert_eq!(d.revision, revision);
    assert_eq!(*d.volume, original);
    // without a dataset there is nothing to draw
    let mut empty = Viewer::new(Arc::new(One));
    assert!(empty.render_masked(&boxes, 0, |_| ()).is_none());
}
