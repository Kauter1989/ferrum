//! The self-contained page the host serves at `/`.
//!
//! One HTML document with inline CSS and JavaScript and no request to any
//! other origin (criterion 25.4-a): it draws the frame, turns the wheel, a
//! drag and a click into `POST /api/input`, and shows the slice, the window
//! and the probe readout.

/// The whole page.
pub(crate) const PAGE: &str = include_str!("page.html");
