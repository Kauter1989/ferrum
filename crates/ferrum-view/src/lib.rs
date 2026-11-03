//! Local view host for a harness: `ferrum-view/1`.
//!
//! A harness shows FERRUM next to its conversation without linking Rust into
//! its GUI. This crate keeps a [`Viewer`](ferrum_app::Viewer) alive for one
//! workspace, draws its slices on the local GPU (or with the CPU reference
//! renderer when there is no adapter) and serves them over loopback together
//! with a small self-contained page:
//!
//! ```text
//! GET  /                the page (HTML, inline CSS and JS, no other origin)
//! GET  /api/state       ferrum-view/1 state: plane, slice, window, segments, probe
//! GET  /api/frame.png   the slice at the volume's native resolution, one pixel per voxel
//! GET  /api/events      what the person did, with sequence numbers
//! POST /api/input       slice, step, plane, window, preset, segments or probe
//! ```
//!
//! The host binds `127.0.0.1` only, and every request needs its token — the
//! one printed by `ferrum-cli view serve`, which is how a harness starts it:
//!
//! ```no_run
//! use ferrum_view::{BackendPreference, ServeOptions, Session, ViewServer};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let session = Session::open(std::path::Path::new("ws/ct1"), BackendPreference::Auto)?;
//! let mut server = ViewServer::start(session, ServeOptions::default())?;
//! println!("{}", server.url());
//! server.wait();
//! # Ok(())
//! # }
//! ```
//!
//! Every operation is a use case of `ferrum-app`: this crate contains no
//! business logic, and the shader and the CPU reference it draws with live in
//! `ferrum-render`. The protocol is specified in
//! [`docs/view-host.md`](../../../docs/view-host.md).

mod cpu;
mod frame;
mod gpu;
mod page;
mod server;
mod session;
mod state;

pub use cpu::CpuFrameSource;
pub use frame::{Backend, BackendPreference, FrameSource, RawFrame};
pub use gpu::GpuFrameSource;
pub use server::{ServeOptions, ViewServer, COOKIE, MAX_BODY};
pub use session::{Frame, Session, ViewError};
pub use state::{plane_name, Input, Probe, ViewState, FORMAT};
