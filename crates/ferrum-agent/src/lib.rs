//! # ferrum-agent
//!
//! FERRUM as an agent skill (`docs/agent-skill.md`): headless commands for
//! AI agents in medical harnesses. Every command takes a JSON object of
//! parameters and returns a `ferrum-agent/1` envelope:
//!
//! ```json
//! { "api": "ferrum-agent/1", "ok": true, "data": { … }, "warnings": [], "provenance": { … } }
//! ```
//!
//! - Work happens in a *workspace* (`ferrum-workspace` v1) that references
//!   and hashes the source data; results are saved there with provenance.
//! - Everything an agent creates is *proposed* until a person confirms it.
//! - The operator configuration decides what may be read and what may
//!   leave FERRUM; identifiers are withheld by default.
//! - Images are rendered on the CPU with a pixel→voxel→patient mapping;
//!   numbers come from `probe`, `stats` and `measure`, not from images.
//!
//! The `ferrum-cli` binary exposes the same commands on the command line
//! and as an MCP server over stdio ([`mcp`]).

pub mod agent;
pub mod commands;
pub mod config;
pub mod envelope;
pub mod evals;
pub mod mcp;
pub mod params;
pub mod points;
pub mod schema;
pub mod study;

pub use agent::Agent;
pub use config::AgentConfig;
pub use envelope::{AgentError, ErrorCode, Output, API};
pub use study::VolumeCache;
