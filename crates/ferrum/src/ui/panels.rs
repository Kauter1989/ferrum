//! Right-hand settings panel. Its tabs follow the view mode, so only
//! settings that affect what is on screen are offered:
//!
//! - **Image** (2D, MPR): window, slices, annotations (2D) and
//!   segmentation — the guide, the region tool, the AI engine and the
//!   segment list;
//! - **Volume** (3D, MPR): technique, transfer function, clipping, the
//!   eraser (3D) and the segment list for display;
//! - **Details** (always): series information.

use egui::{RichText, Slider};
use egui_phosphor::light as icon;
use std::collections::HashMap;

use ferrum_app::{ViewMode, Viewer};
use ferrum_domain::{
    AnnotationId, ClipBox, CtPreset, RenderMode, SliceAxis, SliceKey, TissueThresholds, TransferFunction, WindowLevel,
    WindowPreset,
};

use super::ai_panel::AiPanelState;
use super::segmentation_panel;
use super::segments_panel::{self, SegmentsPanelState};
use super::tf_editor::{self, TfEditorState};
use super::theme::{ACCENT, OVERLAY, TEXT, TEXT_DIM};
use super::widgets::{chip, icon_slider, section_title, segmented, slider_row, tool_button};

/// Tab of the settings panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelTab {
    /// Window/level, slices and annotations.
    #[default]
    Image,
    /// Volume rendering parameters.
    Volume,
    /// Series information.
    Details,
}

impl PanelTab {
    /// Tabs offered in `mode`.
    fn for_mode(mode: ViewMode) -> &'static [PanelTab] {
        match mode {
            ViewMode::Slice2d => &[PanelTab::Image, PanelTab::Details],
            ViewMode::Volume3d => &[PanelTab::Volume, PanelTab::Details],
            ViewMode::Mpr => &[PanelTab::Image, PanelTab::Volume, PanelTab::Details],
        }
    }

    fn label(self) -> &'static str {
        match self {
            PanelTab::Image => "Image",
            PanelTab::Volume => "Volume",
            PanelTab::Details => "Details",
        }
    }
}

/// Persistent UI-only state of the panel.
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    /// Transfer function editor state.
    pub tf_editor: TfEditorState,
    /// Selected tab.
    pub tab: PanelTab,
    /// View mode seen on the previous frame; switching to 2D or 3D selects
    /// the matching tab.
    last_mode: Option<ViewMode>,
    /// Annotation names being edited (kept while the field has focus, so
    /// the text may be empty temporarily).
    name_edits: HashMap<AnnotationId, String>,
    /// Set when the user asks to export annotations; the application
    /// handles it (file dialog) and resets the flag.
    pub export_annotations: bool,
    /// State of the "Segments" section.
    pub segments: SegmentsPanelState,
    /// State of the "AI segmentation" section.
    pub ai: AiPanelState,
    /// State of the "Review" section.
    pub review: super::review_panel::ReviewPanelState,
}

fn collapsible(ui: &mut egui::Ui, id: &str, icon_str: &str, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    let header = RichText::new(format!("{icon_str}  {title}")).size(13.5).strong().color(TEXT);
    egui::CollapsingHeader::new(header).id_salt(id).default_open(open).show(ui, body);
}

/// Draws the panel contents.
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    if viewer.dataset().is_none() {
        return;
    }
    let tabs = PanelTab::for_mode(viewer.view_mode());
    if state.last_mode != Some(viewer.view_mode()) {
        match viewer.view_mode() {
            ViewMode::Slice2d => state.tab = PanelTab::Image,
            ViewMode::Volume3d => state.tab = PanelTab::Volume,
            ViewMode::Mpr => {}
        }
        state.last_mode = Some(viewer.view_mode());
    }
    if !tabs.contains(&state.tab) {
        state.tab = tabs[0];
    }
    let labels: Vec<&str> = tabs.iter().map(|t| t.label()).collect();
    let selected = tabs.iter().position(|t| *t == state.tab);
    if let Some(i) = segmented(ui, &labels, selected) {
        state.tab = tabs[i];
    }
    ui.add_space(4.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match state.tab {
        PanelTab::Image => image_settings(ui, viewer, state),
        PanelTab::Volume => volume_settings(ui, viewer, state),
        PanelTab::Details => details(ui, viewer),
    });
}

fn image_settings(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    let Some(volume) = viewer.dataset().map(|d| d.volume.clone()) else {
        return;
    };
    section_title(ui, icon::CIRCLE_HALF, "Window level");
    ui.horizontal_wrapped(|ui| {
        if chip(ui, &format!("{} Auto", icon::SPARKLE), false).clicked() {
            viewer.auto_window();
        }
        for p in WindowPreset::ALL {
            if chip(ui, p.label(), false).clicked() {
                viewer.apply_window_preset(p);
            }
        }
    });
    let range = volume.range();
    let WindowLevel { mut center, mut width } = viewer.slices.window;
    let span = range.span().max(1.0);
    let mut changed = slider_row(ui, "Window", &mut width, 1.0..=span * 2.0);
    changed |= slider_row(ui, "Level", &mut center, range.min..=range.max);
    if changed {
        viewer.slices.window = WindowLevel::new(center, width.max(0.001));
    }

    section_title(ui, icon::STACK, "Slices");
    let axes: Vec<SliceAxis> =
        if viewer.view_mode() == ViewMode::Mpr { SliceAxis::ALL.to_vec() } else { vec![viewer.slices.axis] };
    for axis in axes {
        let n = axis.slice_count(&volume);
        // slice numbers are shown one-based everywhere in the UI
        let mut number = viewer.slices.index(axis) + 1;
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 16.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, super::slice_view::axis_color(axis));
            ui.label(RichText::new(axis.label()).color(TEXT_DIM));
            if ui.add(Slider::new(&mut number, 1..=n.max(1))).changed() {
                viewer.set_slice_index(axis, number.saturating_sub(1));
            }
        });
    }
    ui.checkbox(&mut viewer.slices.nearest, "Nearest-neighbour sampling");

    if let Some(p) = viewer.probe {
        ui.label(
            RichText::new(format!(
                "{} ({}, {}, {}) = {:.1}",
                icon::CROSSHAIR,
                p.voxel.x,
                p.voxel.y,
                p.voxel.z,
                p.value
            ))
            .monospace()
            .color(OVERLAY),
        );
    }
    if viewer.view_mode() == ViewMode::Slice2d {
        annotation_list(ui, viewer, state);
    }
    ui.add_space(6.0);
    collapsible(ui, "segmentation", icon::MAGIC_WAND, "Segmentation", true, |ui| {
        segmentation_panel::show(ui, viewer, &mut state.ai);
    });
    segments_section(ui, viewer, state);
}

/// Collapsible segment list and review queue, shown in the image and volume
/// tabs.
fn segments_section(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    collapsible(ui, "segments", icon::POLYGON, "Segments", true, |ui| {
        let by = super::review_panel::reviewer(&state.review).map(str::to_owned);
        segments_panel::show(ui, viewer, &mut state.segments, by.as_deref());
    });
    if super::review_panel::relevant(viewer) {
        collapsible(ui, "review", icon::CLIPBOARD_TEXT, "Review", true, |ui| {
            super::review_panel::show(ui, viewer, &mut state.review);
        });
    }
}

/// Annotations of all slices: editable names, value, plane and slice.
/// Clicking a row (or its arrow) shows the annotation's slice.
fn annotation_list(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    section_title(ui, icon::RULER, "Annotations");
    let n = viewer.annotations().len();
    ui.horizontal(|ui| {
        if ui.add_enabled(n > 0, egui::Button::new(format!("{} Export JSON", icon::DOWNLOAD_SIMPLE))).clicked() {
            state.export_annotations = true;
        }
        if ui.add_enabled(n > 0, egui::Button::new(format!("{} Clear all", icon::TRASH))).clicked() {
            viewer.clear_annotations();
            state.name_edits.clear();
        }
    });
    if n == 0 {
        ui.label(RichText::new("Draw with the measurement tools; annotations appear here.").size(12.0).color(TEXT_DIM));
        return;
    }
    let current = SliceKey::new(viewer.slices.axis, viewer.slices.index(viewer.slices.axis));
    let rows: Vec<AnnotationRow> = viewer
        .annotations()
        .iter()
        .map(|(id, key, a, name)| AnnotationRow {
            id,
            key,
            name: name.to_string(),
            summary: format!("{} · {}", a.kind(), a.label()),
            provenance: viewer.annotations().provenance(id).cloned().unwrap_or_default(),
        })
        .collect();
    for row in rows {
        match annotation_row(ui, &row, row.key == current, state) {
            RowAction::None => {}
            RowAction::GoTo => {
                viewer.go_to_annotation(row.id);
            }
            RowAction::Rename(name) => {
                viewer.rename_annotation(row.id, &name);
            }
            RowAction::Delete => {
                viewer.remove_annotation(row.id);
                state.name_edits.remove(&row.id);
            }
            RowAction::Review(status) => {
                let by = super::review_panel::reviewer(&state.review);
                if let Err(e) = viewer.decide(ferrum_domain::ReviewItem::Annotation(row.id), status, by) {
                    viewer.status.errors.push(e);
                }
            }
        }
    }
}

/// Snapshot of one annotation for the list (the viewer is borrowed
/// mutably while rows are drawn).
struct AnnotationRow {
    id: AnnotationId,
    key: SliceKey,
    name: String,
    summary: String,
    provenance: ferrum_domain::Provenance,
}

/// What the user did on one row.
enum RowAction {
    None,
    GoTo,
    Rename(String),
    Delete,
    Review(ferrum_domain::ReviewStatus),
}

fn annotation_row(ui: &mut egui::Ui, row: &AnnotationRow, on_current_slice: bool, state: &mut PanelState) -> RowAction {
    let mut action = RowAction::None;
    let plane = row.key.slice_axis().map(|a| a.label()).unwrap_or("?");
    let frame = egui::Frame::new()
        .fill(if on_current_slice { super::theme::ACCENT_SOFT } else { super::theme::SURFACE })
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(8, 6));
    // the frame's margins plus the two buttons and their spacing
    let inner_width = (ui.available_width() - 16.0).max(120.0);
    frame.show(ui, |ui| {
        ui.set_width(inner_width);
        ui.horizontal(|ui| {
            let buf = state.name_edits.entry(row.id).or_insert_with(|| row.name.clone());
            let name_width = (inner_width - 2.0 * super::widgets::TOOL - 3.0 * ui.spacing().item_spacing.x).max(60.0);
            let edit = ui.add(egui::TextEdit::singleline(buf).desired_width(name_width));
            edit.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Annotation name"));
            if edit.changed() && !buf.trim().is_empty() {
                action = RowAction::Rename(buf.clone());
            }
            if !edit.has_focus() && !edit.changed() {
                state.name_edits.remove(&row.id);
            }
            if tool_button(ui, icon::ARROW_SQUARE_OUT, &format!("Go to {}", row.name), false).clicked() {
                action = RowAction::GoTo;
            }
            if tool_button(ui, icon::TRASH, &format!("Delete {}", row.name), false).clicked() {
                action = RowAction::Delete;
            }
        });
        let color = if on_current_slice { ACCENT } else { TEXT_DIM };
        let info = ui.add(
            egui::Label::new(
                RichText::new(format!("{} · {plane} {}", row.summary, row.key.index + 1)).size(12.0).color(color),
            )
            .sense(egui::Sense::click()),
        );
        if info.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            action = RowAction::GoTo;
        }
        if let Some(review) = super::segments_panel::provenance_line(ui, &row.provenance, &row.name) {
            action = RowAction::Review(review);
        }
    });
    action
}

fn volume_settings(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    section_title(ui, icon::CUBE, "Technique");
    ui.horizontal_wrapped(|ui| {
        for m in RenderMode::ALL {
            if chip(ui, m.label(), viewer.volume.settings.mode == m).clicked() {
                viewer.volume.settings.mode = m;
            }
        }
    });
    let s = &mut viewer.volume.settings;
    match s.mode {
        RenderMode::Isosurface => {
            icon_slider(ui, icon::WAVEFORM, "Isosurface threshold", &mut s.iso_threshold, 0.0..=1.0);
        }
        RenderMode::Tissue => {
            let TissueThresholds { mut low, mut high, mut surface } = s.tissue;
            let mut changed = icon_slider(ui, icon::CARET_RIGHT, "Band start", &mut low, 0.0..=1.0).changed();
            changed |= icon_slider(ui, icon::CARET_DOWN, "Band end", &mut high, 0.0..=1.0).changed();
            changed |= icon_slider(ui, icon::WAVEFORM, "Surface", &mut surface, 0.0..=1.0).changed();
            if changed {
                s.tissue = TissueThresholds::new(low, high, surface);
            }
        }
        RenderMode::Mip | RenderMode::TransferFunction => {}
    }
    icon_slider(ui, icon::DROP, "Opacity", &mut s.opacity, 0.0..=1.0);
    icon_slider(ui, icon::SUN, "Brightness", &mut s.brightness, 0.0..=1.0);
    icon_slider(ui, icon::GAUGE, "Quality", &mut s.quality, 0.0..=1.0);
    if matches!(s.mode, RenderMode::Isosurface | RenderMode::Tissue) {
        ui.checkbox(&mut s.ambient_occlusion, "Ambient occlusion");
    }

    if viewer.volume.settings.mode == RenderMode::TransferFunction {
        section_title(ui, icon::PALETTE, "Transfer function");
        let hist = viewer.dataset().map(|d| d.histogram.normalized(true)).unwrap_or_default();
        let tf = viewer.volume.transfer_function().clone();
        if let Some(edited) = tf_editor::show(ui, &mut state.tf_editor, &tf, &hist) {
            viewer.set_transfer_function(edited);
        }
        ui.horizontal_wrapped(|ui| {
            if chip(ui, "Default", false).clicked() {
                viewer.set_transfer_function(TransferFunction::legacy_default());
            }
            if chip(ui, "Ramp", false).clicked() {
                viewer.set_transfer_function(TransferFunction::linear_ramp());
            }
            if let Some(range) = viewer.dataset().map(|d| d.volume.range()) {
                for p in CtPreset::ALL {
                    if chip(ui, p.label(), false).clicked() {
                        viewer.set_transfer_function(TransferFunction::ct_preset(p, range));
                    }
                }
            }
        });
    }

    ui.add_space(4.0);
    collapsible(ui, "clipping", icon::SCISSORS, "Clipping", true, |ui| {
        let clip = &mut viewer.volume.clip;
        icon_slider(ui, icon::SCISSORS, "Cut (view)", &mut clip.view_cut, 0.0..=1.0);
        icon_slider(
            ui,
            icon::SQUARE_HALF,
            "Cut plane opacity",
            &mut viewer.volume.settings.cut_surface_opacity,
            0.0..=1.0,
        );
        let ClipBox { mut min, mut max } = clip.clip_box;
        for (i, name) in ["X", "Y", "Z"].iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(*name).color(TEXT_DIM).monospace());
                ui.spacing_mut().slider_width = 100.0;
                ui.add(Slider::new(&mut min[i], 0.0..=1.0).show_value(false).trailing_fill(false));
                ui.add(Slider::new(&mut max[i], 0.0..=1.0).show_value(false));
            });
        }
        clip.clip_box = ClipBox { min, max }.normalized();
        ui.checkbox(&mut clip.plane.enabled, "Oblique plane");
        ui.add_enabled_ui(clip.plane.enabled, |ui| {
            let pi = std::f32::consts::PI;
            icon_slider(ui, icon::ARROWS_CLOCKWISE, "Azimuth", &mut clip.plane.azimuth, -pi..=pi);
            icon_slider(ui, icon::ARROW_U_UP_LEFT, "Elevation", &mut clip.plane.elevation, -1.57..=1.57);
            icon_slider(ui, icon::ARROWS_IN, "Offset", &mut clip.plane.distance, -1.0..=1.0);
            ui.checkbox(&mut clip.plane.flip, "Flip side");
        });
        if ui.button(format!("{} Reset clipping", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
            *clip = ferrum_domain::ClipSettings::default();
        }
    });

    if viewer.view_mode() == ViewMode::Volume3d {
        eraser(ui, viewer);
    }

    ui.add_space(6.0);
    segmentation_panel::volume_hint(ui, viewer);
    segments_section(ui, viewer, state);

    collapsible(ui, "perf", icon::LIGHTNING, "Performance", false, |ui| {
        ui.checkbox(&mut viewer.volume.settings.empty_space_skipping, "Empty-space skipping");
        icon_slider(ui, icon::GAUGE, "Resolution while rotating", &mut viewer.volume.interactive_scale, 0.25..=1.0);
    });
}

/// The volume eraser (3D view only: the toolbar there shows its state).
fn eraser(ui: &mut egui::Ui, viewer: &mut Viewer) {
    collapsible(ui, "eraser", icon::ERASER, "Volume eraser", viewer.volume.eraser_enabled, |ui| {
        ui.checkbox(&mut viewer.volume.eraser_enabled, "Erase with the left mouse button");
        icon_slider(ui, icon::CIRCLE_DASHED, "Radius, mm", &mut viewer.volume.brush.radius_mm, 1.0..=50.0);
        icon_slider(ui, icon::ARROWS_IN, "Depth, mm", &mut viewer.volume.brush.depth_mm, 1.0..=100.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    viewer.erase_history_len() > 0,
                    egui::Button::new(format!("{} Undo", icon::ARROW_U_UP_LEFT)),
                )
                .clicked()
            {
                viewer.undo_erase();
            }
            if ui
                .add_enabled(viewer.mask().is_some(), egui::Button::new(format!("{} Restore all", icon::BROOM)))
                .clicked()
            {
                viewer.reset_mask();
            }
        });
    });
}

fn details(ui: &mut egui::Ui, viewer: &Viewer) {
    let Some(d) = viewer.dataset() else {
        return;
    };
    section_title(ui, icon::INFO, "Series information");
    let v = &d.volume;
    let dims = v.dims();
    let sp = v.spacing();
    let rows = [
        ("Dimensions".to_string(), format!("{} × {} × {}", dims.x, dims.y, dims.z)),
        ("Spacing".to_string(), format!("{:.3} × {:.3} × {:.3} mm", sp.x, sp.y, sp.z)),
        ("Intensity range".to_string(), format!("{:.1} … {:.1}", v.range().min, v.range().max)),
    ];
    egui::Grid::new("details").num_columns(2).spacing([12.0, 6.0]).striped(false).show(ui, |ui| {
        for (k, val) in rows.iter().chain(d.metadata.attributes.iter()) {
            ui.label(RichText::new(k).color(TEXT_DIM));
            ui.add(egui::Label::new(RichText::new(val).color(TEXT)).wrap());
            ui.end_row();
        }
    });
}
