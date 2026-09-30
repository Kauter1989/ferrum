//! # dicom_renderer
//!
//! Presentation layer: an eframe/egui desktop application on top of
//! `mri-app`. GPU work goes through egui-wgpu paint callbacks that drive
//! `mri_render::gpu::VolumeRenderer`.

pub mod app;
pub mod gpu_bridge;
pub mod ui;

pub use app::ViewerApp;
