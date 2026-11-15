//! Applying a `ferrum-view/1` view state to the `Viewer` (Stage 28, #70).
//!
//! The only place a [`ViewState`] becomes use cases (ADR 0018): the agent
//! writes the state to the workspace, the live clients call
//! [`Viewer::apply_view_state`]. The person takes over at any time — applying
//! a state locks nothing and enters no mode. `apply_view_state` returns the
//! events the change produced, so the caller records exactly what happened.

use ferrum_domain::{
    Overlays, ViewComponent, ViewEvent, ViewProbe, ViewState, ViewTool,
};
use glam::DVec3;

use super::{ViewMode, Viewer};
use crate::tools::ToolKind;

/// The `ViewMode` of a contract component.
pub fn view_mode_of(component: ViewComponent) -> ViewMode {
    match component {
        ViewComponent::Slice => ViewMode::Slice2d,
        ViewComponent::Mpr => ViewMode::Mpr,
        ViewComponent::Volume => ViewMode::Volume3d,
    }
}

/// The contract component of a `ViewMode`.
pub fn component_of(mode: ViewMode) -> ViewComponent {
    match mode {
        ViewMode::Slice2d => ViewComponent::Slice,
        ViewMode::Mpr => ViewComponent::Mpr,
        ViewMode::Volume3d => ViewComponent::Volume,
    }
}

/// The `ToolKind` of a contract tool.
pub fn tool_kind_of(tool: ViewTool) -> ToolKind {
    match tool {
        ViewTool::Pan => ToolKind::Pan,
        ViewTool::WindowLevel => ToolKind::WindowLevel,
        ViewTool::Probe => ToolKind::Probe,
        ViewTool::Distance => ToolKind::Distance,
        ViewTool::Angle => ToolKind::Angle,
        ViewTool::Area => ToolKind::Area,
        ViewTool::Rect => ToolKind::Rect,
        ViewTool::Text => ToolKind::Text,
        ViewTool::Move => ToolKind::Move,
        ViewTool::Delete => ToolKind::Delete,
        ViewTool::AiPoint => ToolKind::AiPoint,
        ViewTool::AiBox => ToolKind::AiBox,
        ViewTool::AiScribble => ToolKind::AiScribble,
        ViewTool::AiLasso => ToolKind::AiLasso,
        ViewTool::Region => ToolKind::Region,
    }
}

/// The contract tool of a `ToolKind`.
pub fn view_tool_of(tool: ToolKind) -> ViewTool {
    match tool {
        ToolKind::Pan => ViewTool::Pan,
        ToolKind::WindowLevel => ViewTool::WindowLevel,
        ToolKind::Probe => ViewTool::Probe,
        ToolKind::Distance => ViewTool::Distance,
        ToolKind::Angle => ViewTool::Angle,
        ToolKind::Area => ViewTool::Area,
        ToolKind::Rect => ViewTool::Rect,
        ToolKind::Text => ViewTool::Text,
        ToolKind::Move => ViewTool::Move,
        ToolKind::Delete => ViewTool::Delete,
        ToolKind::AiPoint => ViewTool::AiPoint,
        ToolKind::AiBox => ViewTool::AiBox,
        ToolKind::AiScribble => ViewTool::AiScribble,
        ToolKind::AiLasso => ViewTool::AiLasso,
        ToolKind::Region => ViewTool::Region,
    }
}

impl Viewer {
    /// Applies a view state, returning the events the change produced (in
    /// order). A field equal to the current one produces no event; the slice
    /// index is clamped to the volume. The person's next input wins — nothing
    /// is locked (criterion 28.2-a). `probe` is read-only and ignored.
    pub fn apply_view_state(&mut self, state: &ViewState) -> Vec<ViewEvent> {
        let mut events = Vec::new();

        let mode = view_mode_of(state.component);
        if self.view_mode != mode {
            self.set_view_mode(mode);
        }

        if self.slices.axis != state.plane {
            self.slices.axis = state.plane;
            events.push(ViewEvent::PlaneChanged { plane: state.plane, slice_index: self.slice_index(state.plane) });
        }

        let count = self.dataset().map(|d| state.plane.slice_count(&d.volume)).unwrap_or(0);
        let index = state.slice_index.min(count.saturating_sub(1));
        if self.slice_index(state.plane) != index {
            self.set_slice_index(state.plane, index);
            events.push(ViewEvent::SliceChanged { plane: state.plane, slice_index: index });
        }

        if self.window() != state.window {
            self.set_window(state.window);
            events.push(ViewEvent::WindowChanged { center: state.window.center, width: state.window.width });
        }

        let seg = &mut self.segments;
        let overlays_changed = seg.show != state.overlays.segments_shown
            || seg.fill_opacity != state.overlays.fill_opacity
            || seg.style != state.overlays.style;
        seg.show = state.overlays.segments_shown;
        seg.fill_opacity = state.overlays.fill_opacity;
        seg.style = state.overlays.style;
        if overlays_changed {
            events.push(ViewEvent::SegmentsChanged { shown: seg.show });
        }
        seg.selected = state.selected_segment;

        let tool = tool_kind_of(state.tool);
        if self.tool != tool {
            self.tool = tool;
        }

        if let Some(camera) = state.camera {
            if self.volume.camera != camera {
                self.volume.camera = camera;
                events.push(ViewEvent::CameraChanged { camera });
            }
        }

        events
    }

    /// Reads the current view state back (the source hash and revision are
    /// added by the caller from the workspace). `camera` is present for the
    /// MPR and 3D components; `probe` is the last pick, if any.
    pub fn view_state(&self) -> ViewState {
        let component = component_of(self.view_mode);
        let plane = self.slices.axis;
        let camera = matches!(component, ViewComponent::Mpr | ViewComponent::Volume).then(|| self.volume.camera);
        let probe = self.probe.as_ref().and_then(|p| {
            let volume = &self.dataset()?.volume;
            let centre = DVec3::new(f64::from(p.voxel.x) + 0.5, f64::from(p.voxel.y) + 0.5, f64::from(p.voxel.z) + 0.5);
            let mm = volume.voxel_to_patient_f64(centre);
            Some(ViewProbe {
                voxel: [p.voxel.x, p.voxel.y, p.voxel.z],
                patient_mm: [mm.x, mm.y, mm.z],
                value: f64::from(p.value),
                unit: value_unit(self).to_string(),
            })
        });
        ViewState {
            component,
            plane,
            slice_index: self.slice_index(plane),
            window: self.window(),
            overlays: Overlays {
                segments_shown: self.segments.show,
                fill_opacity: self.segments.fill_opacity,
                style: self.segments.style,
            },
            selected_segment: self.segments.selected,
            tool: view_tool_of(self.tool),
            camera,
            probe,
        }
    }
}

/// The unit of a value: HU for a CT, raw otherwise (as the view host writes).
fn value_unit(viewer: &Viewer) -> &'static str {
    let modality = viewer.dataset().map(|d| d.metadata.modality.trim().to_ascii_uppercase()).unwrap_or_default();
    if modality == "CT" {
        "HU"
    } else {
        "raw"
    }
}
