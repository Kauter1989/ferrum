//! # ferrum
//!
//! Presentation layer: an eframe/egui desktop application on top of
//! `ferrum-app`. GPU work goes through egui-wgpu paint callbacks that drive
//! `ferrum_render::gpu::VolumeRenderer`.

pub mod app;
pub mod gpu_bridge;
pub mod ui;

pub use app::ViewerApp;
