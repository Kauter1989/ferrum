//! The loopback server of the view host: `ferrum-view/1` over HTTP.
//!
//! It binds `127.0.0.1` only and every request needs the session's token
//! (a query parameter, a bearer header or the cookie the page sets). The
//! routes are small on purpose: a page, the state, one frame and the event
//! log, so any harness can mount the view with a few lines of JavaScript.

use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::page::PAGE;
use crate::session::{Session, ViewError};

/// The name of the cookie that carries the token after the page loads.
pub const COOKIE: &str = "ferrum-view";

/// Largest body accepted on `POST /api/input`.
pub const MAX_BODY: usize = 64 * 1024;

/// How the host starts: the port the page is served on.
#[derive(Debug, Clone, Copy, Default)]
pub struct ServeOptions {
    /// Port on `127.0.0.1`; `0` lets the operating system choose one.
    pub port: u16,
}

/// A running view host (stopped when dropped).
pub struct ViewServer {
    addr: SocketAddr,
    token: String,
    backend: &'static str,
    workspace: String,
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

impl ViewServer {
    /// Starts serving `session` on loopback.
    pub fn start(session: Session, options: ServeOptions) -> Result<Self, ViewError> {
        let token = new_token()?;
        let backend = session.backend().as_str();
        let workspace = session.workspace().to_string_lossy().into_owned();
        let server = Server::http(("127.0.0.1", options.port)).map_err(|e| ViewError::Internal(e.to_string()))?;
        let addr = server.server_addr().to_ip().ok_or_else(|| ViewError::Internal("not an IP listener".into()))?;
        let server = Arc::new(server);
        let worker = server.clone();
        let token_for_thread = token.clone();
        let thread = std::thread::Builder::new()
            .name("ferrum-view".into())
            .spawn(move || {
                let mut host = Host { session, token: token_for_thread };
                for request in worker.incoming_requests() {
                    host.serve(request);
                }
            })
            .map_err(|e| ViewError::Internal(e.to_string()))?;
        Ok(Self { addr, token, backend, workspace, server, thread: Some(thread) })
    }

    /// The base URL, e.g. `http://127.0.0.1:41234`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The token every request needs.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// `"gpu"` or `"cpu"`.
    pub fn backend(&self) -> &'static str {
        self.backend
    }

    /// The workspace served.
    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    /// Blocks until the server stops (Ctrl-C ends the process).
    pub fn wait(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ViewServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A 32-hex-digit token from the operating system's random source.
fn new_token() -> Result<String, ViewError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| ViewError::Internal(format!("cannot read random bytes: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Everything one thread needs to answer requests.
struct Host {
    session: Session,
    token: String,
}

impl Host {
    fn serve(&mut self, mut request: Request) {
        let method = request.method().clone();
        let url = request.url().to_owned();
        let mut body = Vec::new();
        if method == Method::Post {
            if request.body_length().is_some_and(|n| n > MAX_BODY) {
                self.respond(request, Reply::json(StatusCode(413), json!({"error": "body too large"})));
                return;
            }
            if let Err(e) = std::io::Read::read_to_end(&mut request.as_reader(), &mut body) {
                self.respond(
                    request,
                    Reply::json(StatusCode(400), json!({"error": format!("cannot read the body: {e}")})),
                );
                return;
            }
        }
        // The panel module is public code, not data: a browser imports it as a
        // module and cannot append the token to that request, so it is served
        // without one. Everything that carries a study stays behind the token.
        let public_panel = url.starts_with("/panel/");
        let authorized = public_panel || self.authorized(&request, &url);
        let reply = if !authorized {
            Reply::json(StatusCode(401), json!({"error": "token required"}))
        } else {
            self.route(&method, &url, &body)
        };
        self.respond(request, reply);
    }

    fn authorized(&self, request: &Request, url: &str) -> bool {
        let from_query = query_value(url, "token");
        let from_header = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .and_then(|h| h.value.as_str().strip_prefix("Bearer "))
            .map(str::to_owned);
        let from_cookie = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Cookie"))
            .and_then(|h| cookie_value(h.value.as_str(), COOKIE));
        [from_query, from_header, from_cookie].into_iter().flatten().any(|t| constant_time_eq(&t, &self.token))
    }

    fn route(&mut self, method: &Method, url: &str, body: &[u8]) -> Reply {
        let path = url.split('?').next().unwrap_or(url);
        let get = *method == Method::Get;
        let post = *method == Method::Post;
        match path {
            "/" | "/index.html" if get => Reply::page(self.token.clone()),
            "/api/state" if get => {
                self.session.refresh();
                match self.session.state_json() {
                    Ok(state) => Reply::json(StatusCode(200), state),
                    Err(e) => internal(e),
                }
            }
            "/api/events" if get => {
                let since = query_value(url, "since").and_then(|v| v.parse::<u64>().ok());
                Reply::json(StatusCode(200), self.session.events_json(since))
            }
            // A client that draws the study itself takes the voxels once.
            "/api/volume" if get => match self.session.volume() {
                Ok((header, values)) => Reply {
                    status: StatusCode(200),
                    content_type: "application/octet-stream",
                    headers: vec![("X-Ferrum-Volume".into(), header.to_string())],
                    body: values,
                },
                Err(e) => internal(e),
            },
            // The header alone, so a client can size the download and check a
            // cached copy without fetching the voxels again.
            "/api/volume.json" if get => match self.session.volume() {
                Ok((header, _)) => Reply::json(StatusCode(200), header),
                Err(e) => internal(e),
            },
            "/api/frame.png" if get => match self.session.frame() {
                Ok(frame) => Reply {
                    status: StatusCode(200),
                    content_type: "image/png",
                    headers: vec![("X-Ferrum-Rev".into(), frame.rev.to_string())],
                    body: frame.png.clone(),
                },
                Err(e) => internal(e),
            },
            "/api/input" if post => match self.session.input(body) {
                Ok(()) => match self.session.state_json() {
                    Ok(state) => Reply::json(StatusCode(200), state),
                    Err(e) => internal(e),
                },
                Err(e @ ViewError::BadRequest(_)) => Reply::json(StatusCode(400), json!({ "error": e.to_string() })),
                Err(e) => internal(e),
            },
            // The browser panel: the built wasm module and its JS glue, taken
            // from the folder next to the executable (see
            // integrations/dsh/panel/build.cmd). A client that draws the study
            // itself loads these once.
            panel if get && panel.starts_with("/panel/") => match panel_file(panel) {
                Some((bytes, kind)) => Reply {
                    status: StatusCode(200),
                    content_type: kind,
                    // A panel served from another origin (a harness) may
                    // import the module, but nothing else of the host is
                    // open to it.
                    headers: vec![
                        ("Cache-Control".into(), "max-age=60".into()),
                        ("Access-Control-Allow-Origin".into(), "*".into()),
                    ],
                    body: bytes,
                },
                None => Reply::json(StatusCode(404), json!({ "error": "no such panel file" })),
            },
            "/api/state" | "/api/events" | "/api/frame.png" | "/api/volume" | "/api/volume.json" | "/api/input"
            | "/" | "/index.html" => Reply::json(StatusCode(405), json!({"error": "wrong method"})),
            _ => Reply::json(StatusCode(404), json!({"error": "no such route"})),
        }
    }

    fn respond(&self, request: Request, reply: Reply) {
        let mut response = Response::from_data(reply.body).with_status_code(reply.status);
        let mut headers = reply.headers;
        // A harness panel is another origin: it authorises with the token, and
        // the browser needs this header to let it read the answer. It goes on
        // successful answers only — on a 401 or a 404 the wildcard would let
        // any page on this machine tell that a view host is running, which no
        // page needs to know.
        let authorised = reply.status.0 / 100 == 2;
        if authorised && !headers.iter().any(|(name, _)| name == "Access-Control-Allow-Origin") {
            headers.push(("Access-Control-Allow-Origin".into(), "*".into()));
        }
        if !reply.content_type.is_empty() {
            headers.push(("Content-Type".into(), reply.content_type.to_owned()));
        }
        headers.push(("Cache-Control".into(), "no-store".into()));
        for (name, value) in headers {
            if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
                response.add_header(header);
            }
        }
        if let Err(e) = request.respond(response) {
            log::debug!("ferrum-view: {e}");
        }
    }
}

/// One answer: status, body, content type and extra headers.
struct Reply {
    status: StatusCode,
    content_type: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    fn json(status: StatusCode, value: Value) -> Self {
        Self { status, content_type: "application/json", headers: Vec::new(), body: value.to_string().into_bytes() }
    }

    fn page(token: String) -> Self {
        Self {
            status: StatusCode(200),
            content_type: "text/html; charset=utf-8",
            headers: vec![("Set-Cookie".into(), format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict"))],
            body: PAGE.as_bytes().to_vec(),
        }
    }
}

/// A file of the browser panel (`/panel/<name>`) and its content type.
///
/// The host looks for `ferrum_wasm.js` and `ferrum_wasm_bg.wasm` next to its
/// own executable first, then in a `panel` folder below the working directory;
/// anything else is refused, so the route cannot be turned into a file server.
fn panel_file(path: &str) -> Option<(Vec<u8>, &'static str)> {
    let name = path.strip_prefix("/panel/")?;
    let (bytes, kind) = match name {
        "ferrum_wasm.js" => (std::fs::read(panel_path(name)?).ok()?, "text/javascript"),
        "ferrum_wasm_bg.wasm" => (std::fs::read(panel_path(name)?).ok()?, "application/wasm"),
        _ => return None,
    };
    Some((bytes, kind))
}

/// Where a panel file lives: beside the executable, else in `./panel`.
fn panel_path(name: &str) -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(name));
            candidates.push(dir.join("panel").join(name));
        }
    }
    candidates.push(std::path::Path::new("panel").join(name));
    candidates.into_iter().find(|p| p.is_file())
}

fn internal(error: ViewError) -> Reply {
    log::warn!("ferrum-view: {error}");
    Reply::json(StatusCode(500), json!({ "error": error.to_string() }))
}

/// The value of `name` in a query string (`"?a=1&b=2"`).
fn query_value(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}

/// The value of cookie `name` in a `Cookie` header.
fn cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}

/// Compares two strings without stopping at the first differing byte.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}
