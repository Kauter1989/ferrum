//! The loaded dataset and derived acceleration data.

use std::sync::Arc;

use ferrum_domain::{SeriesMetadata, Volume};
use ferrum_processing::{BrickGrid, Histogram};

/// Number of histogram bins shown by the transfer function editor.
pub const HISTOGRAM_BINS: usize = 256;
/// Brick edge used for empty-space skipping.
pub const BRICK_SIZE: u32 = BrickGrid::DEFAULT_BRICK_SIZE;

/// A volume together with data derived from it once at load time.
#[derive(Debug, Clone)]
pub struct Dataset {
    /// The volume (shared with background jobs without copying).
    pub volume: Arc<Volume>,
    /// Descriptive metadata.
    pub metadata: SeriesMetadata,
    /// Intensity histogram.
    pub histogram: Histogram,
    /// Min/max bricks for empty-space skipping.
    pub bricks: Arc<BrickGrid>,
    /// Incremented whenever the voxel data is replaced.
    pub revision: u64,
}

impl Dataset {
    /// Builds a dataset, computing histogram and bricks in parallel.
    pub fn new(volume: Volume, metadata: SeriesMetadata, revision: u64) -> Self {
        let histogram = Histogram::compute(&volume, HISTOGRAM_BINS);
        let bricks = Arc::new(BrickGrid::compute(&volume, BRICK_SIZE));
        Self { volume: Arc::new(volume), metadata, histogram, bricks, revision }
    }
}
