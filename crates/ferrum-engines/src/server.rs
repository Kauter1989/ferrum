//! Reference `ferrum-engine/1` server: serves any [`SegmentationEngine`]
//! over HTTP. Used by the conformance tests and to demo the AI tools with
//! the [`crate::MockEngine`] (`cargo run -p ferrum-engines --example
//! mock_server`). Bridges for real engines live in `bridges/`.

use std::collections::HashMap;
use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;

use ferrum_domain::{EngineError, InteractiveSession, SegmentationEngine, VoxelBox};
use flate2::read::GzDecoder;
use serde_json::{json, Value};

use crate::wire::{self, VolumeHeader};

/// An HTTP response produced by the router.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// HTTP status.
    pub status: u16,
    /// Content type (empty for no body).
    pub content_type: &'static str,
    /// Body bytes.
    pub body: Vec<u8>,
    /// Extra headers.
    pub headers: Vec<(&'static str, String)>,
}

impl Reply {
    fn json(status: u16, v: &Value) -> Self {
        Self { status, content_type: "application/json", body: v.to_string().into_bytes(), headers: Vec::new() }
    }

    fn empty() -> Self {
        Self { status: 204, content_type: "", body: Vec::new(), headers: Vec::new() }
    }

    fn error(e: &EngineError) -> Self {
        let (status, code) = wire::error_status(e);
        Self::json(status, &wire::error_body(code, &e.to_string()))
    }

    fn code(status: u16, code: &str, message: &str) -> Self {
        Self::json(status, &wire::error_body(code, message))
    }
}

struct Entry {
    header: VolumeHeader,
    session: Option<Box<dyn InteractiveSession>>,
    /// Revision of the target mask, reported with every mask.
    revision: u64,
}

/// Protocol state and routing, independent of the HTTP library.
pub struct Router {
    engine: Arc<dyn SegmentationEngine>,
    token: Option<String>,
    sessions: HashMap<String, Entry>,
    next_id: u64,
}

/// A parsed request.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// HTTP method (upper case).
    pub method: &'a str,
    /// Path including the query string.
    pub url: &'a str,
    /// Value of the `Authorization` header.
    pub authorization: Option<&'a str>,
    /// Decoded body.
    pub body: &'a [u8],
}

impl Router {
    /// Router serving `engine`; requests must carry `token` if given.
    pub fn new(engine: Arc<dyn SegmentationEngine>, token: Option<String>) -> Self {
        Self { engine, token, sessions: HashMap::new(), next_id: 1 }
    }

    /// Number of open sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Handles one request.
    pub fn handle(&mut self, req: Request<'_>) -> Reply {
        if let Some(t) = &self.token {
            if req.authorization != Some(format!("Bearer {t}").as_str()) {
                return Reply::error(&EngineError::Unauthorized);
            }
        }
        let (path, query) = req.url.split_once('?').unwrap_or((req.url, ""));
        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        match (req.method, parts.as_slice()) {
            ("GET", ["v1", "info"]) => match self.engine.info() {
                Ok(info) => Reply::json(200, &wire::info_to_json(&info)),
                Err(e) => Reply::error(&e),
            },
            ("POST", ["v1", "sessions"]) => self.create(req.body),
            ("PUT", ["v1", "sessions", id, "volume"]) => self.upload(id, req.body),
            ("POST", ["v1", "sessions", id, "prompts"]) => self.prompt(id, req.body),
            ("GET", ["v1", "sessions", id, "mask"]) => self.mask(id, query),
            ("POST", ["v1", "sessions", id, "undo"]) => self.with_session(id, |s, rev| {
                let r = s.undo()?;
                *rev = r.revision;
                Ok(Reply::json(200, &wire::result_to_json(&r)))
            }),
            ("POST", ["v1", "sessions", id, "reset"]) => self.with_session(id, |s, rev| {
                s.reset()?;
                *rev += 1;
                Ok(Reply::empty())
            }),
            ("DELETE", ["v1", "sessions", id]) => match self.sessions.remove(*id) {
                Some(_) => Reply::empty(),
                None => Reply::error(&EngineError::NotFound),
            },
            _ => Reply::code(404, "not_found", &format!("no route for {} {path}", req.method)),
        }
    }

    fn create(&mut self, body: &[u8]) -> Reply {
        let header = match serde_json::from_slice::<Value>(body)
            .map_err(|e| EngineError::BadRequest(e.to_string()))
            .and_then(|v| wire::header_from_json(&v))
        {
            Ok(h) => h,
            Err(e) => return Reply::error(&e),
        };
        if let Ok(info) = self.engine.info() {
            if info.max_voxels > 0 && header.dims.voxel_count() as u64 > info.max_voxels {
                return Reply::error(&EngineError::TooLarge(format!(
                    "{} voxels > {}",
                    header.dims.voxel_count(),
                    info.max_voxels
                )));
            }
        }
        let id = format!("s{:08x}", self.next_id);
        self.next_id += 1;
        self.sessions.insert(id.clone(), Entry { header, session: None, revision: 0 });
        Reply::json(201, &json!({ "session_id": id, "expires_in_s": 0 }))
    }

    fn upload(&mut self, id: &str, body: &[u8]) -> Reply {
        let Some(entry) = self.sessions.get_mut(id) else {
            return Reply::error(&EngineError::NotFound);
        };
        let opened =
            wire::decode_volume(&entry.header, body).and_then(|v| self.engine.open_session(&v, &entry.header.modality));
        match opened {
            Ok(s) => {
                entry.session = Some(s);
                Reply::empty()
            }
            Err(e) => Reply::error(&e),
        }
    }

    fn with_session(
        &mut self,
        id: &str,
        f: impl FnOnce(&mut Box<dyn InteractiveSession>, &mut u64) -> Result<Reply, EngineError>,
    ) -> Reply {
        match self.sessions.get_mut(id) {
            None => Reply::error(&EngineError::NotFound),
            Some(Entry { session: None, .. }) => Reply::code(409, "no_volume", "upload the volume first"),
            Some(Entry { session: Some(s), revision, .. }) => f(s, revision).unwrap_or_else(|e| Reply::error(&e)),
        }
    }

    fn prompt(&mut self, id: &str, body: &[u8]) -> Reply {
        let prompt = serde_json::from_slice::<Value>(body)
            .map_err(|e| EngineError::BadRequest(e.to_string()))
            .and_then(|v| wire::prompt_from_json(&v));
        let supported = self.engine.info().map(|i| i.capabilities.prompts).unwrap_or_default();
        self.with_session(id, |s, rev| {
            let p = prompt?;
            if !supported.contains(&p.kind()) {
                return Err(EngineError::Unsupported(format!("{} prompts are not supported", p.kind().as_str())));
            }
            let r = s.prompt(&p)?;
            *rev = r.revision;
            Ok(Reply::json(200, &wire::result_to_json(&r)))
        })
    }

    fn mask(&mut self, id: &str, query: &str) -> Reply {
        let dims = match self.sessions.get(id) {
            Some(e) => e.header.dims,
            None => return Reply::error(&EngineError::NotFound),
        };
        let bx = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("box="))
            .map_or(Ok(VoxelBox::full(dims)), wire::parse_box_query);
        self.with_session(id, |s, rev| {
            let bytes = s.mask(bx?)?;
            let headers = vec![("X-Ferrum-Revision", rev.to_string())];
            Ok(Reply { status: 200, content_type: "application/octet-stream", body: bytes, headers })
        })
    }
}

/// A running reference server (stopped when dropped).
pub struct EngineServer {
    addr: SocketAddr,
    server: Arc<tiny_http::Server>,
    thread: Option<JoinHandle<()>>,
}

impl EngineServer {
    /// Starts serving `engine` on `addr` (e.g. `127.0.0.1:0` for any port).
    pub fn start(engine: Arc<dyn SegmentationEngine>, addr: &str, token: Option<String>) -> std::io::Result<Self> {
        let server = tiny_http::Server::http(addr).map_err(std::io::Error::other)?;
        let addr = server.server_addr().to_ip().ok_or_else(|| std::io::Error::other("not an IP listener"))?;
        let server = Arc::new(server);
        let worker = server.clone();
        let thread = std::thread::Builder::new().name("ferrum-engine-server".into()).spawn(move || {
            let mut router = Router::new(engine, token);
            for request in worker.incoming_requests() {
                serve(&mut router, request);
            }
        })?;
        Ok(Self { addr, server, thread: Some(thread) })
    }

    /// Base URL, e.g. `http://127.0.0.1:41234`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for EngineServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_body(request: &mut tiny_http::Request) -> std::io::Result<Vec<u8>> {
    let gzip = request
        .headers()
        .iter()
        .any(|h| h.field.equiv("Content-Encoding") && h.value.as_str().eq_ignore_ascii_case("gzip"));
    let mut raw = Vec::new();
    request.as_reader().read_to_end(&mut raw)?;
    if !gzip {
        return Ok(raw);
    }
    let mut out = Vec::new();
    GzDecoder::new(&raw[..]).read_to_end(&mut out)?;
    Ok(out)
}

fn serve(router: &mut Router, mut request: tiny_http::Request) {
    let reply = match read_body(&mut request) {
        Ok(body) => {
            let auth =
                request.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.as_str().to_owned());
            let method = request.method().as_str().to_ascii_uppercase();
            let url = request.url().to_owned();
            router.handle(Request { method: &method, url: &url, authorization: auth.as_deref(), body: &body })
        }
        Err(e) => Reply::code(400, "bad_request", &format!("cannot read body: {e}")),
    };
    let mut resp = tiny_http::Response::from_data(reply.body).with_status_code(reply.status);
    let mut headers = reply.headers;
    if !reply.content_type.is_empty() {
        headers.push(("Content-Type", reply.content_type.to_owned()));
    }
    for (k, v) in headers {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp.add_header(h);
        }
    }
    if let Err(e) = request.respond(resp) {
        log::debug!("engine server: {e}");
    }
}
