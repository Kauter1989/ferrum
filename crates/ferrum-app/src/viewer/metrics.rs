//! Quantitative profile of a segment and the modality of the image: the
//! use cases behind the metrics card of the Segments panel (Stage 18).
//!
//! Values come from voxels (`ferrum-radiomics`); a segment is measured on a
//! worker thread, and a result says whether it is still current. Intensity
//! features need to know the unit of the voxel values: a NIfTI file carries
//! no modality, so the person is asked and the answer is kept in the
//! workspace, as the command line does.

use ferrum_domain::{parse_modality, MaskRegion, ResultStore, SegmentationSet};
use ferrum_radiomics::{compute, Input, MetricsResult, Params, Profile, ValueUnit};

use super::Viewer;
use crate::jobs::{JobEvent, MetricsRequest};

/// What became of the latest metrics request.
#[derive(Debug, Clone, PartialEq)]
pub enum MetricsOutcome {
    /// A worker is measuring.
    Pending,
    /// The features.
    Ready(Box<MetricsResult>),
    /// Why there are none, in words for the person.
    Failed(String),
}

/// The latest outcome for a segment and whether it is out of date.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricsView<'a> {
    /// What became of the request.
    pub outcome: &'a MetricsOutcome,
    /// The image or the segment changed since the request.
    pub stale: bool,
    /// The volume of a multi-volume series the values come from.
    pub volume: Option<&'a str>,
}

/// Metrics and modality state of the viewer.
#[derive(Debug, Clone, Default)]
pub struct MetricsState {
    entry: Option<(MetricsRequest, MetricsOutcome)>,
    /// The modality the person declared, or the workspace holds.
    declared: Option<String>,
    /// The person chose not to say what the image is (dataset revision).
    skipped_for: Option<u64>,
    /// The person wants to change the declared image type.
    reask: bool,
    /// Bin width and HU ranges the card is computed with.
    params: Params,
    /// Requests made so far.
    serial: u64,
    /// The volume the latest request measures (multi-volume series).
    volume: Option<String>,
}

fn unit_message(unit: &ValueUnit) -> String {
    match unit {
        ValueUnit::Hu => String::new(),
        ValueUnit::Unknown { modality } if modality.is_empty() => {
            "The image type is not known, so the voxel values cannot be read as HU. Say what the image is.".into()
        }
        ValueUnit::Unknown { modality } => {
            format!("Intensity values are read as HU for CT images only; this image is {modality}.")
        }
    }
}

impl Viewer {
    /// Unit of the voxel values of the loaded image: HU for CT, otherwise
    /// not known.
    pub fn value_unit(&self) -> ValueUnit {
        ValueUnit::from_modality(self.dataset.as_ref().map_or("", |d| d.metadata.modality.as_str()))
    }

    /// `true` while the image says nothing about its modality (a NIfTI
    /// file) and the person has not been asked, or has not said.
    pub fn needs_modality(&self) -> bool {
        self.dataset.as_ref().is_some_and(|d| {
            let unsaid = d.metadata.modality.trim().is_empty() && self.metrics.skipped_for != Some(d.revision);
            unsaid || (self.metrics.reask && self.can_declare_modality())
        })
    }

    /// `true` if the image type can be declared or changed: the image says
    /// nothing about it, or the person declared it earlier.
    pub fn can_declare_modality(&self) -> bool {
        self.dataset.as_ref().is_some_and(|d| d.metadata.modality.is_empty() || self.metrics.declared.is_some())
    }

    /// Asks again what the image is (the person chose "Not now", or wants
    /// to correct the answer).
    pub fn ask_modality_again(&mut self) {
        self.metrics.skipped_for = None;
        self.metrics.reask = true;
    }

    /// The person does not say what the image is for now. Metrics of
    /// intensity then explain what is missing instead of guessing.
    pub fn skip_modality_prompt(&mut self) {
        self.metrics.skipped_for = self.dataset.as_ref().map(|d| d.revision);
        self.metrics.reask = false;
    }

    /// Declares the modality of an image that carries none. An image with
    /// its own modality keeps it. With a workspace open the declaration is
    /// kept in it. Returns the modality code.
    pub fn declare_modality(&mut self, text: &str) -> Result<String, String> {
        let m = parse_modality(text)?;
        let own = self.dataset.as_ref().ok_or("no image is loaded")?.metadata.modality.clone();
        if !own.is_empty() && self.metrics.declared.is_none() {
            return if own.eq_ignore_ascii_case(&m) {
                Ok(own)
            } else {
                Err(format!("the series carries the modality {own}; it cannot be declared {m}"))
            };
        }
        if let Some(w) = &self.workspace {
            w.store.set_modality(&m)?;
        }
        if let Some(d) = self.dataset.as_mut() {
            d.metadata.modality = m.clone();
        }
        self.metrics.declared = Some(m.clone());
        self.metrics.entry = None;
        self.metrics.skipped_for = None;
        self.metrics.reask = false;
        self.status.message = format!("Image type set to {m}");
        Ok(m)
    }

    /// Another volume of the series replaces the image (revision
    /// `old_revision`): it keeps the declared modality and a "Not now".
    pub(super) fn keep_image_type(&mut self, old_revision: Option<u64>, d: &mut crate::dataset::Dataset) {
        if d.metadata.modality.is_empty() {
            if let Some(m) = &self.metrics.declared {
                d.metadata.modality = m.clone();
            }
        }
        if old_revision.is_some() && self.metrics.skipped_for == old_revision {
            self.metrics.skipped_for = Some(d.revision);
        }
    }

    /// Takes the modality a workspace holds for an image that carries none.
    pub(super) fn adopt_workspace_modality(&mut self, store: &dyn ResultStore) {
        let Some(d) = self.dataset.as_mut().filter(|d| d.metadata.modality.is_empty()) else {
            return;
        };
        if let Some(m) = store.modality() {
            d.metadata.modality = m.clone();
            self.metrics.declared = Some(m);
        }
    }

    /// The parameters (histogram bin width, HU ranges) that metrics are
    /// computed with.
    pub fn metrics_params(&self) -> &Params {
        &self.metrics.params
    }

    /// Sets the parameters of the metrics. Nonsense (a bin width that is
    /// not positive, ranges that overlap) is refused and the old
    /// parameters stay. The figures on the card are out of date until the
    /// segment is measured again.
    pub fn set_metrics_params(&mut self, params: Params) -> Result<(), String> {
        params.validate().map_err(|e| e.to_string())?;
        self.metrics.params = params;
        Ok(())
    }

    /// The voxels of segment `label`.
    fn segment_region(&self, label: u8) -> Result<MaskRegion, String> {
        let set = self.segments.set.as_ref().ok_or("there are no segments")?;
        set.segment(label).ok_or_else(|| format!("there is no segment {label}"))?;
        MaskRegion::of_label(set.labels(), label).ok_or_else(|| "the segment has no voxels".to_owned())
    }

    /// Measures segment `label` under `profile` on the calling thread (for
    /// tests and embedding applications; the panel uses
    /// [`Viewer::request_segment_metrics`]).
    pub fn segment_metrics(&self, label: u8, profile: &Profile) -> Result<MetricsResult, String> {
        let region = self.segment_region(label)?;
        let d = self.dataset.as_ref().ok_or("no image is loaded")?;
        let input = Input { region: &region, volume: &d.volume, unit: self.value_unit() };
        compute(&input, profile, &self.metrics.params).map_err(|e| e.to_string())
    }

    /// Starts measuring segment `label` in the background. A profile with
    /// intensity features and an unknown unit is not computed: the outcome
    /// says what to do.
    pub fn request_segment_metrics(&mut self, label: u8, profile: &'static Profile) -> Result<(), String> {
        let region = self.segment_region(label)?;
        let d = self.dataset.as_ref().ok_or("no image is loaded")?;
        let (volume, dataset_revision) = (d.volume.clone(), d.revision);
        self.metrics.serial += 1;
        let request = MetricsRequest {
            dataset_revision,
            segments_revision: self.segments.set.as_ref().map_or(0, SegmentationSet::revision),
            label,
            profile: profile.name,
            serial: self.metrics.serial,
        };
        self.metrics.volume = self.current_volume_label();
        let unit = self.value_unit();
        if profile.needs_unit() && !unit.is_known() {
            self.metrics.entry = Some((request, MetricsOutcome::Failed(unit_message(&unit))));
            return Ok(());
        }
        self.metrics.entry = Some((request, MetricsOutcome::Pending));
        self.jobs.metrics(volume, region, unit, profile, self.metrics.params.clone(), request);
        Ok(())
    }

    /// The latest outcome for segment `label`, if one was requested.
    pub fn segment_metrics_view(&self, label: u8) -> Option<MetricsView<'_>> {
        let (request, outcome) = self.metrics.entry.as_ref().filter(|(r, _)| r.label == label)?;
        let d = self.dataset.as_ref()?;
        let revision = self.segments.set.as_ref().map_or(0, SegmentationSet::revision);
        let stale = request.dataset_revision != d.revision || request.segments_revision != revision;
        Some(MetricsView { outcome, stale, volume: self.metrics.volume.as_deref() })
    }

    /// Stores a finished job if it is the one that was asked for last.
    pub(super) fn finish_metrics(&mut self, event: JobEvent) {
        let JobEvent::Metrics { request, result } = event else { return };
        if self.metrics.entry.as_ref().is_some_and(|(r, _)| *r == request) {
            let outcome = match result {
                Ok(r) => MetricsOutcome::Ready(r),
                Err(e) => MetricsOutcome::Failed(e.to_string()),
            };
            self.metrics.entry = Some((request, outcome));
        }
    }
}
