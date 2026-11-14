//! # ferrum-ui
//!
//! FERRUM's viewer as an egui/wgpu widget ([ADR 0016], [design]): the 2D,
//! MPR and 3D views of a [`ferrum_app::Viewer`] with their overlays,
//! measurements and segments, the tool bar, and the panels (display,
//! segments, AI segmentation, review, metrics). It has no window, menus or
//! file dialogs: the desktop app (`ferrum`) and host applications such as
//! Medical Harness place it in their own layout.
//!
//! A host:
//! 1. calls [`install`] once with its `egui_wgpu::RenderState`;
//! 2. keeps a [`Viewer`](ferrum_app::Viewer) and a [`ViewerWidget`];
//! 3. calls [`ViewerWidget::show`] in the panel where the views go, each
//!    frame, and the panels' `show` functions where it wants them.
//!
//! GPU work goes through egui-wgpu paint callbacks that drive
//! `ferrum_render::gpu::VolumeRenderer`; the image-formation model stays in
//! `ferrum-render`. A host uses the egui, egui-wgpu and wgpu versions this
//! crate is built with (egui 0.36, wgpu 30), re-exported here.
//!
//! [ADR 0016]: https://github.com/Kauter1989/ferrum/blob/develop/docs/decisions/0016-ferrum-ui-crate.md
//! [design]: https://github.com/Kauter1989/ferrum/blob/develop/docs/embeddable-viewer.md

pub use egui;
pub use egui_wgpu;
pub use wgpu;

pub mod ai_panel;
pub mod gpu_bridge;
pub mod metrics_card;
pub mod panels;
pub mod review_panel;
pub mod segmentation_panel;
pub mod segments_panel;
pub mod slice_view;
pub mod tf_editor;
pub mod theme;
pub mod toolbar;
pub mod volume_view;
pub mod widget;
pub mod widgets;

pub use gpu_bridge::install;
pub use widget::ViewerWidget;
