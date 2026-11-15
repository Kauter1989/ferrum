//! What the person did in a frame, for the host ([design] §3.4, 24.2).
//!
//! Events are derived from [`Viewer`] state, not raised inside the tools:
//! [`ViewerWidget::show`](crate::ViewerWidget::show) compares the state
//! after drawing with the state it gave the host last, so every rule stays
//! in `ferrum-app` and a use case the host calls itself is seen too.
//!
//! No event is a decision (24.2-c): confirm and reject stay in the review
//! widget, and an engine's or agent's proposal is not [`ViewerEvent::RoiDrawn`]
//! — only what a person drew. Events hold voxel indices, millimetres, window
//! values, segment labels and annotation ids, nothing from the DICOM header.
//!
//! [design]: https://github.com/Kauter1989/ferrum/blob/develop/docs/embeddable-viewer.md

use std::path::PathBuf;

use ferrum_app::Viewer;
use ferrum_domain::{AnnotationId, Author, OrbitCamera, Volume, WindowLevel};
use glam::UVec3;

/// A point in both of FERRUM's forms (24.2-b): the voxel index and the
/// patient position (LPS, mm) of its centre, the values `probe` resolves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Voxel index in the canonical LPS grid.
    pub voxel: [u32; 3],
    /// Patient position of the voxel centre, in millimetres.
    pub patient_mm: [f64; 3],
}

/// What the person drew: an annotation, or a segment of the label map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoiItem {
    /// A measurement or text annotation, by its id.
    Annotation(AnnotationId),
    /// A segment, by its label value.
    Segment(u8),
}

/// One thing that happened in the widget's frame.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewerEvent {
    /// The probe reading moved to a new voxel (a click with the Probe tool).
    PointPicked {
        /// Where, in both point forms.
        point: Point,
    },
    /// A new annotation or segment of the person appeared, or the person's
    /// own segment's mask changed. Engine and agent proposals are not here.
    RoiDrawn {
        /// What was drawn.
        item: RoiItem,
    },
    /// The slice indices differ from the last frame's; once per frame, with
    /// all three orientations (axial, coronal, sagittal order of
    /// [`SliceAxis::normal_axis`](ferrum_domain::SliceAxis::normal_axis)).
    SliceChanged {
        /// Current slice index per orientation.
        indices: [u32; 3],
    },
    /// The display window changed, given once at the end of a drag (or at
    /// once when no pointer button is down).
    WindowChanged {
        /// Window centre, in the volume's units.
        center: f32,
        /// Window width, in the volume's units.
        width: f32,
    },
    /// The 3D camera changed, given once at the end of a drag or after a
    /// preset.
    CameraChanged,
}

/// The viewer state the events are derived from. Private: the widget keeps
/// the last one it gave the host and diffs against it.
#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    /// The study's source (the series folder or file). The identity is the
    /// source, not the volume `Arc`: a finished filter replaces the volume
    /// of the same study, and a volume switch of the series keeps it too.
    source: Option<PathBuf>,
    probe: Option<UVec3>,
    indices: [u32; 3],
    /// Ids of the annotations a person drew.
    annotations: Vec<AnnotationId>,
    /// Label and voxel count of every segment a person drew.
    segments: Vec<(u8, u64)>,
    revision: u64,
}

impl Snapshot {
    /// Reads the event-relevant state of `viewer`.
    pub(crate) fn take(viewer: &Viewer) -> Self {
        let annotations = viewer
            .annotations()
            .iter()
            .filter(|(id, _, _, _)| viewer.annotations().provenance(*id).is_some_and(|p| p.author == Author::Human))
            .map(|(id, _, _, _)| id)
            .collect();
        let (segments, revision) = match viewer.segmentation().set() {
            Some(set) => (
                set.segments()
                    .iter()
                    .filter(|s| s.provenance.author == Author::Human)
                    .map(|s| (s.label, set.voxel_count(s.label)))
                    .collect(),
                set.revision(),
            ),
            None => (Vec::new(), 0),
        };
        Self {
            source: viewer.dataset().map(|d| d.metadata.source.clone()),
            probe: viewer.probe.map(|p| p.voxel),
            indices: viewer.slices.indices,
            annotations,
            segments,
            revision,
        }
    }

    /// Whether both snapshots show the same study.
    fn same_study(&self, other: &Self) -> bool {
        self.source == other.source
    }

    /// The events between two frames' states. `volume` is the study shown
    /// (for the point's patient position); a study switch gives no events.
    fn diff(&self, after: &Self, volume: Option<&Volume>) -> Vec<ViewerEvent> {
        if !self.same_study(after) {
            return Vec::new();
        }
        let mut events = Vec::new();
        if let (Some(voxel), Some(vol)) = (after.probe, volume) {
            if self.probe != Some(voxel) {
                events.push(ViewerEvent::PointPicked {
                    point: Point {
                        voxel: voxel.to_array(),
                        patient_mm: vol.voxel_to_patient_f64(voxel.as_dvec3()).to_array(),
                    },
                });
            }
        }
        for id in &after.annotations {
            if !self.annotations.contains(id) {
                events.push(ViewerEvent::RoiDrawn { item: RoiItem::Annotation(*id) });
            }
        }
        for (label, count) in &after.segments {
            let before = self.segments.iter().find(|(l, _)| l == label);
            match before {
                None => events.push(ViewerEvent::RoiDrawn { item: RoiItem::Segment(*label) }),
                Some((_, old)) if old != count && self.revision != after.revision => {
                    events.push(ViewerEvent::RoiDrawn { item: RoiItem::Segment(*label) });
                }
                _ => {}
            }
        }
        if self.indices != after.indices {
            events.push(ViewerEvent::SliceChanged { indices: after.indices });
        }
        events
    }
}

/// The window and camera last given to the host: a drag gives one event
/// when it ends, so the baseline only moves when an event goes out.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DragBaseline {
    pub(crate) window: WindowLevel,
    pub(crate) camera: OrbitCamera,
}

impl DragBaseline {
    /// The window and camera events of this frame, if the drag has ended.
    pub(crate) fn events(&mut self, window: WindowLevel, camera: OrbitCamera, pointer_down: bool) -> Vec<ViewerEvent> {
        let mut events = Vec::new();
        if !pointer_down {
            if window != self.window {
                events.push(ViewerEvent::WindowChanged { center: window.center, width: window.width });
                self.window = window;
            }
            if camera != self.camera {
                events.push(ViewerEvent::CameraChanged);
                self.camera = camera;
            }
        }
        events
    }
}

/// Derives one frame's events: the state diff, then the coalesced window
/// and camera. The widget calls it with the previous frame's snapshot.
pub(crate) fn frame_events(
    before: Option<&Snapshot>,
    after: &Snapshot,
    viewer: &Viewer,
    baseline: &mut Option<DragBaseline>,
    pointer_down: bool,
) -> Vec<ViewerEvent> {
    let current = DragBaseline { window: viewer.slices.window, camera: viewer.volume.camera };
    let Some(before) = before else {
        // the first frame (or a new study): establish the baseline, no events
        *baseline = Some(current);
        return Vec::new();
    };
    let volume = viewer.dataset().map(|d| d.volume.as_ref());
    let mut events = before.diff(after, volume);
    if !before.same_study(after) {
        *baseline = Some(current);
        return events;
    }
    if let Some(b) = baseline {
        events.extend(b.events(current.window, current.camera, pointer_down));
    } else {
        *baseline = Some(current);
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot with the given probe, indices and human segments.
    fn snap(probe: Option<UVec3>, indices: [u32; 3], segments: Vec<(u8, u64)>) -> Snapshot {
        Snapshot { source: None, probe, indices, annotations: Vec::new(), segments, revision: 1 }
    }

    #[test]
    fn a_replaced_volume_of_the_same_study_keeps_the_baseline() {
        // a finished filter replaces the volume Arc; the source stays
        let mut a = snap(Some(UVec3::new(1, 2, 3)), [4; 3], Vec::new());
        let mut b = snap(Some(UVec3::new(1, 2, 3)), [5; 3], Vec::new());
        a.source = Some(PathBuf::from("/data/ct"));
        b.source = Some(PathBuf::from("/data/ct"));
        assert_eq!(a.diff(&b, None), vec![ViewerEvent::SliceChanged { indices: [5; 3] }]);
        b.source = Some(PathBuf::from("/data/other"));
        assert!(a.diff(&b, None).is_empty(), "a different study resets the baseline");
    }

    #[test]
    fn a_new_probe_voxel_is_a_pick_and_the_same_voxel_is_not() {
        let a = snap(Some(UVec3::new(1, 2, 3)), [0; 3], Vec::new());
        let b = snap(Some(UVec3::new(1, 2, 3)), [0; 3], Vec::new());
        assert!(a.diff(&b, None).is_empty(), "the same voxel is not a new pick");
        let c = snap(Some(UVec3::new(4, 2, 3)), [0; 3], Vec::new());
        assert!(a.diff(&c, None).is_empty(), "without a volume there is no patient position to give");
    }

    #[test]
    fn a_mask_edit_of_a_persons_segment_is_roidrawn_but_not_without_a_revision() {
        let a = snap(None, [0; 3], vec![(1, 100)]);
        let mut b = snap(None, [0; 3], vec![(1, 120)]);
        b.revision = 2;
        assert_eq!(a.diff(&b, None), vec![ViewerEvent::RoiDrawn { item: RoiItem::Segment(1) }]);
        let c = snap(None, [0; 3], vec![(1, 120)]);
        assert!(a.diff(&c, None).is_empty(), "a count change without a revision is not an edit");
    }

    #[test]
    fn a_drag_gives_one_window_event_when_it_ends() {
        let mut base = DragBaseline { window: WindowLevel::new(40.0, 400.0), camera: OrbitCamera::default() };
        let dragged = WindowLevel::new(60.0, 400.0);
        assert!(base.events(dragged, OrbitCamera::default(), true).is_empty(), "during the drag");
        assert!(base.events(dragged, OrbitCamera::default(), true).is_empty(), "still down");
        let events = base.events(dragged, OrbitCamera::default(), false);
        assert_eq!(events, vec![ViewerEvent::WindowChanged { center: 60.0, width: 400.0 }]);
        assert!(base.events(dragged, OrbitCamera::default(), false).is_empty(), "given once");
    }
}
