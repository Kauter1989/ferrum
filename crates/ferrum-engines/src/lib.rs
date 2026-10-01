//! # ferrum-engines
//!
//! Implementations of the segmentation engine port
//! ([`ferrum_domain::SegmentationEngine`], ADR 0007):
//!
//! * [`HttpEngine`] — client of the FERRUM Engine Protocol
//!   (`ferrum-engine/1`, `docs/engine-protocol.md`), the one way FERRUM
//!   talks to out-of-process engines such as nnInteractive, MONAI Label or
//!   TotalSegmentator through their bridges;
//! * [`MockEngine`] — deterministic in-process engine without a model, for
//!   tests and demos;
//! * [`server`] — reference protocol server for any engine, used by the
//!   conformance tests and the `mock_server` example.

pub mod http;
pub mod mock;
pub mod server;
pub mod wire;

pub use http::{HttpConfig, HttpEngine, HttpSession};
pub use mock::{MockEngine, MockParams, MockSession};
pub use server::EngineServer;
