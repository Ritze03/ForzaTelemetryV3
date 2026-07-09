//! Co-Op: share live telemetry between players over a WebSocket relay, exposed
//! publicly via a cloudflared quick tunnel (no login). One player Hosts; others
//! Join by typing the tunnel's word-slug (e.g. `blue-fox-rapid-owl`).
//!
//! Transport: raw 324-byte FH6 packets in binary WS frames, prefixed with the
//! sender's 16-byte UUID. Roster/identity travels as small JSON text frames.
//! The host is authoritative: it mints a UUID per player and owns the roster.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use uuid::Uuid;

use crate::packet::ForzaPacket;

/// Local port the host's WS server listens on (and cloudflared points at).
pub const DEFAULT_COOP_PORT: u16 = 7071;
const WIRE_LEN: usize = 324; // one FH6 packet
const ID_LEN: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Off,
    Host,
    Client,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: String,
    pub name: String,
    pub hue: f32, // 0..360
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "t")]
enum Control {
    /// Client → host on connect.
    Hello { name: String, hue: f32 },
    /// Host → client right after Hello.
    Welcome { id: String, roster: Vec<PlayerInfo> },
    /// Host → all when the roster changes.
    Roster { players: Vec<PlayerInfo> },
    /// Client → host: change my name/hue.
    Update { name: String, hue: f32 },
    /// A shared map waypoint (world [x,z] + setter's hue), or `None` to clear it.
    /// Either direction; the host re-broadcasts a client's waypoint to everyone.
    /// (Option — not NaN — so it survives JSON, which has no NaN.)
    Waypoint { pos: Option<[f32; 2]>, hue: f32 },
}

/// Per-remote jitter buffer: timestamped packets awaiting playback.
struct RemoteBuf {
    q: VecDeque<(Instant, ForzaPacket)>,
    current: Option<ForzaPacket>,
    last_recv: Instant,
}

impl RemoteBuf {
    fn new() -> Self {
        Self { q: VecDeque::new(), current: None, last_recv: Instant::now() }
    }
    fn push(&mut self, pkt: ForzaPacket) {
        self.last_recv = Instant::now();
        self.q.push_back((self.last_recv, pkt));
        if self.q.len() > 240 {
            self.q.pop_front();
        }
    }
    /// Advance playback to `now - delay`, keeping the newest eligible packet.
    fn advance(&mut self, delay: Duration) {
        let target = Instant::now().checked_sub(delay).unwrap_or_else(Instant::now);
        while let Some(&(t, _)) = self.q.front() {
            if t <= target {
                self.current = Some(self.q.pop_front().unwrap().1);
            } else {
                break;
            }
        }
        // With no delay (or a starved buffer) fall straight to the latest sample.
        if delay.is_zero() {
            if let Some((_, p)) = self.q.pop_back() {
                self.current = Some(p);
                self.q.clear();
            }
        }
    }
}

struct Inner {
    role: Role,
    my_id: String,
    my_id_bytes: [u8; ID_LEN],
    roster: Vec<PlayerInfo>,
    remote: HashMap<String, RemoteBuf>,
    /// Host: one outgoing channel per connected client, for broadcasting.
    clients: Vec<(String, SyncSender<Message>)>,
    /// Client: outgoing channel to the host.
    client_out: Option<SyncSender<Message>>,
    status: String,
    error: Option<String>,
    words: Option<String>,
    lan_url: Option<String>,
    buffer_ms: u32,
    /// Shared map waypoint: (world_x, world_z, setter hue).
    waypoint: Option<(f32, f32, f32)>,
}

/// Best-effort local LAN IP (the address a same-network peer would reach us on).
/// Uses the "connect a UDP socket to pick the outbound route" trick — no packets sent.
fn local_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_loopback() { None } else { Some(ip.to_string()) }
}

impl Inner {
    /// Host: send a message to every client except `skip` (None = all).
    /// Bounded per-client queue: on backpressure we drop this frame (telemetry is
    /// fine to skip — the next one is 16ms away) rather than buffer unboundedly.
    fn broadcast(&mut self, msg: Message, skip: Option<&str>) {
        self.clients.retain(|(id, tx)| {
            if Some(id.as_str()) == skip {
                return true;
            }
            !matches!(tx.try_send(msg.clone()), Err(mpsc::TrySendError::Disconnected(_)))
        });
    }
    fn roster_msg(&self) -> Message {
        let c = Control::Roster { players: self.roster.clone() };
        Message::Text(serde_json::to_string(&c).unwrap_or_default())
    }
}

pub struct CoopState {
    inner: Arc<Mutex<Inner>>,
    stop: Arc<AtomicBool>,
    tunnel: Option<Child>,
    pub port: u16,
}

impl CoopState {
    pub fn new(name: &str, hue: f32, buffer_ms: u32) -> Self {
        let id = Uuid::new_v4();
        let inner = Inner {
            role: Role::Off,
            my_id: id.to_string(),
            my_id_bytes: *id.as_bytes(),
            roster: Vec::new(),
            remote: HashMap::new(),
            clients: Vec::new(),
            client_out: None,
            status: String::new(),
            error: None,
            words: None,
            lan_url: None,
            buffer_ms,
            waypoint: None,
            // seed identity even while Off so the UI preview is stable
        };
        let _ = (name, hue);
        Self {
            inner: Arc::new(Mutex::new(inner)),
            stop: Arc::new(AtomicBool::new(false)),
            tunnel: None,
            port: DEFAULT_COOP_PORT,
        }
    }

    pub fn role(&self) -> Role {
        self.inner.lock().unwrap().role
    }
    pub fn status(&self) -> String {
        self.inner.lock().unwrap().status.clone()
    }
    pub fn error(&self) -> Option<String> {
        self.inner.lock().unwrap().error.clone()
    }
    pub fn words(&self) -> Option<String> {
        self.inner.lock().unwrap().words.clone()
    }
    pub fn lan_url(&self) -> Option<String> {
        self.inner.lock().unwrap().lan_url.clone()
    }
    pub fn my_id(&self) -> String {
        self.inner.lock().unwrap().my_id.clone()
    }
    pub fn roster(&self) -> Vec<PlayerInfo> {
        self.inner.lock().unwrap().roster.clone()
    }
    pub fn set_buffer_ms(&self, ms: u32) {
        self.inner.lock().unwrap().buffer_ms = ms;
    }

    /// The active shared waypoint (world_x, world_z, hue), if any.
    pub fn waypoint(&self) -> Option<(f32, f32, f32)> {
        self.inner.lock().unwrap().waypoint
    }

    /// Drop a shared waypoint at a world position (hue = our colour). Pass `None` to clear.
    pub fn set_waypoint(&self, pos: Option<(f32, f32)>, hue: f32) {
        let mut inner = self.inner.lock().unwrap();
        if inner.role == Role::Off {
            return;
        }
        inner.waypoint = pos.map(|(x, z)| (x, z, hue));
        let msg = Message::Text(
            serde_json::to_string(&Control::Waypoint { pos: pos.map(|(x, z)| [x, z]), hue })
                .unwrap_or_default(),
        );
        match inner.role {
            Role::Host => inner.broadcast(msg, None),
            Role::Client => {
                if let Some(tx) = &inner.client_out {
                    let _ = tx.try_send(msg);
                }
            }
            Role::Off => {}
        }
    }

    /// Push the locally-received telemetry packet out to peers.
    pub fn push_local(&self, pkt: &ForzaPacket) {
        let mut inner = self.inner.lock().unwrap();
        if inner.role == Role::Off {
            return;
        }
        let mut frame = Vec::with_capacity(ID_LEN + WIRE_LEN);
        frame.extend_from_slice(&inner.my_id_bytes);
        frame.extend_from_slice(&pkt.to_bytes());
        let msg = Message::Binary(frame);
        match inner.role {
            Role::Host => inner.broadcast(msg, None),
            Role::Client => {
                if let Some(tx) = &inner.client_out {
                    let _ = tx.try_send(msg);
                }
            }
            Role::Off => {}
        }
    }

    /// Update my displayed identity; propagate to peers.
    pub fn update_identity(&self, name: &str, hue: f32) {
        let mut inner = self.inner.lock().unwrap();
        let my_id = inner.my_id.clone();
        match inner.role {
            Role::Host => {
                if let Some(p) = inner.roster.iter_mut().find(|p| p.id == my_id) {
                    p.name = name.to_string();
                    p.hue = hue;
                }
                let msg = inner.roster_msg();
                inner.broadcast(msg, None);
            }
            Role::Client => {
                if let Some(tx) = &inner.client_out {
                    let c = Control::Update { name: name.to_string(), hue };
                    let _ = tx.try_send(Message::Text(serde_json::to_string(&c).unwrap_or_default()));
                }
            }
            Role::Off => {}
        }
    }

    /// Advance every jitter buffer; call once per frame before rendering.
    pub fn tick(&self) {
        let mut inner = self.inner.lock().unwrap();
        let delay = Duration::from_millis(inner.buffer_ms as u64);
        for buf in inner.remote.values_mut() {
            buf.advance(delay);
        }
    }

    /// Snapshot of remote players that have a recent packet, for the minimap.
    pub fn remote_players(&self) -> Vec<(PlayerInfo, ForzaPacket)> {
        let inner = self.inner.lock().unwrap();
        let mut out = Vec::new();
        for info in &inner.roster {
            if info.id == inner.my_id {
                continue;
            }
            if let Some(buf) = inner.remote.get(&info.id) {
                if buf.last_recv.elapsed() < Duration::from_secs(3) {
                    if let Some(pkt) = &buf.current {
                        out.push((info.clone(), pkt.clone()));
                    }
                }
            }
        }
        out
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.tunnel.take() {
            let _ = child.kill();
        }
        let mut inner = self.inner.lock().unwrap();
        inner.role = Role::Off;
        inner.clients.clear();
        inner.client_out = None;
        inner.remote.clear();
        inner.roster.clear();
        inner.words = None;
        inner.lan_url = None;
        inner.waypoint = None;
        inner.status = "Stopped".into();
        inner.error = None;
    }

    /// Start hosting: WS server + cloudflared quick tunnel. The server still
    /// comes up (usable on LAN) even if cloudflared is missing.
    pub fn start_host(&mut self, port: u16, name: &str, hue: f32, buffer_ms: u32) {
        self.stop();
        self.stop = Arc::new(AtomicBool::new(false));
        self.port = port;

        {
            let mut inner = self.inner.lock().unwrap();
            inner.role = Role::Host;
            inner.buffer_ms = buffer_ms;
            inner.status = "Starting server…".into();
            inner.error = None;
            inner.words = None;
            inner.lan_url = local_ip().map(|ip| format!("ws://{ip}:{port}"));
            inner.remote.clear();
            inner.clients.clear();
            inner.roster = vec![PlayerInfo {
                id: inner.my_id.clone(),
                name: name.to_string(),
                hue,
            }];
        }

        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => {
                let mut inner = self.inner.lock().unwrap();
                inner.role = Role::Off;
                inner.error = Some(format!("bind :{port} failed: {e}"));
                return;
            }
        };
        listener.set_nonblocking(true).ok();

        let inner = self.inner.clone();
        let stop = self.stop.clone();
        std::thread::spawn(move || host_accept_loop(listener, inner, stop));

        // cloudflared quick tunnel → public wss URL
        match ensure_cloudflared()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))
            .and_then(|bin| spawn_tunnel(&bin, port, self.inner.clone(), self.stop.clone()))
        {
            Ok(child) => self.tunnel = Some(child),
            Err(e) => {
                let mut inner = self.inner.lock().unwrap();
                inner.error = Some(format!("cloudflared: {e}"));
                inner.status = "Server up (LAN only — no tunnel)".into();
            }
        }
    }

    /// Join a hosted session by its word-slug.
    pub fn start_client(&mut self, words: &str, name: &str, hue: f32, buffer_ms: u32) {
        self.stop();
        self.stop = Arc::new(AtomicBool::new(false));
        let words = words.trim().trim_matches('/').to_string();

        {
            let mut inner = self.inner.lock().unwrap();
            inner.role = Role::Client;
            inner.buffer_ms = buffer_ms;
            inner.status = "Connecting…".into();
            inner.error = None;
            inner.words = Some(words.clone());
            inner.remote.clear();
            inner.roster.clear();
            inner.client_out = None;
        }

        let url = words_to_url(&words);
        let inner = self.inner.clone();
        let stop = self.stop.clone();
        let name = name.to_string();
        std::thread::spawn(move || client_loop(url, name, hue, inner, stop));
    }
}

impl Drop for CoopState {
    fn drop(&mut self) {
        self.stop();
    }
}

/// `blue-fox-rapid-owl` → `wss://blue-fox-rapid-owl.trycloudflare.com/ws`.
/// Also accepts a full URL pasted in.
fn words_to_url(words: &str) -> String {
    let w = words.trim();
    if w.starts_with("http") || w.starts_with("ws") {
        // normalize http(s)→ws(s)
        let w = w.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
        return if w.contains("/ws") { w } else { format!("{}/ws", w.trim_end_matches('/')) };
    }
    // strip a trailing .trycloudflare.com if the user pasted the host
    let slug = w.trim_end_matches(".trycloudflare.com");
    format!("wss://{slug}.trycloudflare.com/ws")
}

// ── Host ───────────────────────────────────────────────────────────

fn host_accept_loop(listener: TcpListener, inner: Arc<Mutex<Inner>>, stop: Arc<AtomicBool>) {
    inner.lock().unwrap().status = "Waiting for players…".into();
    for stream in listener.incoming() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match stream {
            Ok(s) => {
                let inner = inner.clone();
                let stop = stop.clone();
                std::thread::spawn(move || host_client(s, inner, stop));
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(_) => break,
        }
    }
}

fn host_client(stream: TcpStream, inner: Arc<Mutex<Inner>>, stop: Arc<AtomicBool>) {
    stream.set_nodelay(true).ok();
    let mut ws = match tungstenite::accept(stream) {
        Ok(ws) => ws,
        Err(_) => return, // not a websocket (e.g. a browser hit the URL) — ignore
    };
    ws.get_mut()
        .set_read_timeout(Some(Duration::from_millis(20)))
        .ok();

    // First message must be Hello.
    let (name, hue) = match read_hello(&mut ws) {
        Some(v) => v,
        None => return,
    };
    let id = Uuid::new_v4().to_string();
    let id_bytes = *Uuid::parse_str(&id).unwrap().as_bytes();
    let (tx, rx) = mpsc::sync_channel::<Message>(256);

    // Register + welcome + roster broadcast.
    {
        let mut g = inner.lock().unwrap();
        g.clients.push((id.clone(), tx));
        g.roster.push(PlayerInfo { id: id.clone(), name, hue });
        g.remote.insert(id.clone(), RemoteBuf::new());
        let welcome = Control::Welcome { id: id.clone(), roster: g.roster.clone() };
        let _ = ws.write(Message::Text(
            serde_json::to_string(&welcome).unwrap_or_default(),
        ));
        let msg = g.roster_msg();
        g.broadcast(msg, None);
        g.status = format!("{} player(s)", g.roster.len());
    }

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // Outgoing (relayed packets + roster updates).
        let mut wrote = false;
        while let Ok(m) = rx.try_recv() {
            if ws.write(m).is_err() {
                cleanup_client(&inner, &id);
                return;
            }
            wrote = true;
        }
        if wrote {
            let _ = ws.flush();
        }
        // Incoming.
        match ws.read() {
            Ok(Message::Binary(data)) if data.len() >= ID_LEN + WIRE_LEN => {
                if let Some(pkt) = ForzaPacket::from_bytes(&data[ID_LEN..]) {
                    let mut g = inner.lock().unwrap();
                    if let Some(buf) = g.remote.get_mut(&id) {
                        buf.push(pkt);
                    }
                    // Relay with the host-assigned id (anti-spoof).
                    let mut frame = Vec::with_capacity(data.len());
                    frame.extend_from_slice(&id_bytes);
                    frame.extend_from_slice(&data[ID_LEN..]);
                    g.broadcast(Message::Binary(frame), Some(&id));
                }
            }
            Ok(Message::Text(t)) => match serde_json::from_str::<Control>(&t) {
                Ok(Control::Update { name, hue }) => {
                    let mut g = inner.lock().unwrap();
                    if let Some(p) = g.roster.iter_mut().find(|p| p.id == id) {
                        p.name = name;
                        p.hue = hue;
                    }
                    let msg = g.roster_msg();
                    g.broadcast(msg, None);
                }
                Ok(Control::Waypoint { pos, hue }) => {
                    let mut g = inner.lock().unwrap();
                    g.waypoint = pos.map(|[x, z]| (x, z, hue));
                    let msg = Message::Text(
                        serde_json::to_string(&Control::Waypoint { pos, hue }).unwrap_or_default(),
                    );
                    g.broadcast(msg, Some(&id)); // to the other clients
                }
                _ => {}
            },
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    cleanup_client(&inner, &id);
}

fn read_hello(ws: &mut WebSocket<TcpStream>) -> Option<(String, f32)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match ws.read() {
            Ok(Message::Text(t)) => {
                if let Ok(Control::Hello { name, hue }) = serde_json::from_str::<Control>(&t) {
                    return Some((name, hue));
                }
            }
            Ok(Message::Close(_)) => return None,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
    None
}

fn cleanup_client(inner: &Arc<Mutex<Inner>>, id: &str) {
    let mut g = inner.lock().unwrap();
    g.clients.retain(|(cid, _)| cid != id);
    g.roster.retain(|p| p.id != id);
    g.remote.remove(id);
    let msg = g.roster_msg();
    g.broadcast(msg, None);
    let n = g.roster.len();
    g.status = format!("{n} player(s)");
}

// ── Client ─────────────────────────────────────────────────────────

fn client_loop(url: String, name: String, hue: f32, inner: Arc<Mutex<Inner>>, stop: Arc<AtomicBool>) {
    // A fresh trycloudflare tunnel needs a few seconds for DNS/edge propagation, and
    // quick tunnels can hiccup mid-session — so both the initial connect and any drop
    // retry a few times before giving up.
    const ATTEMPTS: u32 = 6;
    let mut first = true;

    'reconnect: loop {
        // ── connect (with retry) ──
        let mut ws = {
            let mut last_err = String::new();
            let mut connected = None;
            for attempt in 1..=ATTEMPTS {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                match tungstenite::connect(&url) {
                    Ok((w, _resp)) => {
                        connected = Some(w);
                        break;
                    }
                    Err(e) => {
                        last_err = e.to_string();
                        let verb = if first { "Connecting" } else { "Reconnecting" };
                        inner.lock().unwrap().status = format!("{verb}… (try {attempt}/{ATTEMPTS})");
                        if attempt < ATTEMPTS {
                            std::thread::sleep(Duration::from_millis(1500));
                        }
                    }
                }
            }
            match connected {
                Some(w) => w,
                None => {
                    let mut g = inner.lock().unwrap();
                    g.role = Role::Off;
                    g.error = Some(format!("connect failed: {last_err}"));
                    g.status = "Disconnected".into();
                    return;
                }
            }
        };
        set_client_timeout(&mut ws, Some(Duration::from_millis(20)));

        // Say hello; a failure here is treated as a drop and reconnected.
        let hello = Control::Hello { name: name.clone(), hue };
        if ws.write(Message::Text(serde_json::to_string(&hello).unwrap_or_default())).is_err() {
            first = false;
            std::thread::sleep(Duration::from_millis(500));
            continue 'reconnect;
        }

        let (tx, rx) = mpsc::sync_channel::<Message>(256);
        {
            let mut g = inner.lock().unwrap();
            g.client_out = Some(tx);
            g.status = "Connected".into();
            g.error = None;
            g.remote.clear();
        }
        first = false;

        // ── session read/write loop ──
        let mut clean = false;
        loop {
            if stop.load(Ordering::Relaxed) {
                let _ = ws.write(Message::Close(None));
                clean = true;
                break;
            }
            let mut wrote = false;
            let mut send_err = false;
            while let Ok(m) = rx.try_recv() {
                if ws.write(m).is_err() {
                    send_err = true;
                    break;
                }
                wrote = true;
            }
            if wrote {
                let _ = ws.flush();
            }
            if send_err {
                break;
            }
            match ws.read() {
                Ok(Message::Binary(data)) if data.len() >= ID_LEN + WIRE_LEN => {
                    let sender = Uuid::from_slice(&data[..ID_LEN])
                        .map(|u| u.to_string())
                        .unwrap_or_default();
                    let my_id = inner.lock().unwrap().my_id.clone();
                    if sender != my_id {
                        if let Some(pkt) = ForzaPacket::from_bytes(&data[ID_LEN..]) {
                            let mut g = inner.lock().unwrap();
                            g.remote.entry(sender).or_insert_with(RemoteBuf::new).push(pkt);
                        }
                    }
                }
                Ok(Message::Text(t)) => {
                    let mut g = inner.lock().unwrap();
                    match serde_json::from_str::<Control>(&t) {
                        Ok(Control::Welcome { id, roster }) => {
                            g.my_id = id.clone();
                            if let Ok(u) = Uuid::parse_str(&id) {
                                g.my_id_bytes = *u.as_bytes();
                            }
                            g.roster = roster;
                        }
                        Ok(Control::Roster { players }) => {
                            g.roster = players;
                        }
                        Ok(Control::Waypoint { pos, hue }) => {
                            g.waypoint = pos.map(|[x, z]| (x, z, hue));
                        }
                        _ => {}
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }

        if stop.load(Ordering::Relaxed) || clean {
            break 'reconnect;
        }
        // Dropped mid-session — clear remote state and try to reconnect.
        {
            let mut g = inner.lock().unwrap();
            g.client_out = None;
            g.remote.clear();
            g.status = "Reconnecting…".into();
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    let mut g = inner.lock().unwrap();
    g.role = Role::Off;
    g.client_out = None;
    if g.error.is_none() {
        g.status = "Disconnected".into();
    }
}

fn set_client_timeout(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, d: Option<Duration>) {
    match ws.get_mut() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(d);
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.get_ref().set_read_timeout(d);
        }
        _ => {}
    }
}

// ── cloudflared quick tunnel ───────────────────────────────────────

/// Spawn `cloudflared tunnel --url http://localhost:PORT` and scrape the
/// `*.trycloudflare.com` slug from its logs (printed to stderr).
fn spawn_tunnel(
    bin: &std::path::Path,
    port: u16,
    inner: Arc<Mutex<Inner>>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<Child> {
    inner.lock().unwrap().status = "Starting tunnel…".into();
    let mut child = Command::new(bin)
        .args([
            "tunnel",
            "--no-autoupdate",
            "--url",
            &format!("http://localhost:{port}"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    for pipe in [child.stderr.take().map(Br::Err), child.stdout.take().map(Br::Out)]
        .into_iter()
        .flatten()
    {
        let inner = inner.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let reader: Box<dyn BufRead> = match pipe {
                Br::Err(e) => Box::new(BufReader::new(e)),
                Br::Out(o) => Box::new(BufReader::new(o)),
            };
            for line in reader.lines().map_while(Result::ok) {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(words) = extract_words(&line) {
                    let mut g = inner.lock().unwrap();
                    g.words = Some(words);
                    g.status = "Tunnel ready".into();
                }
            }
        });
    }
    Ok(child)
}

enum Br {
    Err(std::process::ChildStderr),
    Out(std::process::ChildStdout),
}

/// Pull the word-slug out of a log line containing `https://xxx.trycloudflare.com`.
fn extract_words(line: &str) -> Option<String> {
    let i = line.find("https://")?;
    let rest = &line[i + "https://".len()..];
    let end = rest.find(|c: char| c == ' ' || c == '|' || c == '\t').unwrap_or(rest.len());
    let host = rest[..end].trim().trim_end_matches('/');
    let slug = host.strip_suffix(".trycloudflare.com")?;
    if slug.is_empty() || slug.contains('.') {
        return None;
    }
    Some(slug.to_string())
}

/// `app_data_dir()/cloudflared`, downloading it if absent (best-effort via curl/wget).
pub fn ensure_cloudflared() -> Result<std::path::PathBuf, String> {
    let path = crate::config::app_data_dir().join("cloudflared");
    if path.exists() {
        return Ok(path);
    }
    // ponytail: shell out to curl/wget rather than pull an HTTP-client dep; the
    // binary is normally already present, this is only the cold-start fallback.
    let url = "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64";
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let ok = Command::new("curl")
        .args(["-fsSL", "-o", &path.to_string_lossy(), url])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        || Command::new("wget")
            .args(["-qO", &path.to_string_lossy(), url])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    if !ok {
        return Err("cloudflared missing and download failed (drop the binary in the data dir)".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).ok();
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tunnel_words() {
        let line = "2026-07-09T10:00:00Z INF |  https://blue-fox-rapid-owl.trycloudflare.com  |";
        assert_eq!(extract_words(line).as_deref(), Some("blue-fox-rapid-owl"));
        assert_eq!(extract_words("no url here"), None);
    }

    #[test]
    fn words_url_roundtrip() {
        assert_eq!(words_to_url("blue-fox"), "wss://blue-fox.trycloudflare.com/ws");
        assert_eq!(
            words_to_url("https://blue-fox.trycloudflare.com"),
            "wss://blue-fox.trycloudflare.com/ws"
        );
        // LAN URL passthrough (used for same-network joins).
        assert_eq!(words_to_url("ws://192.168.1.5:7071"), "ws://192.168.1.5:7071/ws");
    }

    #[test]
    fn control_messages_survive_json() {
        // Regression: the waypoint-clear (pos: None) must survive JSON — a NaN
        // sentinel serialised to `null` and failed to parse back into f32.
        for c in [
            Control::Waypoint { pos: Some([1.5, -2.5]), hue: 200.0 },
            Control::Waypoint { pos: None, hue: 0.0 },
            Control::Hello { name: "Guest".into(), hue: 30.0 },
            Control::Update { name: "Guest2".into(), hue: 140.0 },
        ] {
            let s = serde_json::to_string(&c).expect("serialize");
            let back: Control = serde_json::from_str(&s).expect("deserialize");
            // Control isn't PartialEq; compare by re-serialising.
            assert_eq!(serde_json::to_string(&back).unwrap(), s);
        }
    }
}
