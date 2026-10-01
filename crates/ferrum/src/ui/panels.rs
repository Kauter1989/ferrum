//! Right-hand settings panel with three tabs: image (window, slices,
//! annotations, filters), 3D (technique, transfer function, clipping,
//! eraser) and details (series information).

use egui::{RichText, Slider};
use egui_phosphor::light as icon;
use ferrum_app::{FilterKind, ViewMode, Viewer};
use ferrum_domain::{
    ClipBox, CtPreset, RenderMode, SliceAxis, TissueThresholds, TransferFunction, WindowLevel, WindowPreset,
};

use super::tf_editor::{self, TfEditorState};
use super::theme::{OVERLAY, TEXT, TEXT_DIM};
use super::widgets::{chip, icon_slider, section_title, segmented, slider_row};

/// Tab of the settings panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelTab {
    /// Window/level, slices, annotations and filters.
    #[default]
    Image,
    /// Volume rendering parameters.
    Volume,
    /// Series information.
    Details,
}

impl PanelTab {
    const ALL: [PanelTab; 3] = [PanelTab::Image, PanelTab::Volume, PanelTab::Details];

    fn label(self) -> &'static str {
        match self {
            PanelTab::Image => "Image",
            PanelTab::Volume => "Volume",
            PanelTab::Details => "Details",
        }
    }
}

/// Persistent UI-only state of the panel.
#[derive(Debug, Clone)]
pub struct PanelState {
    /// Transfer function editor state.
    pub tf_editor: TfEditorState,
    /// Sigma of the Gaussian filter.
    pub gaussian_sigma: f32,
    /// Selected tab.
    pub tab: PanelTab,
    /// View mode seen on the previous frame; switching to 2D or 3D selects
    /// the matching tab.
    last_mode: Option<ViewMode>,
}

impl Default for PanelState {
    fn default() -> Self {
        Self { tf_editor: TfEditorState::default(), gaussian_sigma: 1.0, tab: PanelTab::default(), last_mode: None }
    }
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
    if state.last_mode != Some(viewer.view_mode) {
        match viewer.view_mode {
            ViewMode::Slice2d => state.tab = PanelTab::Image,
            ViewMode::Volume3d => state.tab = PanelTab::Volume,
            ViewMode::Mpr => {}
        }
        state.last_mode = Some(viewer.view_mode);
    }
    let labels = PanelTab::ALL.map(PanelTab::label);
    let selected = PanelTab::ALL.iter().position(|t| *t == state.tab);
    if let Some(i) = segmented(ui, &labels, selected) {
        state.tab = PanelTab::ALL[i];
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
        if viewer.view_mode == ViewMode::Mpr { SliceAxis::ALL.to_vec() } else { vec![viewer.slices.axis] };
    for axis in axes {
        let n = axis.slice_count(&volume);
        let mut idx = viewer.slices.index(axis);
        ui.horizontal(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(8.0, 16.0), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 4.0, super::slice_view::axis_color(axis));
            ui.label(RichText::new(axis.label()).color(TEXT_DIM));
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
            .color(OVERLAY),
        );
    }

    ui.add_space(4.0);
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
