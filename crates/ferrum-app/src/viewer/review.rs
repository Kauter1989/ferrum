//! Review use cases of the [`Viewer`]: opening a workspace (results of the
//! agent skill), the queue of proposals, and decisions written back to the
//! workspace and its audit log.

use std::sync::Arc;

use ferrum_domain::{
    AnnotationReport, AnnotationSet, Provenance, ResultStore, ReviewDecision, ReviewItem, ReviewStatus,
    SeriesDescriptor,
};

use super::Viewer;

/// An open workspace and what was last saved to it.
pub struct WorkspaceSession {
    pub(super) store: Arc<dyn ResultStore>,
    saved_annotations: AnnotationSet,
    saved_segments: Option<u64>,
}

/// A row of the review queue.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewEntry {
    /// The item.
    pub item: ReviewItem,
    /// Its name.
    pub name: String,
    /// `Distance`, `Area`, …, or `Segment`.
    pub kind: String,
    /// Who proposed it and when.
    pub provenance: Provenance,
}

impl Viewer {
    /// Opens the study of a workspace: checks the sources, loads the series
    /// in the background and then attaches the workspace's annotations and
    /// segments.
    pub fn open_workspace(&mut self, store: Arc<dyn ResultStore>) -> Result<(), String> {
        store.verify().map_err(|e| format!("Workspace sources changed: {e}"))?;
        let paths = store.source_paths();
        self.status.message = format!("Opening workspace {}…", store.describe());
        self.pending_workspace = Some(store);
        self.open_paths(paths);
        Ok(())
    }

    /// Picks the workspace's series among scanned ones (`None` when no
    /// workspace is being opened). A missing series cancels the opening.
    pub(super) fn take_workspace_series(&mut self, series: &mut Vec<SeriesDescriptor>) -> Option<SeriesDescriptor> {
        let id = self.pending_workspace.as_ref()?.series_id();
        match series.iter().position(|s| s.id == id) {
            Some(i) => Some(series.remove(i)),
            None => {
                self.pending_workspace = None;
                self.status.errors.push("The workspace's series is no longer in its source".into());
                None
            }
        }
    }

    /// Called after a dataset was installed: attaches a workspace being
    /// opened, or closes the open one (another study was loaded).
    pub(super) fn attach_workspace(&mut self) {
        self.workspace = None;
        let (Some(store), Some(d)) = (self.pending_workspace.take(), self.dataset.as_ref()) else {
            return;
        };
        match store.load(&d.volume) {
            Ok((annotations, segments)) => {
                self.annotations = annotations.unwrap_or_default();
                let has_segments = segments.is_some();
                self.segments.set = segments;
                self.segments.generation = self.bump_revision();
                self.segments.show |= has_segments;
                let pending = self.review_queue().len();
                self.status.message = format!(
                    "Workspace {}: {} annotation(s), {} segment(s), {pending} waiting for review",
                    store.describe(),
                    self.annotations.len(),
                    self.segments.set.as_ref().map_or(0, |s| s.segments().len()),
                );
                self.workspace = Some(WorkspaceSession {
                    store,
                    saved_annotations: self.annotations.clone(),
                    saved_segments: self.segments.set.as_ref().map(|s| s.revision()),
                });
            }
            Err(e) => self.status.errors.push(format!("Workspace: {e}")),
        }
    }

    /// The open workspace, if any (its description).
    pub fn workspace(&self) -> Option<String> {
        self.workspace.as_ref().map(|w| w.store.describe())
    }

    /// `true` if annotations or segments changed since the workspace was
    /// opened or saved.
    pub fn workspace_dirty(&self) -> bool {
        self.workspace.as_ref().is_some_and(|w| {
            w.saved_annotations != self.annotations
                || w.saved_segments != self.segments.set.as_ref().map(|s| s.revision())
        })
    }

    /// Saves annotations and segments into the open workspace.
    pub fn save_workspace(&mut self) -> Result<(), String> {
        let (Some(w), Some(d)) = (self.workspace.as_ref(), self.dataset.as_ref()) else {
            return Err("no workspace is open".into());
        };
        let report =
            AnnotationReport::build(d.metadata.source.clone(), d.metadata.study.clone(), &d.volume, &self.annotations);
        let empty;
        let segments = match self.segments.set.as_ref() {
            Some(s) => s,
            None => {
                empty = ferrum_domain::SegmentationSet::new(d.volume.dims());
                &empty
            }
        };
        w.store.save(&report, segments, &d.volume)?;
        let saved_segments = self.segments.set.as_ref().map(|s| s.revision());
        if let Some(w) = self.workspace.as_mut() {
            w.saved_annotations = self.annotations.clone();
            w.saved_segments = saved_segments;
        }
        Ok(())
    }

    /// Annotations and segments waiting for review.
    pub fn review_queue(&self) -> Vec<ReviewEntry> {
        let mut out: Vec<ReviewEntry> = self
            .annotations
            .iter()
            .filter_map(|(id, _, a, name)| {
                let p = self.annotations.provenance(id)?;
                p.is_pending().then(|| ReviewEntry {
                    item: ReviewItem::Annotation(id),
                    name: name.to_owned(),
                    kind: a.kind().to_owned(),
                    provenance: p.clone(),
                })
            })
            .collect();
        if let Some(set) = self.segments.set.as_ref() {
            out.extend(set.segments().iter().filter(|s| s.provenance.is_pending()).map(|s| ReviewEntry {
                item: ReviewItem::Segment(s.label),
                name: s.name.clone(),
                kind: "Segment".into(),
                provenance: s.provenance.clone(),
            }));
        }
        out
    }

    /// Records a review decision by `by`. With a workspace open, the
    /// decision is logged and the results are saved at once.
    pub fn decide(&mut self, item: ReviewItem, status: ReviewStatus, by: Option<&str>) -> Result<(), String> {
        let by = by.map(str::trim).filter(|b| !b.is_empty());
        let name = match item {
            ReviewItem::Annotation(id) => {
                let name = self.annotations.name(id).ok_or("unknown annotation")?.to_owned();
                self.review_annotation(id, status, by);
                name
            }
            ReviewItem::Segment(label) => {
                self.review_segment(label, status, by).map_err(|e| e.to_string())?;
                self.segments.set.as_ref().and_then(|s| s.segment(label)).map(|s| s.name.clone()).unwrap_or_default()
            }
        };
        if let Some(w) = self.workspace.as_ref() {
            let decision = ReviewDecision { item, name, status, by: by.map(str::to_owned) };
            w.store.log(&decision)?;
            self.save_workspace()?;
        }
        Ok(())
    }
}
