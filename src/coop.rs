//! Co-Op: share live telemetry between players over a WebSocket relay, exposed
//! publicly via a cloudflared quick tunnel (no login). One player Hosts; others
//! Join by typing the tunnel's word-slug (e.g. `blue-fox-rapid-owl`).
//!
//! Transport: raw 324-byte FH6 packets in binary WS frames, prefixed with the
//! sender's 16-byte UUID. Roster/identity travels as small JSON text frames.
//! The host is authoritative: it mints a UUID per player and owns the roster.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use uuid::Uuid;

use crate::packet::ForzaPacket;

// Trystero transport: serverless P2P mesh, signalled over public Nostr relays. See
// docs/features/coop.md (transport section) and the module docs of each file.
mod mesh;
mod nostr;
mod rtc;

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
    /// A per-player map ping (setter `id`, world [x,z], setter's hue), or `pos:
    /// None` to clear that player's ping. Each player has their own, so several
    /// show at once. Either direction; the host re-broadcasts with the setter's id.
    /// (Option — not NaN — so it survives JSON, which has no NaN.)
    Waypoint { id: String, pos: Option<[f32; 2]>, hue: f32 },
    /// Mesh (Trystero) only: sent by each side when its data channel opens and again when
    /// its identity changes. Binds the channel to that player id + name/colour.
    Peer { id: String, name: String, hue: f32 },
    /// The room's shared navigation destination (one per room, last write wins on `(ts, id)`):
    /// world [x, z], the setter's colour, their road filters as bits ([`DEST_ROAD`]…; unknown
    /// bits are kept when relaying and ignored by consumers) and curve slider `c` (0..1).
    /// `pos: None` clears it (a tombstone that keeps its `ts`). `ts` = setter's unix ms.
    /// A tag of its own, not extra fields on `Waypoint`: see `docs/features/coop.md`.
    Dest { id: String, pos: Option<[f32; 2]>, hue: f32, f: u8, c: f32, ts: u64 },
}

/// `Control::Dest.f` filter bits. Higher bits are reserved: ignored on receive
/// ([`DEST_FILTER_MASK`]), preserved when relaying.
#[allow(dead_code)]
pub const DEST_ROAD: u8 = 1;
#[allow(dead_code)]
pub const DEST_HIGHWAY: u8 = 2;
#[allow(dead_code)]
pub const DEST_DIRT: u8 = 4;
#[allow(dead_code)]
pub const DEST_TRAIL: u8 = 8;
#[allow(dead_code)]
pub const DEST_CROSS_COUNTRY: u8 = 16;
#[allow(dead_code)]
pub const DEST_JUMPS: u8 = 32;
pub const DEST_FILTER_MASK: u8 = 0b11_1111;

/// The room's shared destination as read by the navigator / UI.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedDest {
    /// Player id of the setter (trusted: connection / channel id). In a mesh a destination
    /// resent to a late joiner is attributed to the resender.
    pub setter_id: String,
    /// Setter's display name from the roster (empty if they left and are not in it).
    pub setter_name: String,
    pub x: f32,
    pub z: f32,
    pub hue: f32,
    /// Raw filter bits as received (reserved bits included); use [`SharedDest::filters`].
    pub filter_bits: u8,
    /// Curve slider 0..1.
    pub curve: f32,
    pub ts: u64,
}

impl SharedDest {
    /// The known filter bits only.
    #[allow(dead_code)]
    pub fn filters(&self) -> u8 {
        self.filter_bits & DEST_FILTER_MASK
    }
}

/// Stored slot: `pos: None` is the tombstone of a clear.
#[derive(Clone, Debug)]
struct DestSlot {
    setter_id: String,
    pos: Option<[f32; 2]>,
    hue: f32,
    f: u8,
    c: f32,
    ts: u64,
}

impl DestSlot {
    fn message(&self) -> Message {
        let c = Control::Dest { id: self.setter_id.clone(), pos: self.pos, hue: self.hue, f: self.f, c: self.c, ts: self.ts };
        Message::Text(serde_json::to_string(&c).unwrap_or_default())
    }
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
    /// The session isn't fully up yet (see [`CoopState::is_connecting`]); drives the status
    /// bar's yellow Co-Op indicator. Mesh: recomputed in `Session::refresh`; Cloudflare: set by
    /// `start_host` / `start_client` and cleared when the tunnel / socket comes up.
    connecting: bool,
    error: Option<String>,
    words: Option<String>,
    lan_url: Option<String>,
    buffer_ms: u32,
    /// Per-player map pings, keyed by setter id: (world_x, world_z, setter hue).
    waypoints: HashMap<String, (f32, f32, f32)>,
    /// The room's shared destination (or its clear tombstone), last write wins on `(ts, id)`.
    dest: Option<DestSlot>,
    /// Bumped on every accepted change of `dest` (and when it is dropped), so readers detect
    /// change with one compare.
    dest_seq: u64,
    /// Running cloudflared tunnel child. Owned here (not on CoopState) so the
    /// background host-start thread can hand it off and `stop()` can still kill it.
    tunnel: Option<Child>,
    /// cloudflared download in flight: (bytes so far, total if known). `None` when
    /// not downloading; drives the progress indicator in the Co-Op tab.
    download: Option<(u64, Option<u64>)>,
    /// Trystero mesh mode: every peer is equal, `clients` holds one queue per open data
    /// channel (keyed by that peer's signalling id) and all sends go through `broadcast`.
    /// Reset in `stop()`. `role` is `Client` while active (the UI matches exhaustively on it).
    mesh: bool,
    /// Our own display identity (mesh: re-sent to peers on open and on change).
    my_name: String,
    my_hue: f32,
    /// Mesh: signalling peer id (channel key) → the player UUID that channel announced.
    mesh_bound: HashMap<String, String>,
}

impl Inner {
    fn new(name: &str, hue: f32, buffer_ms: u32) -> Self {
        let id = Uuid::new_v4();
        Inner {
            role: Role::Off,
            my_id: id.to_string(),
            my_id_bytes: *id.as_bytes(),
            roster: Vec::new(),
            remote: HashMap::new(),
            clients: Vec::new(),
            client_out: None,
            status: String::new(),
            connecting: false,
            error: None,
            words: None,
            lan_url: None,
            buffer_ms,
            waypoints: HashMap::new(),
            dest: None,
            dest_seq: 0,
            tunnel: None,
            download: None,
            mesh: false,
            my_name: name.to_string(),
            my_hue: hue,
            mesh_bound: HashMap::new(),
        }
    }
}

/// Advance every jitter buffer to `now - buffer_ms`. Time-based, so calling it from more
/// than one thread (the UI each frame, the overlay each HUD frame) is harmless.
fn tick(inner: &Mutex<Inner>) {
    let mut inner = inner.lock().unwrap_or_else(PoisonError::into_inner);
    let delay = Duration::from_millis(inner.buffer_ms as u64);
    for buf in inner.remote.values_mut() {
        buf.advance(delay);
    }
}

fn remote_players(inner: &Mutex<Inner>) -> Vec<(PlayerInfo, ForzaPacket)> {
    let inner = inner.lock().unwrap_or_else(PoisonError::into_inner);
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

/// Send one locally-received packet to peers. A no-op (one uncontended lock) while co-op is
/// off. Only `try_send`s under the lock, so it never blocks the caller (the listener thread).
fn push_local(inner: &Mutex<Inner>, pkt: &ForzaPacket) {
    let mut inner = inner.lock().unwrap_or_else(PoisonError::into_inner);
    if inner.role == Role::Off {
        return;
    }
    let mut frame = Vec::with_capacity(ID_LEN + WIRE_LEN);
    frame.extend_from_slice(&inner.my_id_bytes);
    frame.extend_from_slice(&pkt.to_bytes());
    let msg = Message::Binary(frame);
    match inner.role {
        Role::Host => inner.broadcast(msg, None),
        Role::Client if inner.mesh => inner.broadcast(msg, None),
        Role::Client => {
            if let Some(tx) = &inner.client_out {
                let _ = tx.try_send(msg);
            }
        }
        Role::Off => {}
    }
}

/// The packet we relay for `pkt`. A paused game zeroes car class/PI, so while paused carry
/// over the last live values (same as the Car widget) and peers keep seeing our real class.
/// `last` is (class, PI) from the last race-on packet; start it at `(-1, 0)`.
pub fn outgoing(pkt: &ForzaPacket, last: &mut (i32, i32)) -> ForzaPacket {
    if pkt.is_race_on != 0 {
        *last = (pkt.car_class, pkt.car_performance_index);
    }
    let mut out = pkt.clone();
    if pkt.is_paused() && last.1 != 0 {
        (out.car_class, out.car_performance_index) = *last;
    }
    out
}

/// Cheap `Send + Sync` handle on the shared co-op state, for a thread that doesn't own
/// [`CoopState`]: the HUD overlay reads teammates through it, the listener thread sends our
/// telemetry through it. The inner `Arc` is never replaced (stop/host/join reuse
/// it), so a handle taken once stays valid for the app's lifetime.
#[derive(Clone)]
pub struct CoopReader(Arc<Mutex<Inner>>);

impl CoopReader {
    /// Advance the jitter buffers, then snapshot the remote players (as
    /// [`CoopState::remote_players`]). It advances them itself because the UI's `tick` stops
    /// while the game covers the window. Empty while co-op is off (`stop` clears them).
    #[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))] // the overlay exists on Linux and Windows only
    pub fn remote_players(&self) -> Vec<(PlayerInfo, ForzaPacket)> {
        tick(&self.0);
        remote_players(&self.0)
    }

    /// Whether a session is running (role ≠ `Off`): the Minimap's own arrow takes the co-op
    /// colour and trails are recorded only then, as on the Dashboard map.
    #[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
    pub fn in_session(&self) -> bool {
        self.0.lock().unwrap().role != Role::Off
    }

    /// The session's shared waypoints as `(setter_id, world_x, world_z, hue)`
    /// ([`CoopState::waypoints`]).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn waypoints(&self) -> Vec<(String, f32, f32, f32)> {
        self.0.lock().unwrap().waypoints.iter().map(|(id, &(x, z, h))| (id.clone(), x, z, h)).collect()
    }

    /// The room's shared destination, `None` if unset / cleared.
    #[allow(dead_code)] // consumed by the navigator runtime
    pub fn destination(&self) -> Option<SharedDest> {
        self.0.lock().unwrap().dest_view()
    }

    /// Change counter of the shared destination (set, clear, session end): compare to detect
    /// change without cloning.
    #[allow(dead_code)] // consumed by the navigator runtime
    pub fn destination_seq(&self) -> u64 {
        self.0.lock().unwrap().dest_seq
    }

    /// Send our locally-received packet to peers (listener thread, every packet — it runs
    /// while the game covers the window, which the UI loop doesn't). No-op while co-op is off.
    pub fn push_local(&self, pkt: &ForzaPacket) {
        push_local(&self.0, pkt);
    }
}

impl std::fmt::Debug for CoopReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CoopReader")
    }
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
    /// Last-write-wins on `(ts, id)`; `setter` is the trusted id (never the message's own).
    /// Returns whether it was accepted (then the caller relays it). Garbage values are refused.
    fn accept_dest(&mut self, setter: &str, pos: Option<[f32; 2]>, hue: f32, f: u8, c: f32, ts: u64) -> bool {
        if pos.is_some_and(|[x, z]| !x.is_finite() || !z.is_finite()) || !hue.is_finite() || !c.is_finite() {
            return false;
        }
        if let Some(cur) = &self.dest {
            if (ts, setter) <= (cur.ts, cur.setter_id.as_str()) {
                return false;
            }
        }
        self.dest = Some(DestSlot { setter_id: setter.to_string(), pos, hue, f, c: c.clamp(0.0, 1.0), ts });
        self.dest_seq += 1;
        true
    }
    /// Forget the destination (session over).
    fn drop_dest(&mut self) {
        if self.dest.take().is_some() {
            self.dest_seq += 1;
        }
    }
    fn dest_view(&self) -> Option<SharedDest> {
        let d = self.dest.as_ref()?;
        let [x, z] = d.pos?;
        let setter_name = self.roster.iter().find(|p| p.id == d.setter_id).map(|p| p.name.clone()).unwrap_or_default();
        Some(SharedDest {
            setter_id: d.setter_id.clone(),
            setter_name,
            x,
            z,
            hue: d.hue,
            filter_bits: d.f,
            curve: d.c,
            ts: d.ts,
        })
    }
    /// The live destination as a frame for a late joiner (a tombstone needs no resend).
    fn dest_resend(&self) -> Option<Message> {
        self.dest.as_ref().filter(|d| d.pos.is_some()).map(DestSlot::message)
    }
    fn roster_msg(&self) -> Message {
        let c = Control::Roster { players: self.roster.clone() };
        Message::Text(serde_json::to_string(&c).unwrap_or_default())
    }
}

pub struct CoopState {
    inner: Arc<Mutex<Inner>>,
    stop: Arc<AtomicBool>,
    pub port: u16,
}

impl CoopState {
    pub fn new(name: &str, hue: f32, buffer_ms: u32) -> Self {
        // Identity is seeded even while Off so the UI preview is stable.
        let inner = Inner::new(name, hue, buffer_ms);
        Self {
            inner: Arc::new(Mutex::new(inner)),
            stop: Arc::new(AtomicBool::new(false)),
            port: DEFAULT_COOP_PORT,
        }
    }

    pub fn role(&self) -> Role {
        self.inner.lock().unwrap().role
    }
    pub fn status(&self) -> String {
        self.inner.lock().unwrap().status.clone()
    }
    /// True while a running session isn't fully connected: still coming up (Cloudflare host
    /// tunnel not ready / client socket not open, mesh relays not reached) or, on the mesh, any
    /// peer link is mid-handshake. Always false while co-op is off. Cheap (one lock).
    pub fn is_connecting(&self) -> bool {
        let i = self.inner.lock().unwrap();
        i.role != Role::Off && i.connecting
    }
    pub fn error(&self) -> Option<String> {
        self.inner.lock().unwrap().error.clone()
    }
    /// Cloudflared download progress while it's being fetched: (bytes, total?).
    pub fn download(&self) -> Option<(u64, Option<u64>)> {
        self.inner.lock().unwrap().download
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

    /// All active pings as (setter_id, world_x, world_z, hue).
    pub fn waypoints(&self) -> Vec<(String, f32, f32, f32)> {
        self.inner
            .lock()
            .unwrap()
            .waypoints
            .iter()
            .map(|(id, &(x, z, h))| (id.clone(), x, z, h))
            .collect()
    }

    /// Drop our own ping at a world position (hue = our colour). Pass `None` to clear it.
    pub fn set_waypoint(&self, pos: Option<(f32, f32)>, hue: f32) {
        let mut inner = self.inner.lock().unwrap();
        if inner.role == Role::Off {
            return;
        }
        let my_id = inner.my_id.clone();
        match pos {
            Some((x, z)) => {
                inner.waypoints.insert(my_id.clone(), (x, z, hue));
            }
            None => {
                inner.waypoints.remove(&my_id);
            }
        }
        let msg = Message::Text(
            serde_json::to_string(&Control::Waypoint { id: my_id, pos: pos.map(|(x, z)| [x, z]), hue })
                .unwrap_or_default(),
        );
        match inner.role {
            Role::Host => inner.broadcast(msg, None),
            Role::Client if inner.mesh => inner.broadcast(msg, None),
            Role::Client => {
                if let Some(tx) = &inner.client_out {
                    let _ = tx.try_send(msg);
                }
            }
            Role::Off => {}
        }
    }

    /// The room's shared destination, `None` if unset / cleared ([`CoopReader::destination`]).
    #[allow(dead_code)] // consumed by the navigator runtime / navigation tab
    pub fn destination(&self) -> Option<SharedDest> {
        self.inner.lock().unwrap().dest_view()
    }

    /// Change counter of the shared destination ([`CoopReader::destination_seq`]).
    #[allow(dead_code)]
    pub fn destination_seq(&self) -> u64 {
        self.inner.lock().unwrap().dest_seq
    }

    /// Set the room's destination (world `x, z`) with my colour, road filter bits
    /// (`DEST_*`) and curve slider 0..1; replaces whoever's it was. No-op outside a session.
    /// Its `ts` is the wall clock, bumped past the stored one so my write always wins locally.
    #[allow(dead_code)] // called by the navigation tab
    pub fn set_destination(&self, pos: (f32, f32), hue: f32, filters: u8, curve: f32) {
        self.write_destination(Some([pos.0, pos.1]), hue, filters, curve);
    }

    /// Clear the room's destination for everyone. No-op outside a session.
    #[allow(dead_code)] // called by the navigation tab
    pub fn clear_destination(&self) {
        self.write_destination(None, 0.0, 0, 0.0);
    }

    fn write_destination(&self, pos: Option<[f32; 2]>, hue: f32, filters: u8, curve: f32) {
        let mut inner = self.inner.lock().unwrap();
        if inner.role == Role::Off {
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let ts = inner.dest.as_ref().map_or(now, |d| now.max(d.ts + 1));
        let my_id = inner.my_id.clone();
        if !inner.accept_dest(&my_id, pos, hue, filters, curve, ts) {
            return; // non-finite input
        }
        let msg = inner.dest.as_ref().expect("just stored").message();
        match inner.role {
            Role::Host => inner.broadcast(msg, None),
            Role::Client if inner.mesh => inner.broadcast(msg, None),
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
        inner.my_name = name.to_string();
        inner.my_hue = hue;
        match inner.role {
            Role::Client if inner.mesh => {
                // Mesh: our roster entry is local; tell every peer directly.
                if let Some(p) = inner.roster.iter_mut().find(|p| p.id == my_id) {
                    p.name = name.to_string();
                    p.hue = hue;
                }
                let msg = mesh::peer_msg(&inner);
                inner.broadcast(msg, None);
            }
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
        tick(&self.inner);
    }

    /// Snapshot of remote players that have a recent packet, for the minimap.
    pub fn remote_players(&self) -> Vec<(PlayerInfo, ForzaPacket)> {
        remote_players(&self.inner)
    }

    /// A handle for another thread (the listener's outgoing packets, the HUD overlay's
    /// teammate markers).
    pub fn reader(&self) -> CoopReader {
        CoopReader(self.inner.clone())
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap();
        if let Some(mut child) = inner.tunnel.take() {
            let _ = child.kill();
        }
        inner.download = None;
        inner.role = Role::Off;
        inner.mesh = false;
        inner.mesh_bound.clear();
        inner.clients.clear();
        inner.client_out = None;
        inner.remote.clear();
        inner.roster.clear();
        inner.words = None;
        inner.lan_url = None;
        inner.waypoints.clear();
        inner.drop_dest();
        inner.status = "Stopped".into();
        inner.connecting = false;
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
            inner.connecting = true; // until the tunnel is ready (or fails → LAN only)
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

        // cloudflared quick tunnel → public wss URL. Ensuring the binary can mean a
        // multi-MB download, so do it (and the tunnel spawn) on a background thread;
        // progress lands in `inner.download` for the UI.
        let inner = self.inner.clone();
        let stop = self.stop.clone();
        std::thread::spawn(move || {
            let progress = |dl: u64, total: Option<u64>| {
                if let Ok(mut i) = inner.lock() {
                    i.download = Some((dl, total));
                }
            };
            let res = match ensure_cloudflared(&progress, &stop) {
                Ok(bin) => spawn_tunnel(&bin, port, inner.clone(), stop.clone())
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            let mut i = inner.lock().unwrap();
            i.download = None;
            if stop.load(Ordering::Relaxed) {
                // User pressed Stop while we were downloading — tear down the tunnel
                // we just started (dropping a Child does not kill the process).
                if let Ok(mut child) = res {
                    let _ = child.kill();
                }
                return;
            }
            match res {
                Ok(child) => i.tunnel = Some(child),
                Err(e) => {
                    i.error = Some(format!("cloudflared: {e}"));
                    i.status = "Server up (LAN only — no tunnel)".into();
                    i.connecting = false;
                }
            }
        });
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
            inner.connecting = true;
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

impl CoopState {
    /// Join (or create) a Trystero mesh room: serverless P2P, peers find each other by the
    /// shared Room ID over public Nostr relays, then talk over WebRTC data channels. Every
    /// peer is equal (no host). Non-blocking: spawns the relay + WebRTC threads and returns
    /// (it runs from `ForzaApp::new` for auto-connect). Never touches port/tunnel/lan_url.
    pub fn start_trystero(&mut self, room: &str, name: &str, hue: f32, buffer_ms: u32) {
        if let Some(room) = self.begin_trystero(room, name, hue, buffer_ms) {
            mesh::Session::start(self.inner.clone(), self.stop.clone(), &room);
        }
    }

    /// `start_trystero` minus the threads (so tests can cover the state reset without touching
    /// the network). Returns the normalised room, or `None` if it's empty.
    fn begin_trystero(&mut self, room: &str, name: &str, hue: f32, buffer_ms: u32) -> Option<String> {
        let room = normalize_room(room);
        if room.is_empty() {
            return None;
        }
        self.stop();
        self.stop = Arc::new(AtomicBool::new(false));
        {
            let mut inner = self.inner.lock().unwrap();
            // A fresh player id per join: a rejoin (new signalling id) must not look like a
            // second channel claiming our old id to peers that haven't noticed the old link die.
            let id = Uuid::new_v4();
            inner.my_id = id.to_string();
            inner.my_id_bytes = *id.as_bytes();
            inner.buffer_ms = buffer_ms;
            inner.remote.clear();
            inner.clients.clear();
            inner.client_out = None;
            inner.waypoints.clear();
            inner.drop_dest();
            inner.mesh_bound.clear();
            mesh::initial_role_state(&mut inner, &room, name, hue);
        }
        Some(room)
    }
}

/// Canonical form of a typed Room ID: lowercase, no whitespace anywhere, so `K7F2-9QZM-X4PD`,
/// ` k7f2-9qzm-x4pd ` and a copy-paste with a stray line break all name the same room (the ID
/// is hashed into the room's topics and encryption key, so any difference is a different room).
pub fn normalize_room(room: &str) -> String {
    room.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// A fresh shareable Room ID: eight groups of four lowercase Crockford-base32 characters
/// (32 chars, 160 bits, so the ID doubles as the room's encryption secret, can't be guessed and
/// shared public rooms practically never collide; no `i l o u`, so it survives being read out
/// loud). Older, shorter IDs typed by hand still work: `normalize_room` accepts any string.
pub fn generate_room_id() -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut bytes = [0u8; 32];
    nostr::fill_random(&mut bytes);
    let mut out = String::with_capacity(39);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 4 == 0 {
            out.push('-');
        }
        out.push(ALPHABET[(b & 31) as usize] as char);
    }
    out
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
        // Late joiner: the room's current destination (waypoints are not resent).
        if let Some(m) = g.dest_resend() {
            let _ = ws.write(m);
        }
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
                Ok(Control::Waypoint { pos, hue, .. }) => {
                    // Key by the connection's id (authoritative), not the message's.
                    let mut g = inner.lock().unwrap();
                    match pos {
                        Some([x, z]) => { g.waypoints.insert(id.clone(), (x, z, hue)); }
                        None => { g.waypoints.remove(&id); }
                    }
                    let msg = Message::Text(
                        serde_json::to_string(&Control::Waypoint { id: id.clone(), pos, hue })
                            .unwrap_or_default(),
                    );
                    g.broadcast(msg, Some(&id)); // to the other clients
                }
                Ok(Control::Dest { pos, hue, f, c, ts, .. }) => {
                    // Connection id is authoritative; relay only what won (stale = dropped).
                    let mut g = inner.lock().unwrap();
                    if g.accept_dest(&id, pos, hue, f, c, ts) {
                        let msg = g.dest.as_ref().expect("just stored").message();
                        g.broadcast(msg, Some(&id));
                    }
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
    g.waypoints.remove(id);
    let msg = g.roster_msg();
    g.broadcast(msg, None);
    let n = g.roster.len();
    g.status = format!("{n} player(s)");
}

// ── Client ─────────────────────────────────────────────────────────

/// Connect the client WebSocket. Fast path uses the system resolver; if that
/// misses on a `wss://…` host (some resolvers — e.g. a flaky systemd-resolved
/// stub — fail to resolve *fresh* trycloudflare subdomains that public DNS has),
/// resolve the host via 1.1.1.1 / 8.8.8.8 and connect to that IP with the correct
/// TLS SNI so the tunnel still works.
fn connect_ws(url: &str) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, tungstenite::Error> {
    let first = match tungstenite::connect(url) {
        Ok((ws, _)) => return Ok(ws),
        Err(e) => e,
    };
    // Fallback only for wss:// with a real hostname (the tunnel case).
    if let Some(rest) = url.strip_prefix("wss://") {
        let host = rest.split('/').next().unwrap_or("").split(':').next().unwrap_or("");
        if !host.is_empty() && host.parse::<std::net::IpAddr>().is_err() {
            if let Some(ip) = resolve_a(host, "1.1.1.1").or_else(|| resolve_a(host, "8.8.8.8")) {
                if let Ok(tcp) = TcpStream::connect((ip, 443)) {
                    let _ = tcp.set_nodelay(true);
                    if let Ok((ws, _)) = tungstenite::client_tls(url, tcp) {
                        return Ok(ws);
                    }
                }
            }
        }
    }
    Err(first)
}

/// Minimal synchronous DNS A-record lookup against a specific resolver (stdlib
/// UDP only — deliberately tiny; upgrade to a DNS crate only if we ever need
/// CNAME chasing, EDNS or IPv6). Returns the first A record.
fn resolve_a(host: &str, dns: &str) -> Option<Ipv4Addr> {
    let mut q: Vec<u8> = Vec::with_capacity(host.len() + 18);
    q.extend_from_slice(&[0x13, 0x37, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0]); // hdr: id, RD, qd=1
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0x00, 0x00, 0x01, 0x00, 0x01]); // end name, qtype=A, qclass=IN

    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    sock.send_to(&q, (dns, 53)).ok()?;
    let mut buf = [0u8; 512];
    let n = sock.recv(&mut buf).ok()?;
    parse_a_answer(&buf[..n])
}

/// Parse the first A record out of a DNS response (handles name compression).
fn parse_a_answer(resp: &[u8]) -> Option<Ipv4Addr> {
    if resp.len() < 12 {
        return None;
    }
    let ancount = u16::from_be_bytes([resp[6], resp[7]]);
    let mut i = 12;
    // Skip the question name + qtype/qclass.
    while i < resp.len() && resp[i] != 0 {
        if resp[i] & 0xC0 == 0xC0 {
            i += 2;
            break;
        }
        i += 1 + resp[i] as usize;
    }
    if resp.get(i) == Some(&0) {
        i += 1;
    }
    i += 4;
    for _ in 0..ancount {
        // Answer name: a compression pointer (2 bytes) or a sequence of labels.
        if resp.get(i)? & 0xC0 == 0xC0 {
            i += 2;
        } else {
            while i < resp.len() && resp[i] != 0 {
                i += 1 + resp[i] as usize;
            }
            i += 1;
        }
        if i + 10 > resp.len() {
            return None;
        }
        let rtype = u16::from_be_bytes([resp[i], resp[i + 1]]);
        let rdlen = u16::from_be_bytes([resp[i + 8], resp[i + 9]]) as usize;
        i += 10;
        if i + rdlen > resp.len() {
            return None;
        }
        if rtype == 1 && rdlen == 4 {
            return Some(Ipv4Addr::new(resp[i], resp[i + 1], resp[i + 2], resp[i + 3]));
        }
        i += rdlen;
    }
    None
}

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
                match connect_ws(&url) {
                    Ok(w) => {
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
        // Push it out now: the host waits at most 10 s for Hello, and without telemetry (game
        // not running) nothing else would flush it. A would-block is left to the loop's flush.
        let _ = ws.flush();

        let (tx, rx) = mpsc::sync_channel::<Message>(256);
        {
            let mut g = inner.lock().unwrap();
            g.client_out = Some(tx);
            g.status = "Connected".into();
            g.connecting = false;
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
                        Ok(Control::Waypoint { id, pos, hue }) => {
                            match pos {
                                Some([x, z]) => { g.waypoints.insert(id, (x, z, hue)); }
                                None => { g.waypoints.remove(&id); }
                            }
                        }
                        // The host stamps the setter's connection id (as for waypoints).
                        Ok(Control::Dest { id, pos, hue, f, c, ts }) => {
                            g.accept_dest(&id, pos, hue, f, c, ts);
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
            g.connecting = true;
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
    let mut cmd = Command::new(bin);
    cmd.args([
        "tunnel",
        "--no-autoupdate",
        "--url",
        &format!("http://localhost:{port}"),
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = hidden(&mut cmd).spawn()?;

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
                    g.connecting = false;
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
    let end = rest.find([' ', '|', '\t']).unwrap_or(rest.len());
    let host = rest[..end].trim().trim_end_matches('/');
    let slug = host.strip_suffix(".trycloudflare.com")?;
    if slug.is_empty() || slug.contains('.') {
        return None;
    }
    Some(slug.to_string())
}

/// Suppress the console window Windows would otherwise flash when we spawn a
/// helper process (curl / wget / cloudflared). No-op on other platforms.
fn hidden(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// `app_data_dir()/cloudflared`, downloading it if absent. `progress(downloaded,
/// total)` is called periodically during the download (total is `None` until the
/// size is known). Returns immediately if the binary is already present.
pub fn ensure_cloudflared(
    progress: &dyn Fn(u64, Option<u64>),
    stop: &AtomicBool,
) -> Result<std::path::PathBuf, String> {
    // Platform-correct local filename + release asset (x86_64; the game is x64).
    #[cfg(windows)]
    let (bin_name, asset) = ("cloudflared.exe", "cloudflared-windows-amd64.exe");
    #[cfg(not(windows))]
    let (bin_name, asset) = ("cloudflared", "cloudflared-linux-amd64");

    let path = crate::config::app_data_dir().join(bin_name);
    if path.exists() {
        return Ok(path);
    }
    // ponytail: shell out to curl/wget rather than pull an HTTP-client dep; the
    // binary is normally already present, this is only the cold-start fallback.
    // (Windows 10+ ships curl.exe, so this works there too.)
    let url =
        format!("https://github.com/cloudflare/cloudflared/releases/latest/download/{asset}");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let tmp = path.with_extension("part");
    let _ = std::fs::remove_file(&tmp);
    let total = head_content_length(&url);
    progress(0, total);

    // Spawn the downloader as a child and poll the growing file for progress,
    // instead of blocking on it, so the UI can show live bytes.
    let tmp_s = tmp.to_string_lossy().to_string();
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "-o", &tmp_s, url.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = hidden(&mut curl)
        .spawn()
        .or_else(|_| {
            let mut wget = Command::new("wget");
            wget.args(["-qO", &tmp_s, url.as_str()])
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            hidden(&mut wget).spawn()
        })
        .map_err(|_| {
            "cloudflared missing and no curl/wget to fetch it (drop the binary in the data dir)"
                .to_string()
        })?;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let got = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
                if !status.success() || got == 0 {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(
                        "cloudflared download failed (drop the binary in the data dir)".into(),
                    );
                }
                progress(got, total.or(Some(got)));
                break;
            }
            Ok(None) => {
                if stop.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = std::fs::remove_file(&tmp);
                    return Err("cancelled".into());
                }
                let got = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
                progress(got, total);
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(format!("cloudflared download error: {e}"));
            }
        }
    }

    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cloudflared: could not save binary: {e}"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).ok();
    }
    Ok(path)
}

/// Best-effort total download size via a HEAD request (follows redirects); the
/// last `Content-Length` seen is the final asset's. `None` if it can't be read.
fn head_content_length(url: &str) -> Option<u64> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sIL", url]);
    let out = hidden(&mut cmd).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut last = None;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            if let Ok(n) = v.trim().parse::<u64>() {
                last = Some(n);
            }
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outgoing_carries_class_pi_over_pause() {
        let mut last = (-1, 0);
        // Paused before any live packet: nothing cached, sent as-is.
        let paused = ForzaPacket::default();
        assert_eq!(outgoing(&paused, &mut last).car_performance_index, 0);
        // Live packet: sent as-is, and cached.
        let live = ForzaPacket {
            is_race_on: 1,
            car_class: 5,
            car_performance_index: 900,
            position_x: 10.0,
            ..Default::default()
        };
        let out = outgoing(&live, &mut last);
        assert_eq!((out.car_class, out.car_performance_index), (5, 900));
        assert_eq!(last, (5, 900));
        // Paused (zeroed class/PI, at origin): the cached values are substituted.
        let out = outgoing(&paused, &mut last);
        assert_eq!((out.car_class, out.car_performance_index), (5, 900));
        // Not paused but race off (menu with a position): untouched, cache kept.
        let menu = ForzaPacket { position_x: 1.0, ..Default::default() };
        let out = outgoing(&menu, &mut last);
        assert_eq!((out.car_class, out.car_performance_index), (0, 0));
        assert_eq!(last, (5, 900));
    }

    #[test]
    fn parse_tunnel_words() {
        let line = "2026-07-09T10:00:00Z INF |  https://blue-fox-rapid-owl.trycloudflare.com  |";
        assert_eq!(extract_words(line).as_deref(), Some("blue-fox-rapid-owl"));
        assert_eq!(extract_words("no url here"), None);
        // Trailing slash is trimmed.
        assert_eq!(extract_words("https://foo-bar.trycloudflare.com/").as_deref(), Some("foo-bar"));
        // Non-trycloudflare host is ignored.
        assert_eq!(extract_words("https://example.com"), None);
        // A slug with an extra dot (sub-subdomain) is rejected.
        assert_eq!(extract_words("https://a.b.trycloudflare.com"), None);
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
        // A pasted bare host also works.
        assert_eq!(words_to_url("foo-bar.trycloudflare.com"), "wss://foo-bar.trycloudflare.com/ws");
    }

    #[test]
    fn remote_buf_zero_delay_uses_latest() {
        let mut b = RemoteBuf::new();
        let mut p1 = ForzaPacket::default();
        p1.speed = 10.0;
        let mut p2 = ForzaPacket::default();
        p2.speed = 20.0;
        b.push(p1);
        b.push(p2);
        b.advance(Duration::ZERO);
        assert_eq!(b.current.as_ref().unwrap().speed, 20.0, "zero delay = newest sample");
    }

    #[test]
    fn remote_buf_delay_holds_back_fresh_packets() {
        // With a big buffer, a just-arrived packet is not yet eligible for playback.
        let mut b = RemoteBuf::new();
        b.push(ForzaPacket::default());
        b.advance(Duration::from_secs(10));
        assert!(b.current.is_none(), "fresh packet held back by the jitter buffer");
    }

    #[test]
    fn parse_dns_a_answer() {
        // Response for "a.com" → A 1.2.3.4, answer name as a 0xC00C compression pointer.
        let resp: &[u8] = &[
            0x13, 0x37, 0x81, 0x80, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, // header
            0x01, b'a', 0x03, b'c', b'o', b'm', 0x00, 0x00, 0x01, 0x00, 0x01,       // question
            0xc0, 0x0c, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3c,             // answer name+type+class+ttl
            0x00, 0x04, 0x01, 0x02, 0x03, 0x04,                                     // rdlen=4, 1.2.3.4
        ];
        assert_eq!(parse_a_answer(resp), Some(Ipv4Addr::new(1, 2, 3, 4)));
        assert_eq!(parse_a_answer(&[0u8; 4]), None); // too short
    }

    #[test]
    fn control_messages_survive_json() {
        // Regression: the waypoint-clear (pos: None) must survive JSON — a NaN
        // sentinel serialised to `null` and failed to parse back into f32.
        for c in [
            Control::Waypoint { id: "abc".into(), pos: Some([1.5, -2.5]), hue: 200.0 },
            Control::Waypoint { id: "abc".into(), pos: None, hue: 0.0 },
            Control::Hello { name: "Guest".into(), hue: 30.0 },
            Control::Update { name: "Guest2".into(), hue: 140.0 },
            Control::Peer { id: "0b7e4c1e-6f0e-4c58-9a51-3d1f2a9b8c77".into(), name: "Zoë".into(), hue: 210.5 },
        ] {
            let s = serde_json::to_string(&c).expect("serialize");
            let back: Control = serde_json::from_str(&s).expect("deserialize");
            // Control isn't PartialEq; compare by re-serialising.
            assert_eq!(serde_json::to_string(&back).unwrap(), s);
        }
        // The new variant is additive: it has its own tag and old variants still parse.
        let s = serde_json::to_string(&Control::Peer { id: "i".into(), name: "n".into(), hue: 1.0 }).unwrap();
        assert_eq!(s, r#"{"t":"Peer","id":"i","name":"n","hue":1.0}"#);
    }

    #[test]
    fn room_normalisation() {
        assert_eq!(normalize_room("  K7F2-9QZM-x4pd\n"), "k7f2-9qzm-x4pd");
        assert_eq!(normalize_room("k7f2 9qzm\tx4pd"), "k7f29qzmx4pd");
        assert_eq!(normalize_room(" \t "), "");
        let id = generate_room_id();
        assert_eq!(normalize_room(&id), id, "generated IDs are already canonical");
        let mut st = CoopState::new("Me", 10.0, 0);
        let before = st.my_id();
        assert_eq!(st.begin_trystero(" AbC-Def ", "Me", 10.0, 0).as_deref(), Some("abc-def"));
        assert_eq!(st.words().as_deref(), Some("abc-def"));
        assert_ne!(st.my_id(), before, "a new join gets a fresh player id");
        assert_eq!(st.roster()[0].id, st.my_id());
        assert_eq!(st.begin_trystero("  ", "Me", 10.0, 0), None);
        st.stop();
    }

    #[test]
    fn room_id_format() {
        for _ in 0..50 {
            let id = generate_room_id();
            let groups: Vec<&str> = id.split('-').collect();
            assert_eq!(groups.len(), 8, "{id}");
            assert_eq!(id.len(), 39, "{id}");
            for g in groups {
                assert_eq!(g.len(), 4, "{id}");
                assert!(g.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()), "{id}");
            }
        }
        assert_ne!(generate_room_id(), generate_room_id());
        // Old 12-char IDs still normalise and join unchanged.
        assert_eq!(normalize_room("K7F2-9QZM-X4PD"), "k7f2-9qzm-x4pd");
        let mut st = CoopState::new("Me", 10.0, 0);
        assert_eq!(st.begin_trystero("K7F2-9QZM-X4PD", "Me", 10.0, 0).as_deref(), Some("k7f2-9qzm-x4pd"));
        st.stop();
    }

    /// Mesh-mode plumbing of the public API on a bare state (no threads, no network).
    #[test]
    fn mesh_mode_sends_go_to_every_peer_and_stop_resets() {
        let mut st = CoopState::new("Me", 10.0, 0);
        let (tx1, rx1) = mpsc::sync_channel::<Message>(16);
        let (tx2, rx2) = mpsc::sync_channel::<Message>(16);
        {
            let mut i = st.inner.lock().unwrap();
            mesh::initial_role_state(&mut i, "abcd-efgh-jklm", "Me", 10.0);
            i.clients.push(("p1".into(), tx1));
            i.clients.push(("p2".into(), tx2));
        }
        assert!(st.role() == Role::Client);
        assert_eq!(st.words().as_deref(), Some("abcd-efgh-jklm"));
        assert_eq!(st.lan_url(), None);
        assert_eq!(st.download(), None);
        assert_eq!(st.roster().len(), 1);

        st.reader().push_local(&ForzaPacket::default());
        st.set_waypoint(Some((1.0, 2.0)), 30.0);
        st.update_identity("Renamed", 99.0);
        for rx in [&rx1, &rx2] {
            assert!(matches!(rx.try_recv(), Ok(Message::Binary(b)) if b.len() == ID_LEN + WIRE_LEN));
            assert!(matches!(rx.try_recv(), Ok(Message::Text(t)) if t.contains("Waypoint")));
            assert!(matches!(rx.try_recv(), Ok(Message::Text(t)) if t.contains("Peer") && t.contains("Renamed")));
        }
        assert_eq!(st.roster()[0].name, "Renamed");
        assert_eq!(st.waypoints().len(), 1);

        st.stop();
        assert!(st.role() == Role::Off);
        let i = st.inner.lock().unwrap();
        assert!(!i.mesh && i.clients.is_empty() && i.roster.is_empty() && i.words.is_none());
        assert!(i.waypoints.is_empty() && i.mesh_bound.is_empty());
    }

    // ── shared destination (Control::Dest) ─────────────────────────────

    /// Today's `Control` minus `Dest`: what an older app's host / client / mesh path parses
    /// with (each ends in `_ => {}` / ignores a parse error).
    #[derive(Deserialize)]
    #[serde(tag = "t")]
    #[allow(dead_code)]
    enum OldControl {
        Hello { name: String, hue: f32 },
        Welcome { id: String, roster: Vec<PlayerInfo> },
        Roster { players: Vec<PlayerInfo> },
        Update { name: String, hue: f32 },
        Waypoint { id: String, pos: Option<[f32; 2]>, hue: f32 },
        Peer { id: String, name: String, hue: f32 },
    }

    #[test]
    fn dest_json_set_clear_and_filter_bits_round_trip() {
        let set = Control::Dest { id: "abc".into(), pos: Some([1.5, -2.5]), hue: 200.0, f: 0b1000_0101, c: 0.25, ts: 1_700_000_000_123 };
        let s = serde_json::to_string(&set).unwrap();
        assert_eq!(s, r#"{"t":"Dest","id":"abc","pos":[1.5,-2.5],"hue":200.0,"f":133,"c":0.25,"ts":1700000000123}"#);
        let clear = Control::Dest { id: "abc".into(), pos: None, hue: 0.0, f: 0, c: 0.0, ts: 7 };
        let s2 = serde_json::to_string(&clear).unwrap();
        assert_eq!(s2, r#"{"t":"Dest","id":"abc","pos":null,"hue":0.0,"f":0,"c":0.0,"ts":7}"#);
        match serde_json::from_str::<Control>(&s).unwrap() {
            Control::Dest { pos: Some([x, z]), f, c, ts, .. } => {
                assert_eq!((x, z, f, c, ts), (1.5, -2.5, 133, 0.25, 1_700_000_000_123));
            }
            _ => panic!("set"),
        }
        assert!(matches!(serde_json::from_str::<Control>(&s2).unwrap(), Control::Dest { pos: None, ts: 7, .. }));
    }

    #[test]
    fn old_apps_ignore_dest_in_every_path() {
        // The host, client and mesh handlers of an older app all do
        // `match serde_json::from_str::<Control>(&t) { Ok(known…) => …, _ => {} }`: an unknown
        // tag must be a parse error (ignored), and the messages they do know must still parse.
        let dest = serde_json::to_string(&Control::Dest { id: "x".into(), pos: Some([1.0, 2.0]), hue: 1.0, f: 3, c: 0.5, ts: 9 }).unwrap();
        assert!(serde_json::from_str::<OldControl>(&dest).is_err());
        let wp = serde_json::to_string(&Control::Waypoint { id: "x".into(), pos: None, hue: 1.0 }).unwrap();
        assert!(serde_json::from_str::<OldControl>(&wp).is_ok());
    }

    #[test]
    fn dest_last_write_wins_on_ts_then_id() {
        let mut i = Inner::new("Me", 10.0, 0);
        assert!(i.accept_dest("b", Some([1.0, 1.0]), 1.0, 0, 0.0, 100));
        assert_eq!(i.dest_seq, 1);
        // Older loses (out-of-order arrival), equal ts with smaller / equal id loses.
        assert!(!i.accept_dest("z", Some([9.0, 9.0]), 1.0, 0, 0.0, 99));
        assert!(!i.accept_dest("a", Some([9.0, 9.0]), 1.0, 0, 0.0, 100));
        assert!(!i.accept_dest("b", Some([9.0, 9.0]), 1.0, 0, 0.0, 100));
        assert_eq!(i.dest_seq, 1);
        assert_eq!(i.dest_view().unwrap().setter_id, "b");
        // Equal ts, larger id wins.
        assert!(i.accept_dest("c", Some([2.0, 2.0]), 1.0, 0, 0.0, 100));
        assert_eq!(i.dest_view().unwrap().x, 2.0);
        // Convergence: both arrival orders of two simultaneous setters end the same.
        let mut j = Inner::new("Me", 10.0, 0);
        assert!(j.accept_dest("c", Some([2.0, 2.0]), 1.0, 0, 0.0, 100));
        assert!(!j.accept_dest("b", Some([1.0, 1.0]), 1.0, 0, 0.0, 100));
        assert_eq!(j.dest_view().unwrap().setter_id, "c");
        // A clear is a tombstone with its ts: a stale set cannot resurrect, a newer one can.
        assert!(i.accept_dest("a", None, 0.0, 0, 0.0, 150));
        assert!(i.dest_view().is_none() && i.dest.is_some());
        assert!(!i.accept_dest("z", Some([5.0, 5.0]), 1.0, 0, 0.0, 120));
        assert!(i.dest_view().is_none());
        assert!(i.dest_resend().is_none(), "a tombstone is not resent");
        assert!(i.accept_dest("a", Some([5.0, 5.0]), 1.0, 0, 0.0, 151));
        assert_eq!(i.dest_view().unwrap().x, 5.0);
        // Garbage is refused; the curve is clamped.
        assert!(!i.accept_dest("a", Some([f32::NAN, 0.0]), 1.0, 0, 0.0, 999));
        assert!(!i.accept_dest("a", Some([0.0, 0.0]), 1.0, 0, f32::INFINITY, 999));
        assert!(i.accept_dest("a", Some([0.0, 0.0]), 1.0, 0, 7.0, 999));
        assert_eq!(i.dest_view().unwrap().curve, 1.0);
    }

    #[test]
    fn dest_unknown_filter_bits_are_ignored_but_preserved() {
        let mut i = Inner::new("Me", 10.0, 0);
        assert!(i.accept_dest("a", Some([1.0, 2.0]), 1.0, DEST_ROAD | DEST_JUMPS | 0x80, 0.5, 1));
        let d = i.dest_view().unwrap();
        assert_eq!(d.filters(), DEST_ROAD | DEST_JUMPS, "consumers see known bits only");
        assert_eq!(d.filter_bits, 0b1010_0001);
        // …but a relay / resend carries the raw byte on.
        match i.dest_resend() {
            Some(Message::Text(t)) => assert!(t.contains(r#""f":161"#), "{t}"),
            _ => panic!("expected a resend frame"),
        }
    }

    #[test]
    fn destination_api_syncs_resets_on_stop_and_wins_locally() {
        let mut st = CoopState::new("Me", 10.0, 0);
        st.set_destination((1.0, 2.0), 30.0, DEST_ROAD, 0.5);
        assert!(st.destination().is_none(), "no-op outside a session");
        let (tx, rx) = mpsc::sync_channel::<Message>(16);
        {
            let mut i = st.inner.lock().unwrap();
            mesh::initial_role_state(&mut i, "abcd-efgh-jklm", "Me", 10.0);
            i.clients.push(("p1".into(), tx));
        }
        let seq0 = st.destination_seq();
        st.set_destination((1.0, 2.0), 30.0, DEST_ROAD | DEST_DIRT, 0.5);
        let d = st.destination().unwrap();
        assert_eq!((d.x, d.z, d.hue, d.filters(), d.curve), (1.0, 2.0, 30.0, 5, 0.5));
        assert_eq!((d.setter_id.as_str(), d.setter_name.as_str()), (st.my_id().as_str(), "Me"));
        assert_eq!(st.destination_seq(), seq0 + 1);
        assert_eq!(st.reader().destination(), Some(d.clone()));
        assert!(matches!(rx.try_recv(), Ok(Message::Text(t)) if t.contains(r#""t":"Dest""#) && t.contains(r#""f":5"#)));
        // A remote write from the future is beaten by our next one even with a skewed clock.
        {
            let mut i = st.inner.lock().unwrap();
            let ts = i.dest.as_ref().unwrap().ts + 1_000_000;
            assert!(i.accept_dest("zzz", Some([7.0, 7.0]), 1.0, 0, 0.0, ts));
        }
        st.clear_destination();
        assert!(st.destination().is_none(), "my clear wins over the skewed remote set");
        assert!(matches!(rx.try_recv(), Ok(Message::Text(t)) if t.contains(r#""pos":null"#)));
        st.set_destination((3.0, 4.0), 30.0, 0, 0.0);
        assert!(st.destination().is_some());
        st.stop();
        assert!(st.destination().is_none());
        assert!(st.inner.lock().unwrap().dest.is_none(), "tombstone dropped too");
    }

    // ── loopback: the real Cloudflare host and client paths ────────────

    fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(Instant::now() < end, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    type RawWs = WebSocket<MaybeTlsStream<TcpStream>>;

    /// A bare client: Hello, then read until Welcome; returns the socket and its host-given id.
    fn raw_join(port: u16, name: &str) -> (RawWs, String) {
        let (mut ws, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();
        set_client_timeout(&mut ws, Some(Duration::from_secs(5)));
        let hello = Control::Hello { name: name.into(), hue: 1.0 };
        ws.send(Message::Text(serde_json::to_string(&hello).unwrap())).unwrap();
        loop {
            if let Message::Text(t) = ws.read().unwrap() {
                if let Ok(Control::Welcome { id, .. }) = serde_json::from_str::<Control>(&t) {
                    return (ws, id);
                }
            }
        }
    }

    /// Next `Dest` frame (other frames skipped).
    fn next_dest(ws: &mut RawWs) -> (String, Option<[f32; 2]>, u8, u64) {
        loop {
            if let Message::Text(t) = ws.read().expect("a Dest frame") {
                if let Ok(Control::Dest { id, pos, f, ts, .. }) = serde_json::from_str::<Control>(&t) {
                    return (id, pos, f, ts);
                }
            }
        }
    }

    fn send_dest(ws: &mut RawWs, id: &str, pos: Option<[f32; 2]>, f: u8, ts: u64) {
        let c = Control::Dest { id: id.into(), pos, hue: 5.0, f, c: 0.5, ts };
        ws.send(Message::Text(serde_json::to_string(&c).unwrap())).unwrap();
    }

    #[test]
    fn host_relays_dest_by_connection_id_and_resends_to_late_joiners() {
        let st = CoopState::new("Host", 10.0, 0);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        st.inner.lock().unwrap().role = Role::Host;
        {
            let (inner, stop) = (st.inner.clone(), stop.clone());
            std::thread::spawn(move || host_accept_loop(listener, inner, stop));
        }
        st.set_destination((10.0, 20.0), 30.0, DEST_ROAD, 0.0);

        // A joins late: gets the host's destination right after Welcome, with the host's id.
        let (mut a, a_id) = raw_join(port, "A");
        let (id, pos, _, _) = next_dest(&mut a);
        assert_eq!((id.as_str(), pos), (st.my_id().as_str(), Some([10.0, 20.0])));
        let (mut b, b_id) = raw_join(port, "B");
        assert_eq!(next_dest(&mut b).1, Some([10.0, 20.0]));

        // B sets one with a forged id and a reserved filter bit: host stores it under B's
        // connection id and relays that (not the forged one), raw bits intact, to A and itself.
        let host_ts = st.destination().unwrap().ts;
        send_dest(&mut b, "forged", Some([1.0, 2.0]), DEST_JUMPS | 0x40, host_ts + 10);
        let (id, pos, f, ts) = next_dest(&mut a);
        assert_eq!((id.as_str(), pos, f, ts), (b_id.as_str(), Some([1.0, 2.0]), DEST_JUMPS | 0x40, host_ts + 10));
        let d = st.destination().unwrap();
        assert_eq!((d.setter_id.as_str(), d.setter_name.as_str()), (b_id.as_str(), "B"));

        // A stale write (older ts) is neither stored nor relayed; A's clear is, to B.
        send_dest(&mut a, "a", Some([9.0, 9.0]), 0, host_ts + 5);
        send_dest(&mut a, "a", None, 0, host_ts + 20);
        let (id, pos, _, _) = next_dest(&mut b);
        assert_eq!((id.as_str(), pos), (a_id.as_str(), None), "the stale frame was not relayed");
        assert!(st.destination().is_none());

        // The host's own clear reaches both; a client joining after a clear gets nothing.
        st.set_destination((3.0, 3.0), 30.0, 0, 0.0);
        assert_eq!(next_dest(&mut a).1, Some([3.0, 3.0]));
        assert_eq!(next_dest(&mut b).1, Some([3.0, 3.0]));
        st.clear_destination();
        assert_eq!(next_dest(&mut a).1, None);
        let (mut c, _) = raw_join(port, "C");
        set_client_timeout(&mut c, Some(Duration::from_millis(300)));
        let got_dest = (0..5).any(|_| match c.read() {
            Ok(Message::Text(t)) => matches!(serde_json::from_str::<Control>(&t), Ok(Control::Dest { .. })),
            _ => false,
        });
        assert!(!got_dest, "a cleared destination is not resent");
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn real_client_loop_adopts_and_sends_dest() {
        let host = CoopState::new("Host", 10.0, 0);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        host.inner.lock().unwrap().role = Role::Host;
        {
            let (inner, stop) = (host.inner.clone(), stop.clone());
            std::thread::spawn(move || host_accept_loop(listener, inner, stop));
        }
        host.set_destination((10.0, 20.0), 30.0, DEST_HIGHWAY, 0.75);

        let mut guest = Inner::new("Guest", 50.0, 0);
        guest.role = Role::Client;
        let guest = Arc::new(Mutex::new(guest));
        {
            let (g, stop) = (guest.clone(), stop.clone());
            let url = format!("ws://127.0.0.1:{port}");
            std::thread::spawn(move || client_loop(url, "Guest".into(), 50.0, g, stop));
        }
        // Hello is flushed on its own: no telemetry / waypoint is needed for the host to welcome
        // the client (the host gives up after 10 s without one).
        let st = CoopState { inner: guest.clone(), stop: stop.clone(), port: 0 };
        wait_for("client connects", || guest.lock().unwrap().client_out.is_some());
        // Late-join resend, taken over by the client path with the host's id.
        wait_for("client adopts the host's destination", || guest.lock().unwrap().dest_view().is_some());
        let d = guest.lock().unwrap().dest_view().unwrap();
        assert_eq!((d.x, d.z, d.filters(), d.curve, d.setter_id.as_str()), (10.0, 20.0, DEST_HIGHWAY, 0.75, host.my_id().as_str()));

        // The client's own write goes up to the host (stored under the connection id).
        let my_id = guest.lock().unwrap().my_id.clone();
        st.set_destination((7.0, 8.0), 50.0, DEST_TRAIL, 0.1);
        wait_for("host receives the client's destination", || host.destination().is_some_and(|d| d.x == 7.0));
        assert_eq!(host.destination().unwrap().setter_id, my_id);
        // …and a clear from the host comes back down.
        host.clear_destination();
        wait_for("client sees the clear", || guest.lock().unwrap().dest_view().is_none());
        stop.store(true, Ordering::Relaxed);
    }
}
