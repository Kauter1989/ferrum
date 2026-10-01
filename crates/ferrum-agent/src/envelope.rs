//! The JSON envelope every command returns (`ferrum-agent/1`) and the
//! error codes an agent can act on (`docs/agent-skill.md` §8).

use serde_json::{json, Value};

/// Value of the `api` field.
pub const API: &str = "ferrum-agent/1";

/// Error codes of the envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// Invalid parameters.
    BadRequest,
    /// No study is open in the workspace.
    NoStudy,
    /// Unknown series, segment, annotation or render.
    NotFound,
    /// Point or box outside the grid.
    OutOfVolume,
    /// Source data differs from the hashes in the workspace.
    SourceChanged,
    /// No engine configured or reachable.
    EngineUnavailable,
    /// Blocked by the operator configuration.
    Forbidden,
    /// Size, time or count limit reached.
    Limit,
    /// Anything else.
    Internal,
}

impl ErrorCode {
    /// Wire name, e.g. `out_of_volume`.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::NoStudy => "no_study",
            ErrorCode::NotFound => "not_found",
            ErrorCode::OutOfVolume => "out_of_volume",
            ErrorCode::SourceChanged => "source_changed",
            ErrorCode::EngineUnavailable => "engine_unavailable",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::Limit => "limit",
            ErrorCode::Internal => "internal",
        }
    }
}

/// A command error: code, message and a hint the agent can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentError {
    /// Error code.
    pub code: ErrorCode,
    /// What went wrong.
    pub message: String,
    /// What to do about it, if anything useful can be said.
    pub hint: Option<String>,
}

impl AgentError {
    /// An error without a hint.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None }
    }

    /// Adds a hint.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// `bad_request` with `message`.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }

    /// `not_found` with `message`.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// `internal` with `message`.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for AgentError {}

impl From<ferrum_io::IoError> for AgentError {
    fn from(e: ferrum_io::IoError) -> Self {
        match e {
            ferrum_io::IoError::SourceChanged { .. } => Self::new(ErrorCode::SourceChanged, e.to_string())
                .hint("the source data changed after the workspace was created; open the study in a new workspace"),
            other => Self::internal(other.to_string()),
        }
    }
}

/// Result of a command: data, warnings and provenance of the call.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    /// Command data.
    pub data: Value,
    /// Things the agent should know (e.g. irregular spacing).
    pub warnings: Vec<String>,
}

impl Output {
    /// Data without warnings.
    pub fn new(data: Value) -> Self {
        Self { data, warnings: Vec::new() }
    }

    /// Adds a warning.
    pub fn warn(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

/// Success envelope.
pub fn ok_envelope(out: &Output, provenance: Value) -> Value {
    json!({ "api": API, "ok": true, "data": out.data, "warnings": out.warnings, "provenance": provenance })
}

/// Error envelope.
pub fn error_envelope(e: &AgentError) -> Value {
    json!({ "api": API, "ok": false, "error": { "code": e.code.as_str(), "message": e.message, "hint": e.hint } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes() {
        let out = Output::new(json!({ "value": 1 })).warn("w");
        let v = ok_envelope(&out, json!({ "command": "probe" }));
        assert_eq!(
            (v["api"].as_str(), v["ok"].as_bool(), v["warnings"][0].as_str()),
            (Some(API), Some(true), Some("w"))
        );
        let e = AgentError::new(ErrorCode::OutOfVolume, "outside").hint("0-based");
        let v = error_envelope(&e);
        assert_eq!(v["error"]["code"], "out_of_volume");
        assert_eq!(v["error"]["hint"], "0-based");
        assert_eq!(e.to_string(), "out_of_volume: outside");
        let codes = [
            ErrorCode::BadRequest,
            ErrorCode::NoStudy,
            ErrorCode::NotFound,
            ErrorCode::OutOfVolume,
            ErrorCode::SourceChanged,
            ErrorCode::EngineUnavailable,
            ErrorCode::Forbidden,
            ErrorCode::Limit,
            ErrorCode::Internal,
        ];
        let names: std::collections::HashSet<_> = codes.iter().map(|c| c.as_str()).collect();
        assert_eq!(names.len(), codes.len());
        let changed = AgentError::from(ferrum_io::IoError::SourceChanged { path: "/x".into(), reason: "r".into() });
        assert_eq!(changed.code, ErrorCode::SourceChanged);
        assert_eq!(AgentError::from(ferrum_io::IoError::Cancelled).code, ErrorCode::Internal);
    }
}
