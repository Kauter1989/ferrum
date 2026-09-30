//! # mri-domain
//!
//! Pure domain model of the MRI viewer. This crate owns the *vocabulary* of the
//! application and the invariants that go with it. It performs no I/O, knows
//! nothing about GPUs, DICOM parsing or user interfaces, and therefore can be
//! tested exhaustively and reused by any front end.
//!
//! ## Coordinate conventions
//!
//! * **Voxel index** `(i, j, k)`: `i` runs along image columns (x), `j` along
//!   image rows (y), `k` along slices (z). Linear index is
//!   `i + j * nx + k * nx * ny`.
//! * **Texture space** `u ∈ [0, 1]³`: voxel centres sit at `(i + 0.5) / nx`.
//!   This matches GPU texture sampling with `ClampToEdge` + linear filtering.
//! * **Model space** `p`: the volume box centred at the origin, scaled so that
//!   its longest *physical* side has length 1 (`p = (u - 0.5) * extent`).
//!   See [`volume::Volume::model_extent`].

pub mod annotation;
pub mod camera;
pub mod clip;
pub mod color;
pub mod geometry;
pub mod mask;
pub mod render_settings;
pub mod repository;
pub mod slice;
pub mod transfer;
pub mod volume;
pub mod window;

pub use annotation::{Annotation, AnnotationId, AnnotationSet, SliceKey};
pub use camera::{OrbitCamera, ViewPreset};
pub use clip::{ClipBox, ClipPlane, ClipSettings};
pub use color::{Rgb, Rgba8};
pub use geometry::{Aabb, Dims3, Ray};
pub use mask::{EraseStroke, EraserBrush, MaskHistory, VoxelMask};
pub use render_settings::{RenderMode, RenderSettings, TissueThresholds};
pub use repository::{
    CancelFlag, LoadedSeries, NoProgress, ProgressSink, RepositoryError, SeriesDescriptor, SeriesMetadata,
    VolumeRepository,
};
pub use slice::{SliceAxis, SliceImage, SliceView};
pub use transfer::{ControlPoint, CtPreset, TransferFunction, TransferFunctionError};
pub use volume::{IntensityRange, Volume, VolumeError};
pub use window::{WindowLevel, WindowPreset};
