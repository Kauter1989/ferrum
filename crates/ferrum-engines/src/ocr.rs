//! The OCR bridge kind of `ferrum-engine/1` (Stage 31, #75, criterion
//! 31.3-b; ADR 0020 decision 5): an **optional** out-of-process check for
//! burned-in text in a rendered image.
//!
//! No OCR runs in Rust and no weights are bundled (constitution): the check
//! runs behind a bridge that speaks the engine protocol's conventions
//! (`/v1`, JSON control messages, the error envelope) with one extra
//! capability, `ocr`, and one extra endpoint, `POST /v1/ocr`
//! (`docs/engine-protocol.md` §7). Without a bridge the step is **skipped
//! and reported** — [`OcrOutcome::Skipped`] — never silently passed; a
//! bridge that cannot be reached or answers wrongly is
//! [`OcrOutcome::Failed`], also reported.
//!
//! The bridge reports detected text regions in image pixel coordinates;
//! the caller turns them into mask regions (the agent's `view slice
//! --ocr --masked` does, conservatively covering each region's bounding
//! box). A production OCR bridge is a follow-up; the interface and the
//! skip/report behaviour are this PR's scope.

use ferrum_domain::{EngineError, ENGINE_PROTOCOL};
use serde_json::{json, Value};

use crate::http::{transport, Client, HttpConfig};

/// A detected text region in image pixel coordinates: 0-based, half-open
/// (`x ≤ col < x + w`), as the mask regions of Clean Pixel Data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrRegion {
    /// First column (0-based).
    pub x: u32,
    /// First row (0-based).
    pub y: u32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

/// The outcome of the optional OCR check. Every case is reportable: a
/// caller writes [`OcrOutcome::json`] into its output, so "skipped" and
/// "failed" are visible, never a silent pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcrOutcome {
    /// No bridge is configured: the step did not run.
    Skipped {
        /// Why the step did not run.
        reason: String,
    },
    /// The bridge ran and reported these text regions (possibly none).
    Detected {
        /// The detected text regions, in image pixel coordinates.
        regions: Vec<OcrRegion>,
    },
    /// A bridge is configured but the check could not complete.
    Failed {
        /// What went wrong (unreachable, wrong protocol, bad answer).
        error: String,
    },
}

impl OcrOutcome {
    /// The report JSON: `status` plus the case's detail.
    pub fn json(&self) -> Value {
        match self {
            OcrOutcome::Skipped { reason } => json!({ "status": "skipped", "reason": reason }),
            OcrOutcome::Detected { regions } => json!({
                "status": "detected",
                "regions": regions.iter().map(|r| json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h })).collect::<Vec<_>>(),
            }),
            OcrOutcome::Failed { error } => json!({ "status": "failed", "error": error }),
        }
    }

    /// The detected regions; empty unless [`OcrOutcome::Detected`].
    pub fn regions(&self) -> &[OcrRegion] {
        match self {
            OcrOutcome::Detected { regions } => regions,
            _ => &[],
        }
    }
}

/// Client of an OCR bridge: `ferrum-engine/1` with `capabilities.ocr`.
#[derive(Debug, Clone)]
pub struct OcrBridge {
    client: Client,
}

impl OcrBridge {
    /// A client for the bridge at `config.base_url`; no request is made
    /// until [`OcrBridge::info`] or [`OcrBridge::detect`].
    pub fn new(config: HttpConfig) -> Self {
        Self { client: Client { config } }
    }

    /// Connection settings.
    pub fn config(&self) -> &HttpConfig {
        &self.client.config
    }

    /// `GET /v1/info`: the bridge must speak `ferrum-engine/1` and offer
    /// the `ocr` capability; anything else is refused.
    pub fn info(&self) -> Result<(), EngineError> {
        let v = self.client.get_json("/info")?;
        let protocol = v["protocol"].as_str().unwrap_or_default();
        if protocol != ENGINE_PROTOCOL {
            return Err(EngineError::Protocol(format!("bridge speaks {protocol:?}, FERRUM needs {ENGINE_PROTOCOL}")));
        }
        if v["capabilities"]["ocr"].as_bool() != Some(true) {
            return Err(EngineError::Unsupported("the bridge does not offer the ocr capability".into()));
        }
        Ok(())
    }

    /// `POST /v1/ocr` with the image as PNG bytes; the bridge answers the
    /// detected text regions (`{"regions": [{"x", "y", "w", "h"}]}`).
    pub fn detect(&self, png: &[u8]) -> Result<Vec<OcrRegion>, EngineError> {
        let req = self.client.auth(self.client.agent(self.client.config.timeout).post(&self.client.url("/ocr")));
        let resp = req.header("Content-Type", "image/png").send(png).map_err(transport)?;
        let v = Client::json(resp)?;
        regions_from_json(&v)
    }
}

/// Parses the `regions` array of an OCR answer.
fn regions_from_json(v: &Value) -> Result<Vec<OcrRegion>, EngineError> {
    let arr = v["regions"].as_array().ok_or_else(|| EngineError::Protocol("ocr answer has no regions array".into()))?;
    let mut out = Vec::with_capacity(arr.len());
    for r in arr {
        let num = |k: &str| -> Result<u32, EngineError> {
            r[k].as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| EngineError::Protocol(format!("ocr region field {k:?} is not a non-negative integer")))
        };
        out.push(OcrRegion { x: num("x")?, y: num("y")?, w: num("w")?, h: num("h")? });
    }
    Ok(out)
}

/// Runs the optional OCR check on a rendered image. Without a bridge the
/// step is skipped and reported; with one, its info is verified and the
/// image checked. Every outcome is reportable — never silently passed.
pub fn check(bridge: Option<&OcrBridge>, png: &[u8]) -> OcrOutcome {
    let Some(bridge) = bridge else {
        return OcrOutcome::Skipped { reason: "no OCR bridge configured ([network] ocr_bridge)".into() };
    };
    if let Err(e) = bridge.info() {
        return OcrOutcome::Failed { error: e.to_string() };
    }
    match bridge.detect(png) {
        Ok(regions) => OcrOutcome::Detected { regions },
        Err(e) => OcrOutcome::Failed { error: e.to_string() },
    }
}
