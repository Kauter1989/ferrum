//! [`ViewerWidget`]: the views of a [`Viewer`] (2D, MPR or 3D, as the
//! viewer's view mode says) in any `egui::Ui`, with the optional tool bar.

use egui::{Rect, UiBuilder, Vec2};
use ferrum_app::{GpuSyncState, ViewMode, Viewer};
use ferrum_domain::SliceAxis;
use ferrum_render::gpu::VolumeRenderer;

use crate::gpu_bridge::RendererSink;
use crate::slice_view::{self, SliceViewState};
use crate::volume_view::{self, VolumeViewState};

/// Height of the tool bar row when the widget draws it.
const TOOLBAR_HEIGHT: f32 = 48.0;
/// Width of the view-mode switch at the start of the widget's tool bar.
const MODE_SWITCH_WIDTH: f32 = 150.0;

/// The views of one [`Viewer`]; keep one per window, next to the viewer.
pub struct ViewerWidget {
    slice_states: [SliceViewState; 3],
    volume_state: VolumeViewState,
    sync: GpuSyncState,
    toolbar: bool,
    theme: bool,
    icon_font: bool,
    styled: bool,
    /// The egui pass in which [`ViewerWidget::begin_frame`] last ran, and
    /// whether it uploaded to the GPU.
    prepared: Option<(u64, bool)>,
}

impl Default for ViewerWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ViewerWidget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewerWidget")
            .field("toolbar", &self.toolbar)
            .field("theme", &self.theme)
            .finish_non_exhaustive()
    }
}

impl ViewerWidget {
    /// A widget with the tool bar and FERRUM's theme.
    pub fn new() -> Self {
        Self {
            slice_states: Default::default(),
            volume_state: VolumeViewState::default(),
            sync: GpuSyncState::default(),
            toolbar: true,
            theme: true,
            icon_font: true,
            styled: false,
            prepared: None,
        }
    }

    /// Whether [`ViewerWidget::show`] draws the tool bar above the views
    /// (default). A host that places it elsewhere turns it off and calls
    /// [`crate::toolbar::show`] itself.
    pub fn with_toolbar(mut self, on: bool) -> Self {
        self.toolbar = on;
        self
    }

    /// Whether the widget applies FERRUM's dark colours to the context on
    /// its first frame (default). A host with its own style turns it off;
    /// the host's fonts are kept either way.
    pub fn with_theme(mut self, on: bool) -> Self {
        self.theme = on;
        self
    }

    /// Whether the widget adds its icon font to the context on its first
    /// frame (default; the tool bar and panels need it). Off only for a host
    /// that installs it itself, as the desktop app does with
    /// [`crate::theme::install`].
    pub fn with_icon_font(mut self, on: bool) -> Self {
        self.icon_font = on;
        self
    }

    /// Applies finished background work of `viewer`, asks for another frame
    /// while work is running, and uploads what changed to the GPU
    /// (`render_state` absent: nothing is uploaded, as in a test without a
    /// GPU). [`ViewerWidget::show`] calls it unless the host did earlier in
    /// the frame (e.g. before drawing panels that show the viewer's status);
    /// the viewer is polled once per frame, and the GPU synced once a render
    /// state is given.
    pub fn begin_frame(
        &mut self,
        ctx: &egui::Context,
        viewer: &mut Viewer,
        render_state: Option<&egui_wgpu::RenderState>,
    ) {
        let pass = ctx.cumulative_pass_nr();
        if self.prepared.is_none_or(|(p, _)| p != pass) {
            if !self.styled {
                if self.icon_font {
                    crate::theme::add_icon_font(ctx);
                }
                if self.theme {
                    crate::theme::visuals(ctx);
                }
                self.styled = true;
            }
            viewer.poll();
            // also while no study is shown yet: a load in progress needs frames to finish
            if viewer.is_loading() || viewer.is_computing() || viewer.ai().is_busy() || viewer.volume.interacting {
                ctx.request_repaint();
            }
        }
        let synced = self.prepared.is_some_and(|(p, synced)| p == pass && synced);
        self.prepared = Some((pass, synced || render_state.is_some()));
        if synced {
            return;
        }
        let Some(rs) = render_state else { return };
        let mut guard = rs.renderer.write();
        if let Some(renderer) = guard.callback_resources.get_mut::<VolumeRenderer>() {
            let mut sink = RendererSink { device: &rs.device, queue: &rs.queue, renderer };
            viewer.sync_gpu(&mut self.sync, &mut sink);
        }
    }

    /// Draws the views of `viewer` into `ui`, filling it: one slice view, the
    /// MPR quad (axial, 3D, coronal, sagittal) or the 3D view. Nothing is
    /// drawn before a study is loaded.
    pub fn show(&mut self, ui: &mut egui::Ui, viewer: &mut Viewer, render_state: Option<&egui_wgpu::RenderState>) {
        self.begin_frame(&ui.ctx().clone(), viewer, render_state);
        if viewer.dataset().is_none() {
            return;
        }
        if self.toolbar {
            let full = ui.available_rect_before_wrap();
            let bar = Rect::from_min_size(full.min, Vec2::new(full.width(), TOOLBAR_HEIGHT));
            let (modes, tools) = bar.split_left_right_at_x(bar.min.x + MODE_SWITCH_WIDTH);
            ui.scope_builder(UiBuilder::new().max_rect(modes), |ui| crate::toolbar::mode_switch(ui, viewer));
            ui.scope_builder(UiBuilder::new().max_rect(tools), |ui| crate::toolbar::show(ui, viewer));
            ui.allocate_rect(bar, egui::Sense::hover());
        }
        self.views(ui, viewer);
    }

    fn views(&mut self, ui: &mut egui::Ui, viewer: &mut Viewer) {
        match viewer.view_mode() {
            ViewMode::Slice2d => {
                let axis = viewer.slices.axis;
                slice_view::show(ui, viewer, axis, &mut self.slice_states[axis.normal_axis()]);
            }
            ViewMode::Volume3d => volume_view::show(ui, viewer, &mut self.volume_state),
            ViewMode::Mpr => {
                let full = ui.available_rect_before_wrap();
                let gap = 8.0;
                let half = (full.size() - Vec2::splat(gap)) * 0.5;
                let cell = |col: f32, row: f32| {
                    Rect::from_min_size(full.min + Vec2::new(col * (half.x + gap), row * (half.y + gap)), half)
                };
                let layout = [
                    (cell(0.0, 0.0), Some(SliceAxis::Axial)),
                    (cell(1.0, 0.0), None),
                    (cell(0.0, 1.0), Some(SliceAxis::Coronal)),
                    (cell(1.0, 1.0), Some(SliceAxis::Sagittal)),
                ];
                for (rect, axis) in layout {
                    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| match axis {
                        Some(a) => slice_view::show(ui, viewer, a, &mut self.slice_states[a.normal_axis()]),
                        None => volume_view::show(ui, viewer, &mut self.volume_state),
                    });
                }
                ui.allocate_rect(full, egui::Sense::hover());
            }
        }
    }
}
