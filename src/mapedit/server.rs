//! The map editor's local web server (plan D56, I26b). A hand-rolled `std::net::TcpListener`
//! HTTP/1.1 server (no new crate): the editor pages and libraries are served from memory
//! (embedded), the generated game data from [`super::data::EditorData`], and `POST save` writes
//! the user's override road-type file. See `docs/game-data/fh6-map-tooling.md` ("Local server")
//! for the why of every check below.
//!
//! **Security scheme** (the server holds the user's files; any web page can reach 127.0.0.1):
//! loopback bind only; a per-session random token as the first *path segment*
//! (`http://127.0.0.1:<port>/<token>/`, so the pages' relative URLs carry it for free), wrong or
//! missing token = empty 404; `Host` must be exactly `127.0.0.1:<port>` (DNS rebinding); `POST`
//! needs `Content-Type: application/json` and, when an `Origin` is sent, our own origin; `OPTIONS`
//! is never answered and no CORS header is ever sent (so a foreign page cannot make a JSON POST);
//! URLs are never mapped onto the filesystem (static table + strictly parsed generated paths).

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::gamedata::nav::Nav;
use crate::gamedata::roadtypes::{self, Current, RoadTypes, Source};

use super::data::{write_atomic, EditorData, Served};

/// Request head cap (request line + headers).
const MAX_HEAD: usize = 16 * 1024;
/// POST body cap (a Save is ~1 MB).
const MAX_BODY: usize = 16 * 1024 * 1024;
/// Per-read timeout and total time allowed for receiving a request.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

const CSP: &str = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; connect-src 'self'; frame-src 'self'; worker-src blob:";

// ------------------------------------------------------------------------------------------------ embedded pages

const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const JS: &str = "text/javascript; charset=utf-8";
const PNG: &str = "image/png";

/// Where the page asks the data scripts to be loaded, in this order (`app` last).
const DATA_SCRIPTS: [&str; 6] = ["meta", "roads", "roaded", "canon", "elevation", "app"];
const DATA_MARKER: &str = "<!--@DATA_SCRIPTS@-->";
/// What the page's `window.FH6.app` says: the Save endpoint and the project file to reset to.
const APP_JS: &str = "(window.FH6=window.FH6||{}).app={save:\"save\",project:\"project.json\"};";
/// The embedded project road-type file (also embedded by `roadtypes.rs`; a test checks they agree).
const PROJECT_JSON: &str = include_str!("../../assets/map/fh6-road-types.json");

const INDEX_HTML: &str = include_str!("../../assets/editor/index.html");

/// Static files served from memory: `(served path, content type, bytes)`. Anything not in this
/// table and not a generated path is a 404: URLs never touch the filesystem.
const STATIC: &[(&str, &str, &[u8])] = &[
    ("preview-3d.html", HTML, include_bytes!("../../assets/editor/preview-3d.html")),
    ("lib/leaflet.css", CSS, include_bytes!("../../assets/editor/lib/leaflet.css")),
    ("lib/MarkerCluster.css", CSS, include_bytes!("../../assets/editor/lib/MarkerCluster.css")),
    ("lib/MarkerCluster.Default.css", CSS, include_bytes!("../../assets/editor/lib/MarkerCluster.Default.css")),
    ("lib/leaflet.js", JS, include_bytes!("../../assets/editor/lib/leaflet.js")),
    ("lib/leaflet.markercluster.js", JS, include_bytes!("../../assets/editor/lib/leaflet.markercluster.js")),
    ("lib/three.min.js", JS, include_bytes!("../../assets/editor/lib/three.min.js")),
    ("lib/images/layers-2x.png", PNG, include_bytes!("../../assets/editor/lib/images/layers-2x.png")),
    ("lib/images/layers.png", PNG, include_bytes!("../../assets/editor/lib/images/layers.png")),
    ("lib/images/marker-icon-2x.png", PNG, include_bytes!("../../assets/editor/lib/images/marker-icon-2x.png")),
    ("lib/images/marker-icon.png", PNG, include_bytes!("../../assets/editor/lib/images/marker-icon.png")),
    ("lib/images/marker-shadow.png", PNG, include_bytes!("../../assets/editor/lib/images/marker-shadow.png")),
];

/// The editor page with the data `<script>` tags substituted for the marker.
fn index_html() -> String {
    let tags: Vec<String> = DATA_SCRIPTS.iter().map(|n| format!("<script charset=\"utf-8\" src=\"data/{n}.js\"></script>")).collect();
    INDEX_HTML.replace(DATA_MARKER, &tags.join("\n"))
}

// ------------------------------------------------------------------------------------------------ public API

/// What the editor opens with (D57). Save always writes the override file, whatever this is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)] // I27's "Open map editor" buttons construct these
pub enum StartFrom {
    /// The game's nav graph with no road types at all.
    Raw,
    /// The app's current road types: the user's override if valid, else the project file.
    Current,
}

/// What the server tells the app (over the channel given to [`MapServer::start`]).
#[derive(Clone, PartialEq, Debug)]
pub enum MapEvent {
    /// The data is built; the app should open `url` in the browser.
    Ready { url: String },
    /// A Save was written and is now the app's current road types. `edges` = typed game edges +
    /// user links, `points` = user points, `bytes` = size of the file written.
    Saved { edges: usize, points: usize, bytes: usize },
    /// Building the data or a Save failed (English message).
    Error(String),
}

/// [`MapServer::state`].
#[derive(Clone, PartialEq, Debug)]
pub enum MapServerState {
    /// Building the map data (`progress` 0..=1).
    Preparing { progress: f32 },
    Ready,
    /// The build failed; the server only answers 503. Drop it and start a new one to retry.
    Failed(String),
}

/// What the server needs from the generated data (so the HTTP layer is testable without an install).
pub trait Backend: Send + Sync {
    /// The generated file at `path` (no token, no leading slash); `None` = 404.
    fn resolve(&self, path: &str) -> Option<Result<Served, String>>;
    /// The game's road graph (for validating a Save).
    fn nav(&self) -> &Nav;
    /// New current road types after a Save.
    fn set_road_types(&self, rt: &RoadTypes);
}

impl Backend for EditorData {
    fn resolve(&self, path: &str) -> Option<Result<Served, String>> {
        EditorData::resolve(self, path)
    }
    fn nav(&self) -> &Nav {
        EditorData::nav(self)
    }
    fn set_road_types(&self, rt: &RoadTypes) {
        EditorData::set_road_types(self, rt)
    }
}

/// Where the server keeps its files (injectable for tests).
#[derive(Clone, Debug)]
pub struct Paths {
    /// The user's override road-type file (`roadtypes::override_path()`).
    pub override_file: PathBuf,
    /// The last-used port (`<app_data_dir>/map_editor/port`).
    pub port_file: PathBuf,
}

impl Paths {
    pub fn default_paths() -> Paths {
        Paths {
            override_file: roadtypes::override_path(),
            port_file: crate::config::app_data_dir().join("map_editor").join("port"),
        }
    }
}

/// The running server. Dropping it stops the accept thread (the build thread, if still running,
/// finishes on its own and is ignored).
pub struct MapServer {
    url: String,
    port: u16,
    shared: Arc<Shared>,
    join: Option<JoinHandle<()>>,
}

impl MapServer {
    /// Bind (the sticky port first, else any free one), start serving, and build the map data
    /// from the install at `media` on a background thread (a second or so with warm caches,
    /// longer on the very first run). The page is not served until that is done
    /// ([`MapEvent::Ready`] follows, then the app opens [`MapServer::url`]); until then requests
    /// get 503. `ctx` is only used to wake the UI after each event.
    pub fn start(ctx: egui::Context, media: PathBuf, start_from: StartFrom, events: Sender<MapEvent>) -> io::Result<MapServer> {
        let paths = Paths::default_paths();
        let server = Self::bind(ctx, paths, events)?;
        let sh = server.shared.clone();
        thread::Builder::new().name("mapedit-build".into()).spawn(move || {
            let built = (|| -> Result<(EditorData, Current), String> {
                let nav = Nav::load(&media)?;
                let current = RoadTypes::current(&sh.paths.override_file, &nav);
                let initial = match start_from {
                    StartFrom::Raw => RoadTypes::raw(),
                    StartFrom::Current => current.types.clone(),
                };
                let data = EditorData::build(&media, &initial, &|p| sh.progress.store(p.to_bits(), Ordering::Relaxed))?;
                Ok((data, current))
            })();
            match built {
                Ok((data, current)) => sh.install(Arc::new(data), current),
                Err(e) => sh.fail(e),
            }
        })?;
        Ok(server)
    }

    /// Bind and serve with no backend yet (503 until [`MapServer::install`]).
    pub(crate) fn bind(ctx: egui::Context, paths: Paths, events: Sender<MapEvent>) -> io::Result<MapServer> {
        let listener = bind_sticky(&paths.port_file)?;
        let port = listener.local_addr()?.port();
        let mut tok = [0u8; 16];
        getrandom::fill(&mut tok).map_err(|e| io::Error::other(format!("no random source: {e}")))?;
        let token: String = tok.iter().map(|b| format!("{b:02x}")).collect();
        let url = format!("http://127.0.0.1:{port}/{token}/");
        let shared = Arc::new(Shared {
            token,
            host: format!("127.0.0.1:{port}"),
            origin: format!("http://127.0.0.1:{port}"),
            index: index_html(),
            paths,
            stop: AtomicBool::new(false),
            progress: AtomicU32::new(0f32.to_bits()),
            state: RwLock::new(Inner { backend: None, current: None, failed: None }),
            save_lock: Mutex::new(()),
            events: Mutex::new(events),
            ctx,
            url: url.clone(),
        });
        let sh = shared.clone();
        let join = thread::Builder::new().name("mapedit-accept".into()).spawn(move || accept_loop(listener, sh))?;
        Ok(MapServer { url, port, shared, join: Some(join) })
    }

    /// Hand the server its data (what the build thread does; tests pass a fake). Sends
    /// [`MapEvent::Ready`].
    #[cfg(test)]
    pub(crate) fn install(&self, backend: Arc<dyn Backend>, current: Current) {
        self.shared.install(backend, current);
    }

    /// `http://127.0.0.1:<port>/<token>/` — contains the secret token: open it, don't log it.
    pub fn url(&self) -> &str {
        &self.url
    }

    #[allow(dead_code)] // for I27 / diagnostics; tests use it
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn state(&self) -> MapServerState {
        let st = self.shared.state.read().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = &st.failed {
            MapServerState::Failed(e.clone())
        } else if st.backend.is_some() {
            MapServerState::Ready
        } else {
            MapServerState::Preparing { progress: f32::from_bits(self.shared.progress.load(Ordering::Relaxed)) }
        }
    }

    /// The app's current road types as of the last Save (or as loaded when the server started):
    /// `types` + `source` + `note` + `project_updated_since_save`. `None` until ready.
    pub fn current(&self) -> Option<Arc<Current>> {
        self.shared.state.read().unwrap_or_else(|e| e.into_inner()).current.clone()
    }
}

impl Drop for MapServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        // accept() blocks: connect once so the loop sees the flag (same idea as network.rs's
        // stop flag, which relies on a read timeout instead).
        let _ = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)), Duration::from_millis(500));
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

// ------------------------------------------------------------------------------------------------ state

struct Inner {
    backend: Option<Arc<dyn Backend>>,
    current: Option<Arc<Current>>,
    failed: Option<String>,
}

struct Shared {
    token: String,
    /// The only `Host` header value accepted.
    host: String,
    /// The only `Origin` accepted on a POST.
    origin: String,
    /// `index.html` with the data script tags substituted.
    index: String,
    paths: Paths,
    stop: AtomicBool,
    progress: AtomicU32,
    state: RwLock<Inner>,
    /// One Save at a time (write + state swap are one step).
    save_lock: Mutex<()>,
    events: Mutex<Sender<MapEvent>>,
    ctx: egui::Context,
    url: String,
}

impl Shared {
    fn emit(&self, ev: MapEvent) {
        if self.stop.load(Ordering::Relaxed) {
            return;
        }
        let _ = self.events.lock().unwrap_or_else(|e| e.into_inner()).send(ev);
        self.ctx.request_repaint();
    }

    fn install(&self, backend: Arc<dyn Backend>, current: Current) {
        {
            let mut st = self.state.write().unwrap_or_else(|e| e.into_inner());
            st.backend = Some(backend);
            st.current = Some(Arc::new(current));
        }
        self.emit(MapEvent::Ready { url: self.url.clone() });
    }

    fn fail(&self, e: String) {
        self.state.write().unwrap_or_else(|e| e.into_inner()).failed = Some(e.clone());
        self.emit(MapEvent::Error(e));
    }

    fn backend(&self) -> Option<Arc<dyn Backend>> {
        self.state.read().unwrap_or_else(|e| e.into_inner()).backend.clone()
    }
}

/// The port from the last session if it's free, else any free one (written back to the file).
/// **Why sticky:** a new port is a new browser origin, so the editor's `localStorage` (view,
/// season, layers, language...) would start empty every session.
fn bind_sticky(port_file: &Path) -> io::Result<TcpListener> {
    let preferred = std::fs::read_to_string(port_file).ok().and_then(|s| s.trim().parse::<u16>().ok()).filter(|p| *p != 0);
    if let Some(p) = preferred {
        if let Ok(l) = TcpListener::bind((Ipv4Addr::LOCALHOST, p)) {
            return Ok(l);
        }
    }
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = l.local_addr()?.port();
    if let Some(dir) = port_file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(port_file, port.to_string()); // best effort: only costs the sticky port
    Ok(l)
}

fn accept_loop(listener: TcpListener, sh: Arc<Shared>) {
    for conn in listener.incoming() {
        if sh.stop.load(Ordering::Relaxed) {
            break;
        }
        match conn {
            Ok(stream) => {
                let sh = sh.clone();
                // Thread per connection: the browser opens ~6 at once, Leaflet asks for dozens of tiles.
                let _ = thread::Builder::new().name("mapedit-conn".into()).spawn(move || handle(&sh, stream));
            }
            Err(_) => thread::sleep(Duration::from_millis(20)), // e.g. out of fds: don't spin
        }
    }
}

// ------------------------------------------------------------------------------------------------ HTTP

struct Request {
    method: String,
    target: String,
    /// Lower-case names.
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
    fn count(&self, name: &str) -> usize {
        self.headers.iter().filter(|(k, _)| k == name).count()
    }
}

struct Resp {
    status: u16,
    content_type: &'static str,
    cache: &'static str,
    body: Vec<u8>,
    allow: Option<&'static str>,
}

const NO_STORE: &str = "no-store";
const CACHE_1H: &str = "private, max-age=3600";

impl Resp {
    fn empty(status: u16) -> Resp {
        Resp { status, content_type: "text/plain; charset=utf-8", cache: NO_STORE, body: Vec::new(), allow: None }
    }
    fn json(status: u16, v: serde_json::Value) -> Resp {
        Resp { status, content_type: "application/json; charset=utf-8", cache: NO_STORE, body: v.to_string().into_bytes(), allow: None }
    }
    fn err(status: u16, msg: &str) -> Resp {
        Self::json(status, serde_json::json!({ "error": msg }))
    }
    fn file(content_type: &'static str, body: Vec<u8>, cache: &'static str) -> Resp {
        Resp { status: 200, content_type, cache, body, allow: None }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn write_response(s: &mut TcpStream, r: &Resp) -> io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: {}\r\nConnection: close\r\n\
         X-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nX-Frame-Options: SAMEORIGIN\r\nContent-Security-Policy: {}\r\n",
        r.status,
        reason(r.status),
        r.content_type,
        r.body.len(),
        r.cache,
        CSP
    );
    if let Some(a) = r.allow {
        head.push_str(&format!("Allow: {a}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes())?;
    s.write_all(&r.body)?;
    s.flush()
}

enum HeadError {
    /// Nothing usable arrived (closed, timed out): drop the connection silently.
    Gone,
    TooLarge,
    Malformed,
}

/// Read up to the blank line; returns the parsed head and any body bytes already received.
fn read_head(s: &mut TcpStream, deadline: Instant) -> Result<(Request, Vec<u8>), HeadError> {
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    let end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p;
        }
        if buf.len() > MAX_HEAD {
            return Err(HeadError::TooLarge);
        }
        if Instant::now() > deadline {
            return Err(HeadError::Gone);
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return Err(HeadError::Gone),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    };
    if end > MAX_HEAD {
        return Err(HeadError::TooLarge);
    }
    let head = std::str::from_utf8(&buf[..end]).map_err(|_| HeadError::Malformed)?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let (method, target, version) = (first.next().unwrap_or(""), first.next().unwrap_or(""), first.next().unwrap_or(""));
    if method.is_empty() || target.is_empty() || !version.starts_with("HTTP/1.") || first.next().is_some() {
        return Err(HeadError::Malformed);
    }
    let mut headers = Vec::new();
    for l in lines {
        let (k, v) = l.split_once(':').ok_or(HeadError::Malformed)?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let rest = buf[end + 4..].to_vec();
    Ok((Request { method: method.to_string(), target: target.to_string(), headers }, rest))
}

fn handle(sh: &Shared, mut s: TcpStream) {
    let _ = s.set_read_timeout(Some(IO_TIMEOUT));
    let _ = s.set_write_timeout(Some(IO_TIMEOUT));
    let deadline = Instant::now() + IO_TIMEOUT;
    let resp = match read_head(&mut s, deadline) {
        Err(HeadError::Gone) => return,
        Err(HeadError::TooLarge) => Resp::empty(431),
        Err(HeadError::Malformed) => Resp::empty(400),
        Ok((req, rest)) => route(sh, &req, rest, &mut s, deadline),
    };
    if write_response(&mut s, &resp).is_ok() {
        // The client may still be sending (an oversize body we refused): closing with unread
        // data makes the OS reset the connection and the client never sees our answer.
        let _ = s.shutdown(Shutdown::Write);
        let _ = s.set_read_timeout(Some(Duration::from_millis(200)));
        let mut sink = [0u8; 4096];
        let mut left = 256 * 1024;
        while left > 0 {
            match s.read(&mut sink) {
                Ok(0) | Err(_) => break,
                Ok(n) => left -= n.min(left),
            }
        }
    }
}

/// Constant-time equality (the token is the only secret; don't leak a prefix match by timing).
fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn route(sh: &Shared, req: &Request, rest: Vec<u8>, s: &mut TcpStream, deadline: Instant) -> Resp {
    // 1. DNS rebinding: a page on evil.example resolving to 127.0.0.1 sends Host: evil.example.
    if req.count("host") != 1 || req.header("host") != Some(sh.host.as_str()) {
        return Resp::empty(400);
    }
    // 2. The token is the first path segment; no query/fragment is ever meaningful.
    let target = req.target.split(['?', '#']).next().unwrap_or("");
    let Some(after_slash) = target.strip_prefix('/') else { return Resp::empty(404) };
    let Some((token, path)) = after_slash.split_once('/') else { return Resp::empty(404) };
    if !ct_eq(token, &sh.token) {
        return Resp::empty(404);
    }
    // 3. Methods: GET and the one POST. Never OPTIONS (no preflight = no cross-origin JSON POST).
    match req.method.as_str() {
        "GET" => get(sh, path),
        "POST" if path == "save" => save(sh, req, rest, s, deadline),
        "POST" => Resp::empty(404),
        _ => Resp { allow: Some("GET, POST"), ..Resp::empty(405) },
    }
}

fn get(sh: &Shared, path: &str) -> Resp {
    match path {
        "" | "index.html" => return Resp::file(HTML, sh.index.clone().into_bytes(), NO_STORE),
        "data/app.js" => return Resp::file(JS, APP_JS.as_bytes().to_vec(), NO_STORE),
        "project.json" => return Resp::file("application/json; charset=utf-8", PROJECT_JSON.as_bytes().to_vec(), NO_STORE),
        _ => {}
    }
    if let Some((_, ct, bytes)) = STATIC.iter().find(|(p, _, _)| *p == path) {
        return Resp::file(ct, bytes.to_vec(), if path.starts_with("lib/") { CACHE_1H } else { NO_STORE });
    }
    let Some(backend) = sh.backend() else {
        return Resp::empty(503); // still building (or failed): the page isn't opened before Ready
    };
    match backend.resolve(path) {
        None => Resp::empty(404),
        Some(Ok(served)) => Resp::file(served.content_type, served.bytes, if served.cacheable { CACHE_1H } else { NO_STORE }),
        Some(Err(e)) => {
            eprintln!("map editor: {path}: {e}");
            Resp::empty(500)
        }
    }
}

fn save(sh: &Shared, req: &Request, mut body: Vec<u8>, s: &mut TcpStream, deadline: Instant) -> Resp {
    // Only our own page may POST: a foreign page can't send application/json without a
    // preflight (which we never answer), and the Origin check covers the rest.
    if req.header("origin").is_some_and(|o| o != sh.origin) {
        return Resp::err(403, "foreign origin");
    }
    let is_json = req.header("content-type").is_some_and(|c| c.split(';').next().is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json")));
    if !is_json {
        return Resp::err(415, "Content-Type must be application/json");
    }
    if req.header("transfer-encoding").is_some() {
        return Resp::err(411, "Content-Length required");
    }
    let Some(len) = req.header("content-length").and_then(|v| v.parse::<usize>().ok()) else {
        return Resp::err(411, "Content-Length required");
    };
    if len > MAX_BODY {
        return Resp::err(413, "file too large");
    }
    let mut chunk = [0u8; 16 * 1024];
    while body.len() < len {
        if Instant::now() > deadline {
            return Resp::err(400, "request timed out");
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return Resp::err(400, "incomplete body"),
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    body.truncate(len);
    let Some(backend) = sh.backend() else { return Resp::err(503, "map data is still being prepared") };
    let Ok(text) = String::from_utf8(body) else { return Resp::err(400, "not UTF-8") };

    let _one_at_a_time = sh.save_lock.lock().unwrap_or_else(|e| e.into_inner());
    // Format, version, known types, and that it is for *this* game's road graph.
    let mut rt = match RoadTypes::validate_for_save(&text, backend.nav()) {
        Ok(rt) => rt,
        Err(e) => {
            sh.emit(MapEvent::Error(format!("Save rejected: {e}")));
            return Resp::err(400, &e);
        }
    };
    // D60: the override replaces the project file wholesale; `based_on` says which project file it
    // was made from (the page never writes it) so a later project update can be noticed.
    rt.based_on = Some(RoadTypes::project_sha1());
    let out = rt.to_json_string();
    if let Err(e) = write_atomic(&sh.paths.override_file, out.as_bytes()) {
        let msg = format!("could not write the road-type file: {e}");
        sh.emit(MapEvent::Error(msg.clone()));
        return Resp::err(500, &msg);
    }
    backend.set_road_types(&rt);
    let (edges, points, bytes) = (rt.types.len() + rt.added.len(), rt.points.len(), out.len());
    sh.state.write().unwrap_or_else(|e| e.into_inner()).current =
        Some(Arc::new(Current { types: rt, source: Source::Override, note: None, project_updated_since_save: false }));
    sh.emit(MapEvent::Saved { edges, points, bytes });
    Resp::json(200, serde_json::json!({ "ok": true, "bytes": bytes }))
}

// ------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamedata::tempdir;
    use sha1::{Digest, Sha1};
    use std::sync::mpsc::{channel, Receiver};

    /// Serves a few canned files; its nav is made to match the project file's `nav` block.
    struct Fake {
        nav: Nav,
        set: Mutex<Vec<usize>>,
    }

    impl Backend for Fake {
        fn resolve(&self, path: &str) -> Option<Result<Served, String>> {
            match path {
                "data/meta.js" => Some(Ok(Served { bytes: b"(window.FH6=window.FH6||{}).meta={};\n".to_vec(), content_type: JS, cacheable: false })),
                "tiles/Summer/0/0/0.jpg" => Some(Ok(Served { bytes: vec![0xff, 0xd8, 0xff], content_type: "image/jpeg", cacheable: true })),
                "tiles/Summer/9/9/9.jpg" => Some(Err("boom".into())),
                _ => None,
            }
        }
        fn nav(&self) -> &Nav {
            &self.nav
        }
        fn set_road_types(&self, rt: &RoadTypes) {
            self.set.lock().unwrap().push(rt.types.len());
        }
    }

    fn project_nav() -> Nav {
        let r = RoadTypes::project().nav.expect("project has a nav block");
        Nav { sha1: r.sha1, nodes: r.nodes as usize, polys: vec![], cls: vec![], hi: vec![], orphans: vec![] }
    }

    struct Rig {
        server: MapServer,
        rx: Receiver<MapEvent>,
        fake: Arc<Fake>,
        dir: PathBuf,
        base: String, // /<token>/
    }

    fn rig(tag: &str) -> Rig {
        let dir = tempdir(tag);
        let (tx, rx) = channel();
        let paths = Paths { override_file: dir.join("map_editor").join("o.json"), port_file: dir.join("map_editor").join("port") };
        let server = MapServer::bind(egui::Context::default(), paths, tx).unwrap();
        let fake = Arc::new(Fake { nav: project_nav(), set: Mutex::new(vec![]) });
        server.install(fake.clone(), RoadTypes::current(&dir.join("none.json"), &fake.nav));
        let base = format!("/{}/", server.url().trim_end_matches('/').rsplit('/').next().unwrap());
        Rig { server, rx, fake, dir, base }
    }

    struct Reply {
        status: u16,
        headers: String,
        body: Vec<u8>,
    }

    impl Reply {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }
        fn header(&self, name: &str) -> Option<String> {
            self.headers.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string()))
        }
    }

    /// Send raw bytes to the server, read until it closes.
    fn raw(port: u16, req: &[u8]) -> Reply {
        let mut c = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        c.write_all(req).unwrap();
        let mut all = Vec::new();
        let _ = c.read_to_end(&mut all);
        let split = all.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or_else(|| panic!("no response head: {all:?}"));
        let head = String::from_utf8_lossy(&all[..split]).into_owned();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        Reply { status, headers: head, body: all[split + 4..].to_vec() }
    }

    impl Rig {
        fn host(&self) -> String {
            format!("127.0.0.1:{}", self.server.port())
        }
        fn get_with(&self, target: &str, host: &str) -> Reply {
            raw(self.server.port(), format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes())
        }
        fn get(&self, path: &str) -> Reply {
            self.get_with(&format!("{}{}", self.base, path), &self.host())
        }
        fn post_with(&self, path: &str, extra: &str, body: &[u8]) -> Reply {
            let head = format!("POST {}{} HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\n{extra}\r\n", self.base, path, self.host(), body.len());
            let mut req = head.into_bytes();
            req.extend_from_slice(body);
            raw(self.server.port(), &req)
        }
        fn save(&self, body: &[u8]) -> Reply {
            self.post_with("save", "Content-Type: application/json\r\n", body)
        }
        fn override_file(&self) -> PathBuf {
            self.dir.join("map_editor").join("o.json")
        }
    }

    #[test]
    fn project_json_matches_roadtypes_embed() {
        let sha = Sha1::digest(PROJECT_JSON.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(sha, RoadTypes::project_sha1());
    }

    #[test]
    fn token_is_required() {
        let r = rig("tok");
        for target in ["/", "/index.html", "/data/meta.js", "/wrongtoken/", "/wrongtoken/data/meta.js"] {
            let rep = r.get_with(target, &r.host());
            assert_eq!(rep.status, 404, "{target}");
            assert!(rep.body.is_empty(), "{target}");
        }
        // Prefix of the token, token without the slash, token as a query, longer token.
        let tok = r.base.trim_matches('/').to_string();
        for t in [format!("/{}/", &tok[..tok.len() - 1]), format!("/{tok}"), format!("/?{tok}"), format!("/{tok}x/"), format!("/x/{tok}/")] {
            assert_eq!(r.get_with(&t, &r.host()).status, 404, "{t}");
        }
        assert_eq!(r.get("data/meta.js").status, 200);
        assert_eq!(tok.len(), 32);
        assert!(r.server.url().starts_with(&format!("http://127.0.0.1:{}/", r.server.port())));
    }

    #[test]
    fn host_must_be_ours() {
        let r = rig("host");
        let t = format!("{}data/meta.js", r.base);
        assert_eq!(r.get_with(&t, "evil.example").status, 400);
        assert_eq!(r.get_with(&t, &format!("localhost:{}", r.server.port())).status, 400);
        assert_eq!(r.get_with(&t, "127.0.0.1").status, 400);
        // No Host at all, and two Hosts.
        assert_eq!(raw(r.server.port(), format!("GET {t} HTTP/1.0\r\n\r\n").as_bytes()).status, 400);
        let h = r.host();
        assert_eq!(raw(r.server.port(), format!("GET {t} HTTP/1.1\r\nHost: {h}\r\nHost: evil\r\n\r\n").as_bytes()).status, 400);
        assert_eq!(r.get_with(&t, &h).status, 200);
    }

    #[test]
    fn post_needs_json_and_our_origin() {
        let r = rig("post");
        let body = RoadTypes::project().to_json_string();
        // No / wrong content type (a "simple" cross-site form POST).
        assert_eq!(r.post_with("save", "", body.as_bytes()).status, 415);
        assert_eq!(r.post_with("save", "Content-Type: text/plain\r\n", body.as_bytes()).status, 415);
        assert_eq!(r.post_with("save", "Content-Type: application/x-www-form-urlencoded\r\n", body.as_bytes()).status, 415);
        // Foreign origins, including a different port on loopback and "null".
        for o in ["http://evil.example", "http://127.0.0.1:1", "null", "http://localhost"] {
            let rep = r.post_with("save", &format!("Content-Type: application/json\r\nOrigin: {o}\r\n"), body.as_bytes());
            assert_eq!(rep.status, 403, "{o}");
        }
        assert!(!r.override_file().exists());
        // Our own origin passes (and charset parameters are fine).
        let ok = r.post_with("save", &format!("Content-Type: application/json; charset=utf-8\r\nOrigin: http://{}\r\n", r.host()), body.as_bytes());
        assert_eq!(ok.status, 200, "{}", ok.text());
        // Not a POST target.
        assert_eq!(r.post_with("data/meta.js", "Content-Type: application/json\r\n", b"{}").status, 404);
        assert_eq!(r.get("save").status, 404);
    }

    #[test]
    fn options_and_other_methods_are_refused_without_cors() {
        let r = rig("opt");
        for m in ["OPTIONS", "PUT", "DELETE", "HEAD", "PATCH"] {
            let rep = raw(r.server.port(), format!("{m} {}save HTTP/1.1\r\nHost: {}\r\nOrigin: http://evil.example\r\nAccess-Control-Request-Method: POST\r\n\r\n", r.base, r.host()).as_bytes());
            assert_eq!(rep.status, 405, "{m}");
            assert!(rep.headers.to_ascii_lowercase().find("access-control").is_none(), "{m}: CORS header sent");
        }
        // Without the token it's the usual empty 404.
        assert_eq!(raw(r.server.port(), format!("OPTIONS /save HTTP/1.1\r\nHost: {}\r\n\r\n", r.host()).as_bytes()).status, 404);
    }

    #[test]
    fn index_has_data_scripts_and_assets_are_typed() {
        let r = rig("idx");
        for p in ["", "index.html"] {
            let rep = r.get(p);
            assert_eq!(rep.status, 200);
            let t = rep.text();
            assert!(!t.contains(DATA_MARKER), "marker left in the page");
            let mut at = 0;
            for n in DATA_SCRIPTS {
                let tag = format!("<script charset=\"utf-8\" src=\"data/{n}.js\"></script>");
                let i = t[at..].find(&tag).unwrap_or_else(|| panic!("{tag} missing or out of order"));
                at += i + tag.len();
            }
            assert!(t.contains("data/app.js"));
            assert_eq!(rep.header("content-type").unwrap(), HTML);
        }
        let js = r.get("lib/leaflet.js");
        assert_eq!(js.status, 200);
        assert_eq!(js.header("content-type").unwrap(), JS);
        assert!(js.body.len() > 100_000);
        assert_eq!(r.get("lib/images/layers.png").header("content-type").unwrap(), PNG);
        assert_eq!(r.get("lib/leaflet.css").header("content-type").unwrap(), CSS);
        assert_eq!(r.get("preview-3d.html").status, 200);
        assert_eq!(r.get("lib/three.min.js").status, 200);
        assert_eq!(r.get("data/app.js").text(), APP_JS);
        assert_eq!(r.get("project.json").text(), PROJECT_JSON);
        // Every static table entry serves.
        for (p, _, b) in STATIC {
            assert_eq!(r.get(p).body.len(), b.len(), "{p}");
        }
    }

    #[test]
    fn response_headers() {
        let r = rig("hdr");
        for path in ["", "data/meta.js", "tiles/Summer/0/0/0.jpg", "lib/leaflet.js", "nothing-here"] {
            let rep = r.get(path);
            assert_eq!(rep.header("x-content-type-options").unwrap(), "nosniff", "{path}");
            assert_eq!(rep.header("referrer-policy").unwrap(), "no-referrer", "{path}");
            assert_eq!(rep.header("x-frame-options").unwrap(), "SAMEORIGIN", "{path}");
            assert_eq!(rep.header("content-security-policy").unwrap(), CSP, "{path}");
            assert!(rep.header("access-control-allow-origin").is_none(), "{path}");
        }
        assert_eq!(r.get("data/meta.js").header("cache-control").unwrap(), "no-store");
        assert_eq!(r.get("").header("cache-control").unwrap(), "no-store");
        assert_eq!(r.get("tiles/Summer/0/0/0.jpg").header("cache-control").unwrap(), "private, max-age=3600");
        assert_eq!(r.get("tiles/Summer/9/9/9.jpg").status, 500);
    }

    #[test]
    fn path_traversal_never_reaches_the_filesystem() {
        let r = rig("trav");
        let secret = r.dir.join("secret.txt");
        std::fs::write(&secret, "SECRET").unwrap();
        let abs = secret.to_string_lossy().to_string();
        for p in [
            "../secret.txt",
            "..%2fsecret.txt",
            "%2e%2e/secret.txt",
            "%2e%2e%2fsecret.txt",
            "lib/../../Cargo.toml",
            "lib/%2e%2e/%2e%2e/Cargo.toml",
            "lib/..\\..\\Cargo.toml",
            "lib/leaflet.js/..",
            "lib//leaflet.js",
            "lib/",
            "lib",
            "src/main.rs",
            "tiles/../../etc/passwd",
            "tiles/Summer/0/0/0.jpg/../1.jpg",
            "data/meta.js%00.png",
            &abs,
            &format!("/{abs}"),
        ] {
            let rep = r.get(p);
            assert_eq!(rep.status, 404, "{p}: {}", rep.text());
            assert!(!rep.text().contains("SECRET"), "{p}");
        }
        // Raw (un-normalised) request targets, and absolute-form / authority-form targets.
        let h = r.host();
        let tok = &r.base;
        for t in [
            format!("/{}..{}secret.txt", tok.trim_end_matches('/'), "/"),
            format!("{tok}../../etc/passwd"),
            format!("http://{h}{tok}data/meta.js"),
            h.clone(),
            "*".to_string(),
            "/../../etc/passwd".to_string(),
            "//etc/passwd".to_string(),
        ] {
            let rep = r.get_with(&t, &h);
            assert!(matches!(rep.status, 400 | 404), "{t}: {}", rep.status);
            assert!(rep.body.is_empty() || rep.status == 400, "{t}");
        }
    }

    #[test]
    fn malformed_and_huge_heads() {
        let r = rig("head");
        let port = r.server.port();
        assert_eq!(raw(port, b"garbage\r\n\r\n").status, 400);
        assert_eq!(raw(port, b"GET / HTTP/1.1 extra\r\n\r\n").status, 400);
        assert_eq!(raw(port, format!("GET {} HTTP/1.1\r\nHost: {}\r\nNoColonHere\r\n\r\n", r.base, r.host()).as_bytes()).status, 400);
        let mut big = format!("GET {}data/meta.js HTTP/1.1\r\nHost: {}\r\nX-Pad: ", r.base, r.host()).into_bytes();
        big.extend(std::iter::repeat_n(b'a', 20 * 1024));
        big.extend_from_slice(b"\r\n\r\n");
        assert_eq!(raw(port, &big).status, 431);
        // The server is still fine afterwards.
        assert_eq!(r.get("data/meta.js").status, 200);
    }

    #[test]
    fn oversize_body_is_413_without_reading_it() {
        let r = rig("big");
        // Claim 17 MB, send only the head: refused up front.
        let rep = r.post_with_len("Content-Type: application/json\r\n", MAX_BODY + 1);
        assert_eq!(rep.status, 413);
        assert!(!r.override_file().exists());
        // Missing Content-Length / chunked.
        let raw_req = format!("POST {}save HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n\r\n", r.base, r.host());
        assert_eq!(raw(r.server.port(), raw_req.as_bytes()).status, 411);
        let chunked = format!("POST {}save HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", r.base, r.host());
        assert_eq!(raw(r.server.port(), chunked.as_bytes()).status, 411);
        // Truncated body (client closes early).
        let short = format!("POST {}save HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{\"a\":1}}", r.base, r.host());
        let mut c = TcpStream::connect((Ipv4Addr::LOCALHOST, r.server.port())).unwrap();
        c.write_all(short.as_bytes()).unwrap();
        c.shutdown(Shutdown::Write).unwrap();
        let mut out = String::new();
        let _ = c.read_to_string(&mut out);
        assert!(out.starts_with("HTTP/1.1 400"), "{out}");
    }

    impl Rig {
        fn post_with_len(&self, extra: &str, len: usize) -> Reply {
            let head = format!("POST {}save HTTP/1.1\r\nHost: {}\r\nContent-Length: {len}\r\n{extra}\r\n", self.base, self.host());
            raw(self.server.port(), head.as_bytes())
        }
    }

    #[test]
    fn save_project_file_stamps_based_on_and_keeps_races() {
        let r = rig("save");
        assert!(matches!(r.rx.try_recv(), Ok(MapEvent::Ready { .. })));
        let project = RoadTypes::project();
        let n_types = project.types.len();
        assert_eq!(project.races.len(), 94, "the project file carries 94 race marks");
        let rep = r.save(project.to_json_string().as_bytes());
        assert_eq!(rep.status, 200, "{}", rep.text());
        let v: serde_json::Value = serde_json::from_slice(&rep.body).unwrap();
        assert_eq!(v["ok"], true);
        let written = std::fs::read_to_string(r.override_file()).unwrap();
        assert_eq!(v["bytes"].as_u64().unwrap() as usize, written.len());
        let back = RoadTypes::parse(&written).unwrap();
        assert_eq!(back.races.len(), 94);
        assert_eq!(back.races, project.races);
        assert_eq!(back.based_on.as_deref(), Some(RoadTypes::project_sha1().as_str()));
        assert_eq!(back.types.len(), n_types);
        assert!(written.contains(&format!("\"based_on\":\"{}\"", RoadTypes::project_sha1())));
        // The override is now what "current" returns, with nothing stale about it.
        let cur = RoadTypes::current(&r.override_file(), &r.fake.nav);
        assert_eq!(cur.source, Source::Override);
        assert!(!cur.project_updated_since_save);
        // Backend + shared state + event.
        assert_eq!(r.fake.set.lock().unwrap().as_slice(), &[n_types]);
        let c = r.server.current().unwrap();
        assert_eq!(c.source, Source::Override);
        assert_eq!(c.types.types.len(), n_types);
        assert!(c.note.is_none() && !c.project_updated_since_save);
        match r.rx.try_recv() {
            Ok(MapEvent::Saved { edges, points, bytes }) => {
                assert_eq!(edges, n_types + project.added.len());
                assert_eq!(points, project.points.len());
                assert_eq!(bytes, written.len());
            }
            other => panic!("expected Saved, got {other:?}"),
        }
        // A client-supplied based_on is replaced, not trusted.
        let mut forged = project.clone();
        forged.based_on = Some("0".repeat(40));
        assert_eq!(r.save(forged.to_json_string().as_bytes()).status, 200);
        let back = RoadTypes::parse(&std::fs::read_to_string(r.override_file()).unwrap()).unwrap();
        assert_eq!(back.based_on.as_deref(), Some(RoadTypes::project_sha1().as_str()));
        // No temp files left behind.
        let left: Vec<_> = std::fs::read_dir(r.dir.join("map_editor")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(left.iter().all(|f| f == "o.json" || f == "port"), "{left:?}");
    }

    #[test]
    fn invalid_save_is_400_and_writes_nothing() {
        let r = rig("bad");
        let _ = r.rx.try_recv(); // Ready
        let mut wrong_nav = RoadTypes::project();
        wrong_nav.nav.as_mut().unwrap().sha1 = "0".repeat(40);
        let mut no_nav = RoadTypes::project();
        no_nav.nav = None;
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("garbage", b"this is not json".to_vec()),
            ("empty", Vec::new()),
            ("wrong format", br#"{"format":"other","version":2}"#.to_vec()),
            ("wrong version", br#"{"format":"fh6-road-types","version":9}"#.to_vec()),
            ("wrong nav", wrong_nav.to_json_string().into_bytes()),
            ("no nav", no_nav.to_json_string().into_bytes()),
            ("not utf-8", vec![0xff, 0xfe, 0xfd]),
        ];
        for (what, body) in cases {
            let rep = r.save(&body);
            assert_eq!(rep.status, 400, "{what}: {}", rep.text());
            assert!(!r.override_file().exists(), "{what}: file written");
            if what != "not utf-8" {
                assert!(serde_json::from_slice::<serde_json::Value>(&rep.body).unwrap()["error"].is_string(), "{what}");
            }
        }
        assert!(r.fake.set.lock().unwrap().is_empty());
        assert_eq!(r.server.current().unwrap().source, Source::Project);
        assert!(matches!(r.rx.try_recv(), Ok(MapEvent::Error(_))));
        // A good override must survive a later bad save untouched.
        assert_eq!(r.save(RoadTypes::project().to_json_string().as_bytes()).status, 200);
        let before = std::fs::read(r.override_file()).unwrap();
        assert_eq!(r.save(b"{}").status, 400);
        assert_eq!(std::fs::read(r.override_file()).unwrap(), before);
    }

    #[test]
    fn preparing_server_answers_503_then_serves() {
        let dir = tempdir("prep");
        let (tx, rx) = channel();
        let paths = Paths { override_file: dir.join("o.json"), port_file: dir.join("port") };
        let server = MapServer::bind(egui::Context::default(), paths, tx).unwrap();
        assert!(matches!(server.state(), MapServerState::Preparing { .. }));
        let base = server.url().trim_end_matches('/').rsplit('/').next().unwrap().to_string();
        let host = format!("127.0.0.1:{}", server.port());
        let get = |p: &str| raw(server.port(), format!("GET /{base}/{p} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes()).status;
        assert_eq!(get("data/meta.js"), 503);
        assert_eq!(get("index.html"), 200); // embedded, needs no data
        assert_eq!(get("nothing"), 503);
        let fake = Arc::new(Fake { nav: project_nav(), set: Mutex::new(vec![]) });
        server.install(fake.clone(), RoadTypes::current(&dir.join("none.json"), &fake.nav));
        assert_eq!(server.state(), MapServerState::Ready);
        assert_eq!(get("data/meta.js"), 200);
        assert_eq!(get("nothing"), 404);
        match rx.try_recv() {
            Ok(MapEvent::Ready { url }) => assert_eq!(url, server.url()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sticky_port_is_reused_and_drop_frees_it() {
        let dir = tempdir("sticky");
        let paths = Paths { override_file: dir.join("o.json"), port_file: dir.join("sub").join("port") };
        let (tx, _rx) = channel();
        let a = MapServer::bind(egui::Context::default(), paths.clone(), tx.clone()).unwrap();
        let (port, url_a) = (a.port(), a.url().to_string());
        assert_eq!(std::fs::read_to_string(&paths.port_file).unwrap(), port.to_string());
        // While A runs, a second server can't have the port: it falls back to another free one.
        let b = MapServer::bind(egui::Context::default(), paths.clone(), tx.clone()).unwrap();
        assert_ne!(b.port(), port);
        drop(b);
        // B rewrote the port file; put A's port back as the previous session would have left it.
        std::fs::write(&paths.port_file, port.to_string()).unwrap();
        drop(a); // stops the accept thread and releases the port
        assert!(TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_millis(300)).is_err(), "port still open after drop");
        let c = MapServer::bind(egui::Context::default(), paths.clone(), tx).unwrap();
        assert_eq!(c.port(), port, "sticky port not reused");
        assert_ne!(c.url(), url_a, "the token must change every session");
        // Garbage in the port file is ignored.
        std::fs::write(&paths.port_file, "not a port").unwrap();
        let (tx2, _rx2) = channel();
        let d = MapServer::bind(egui::Context::default(), paths, tx2).unwrap();
        assert_ne!(d.port(), 0);
    }

    #[test]
    fn concurrent_requests() {
        let r = rig("conc");
        let (port, base, host) = (r.server.port(), r.base.clone(), r.host());
        let hs: Vec<_> = (0..24)
            .map(|_| {
                let (base, host) = (base.clone(), host.clone());
                thread::spawn(move || raw(port, format!("GET {base}lib/leaflet.js HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes()).status)
            })
            .collect();
        for h in hs {
            assert_eq!(h.join().unwrap(), 200);
        }
    }

    /// End to end against the real install; also the harness the Playwright check uses (see
    /// `docs/game-data/fh6-map-tooling.md`). Run with
    /// `FORZA_DATA_DIR=<scratch> cargo test --release real_install -- --ignored --nocapture`;
    /// `MAPEDIT_SERVE_SECS=<n>` keeps the server up for n seconds and prints `MAPEDIT_URL=...`.
    #[test]
    #[ignore = "needs the FH6 install; writes the cache under FORZA_DATA_DIR"]
    fn real_install_serves_the_editor() {
        let data_dir = std::env::var_os("FORZA_DATA_DIR").expect("set FORZA_DATA_DIR to a scratch folder (never your real app data)");
        assert!(!data_dir.is_empty());
        let media = crate::gamedata::install::find_media(None).expect("FH6 install");
        let (tx, rx) = channel();
        let t0 = Instant::now();
        let server = MapServer::start(egui::Context::default(), media, StartFrom::Current, tx).unwrap();
        let url = match rx.recv_timeout(Duration::from_secs(600)).expect("Ready") {
            MapEvent::Ready { url } => url,
            other => panic!("{other:?}"),
        };
        println!("MAPEDIT_READY_MS={}", t0.elapsed().as_millis());
        assert_eq!(url, server.url());
        let host = format!("127.0.0.1:{}", server.port());
        let path = url.strip_prefix(&format!("http://{host}")).unwrap().to_string();
        let get = |p: &str| raw(server.port(), format!("GET {path}{p} HTTP/1.1\r\nHost: {host}\r\n\r\n").as_bytes());
        for p in ["", "data/meta.js", "data/roads.js", "data/roaded.js", "data/canon.js", "data/elevation.js", "preview3d/meta.js", "preview3d/terrain.js", "preview3d/roads.js", "project.json"] {
            let t = Instant::now();
            let rep = get(p);
            assert_eq!(rep.status, 200, "{p}");
            println!("{p}: {} bytes, {} ms", rep.body.len(), t.elapsed().as_millis());
        }
        let tile = get("tiles/Summer/0/0/0.jpg");
        assert_eq!(tile.status, 200);
        assert_eq!(&tile.body[..2], &[0xff, 0xd8]);
        assert_eq!(get("preview3d/tex_Summer.jpg").status, 200);
        assert_eq!(get("tiles/Winter/9/0/0.jpg").status, 404);
        if let Some(secs) = std::env::var("MAPEDIT_SERVE_SECS").ok().and_then(|s| s.parse::<u64>().ok()) {
            println!("MAPEDIT_URL={url}");
            let end = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < end {
                if let Ok(ev) = rx.recv_timeout(Duration::from_millis(200)) {
                    println!("MAPEDIT_EVENT={ev:?}");
                }
            }
        }
    }
}
