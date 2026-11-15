//! The view state and events of `ferrum-view/1` (Stage 28, #70).
//!
//! Pure value types: an agent drives what the person sees by writing a
//! [`ViewState`] to the workspace, and the live clients apply it through
//! `ferrum-app`. The JSON contract lives in `ferrum-io` (the domain has no
//! serde); these types carry the wire names so every layer agrees. A state
//! holds numbers and labels only — no path, UID, name or header value
//! (criterion 28.5-a). See `docs/view-state.md` and ADR 0018.

use crate::camera::OrbitCamera;
use crate::segmentation::SegmentStyle;
use crate::slice::SliceAxis;
use crate::window::WindowLevel;

/// Which view component is shown: one slice, the MPR grid, or the 3D view.
///
/// The wire names are the issue's (`slice`, `mpr`, `volume`); `ferrum-app`
/// maps them to its `ViewMode` (whose desktop labels are "2D"/"MPR"/"3D").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ViewComponent {
    /// One 2D slice view.
    #[default]
    Slice,
    /// Axial, coronal, sagittal and 3D in a grid.
    Mpr,
    /// One 3D view.
    Volume,
}

impl ViewComponent {
    /// All components.
    pub const ALL: [ViewComponent; 3] = [ViewComponent::Slice, ViewComponent::Mpr, ViewComponent::Volume];

    /// Wire name: `slice`, `mpr` or `volume`.
    pub fn as_str(self) -> &'static str {
        match self {
            ViewComponent::Slice => "slice",
            ViewComponent::Mpr => "mpr",
            ViewComponent::Volume => "volume",
        }
    }

    /// Inverse of [`ViewComponent::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// The active tool, by its wire name (`ferrum-app` maps it to `ToolKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ViewTool {
    /// Drag to pan, wheel to zoom.
    #[default]
    Pan,
    /// Drag to change window centre/width.
    WindowLevel,
    /// Show the voxel value under the pointer.
    Probe,
    /// Measure a distance.
    Distance,
    /// Measure an angle.
    Angle,
    /// Measure a free-form area.
    Area,
    /// Measure a rectangular area.
    Rect,
    /// Place a text note.
    Text,
    /// Move annotations.
    Move,
    /// Delete annotations.
    Delete,
    /// AI: click a point inside or outside the object.
    AiPoint,
    /// AI: drag a box around the object.
    AiBox,
    /// AI: paint strokes over the object.
    AiScribble,
    /// AI: draw a closed outline around the object.
    AiLasso,
    /// Click inside a structure to segment its connected region.
    Region,
}

impl ViewTool {
    /// All tools.
    pub const ALL: [ViewTool; 15] = [
        ViewTool::Pan,
        ViewTool::WindowLevel,
        ViewTool::Probe,
        ViewTool::Distance,
        ViewTool::Angle,
        ViewTool::Area,
        ViewTool::Rect,
        ViewTool::Text,
        ViewTool::Move,
        ViewTool::Delete,
        ViewTool::AiPoint,
        ViewTool::AiBox,
        ViewTool::AiScribble,
        ViewTool::AiLasso,
        ViewTool::Region,
    ];

    /// Wire name (snake_case).
    pub fn as_str(self) -> &'static str {
        match self {
            ViewTool::Pan => "pan",
            ViewTool::WindowLevel => "window_level",
            ViewTool::Probe => "probe",
            ViewTool::Distance => "distance",
            ViewTool::Angle => "angle",
            ViewTool::Area => "area",
            ViewTool::Rect => "rect",
            ViewTool::Text => "text",
            ViewTool::Move => "move",
            ViewTool::Delete => "delete",
            ViewTool::AiPoint => "ai_point",
            ViewTool::AiBox => "ai_box",
            ViewTool::AiScribble => "ai_scribble",
            ViewTool::AiLasso => "ai_lasso",
            ViewTool::Region => "region",
        }
    }

    /// Inverse of [`ViewTool::as_str`].
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

/// How the segment overlay is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overlays {
    /// Whether the overlay is shown.
    pub segments_shown: bool,
    /// The display's fill opacity (segment opacity multiplies it).
    pub fill_opacity: f32,
    /// Outline, fill, or both.
    pub style: SegmentStyle,
}

impl Default for Overlays {
    fn default() -> Self {
        Self { segments_shown: false, fill_opacity: crate::segmentation::DEFAULT_OPACITY, style: SegmentStyle::default() }
    }
}

/// A value read from the voxel under a pick (read-only in a state).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewProbe {
    /// The voxel of the pick.
    pub voxel: [u32; 3],
    /// Its centre in patient (LPS) millimetres.
    pub patient_mm: [f64; 3],
    /// The voxel's value in the volume's units.
    pub value: f64,
    /// `HU` for a CT, `raw` otherwise.
    pub unit: String,
}

/// What a client shows: the full `ferrum-view/1` state (Stage 28).
///
/// A state with no component is `slice` (backward compatible). `probe` is
/// read-only — a `view set` ignores it. `camera` is present for `mpr`/`volume`.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    /// Which component is shown.
    pub component: ViewComponent,
    /// The plane shown (for `slice`, and the active plane of `mpr`).
    pub plane: SliceAxis,
    /// The slice index (0-based, clamped to the volume by the applier).
    pub slice_index: u32,
    /// The display window in the volume's units.
    pub window: WindowLevel,
    /// The segment overlay.
    pub overlays: Overlays,
    /// The selected segment's label, if any.
    pub selected_segment: Option<u8>,
    /// The active tool.
    pub tool: ViewTool,
    /// The 3D camera (for `mpr`/`volume`).
    pub camera: Option<OrbitCamera>,
    /// The last pick (read-only).
    pub probe: Option<ViewProbe>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            component: ViewComponent::default(),
            plane: SliceAxis::default(),
            slice_index: 0,
            window: WindowLevel::new(0.5, 1.0),
            overlays: Overlays::default(),
            selected_segment: None,
            tool: ViewTool::default(),
            camera: None,
            probe: None,
        }
    }
}

/// The item a `roi_drawn` event names: a person's annotation or segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoiItem {
    /// An annotation by id.
    Annotation(u64),
    /// A segment by label.
    Segment(u8),
}

/// One thing the person did, as the agent reads it back (Stage 28).
///
/// The `seq` is assigned by the client that records the event; the JSON form
/// (with `seq` and `kind`) is built in `ferrum-io`. No event is a decision.
#[derive(Debug, Clone, PartialEq)]
pub enum ViewEvent {
    /// The slice index changed.
    SliceChanged {
        /// The plane.
        plane: SliceAxis,
        /// The new index.
        slice_index: u32,
    },
    /// The plane changed.
    PlaneChanged {
        /// The new plane.
        plane: SliceAxis,
        /// Its slice index.
        slice_index: u32,
    },
    /// The window changed.
    WindowChanged {
        /// Window centre.
        center: f32,
        /// Window width.
        width: f32,
    },
    /// The overlay's visibility changed.
    SegmentsChanged {
        /// Whether it is shown.
        shown: bool,
    },
    /// The 3D camera changed.
    CameraChanged {
        /// The new camera.
        camera: OrbitCamera,
    },
    /// The person drew or edited their own annotation or segment.
    RoiDrawn {
        /// The item.
        item: RoiItem,
    },
    /// The person read the voxel under a pick.
    PointPicked {
        /// The reading.
        probe: ViewProbe,
    },
}

impl ViewEvent {
    /// The wire kind (`slice_changed`, `point_picked`, …).
    pub fn kind(&self) -> &'static str {
        match self {
            ViewEvent::SliceChanged { .. } => "slice_changed",
            ViewEvent::PlaneChanged { .. } => "plane_changed",
            ViewEvent::WindowChanged { .. } => "window_changed",
            ViewEvent::SegmentsChanged { .. } => "segments_changed",
            ViewEvent::CameraChanged { .. } => "camera_changed",
            ViewEvent::RoiDrawn { .. } => "roi_drawn",
            ViewEvent::PointPicked { .. } => "point_picked",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // covers 28.1-a
    #[test]
    fn component_and_tool_wire_names_round_trip() {
        for c in ViewComponent::ALL {
            assert_eq!(ViewComponent::parse(c.as_str()), Some(c));
        }
        for t in ViewTool::ALL {
            assert_eq!(ViewTool::parse(t.as_str()), Some(t));
        }
        assert_eq!(ViewComponent::parse("3d"), None);
        assert_eq!(ViewTool::parse("unknown"), None);
    }

    // covers 28.1-b
    #[test]
    fn event_kinds_are_the_contract_names() {
        let cam = OrbitCamera::default();
        let probe = ViewProbe { voxel: [0, 0, 0], patient_mm: [0.0; 3], value: 0.0, unit: "HU".into() };
        let cases = [
            (ViewEvent::SliceChanged { plane: SliceAxis::Axial, slice_index: 1 }, "slice_changed"),
            (ViewEvent::PlaneChanged { plane: SliceAxis::Coronal, slice_index: 1 }, "plane_changed"),
            (ViewEvent::WindowChanged { center: 0.0, width: 1.0 }, "window_changed"),
            (ViewEvent::SegmentsChanged { shown: true }, "segments_changed"),
            (ViewEvent::CameraChanged { camera: cam }, "camera_changed"),
            (ViewEvent::RoiDrawn { item: RoiItem::Segment(3) }, "roi_drawn"),
            (ViewEvent::PointPicked { probe }, "point_picked"),
        ];
        for (e, kind) in cases {
            assert_eq!(e.kind(), kind);
        }
    }
}
