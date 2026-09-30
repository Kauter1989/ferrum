//! Settings side panel for 2D and 3D modes.

use egui::{RichText, Slider};
use mri_app::{FilterKind, ToolKind, ViewMode, Viewer};
use mri_domain::{ClipBox, RenderMode, SliceAxis, TissueThresholds, TransferFunction, WindowLevel, WindowPreset};

use super::tf_editor::{self, TfEditorState};

/// Persistent UI-only state of the side panel.
#[derive(Debug, Clone)]
pub struct PanelState {
    /// Transfer function editor state.
    pub tf_editor: TfEditorState,
    /// Sigma of the Gaussian filter.
    pub gaussian_sigma: f32,
}

impl Default for PanelState {
    fn default() -> Self {
        Self { tf_editor: TfEditorState::default(), gaussian_sigma: 1.0 }
    }
}

fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong()).default_open(open).show(ui, body);
}

/// Draws the side panel contents.
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    if viewer.dataset().is_none() {
        ui.label("Load data to see settings.");
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| match viewer.view_mode {
        ViewMode::Slice2d => {
            slice_settings(ui, viewer, true);
            tools(ui, viewer);
            filters(ui, viewer, state);
        }
        ViewMode::Volume3d => volume_settings(ui, viewer, state),
        ViewMode::Mpr => {
            slice_settings(ui, viewer, false);
            tools(ui, viewer);
            volume_settings(ui, viewer, state);
        }
    });
}

fn slice_settings(ui: &mut egui::Ui, viewer: &mut Viewer, with_axis: bool) {
    section(ui, "Slices", true, |ui| {
        if with_axis {
            ui.horizontal(|ui| {
                for axis in SliceAxis::ALL {
                    if ui.selectable_label(viewer.slices.axis == axis, axis.label()).clicked() {
                        viewer.slices.axis = axis;
                    }
                }
            });
        }
        let Some(volume) = viewer.dataset().map(|d| d.volume.clone()) else {
            return;
        };
        let axes: Vec<SliceAxis> = if with_axis { vec![viewer.slices.axis] } else { SliceAxis::ALL.to_vec() };
        for axis in axes {
            let n = axis.slice_count(&volume);
            let mut idx = viewer.slices.index(axis);
            if ui.add(Slider::new(&mut idx, 0..=n.saturating_sub(1)).text(axis.label())).changed() {
                viewer.set_slice_index(axis, idx);
            }
        }
        ui.separator();
        let range = volume.range();
        let WindowLevel { mut center, mut width } = viewer.slices.window;
        let speed = range.span() / 500.0;
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Level");
            changed |= ui.add(egui::DragValue::new(&mut center).speed(speed)).changed();
            ui.label("Width");
            changed |= ui.add(egui::DragValue::new(&mut width).speed(speed).range(0.001..=f32::MAX)).changed();
        });
        if changed {
            viewer.slices.window = WindowLevel::new(center, width);
        }
        ui.horizontal_wrapped(|ui| {
            for p in WindowPreset::ALL {
                if ui.small_button(p.label()).clicked() {
                    viewer.apply_window_preset(p);
                }
            }
            if ui.small_button("Auto").clicked() {
                viewer.auto_window();
            }
        });
        ui.checkbox(&mut viewer.slices.nearest, "Nearest-neighbour sampling");
    });
}

fn tools(ui: &mut egui::Ui, viewer: &mut Viewer) {
    section(ui, "Tools", true, |ui| {
        ui.horizontal_wrapped(|ui| {
            for t in ToolKind::ALL {
                if ui.selectable_label(viewer.tool == t, t.label()).on_hover_text(t.hint()).clicked() {
                    viewer.select_tool(t);
                }
            }
        });
        ui.label(RichText::new(viewer.tool.hint()).small().weak());
        if let Some(p) = viewer.probe {
            ui.label(format!("Voxel ({}, {}, {}) = {:.1}", p.voxel.x, p.voxel.y, p.voxel.z, p.value));
        }
        let n = viewer.annotations().len();
        ui.horizontal(|ui| {
            ui.label(format!("{n} annotation(s)"));
            if ui.add_enabled(n > 0, egui::Button::new("Clear all")).clicked() {
                viewer.clear_annotations();
            }
        });
        ui.label(RichText::new("Scroll: slice · Ctrl+scroll: zoom · MPR: right-click to navigate").small().weak());
    });
}

fn filters(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    section(ui, "Filters", false, |ui| {
        let busy = viewer.is_computing();
        ui.horizontal(|ui| {
            ui.add(Slider::new(&mut state.gaussian_sigma, 0.5..=3.0).text("σ"));
            if ui.add_enabled(!busy, egui::Button::new("Smooth")).clicked() {
                viewer.apply_filter(FilterKind::Gaussian(state.gaussian_sigma));
            }
        });
        if ui.add_enabled(!busy, egui::Button::new("Sobel edges")).clicked() {
            viewer.apply_filter(FilterKind::Sobel);
        }
    });
}

fn volume_settings(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    section(ui, "Rendering", true, |ui| {
        ui.horizontal_wrapped(|ui| {
            for m in RenderMode::ALL {
                ui.selectable_value(&mut viewer.volume.settings.mode, m, m.label());
            }
        });
        let s = &mut viewer.volume.settings;
        match s.mode {
            RenderMode::Isosurface => {
                ui.add(Slider::new(&mut s.iso_threshold, 0.0..=1.0).text("Isosurface"));
            }
            RenderMode::Tissue => {
                let TissueThresholds { mut low, mut high, mut surface } = s.tissue;
                let mut changed = ui.add(Slider::new(&mut low, 0.0..=1.0).text("Band start")).changed();
                changed |= ui.add(Slider::new(&mut high, 0.0..=1.0).text("Band end")).changed();
                changed |= ui.add(Slider::new(&mut surface, 0.0..=1.0).text("Surface")).changed();
                if changed {
                    s.tissue = TissueThresholds::new(low, high, surface);
                }
            }
            RenderMode::Mip | RenderMode::TransferFunction => {}
        }
        ui.add(Slider::new(&mut s.opacity, 0.0..=1.0).text("Opacity"));
        ui.add(Slider::new(&mut s.brightness, 0.0..=1.0).text("Brightness"));
        ui.add(Slider::new(&mut s.quality, 0.0..=1.0).text("Quality"));
        if matches!(s.mode, RenderMode::Isosurface | RenderMode::Tissue) {
            ui.checkbox(&mut s.ambient_occlusion, "Ambient occlusion");
        }
    });
    if viewer.volume.settings.mode == RenderMode::TransferFunction || viewer.view_mode == ViewMode::Volume3d {
        section(ui, "Transfer function", viewer.volume.settings.mode == RenderMode::TransferFunction, |ui| {
            let hist = viewer.dataset().map(|d| d.histogram.normalized(true)).unwrap_or_default();
            let tf = viewer.volume.transfer_function().clone();
            if let Some(edited) = tf_editor::show(ui, &mut state.tf_editor, &tf, &hist) {
                viewer.set_transfer_function(edited);
            }
            ui.horizontal(|ui| {
                ui.label("Presets:");
                if ui.small_button("Default").clicked() {
                    viewer.set_transfer_function(TransferFunction::legacy_default());
                }
                if ui.small_button("Ramp").clicked() {
                    viewer.set_transfer_function(TransferFunction::linear_ramp());
                }
                if ui.small_button("Bone").clicked() {
                    viewer.set_transfer_function(TransferFunction::bone(0.45));
                }
            });
        });
    }
    section(ui, "Clipping", false, |ui| {
        let clip = &mut viewer.volume.clip;
        ui.add(Slider::new(&mut clip.view_cut, 0.0..=1.0).text("Cut (view)"));
        ui.add(Slider::new(&mut viewer.volume.settings.cut_surface_opacity, 0.0..=1.0).text("Cut plane opacity"));
        ui.label("Clip box");
        let ClipBox { mut min, mut max } = clip.clip_box;
        for (i, name) in ["X", "Y", "Z"].iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(*name);
                ui.add(Slider::new(&mut min[i], 0.0..=1.0).show_value(false));
                ui.add(Slider::new(&mut max[i], 0.0..=1.0).show_value(false));
            });
        }
        clip.clip_box = ClipBox { min, max }.normalized();
        ui.checkbox(&mut clip.plane.enabled, "Oblique plane");
        ui.add_enabled_ui(clip.plane.enabled, |ui| {
            ui.add(Slider::new(&mut clip.plane.azimuth, -std::f32::consts::PI..=std::f32::consts::PI).text("Azimuth"));
            ui.add(Slider::new(&mut clip.plane.elevation, -1.57..=1.57).text("Elevation"));
            ui.add(Slider::new(&mut clip.plane.distance, -1.0..=1.0).text("Offset"));
            ui.checkbox(&mut clip.plane.flip, "Flip side");
        });
        if ui.button("Reset clipping").clicked() {
            *clip = mri_domain::ClipSettings::default();
        }
    });
    section(ui, "Volume eraser", false, |ui| {
        ui.checkbox(&mut viewer.volume.eraser_enabled, "Erase with left mouse button");
        ui.add(Slider::new(&mut viewer.volume.brush.radius_mm, 1.0..=50.0).text("Radius, mm"));
        ui.add(Slider::new(&mut viewer.volume.brush.depth_mm, 1.0..=100.0).text("Depth, mm"));
        ui.horizontal(|ui| {
            if ui.add_enabled(viewer.erase_history_len() > 0, egui::Button::new("Undo")).clicked() {
                viewer.undo_erase();
            }
            if ui.add_enabled(viewer.mask().is_some(), egui::Button::new("Restore all")).clicked() {
                viewer.reset_mask();
            }
        });
    });
    section(ui, "View & performance", false, |ui| {
        if ui.button("Reset camera").clicked() {
            viewer.reset_view_3d();
        }
        ui.checkbox(&mut viewer.volume.settings.empty_space_skipping, "Empty-space skipping");
        ui.add(Slider::new(&mut viewer.volume.interactive_scale, 0.25..=1.0).text("Resolution while rotating"));
        ui.label(
            RichText::new("Drag: rotate · right/middle drag: pan · scroll: zoom · double-click: reset").small().weak(),
        );
    });
}
