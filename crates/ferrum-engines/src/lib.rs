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
//!   conformance tests and the `mock_server` example;
//! * [`gpu`] — GPU groups: the lease an engine call of a shared card takes
//!   (`ferrum-gpu-lock/1`, ADR 0015);
//! * [`ocr`] — the optional OCR bridge kind (Clean Pixel Data, Stage 31):
//!   an out-of-process burned-in text check, skipped and reported without
//!   a bridge.

pub mod gpu;
pub mod http;
pub mod mock;
pub mod ocr;
pub mod server;
pub mod wire;

pub use http::{HttpConfig, HttpEngine, HttpSession};
pub use mock::{MockEngine, MockParams, MockSession};
pub use ocr::{OcrBridge, OcrOutcome, OcrRegion};
pub use server::EngineServer;
