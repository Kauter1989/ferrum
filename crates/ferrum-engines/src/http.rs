//! `ferrum-engine/1` client: a [`SegmentationEngine`] that talks to an
//! engine (usually a bridge) over HTTP.

use std::io::{Read, Write};
use std::time::Duration;

use ferrum_domain::{
    Dims3, EngineError, EngineInfo, InteractiveSession, JobStatus, Prompt, PromptResult, SegmentationEngine, Volume,
    VoxelBox,
};
use flate2::write::GzEncoder;
use serde_json::Value;
use ureq::http::Response;
use ureq::Body;

use crate::wire;

/// Connection settings of an [`HttpEngine`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    /// Base URL without `/v1`, e.g. `http://127.0.0.1:8765`.
    pub base_url: String,
    /// Bearer token, if the engine requires one.
    pub token: Option<String>,
    /// Timeout of control requests.
    pub timeout: Duration,
    /// Timeout of the volume upload (engines may pre-compute features).
    pub upload_timeout: Duration,
}

impl HttpConfig {
    /// Settings for `base_url` with default timeouts and no token.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            token: None,
            timeout: Duration::from_secs(30),
            upload_timeout: Duration::from_secs(600),
        }
    }
}

/// Engine reached over HTTP.
#[derive(Debug, Clone)]
pub struct HttpEngine {
    config: HttpConfig,
}

impl HttpEngine {
    /// Creates a client; no request is made until [`SegmentationEngine::info`].
    pub fn new(config: HttpConfig) -> Self {
        Self { config }
    }

    /// Connection settings.
    pub fn config(&self) -> &HttpConfig {
        &self.config
    }
}

/// Shared request plumbing of the engine and its sessions.
#[derive(Debug, Clone)]
struct Client {
    config: HttpConfig,
}

fn transport(e: ureq::Error) -> EngineError {
    EngineError::Unreachable(e.to_string())
}

impl Client {
    fn agent(&self, timeout: Duration) -> ureq::Agent {
        ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build().into()
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1{path}", self.config.base_url)
    }

    fn auth<B>(&self, req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        match &self.config.token {
            Some(t) => req.header("Authorization", format!("Bearer {t}")),
            None => req,
        }
    }

    /// Turns an error status into an [`EngineError`].
    fn check(resp: Response<Body>) -> Result<Response<Body>, EngineError> {
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(resp);
        }
        let retry = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.trim().parse().ok());
        let mut resp = resp;
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        Err(wire::error_from_response(status, &body, retry))
    }

    fn json(resp: Response<Body>) -> Result<Value, EngineError> {
        let mut resp = Self::check(resp)?;
        let text = resp.body_mut().read_to_string().map_err(|e| EngineError::Protocol(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| EngineError::Protocol(format!("invalid JSON: {e}")))
    }

    fn get_json(&self, path: &str) -> Result<Value, EngineError> {
        let req = self.auth(self.agent(self.config.timeout).get(&self.url(path)));
        Self::json(req.call().map_err(transport)?)
    }

    fn post_json(&self, path: &str, body: &Value) -> Result<Value, EngineError> {
        let req = self.auth(self.agent(self.config.timeout).post(&self.url(path)));
        let resp = req.header("Content-Type", "application/json").send(body.to_string()).map_err(transport)?;
        Self::json(resp)
    }

    fn post_empty(&self, path: &str) -> Result<Response<Body>, EngineError> {
        let req = self.auth(self.agent(self.config.timeout).post(&self.url(path)));
        Self::check(req.header("Content-Type", "application/json").send("{}").map_err(transport)?)
    }

    fn put_gzip(&self, path: &str, bytes: &[u8]) -> Result<(), EngineError> {
        let mut gz = GzEncoder::new(Vec::with_capacity(bytes.len() / 4), flate2::Compression::fast());
        gz.write_all(bytes).map_err(|e| EngineError::Internal(e.to_string()))?;
        let body = gz.finish().map_err(|e| EngineError::Internal(e.to_string()))?;
        let req = self.auth(self.agent(self.config.upload_timeout).put(&self.url(path)));
        let resp = req
            .header("Content-Type", "application/octet-stream")
            .header("Content-Encoding", "gzip")
            .send(&body[..])
            .map_err(transport)?;
        Self::check(resp).map(|_| ())
    }

    fn get_bytes(&self, path: &str) -> Result<Vec<u8>, EngineError> {
        let req = self.auth(self.agent(self.config.timeout).get(&self.url(path)));
        let mut resp = Self::check(req.call().map_err(transport)?)?;
        let mut out = Vec::new();
        resp.body_mut()
            .with_config()
            .limit(u64::MAX)
            .reader()
            .read_to_end(&mut out)
            .map_err(|e| EngineError::Protocol(e.to_string()))?;
        Ok(out)
    }

    fn delete(&self, path: &str) -> Result<(), EngineError> {
        let req = self.auth(self.agent(self.config.timeout).delete(&self.url(path)));
        Self::check(req.call().map_err(transport)?).map(|_| ())
    }
}

impl SegmentationEngine for HttpEngine {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        let client = Client { config: self.config.clone() };
        wire::info_from_json(&client.get_json("/info")?)
    }

    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        let client = Client { config: self.config.clone() };
        let (header, bytes) = wire::encode_volume(volume, modality);
        let created = client.post_json("/sessions", &wire::header_to_json(&header))?;
        let id = created
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .ok_or_else(|| EngineError::Protocol("invalid session_id".into()))?
            .to_owned();
        let session = HttpSession { client, id, dims: volume.dims(), closed: false };
        client_upload(&session, &bytes)?;
        Ok(Box::new(session))
    }
}

fn client_upload(s: &HttpSession, bytes: &[u8]) -> Result<(), EngineError> {
    s.client.put_gzip(&format!("/sessions/{}/volume", s.id), bytes)
}

/// Session on an [`HttpEngine`]; deleted on the engine when dropped.
#[derive(Debug)]
pub struct HttpSession {
    client: Client,
    id: String,
    dims: Dims3,
    closed: bool,
}

impl HttpSession {
    /// Engine-side session id.
    pub fn id(&self) -> &str {
        &self.id
    }

    fn path(&self, suffix: &str) -> String {
        format!("/sessions/{}{suffix}", self.id)
    }
}

impl InteractiveSession for HttpSession {
    fn prompt(&mut self, prompt: &Prompt) -> Result<PromptResult, EngineError> {
        prompt.validate(self.dims)?;
        wire::result_from_json(&self.client.post_json(&self.path("/prompts"), &wire::prompt_to_json(prompt))?)
    }

    fn mask(&mut self, bx: VoxelBox) -> Result<Vec<u8>, EngineError> {
        if !bx.fits(self.dims) {
            return Err(EngineError::BadRequest(format!("box {}..{} is empty or outside the volume", bx.min, bx.max)));
        }
        let bytes = self.client.get_bytes(&self.path(&format!("/mask?box={}", wire::box_query(bx))))?;
        if bytes.len() != bx.voxel_count() {
            return Err(EngineError::Protocol(format!(
                "mask has {} bytes, the box has {} voxels",
                bytes.len(),
                bx.voxel_count()
            )));
        }
        Ok(bytes)
    }

    fn undo(&mut self) -> Result<PromptResult, EngineError> {
        wire::result_from_json(&self.client.post_json(&self.path("/undo"), &Value::Object(Default::default()))?)
    }

    fn reset(&mut self) -> Result<(), EngineError> {
        self.client.post_empty(&self.path("/reset")).map(|_| ())
    }

    fn start_job(&mut self, labels: Option<&[String]>) -> Result<String, EngineError> {
        let v = self.client.post_json(&self.path("/segment"), &wire::labels_to_json(labels))?;
        v.get("job_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .map(str::to_owned)
            .ok_or_else(|| EngineError::Protocol("invalid job_id".into()))
    }

    fn job_status(&mut self, job: &str) -> Result<JobStatus, EngineError> {
        wire::job_from_json(&self.client.get_json(&format!("/jobs/{job}"))?)
    }

    fn cancel_job(&mut self, job: &str) -> Result<(), EngineError> {
        self.client.delete(&format!("/jobs/{job}"))
    }

    fn label_map(&mut self) -> Result<Vec<u16>, EngineError> {
        let bytes = self.client.get_bytes(&self.path("/labelmap"))?;
        wire::decode_label_map(&bytes, self.dims.voxel_count())
    }
}

impl Drop for HttpSession {
    fn drop(&mut self) {
        if !self.closed {
            self.closed = true;
            if let Err(e) = self.client.delete(&self.path("")) {
                log::debug!("closing engine session {}: {e}", self.id);
            }
        }
    }
}
