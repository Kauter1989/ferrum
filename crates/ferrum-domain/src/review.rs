//! Reviewing the results of a study: the port to where results are kept
//! (a workspace written by the agent skill) and the decisions people make.

use std::path::PathBuf;

use crate::annotation::{AnnotationId, AnnotationSet};
use crate::provenance::ReviewStatus;
use crate::report::AnnotationReport;
use crate::segmentation::SegmentationSet;
use crate::volume::Volume;

/// An item that can be reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReviewItem {
    /// An annotation by id.
    Annotation(AnnotationId),
    /// A segment by label.
    Segment(u8),
}

/// A review decision, as written to the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDecision {
    /// The item.
    pub item: ReviewItem,
    /// Its name at the time of the decision.
    pub name: String,
    /// The decision.
    pub status: ReviewStatus,
    /// Who decided, if given.
    pub by: Option<String>,
}

/// Results of one study kept outside the viewer (port; implemented by the
/// data layer for `ferrum-workspace` directories).
pub trait ResultStore: Send + Sync {
    /// Short description for the status line (e.g. the workspace path).
    fn describe(&self) -> String;

    /// Files or folders to open to load the study's series.
    fn source_paths(&self) -> Vec<PathBuf>;

    /// Id of the series among those the source paths contain.
    fn series_id(&self) -> String;

    /// Checks that the source data is unchanged.
    fn verify(&self) -> Result<(), String>;

    /// Annotations and segments of the study on the grid of `volume`
    /// (`None` where there are none).
    fn load(&self, volume: &Volume) -> Result<(Option<AnnotationSet>, Option<SegmentationSet>), String>;

    /// Saves annotations and segments.
    fn save(&self, annotations: &AnnotationReport, segments: &SegmentationSet, volume: &Volume) -> Result<(), String>;

    /// Appends a review decision to the audit log.
    fn log(&self, decision: &ReviewDecision) -> Result<(), String>;
}
