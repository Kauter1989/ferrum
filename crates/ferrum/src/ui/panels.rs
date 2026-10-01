//! Floating settings panel. Tools, modes and orientations live in the docks
//! (see `app.rs`); this panel holds the continuous parameters.

use egui::{RichText, Slider};
use egui_phosphor::light as icon;
use ferrum_app::{FilterKind, ViewMode, Viewer};
use ferrum_domain::{
    ClipBox, CtPreset, RenderMode, SliceAxis, TissueThresholds, TransferFunction, WindowLevel, WindowPreset,
};

use super::tf_editor::{self, TfEditorState};
use super::theme::{HUD, TEXT_DIM};
use super::widgets::{chip, icon_slider, section_title};

/// Which parameter group the panel shows in MPR mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelTab {
    /// Slice parameters.
    #[default]
    Slices,
    /// Volume rendering parameters.
    Volume,
}

/// Persistent UI-only state of the panel.
#[derive(Debug, Clone)]
pub struct PanelState {
    /// Transfer function editor state.
    pub tf_editor: TfEditorState,
    /// Sigma of the Gaussian filter.
    pub gaussian_sigma: f32,
    /// Tab shown in MPR mode.
    pub tab: PanelTab,
}

impl Default for PanelState {
    fn default() -> Self {
        Self { tf_editor: TfEditorState::default(), gaussian_sigma: 1.0, tab: PanelTab::default() }
    }
}

fn collapsible(ui: &mut egui::Ui, id: &str, icon_str: &str, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    let header = RichText::new(format!("{icon_str}  {}", title.to_uppercase())).size(11.5).strong().color(TEXT_DIM);
    egui::CollapsingHeader::new(header).id_salt(id).default_open(open).show(ui, body);
}

/// Draws the panel contents.
pub fn show(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    if viewer.dataset().is_none() {
        return;
    }
    let tab = match viewer.view_mode {
        ViewMode::Slice2d => PanelTab::Slices,
        ViewMode::Volume3d => PanelTab::Volume,
        ViewMode::Mpr => {
            ui.horizontal(|ui| {
                if chip(ui, &format!("{}  Slices", icon::SQUARES_FOUR), state.tab == PanelTab::Slices).clicked() {
                    state.tab = PanelTab::Slices;
                }
                if chip(ui, &format!("{}  Volume", icon::CUBE), state.tab == PanelTab::Volume).clicked() {
                    state.tab = PanelTab::Volume;
                }
            });
            ui.add_space(4.0);
            state.tab
        }
    };
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| match tab {
        PanelTab::Slices => slice_settings(ui, viewer, state),
        PanelTab::Volume => volume_settings(ui, viewer, state),
    });
}

fn slice_settings(ui: &mut egui::Ui, viewer: &mut Viewer, state: &mut PanelState) {
    let Some(volume) = viewer.dataset().map(|d| d.volume.clone()) else {
        return;
    };
    section_title(ui, icon::CIRCLE_HALF, "Window");
    let range = volume.range();
    let WindowLevel { mut center, mut width } = viewer.slices.window;
    let speed = range.span() / 500.0;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new("L").color(TEXT_DIM));
        changed |= ui.add(egui::DragValue::new(&mut center).speed(speed).max_decimals(0)).changed();
        ui.label(RichText::new("W").color(TEXT_DIM));
        changed |=
            ui.add(egui::DragValue::new(&mut width).speed(speed).range(0.001..=f32::MAX).max_decimals(0)).changed();
    });
    if changed {
        viewer.slices.window = WindowLevel::new(center, width);
    }
    ui.horizontal_wrapped(|ui| {
        for p in WindowPreset::ALL {
            if chip(ui, p.label(), false).clicked() {
                viewer.apply_window_preset(p);
            }
        }
        if chip(ui, &format!("{} Auto", icon::SPARKLE), false).clicked() {
            viewer.auto_window();
        }
    });

    section_title(ui, icon::STACK, "Slices");
    let axes: Vec<SliceAxis> =
        if viewer.view_mode == ViewMode::Mpr { SliceAxis::ALL.to_vec() } else { vec![viewer.slices.axis] };
    for axis in axes {
        let n = axis.slice_count(&volume);
        let mut idx = viewer.slices.index(axis);
        ui.horizontal(|ui| {
            ui.label(RichText::new(&axis.label()[..1]).color(super::slice_view::axis_color(axis)).strong());
            if ui.add(Slider::new(&mut idx, 0..=n.saturating_sub(1))).changed() {
                viewer.set_slice_index(axis, idx);
            }
        });
    }
    ui.checkbox(&mut viewer.slices.nearest, "Nearest-neighbour sampling");

    section_title(ui, icon::RULER, "Annotations");
    let n = viewer.annotations().len();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{n} on all slices")).color(TEXT_DIM));
        if ui.add_enabled(n > 0, egui::Button::new(format!("{} Clear all", icon::TRASH))).clicked() {
            viewer.clear_annotations();
        }
    });
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
            .color(HUD),
        );
    }

    collapsible(ui, "filters", icon::FUNNEL, "Filters", false, |ui| {
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
                ui.spacing_mut().slider_width = 110.0;
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

    collapsible(ui, "perf", icon::LIGHTNING, "Performance", false, |ui| {
        ui.checkbox(&mut viewer.volume.settings.empty_space_skipping, "Empty-space skipping");
        icon_slider(ui, icon::GAUGE, "Resolution while rotating", &mut viewer.volume.interactive_scale, 0.25..=1.0);
    });
}
