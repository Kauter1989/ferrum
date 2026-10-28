//! # ferrum
//!
//! Presentation layer: the eframe desktop application, a shell (header,
//! start screen, studies, dialogs, shortcuts) around the views and panels of
//! `ferrum-ui`.

pub mod app;
pub mod ui;

pub use ferrum_ui::gpu_bridge;

pub use app::ViewerApp;
