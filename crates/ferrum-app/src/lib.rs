//! # ferrum-app
//!
//! Application layer of the viewer: owns the state, exposes use cases and
//! runs background jobs. It depends only on the domain, processing and the
//! pure part of the renderer (frame model and CPU picker) — no UI toolkit
//! and no GPU API — so every use case is unit-testable headlessly.
//! The presentation layer talks to the GPU through the [`GpuSink`] port.

pub mod dataset;
pub mod jobs;
pub mod prompts;
pub mod tools;
pub mod viewer;

pub use dataset::Dataset;
pub use jobs::FilterKind;
pub use tools::{InputKind, ProbeReading, ToolKind, ToolOutcome};
pub use viewer::{
    AiState, AiStatus, GpuSink, GpuSyncState, ReviewEntry, SegmentSummary, SegmentationState, SliceState, Status,
    ViewMode, Viewer, VolumeViewState,
};
