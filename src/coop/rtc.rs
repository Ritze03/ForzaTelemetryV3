//! WebRTC half of the Trystero transport: one peer connection + one reliable ordered data
//! channel ("data") per remote player, all driven by a single dedicated thread running a smol
//! executor (the app has no tokio; `webrtc` is built with its smol runtime).
//!
//! ICE is non-trickle: we wait for gathering to finish and ship the complete SDP (~2 KB) in
//! one signalling message each way, so the relays only ever carry two messages per pair.
//!
//! Why a `LocalExecutor` on our own thread instead of `smol::spawn`: shutdown is then
//! deterministic — when the session stops we drop every link's kill switch, each task closes
//! its peer connection, and the thread ends. Nothing lingers on smol's global executor.

use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use bytes::BytesMut;
use smol::future::or;
use smol::{LocalExecutor, Timer};
use tungstenite::Message;
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelState};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState, RTCSessionDescription,
};
use webrtc::runtime::SmolRuntime;

use super::mesh::Session;

pub const ICE_SERVERS: &[&str] = &[
    "stun:stun.l.google.com:19302",
    "stun:stun1.l.google.com:19302",
    "stun:stun.cloudflare.com:3478",
];

/// How long an outgoing offer waits for its answer (Trystero's offer TTL).
const OFFER_TTL: Duration = Duration::from_secs(57);
/// After the SDP exchange: how long until the data channel must be open.
const CONNECT_TTL: Duration = Duration::from_secs(30);
/// Non-trickle: how long to wait for ICE gathering to finish before sending what we have
/// (an unreachable STUN server must not stall the handshake).
const GATHER_MAX: Duration = Duration::from_secs(8);
/// A `Disconnected` connection may still recover; give it this long before giving up.
const DISCONNECT_GRACE: Duration = Duration::from_secs(12);

pub enum Cmd {
    /// Create an offer for `peer` (we saw their announce).
    Offer { peer: String, offer_id: String, gen: u64 },
    /// Answer `peer`'s offer `sdp`.
    Answer { peer: String, offer_id: String, gen: u64, sdp: String },
    /// `peer` answered our offer.
    Accept { peer: String, gen: u64, sdp: String },
}

/// How a link ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Replaced or shut down by us; the session bookkeeping was already handled.
    Killed,
    Closed,
    /// Never got (or lost) a route. `nat`: it failed while connecting, i.e. no direct path.
    Failed { nat: bool },
}

pub async fn timeout<T>(d: Duration, f: impl Future<Output = T>) -> Option<T> {
    or(async { Some(f.await) }, async {
        Timer::after(d).await;
        None
    })
    .await
}

// ── peer connection wrapper ────────────────────────────────────────

struct Handler {
    gather: Sender<()>,
    dc: Sender<Arc<dyn DataChannel>>,
    state: Sender<RTCPeerConnectionState>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, s: RTCIceGatheringState) {
        if s == RTCIceGatheringState::Complete {
            let _ = self.gather.try_send(());
        }
    }
    async fn on_connection_state_change(&self, s: RTCPeerConnectionState) {
        let _ = self.state.try_send(s);
    }
    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        let _ = self.dc.try_send(dc);
    }
}

pub struct Link {
    pub pc: Arc<dyn PeerConnection>,
    gather: Receiver<()>,
    /// Answerer side: the channel the offerer opened.
    pub dc_in: Receiver<Arc<dyn DataChannel>>,
    pub state: Receiver<RTCPeerConnectionState>,
}

fn es(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Link {
    /// `ice`: STUN URLs (empty in tests → host candidates only). `udp`: local bind address;
    /// `"0.0.0.0:0"` binds one socket per interface.
    pub async fn new(ice: &[&str], udp: &'static str) -> Result<Link, String> {
        let (gt, gather) = async_channel::bounded(1);
        let (dt, dc_in) = async_channel::bounded(4);
        let (st, state) = async_channel::bounded(16);
        let servers = if ice.is_empty() {
            vec![]
        } else {
            vec![RTCIceServer { urls: ice.iter().map(|s| s.to_string()).collect(), ..Default::default() }]
        };
        let pc = PeerConnectionBuilder::new()
            .with_configuration(RTCConfigurationBuilder::default().with_ice_servers(servers).build())
            .with_runtime(Arc::new(SmolRuntime))
            .with_handler(Arc::new(Handler { gather: gt, dc: dt, state: st }))
            .with_udp_addrs(vec![udp])
            // A stalled peer must not grow our memory: past 1 MiB `send` blocks, the writer
            // stops draining, and the bounded outgoing queue then drops telemetry frames.
            .with_data_channel_send_buffer_limit(1 << 20)
            .build()
            .await
            .map_err(es)?;
        Ok(Link { pc: Arc::new(pc), gather, dc_in, state })
    }

    async fn local_sdp(&self) -> Result<String, String> {
        let _ = timeout(GATHER_MAX, self.gather.recv()).await;
        self.pc.local_description().await.map(|d| d.sdp).ok_or_else(|| "no local description".into())
    }

    /// Offerer: create the data channel + full (gathered) offer SDP.
    pub async fn offer(&self) -> Result<(Arc<dyn DataChannel>, String), String> {
        let dc = self.pc.create_data_channel("data", None).await.map_err(es)?;
        let offer = self.pc.create_offer(None).await.map_err(es)?;
        self.pc.set_local_description(offer).await.map_err(es)?;
        Ok((dc, self.local_sdp().await?))
    }

    /// Answerer: apply the remote offer, return the full (gathered) answer SDP.
    pub async fn answer(&self, offer_sdp: String) -> Result<String, String> {
        self.pc
            .set_remote_description(RTCSessionDescription::offer(offer_sdp).map_err(es)?)
            .await
            .map_err(es)?;
        let ans = self.pc.create_answer(None).await.map_err(es)?;
        self.pc.set_local_description(ans).await.map_err(es)?;
        self.local_sdp().await
    }

    /// Offerer: apply the remote answer.
    pub async fn accept(&self, answer_sdp: String) -> Result<(), String> {
        self.pc
            .set_remote_description(RTCSessionDescription::answer(answer_sdp).map_err(es)?)
            .await
            .map_err(es)
    }
}

// ── data channel pump ──────────────────────────────────────────────

pub enum ChanEnd {
    NeverOpened,
    /// `on_open` refused (session stopped / slot replaced).
    Rejected,
    Closed,
}

/// Wait for `dc` to open, hand `on_open` the outgoing queue's sender, then pump: incoming
/// frames go to `on_msg` (text → `Message::Text`, binary → `Message::Binary`), and whatever
/// is queued on the outgoing queue is written to the channel. Ends when the channel closes or
/// the queue's sender side is dropped (someone removed us from `Inner::clients`).
///
/// Why the writer polls (4 ms) instead of awaiting: the shared `Inner::clients` queue is a
/// plain `std::sync::mpsc` (the same `Message` seam the WebSocket transports use, so
/// `Inner::broadcast` works untouched) and can't wake an async task.
pub async fn run_channel(
    dc: Arc<dyn DataChannel>,
    open_timeout: Duration,
    on_open: impl FnOnce(SyncSender<Message>) -> bool,
    on_msg: impl Fn(Message),
) -> ChanEnd {
    // Phase 1 polls the state instead of `poll()`ing events: an already-open channel (answerer
    // side) may never deliver an OnOpen, and dropping a half-awaited `poll` is not something
    // to rely on.
    let deadline = Instant::now() + open_timeout;
    loop {
        match dc.ready_state().await {
            Ok(RTCDataChannelState::Open) => break,
            Ok(RTCDataChannelState::Closing) | Ok(RTCDataChannelState::Closed) | Err(_) => {
                return ChanEnd::NeverOpened
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return ChanEnd::NeverOpened;
        }
        Timer::after(Duration::from_millis(50)).await;
    }

    let (tx, rx) = mpsc::sync_channel::<Message>(256);
    if !on_open(tx) {
        return ChanEnd::Rejected;
    }

    let reader = async {
        while let Some(ev) = dc.poll().await {
            match ev {
                DataChannelEvent::OnMessage(m) => on_msg(if m.is_string {
                    Message::Text(String::from_utf8_lossy(&m.data).into_owned())
                } else {
                    Message::Binary(m.data.to_vec())
                }),
                DataChannelEvent::OnClose => break,
                _ => {}
            }
        }
    };
    let writer = async {
        loop {
            loop {
                let sent = match rx.try_recv() {
                    Ok(Message::Text(t)) => dc.send_text(&t).await,
                    Ok(Message::Binary(b)) => dc.send(BytesMut::from(&b[..])).await,
                    Ok(_) => Ok(()),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                };
                if sent.is_err() {
                    return;
                }
            }
            Timer::after(Duration::from_millis(4)).await;
        }
    };
    or(reader, writer).await;
    ChanEnd::Closed
}

/// Watches the connection state: `Failed`/`Closed` end the link, `Disconnected` only after a
/// grace period (it can recover on a network blip).
pub async fn monitor(state: &Receiver<RTCPeerConnectionState>, opened: &Cell<bool>) -> Outcome {
    let mut disc_since: Option<Instant> = None;
    loop {
        let ev = match disc_since {
            Some(t) => timeout(DISCONNECT_GRACE.saturating_sub(t.elapsed()), state.recv()).await,
            None => Some(state.recv().await),
        };
        match ev {
            None => return Outcome::Failed { nat: false },
            Some(Err(_)) => std::future::pending::<()>().await,
            Some(Ok(RTCPeerConnectionState::Failed)) => return Outcome::Failed { nat: !opened.get() },
            Some(Ok(RTCPeerConnectionState::Closed)) => return Outcome::Closed,
            Some(Ok(RTCPeerConnectionState::Disconnected)) => {
                disc_since.get_or_insert_with(Instant::now);
            }
            Some(Ok(RTCPeerConnectionState::Connected)) => disc_since = None,
            Some(Ok(_)) => {}
        }
    }
}

// ── per-link task ──────────────────────────────────────────────────

enum Mode {
    Offer(String),
    Answer(String, String),
}

struct LinkCtl {
    gen: u64,
    /// Dropping this stops the link's task (which then closes its peer connection).
    _kill: Sender<()>,
    ans: Sender<String>,
}

async fn drive(
    sess: &Session,
    link: &Link,
    peer: &str,
    gen: u64,
    mode: Mode,
    ans: &Receiver<String>,
    opened: &Cell<bool>,
) -> Outcome {
    let dc = match mode {
        Mode::Offer(offer_id) => {
            let Ok((dc, sdp)) = link.offer().await else { return Outcome::Closed };
            sess.send_offer(peer, &offer_id, &sdp);
            // No answer is the normal "peer left before replying" case: not a NAT problem.
            let Some(Ok(answer)) = timeout(OFFER_TTL, ans.recv()).await else {
                return Outcome::Closed;
            };
            if link.accept(answer).await.is_err() {
                return Outcome::Closed;
            }
            dc
        }
        Mode::Answer(offer_id, offer_sdp) => {
            let Ok(sdp) = link.answer(offer_sdp).await else { return Outcome::Closed };
            sess.send_answer(peer, &offer_id, &sdp);
            match timeout(CONNECT_TTL, link.dc_in.recv()).await {
                Some(Ok(dc)) => dc,
                _ => return Outcome::Failed { nat: true },
            }
        }
    };
    let end = run_channel(
        dc,
        CONNECT_TTL,
        |tx| {
            let ok = sess.link_open(peer, gen, tx);
            opened.set(ok);
            ok
        },
        |m| sess.on_msg(peer, m),
    )
    .await;
    match end {
        ChanEnd::NeverOpened => Outcome::Failed { nat: true },
        ChanEnd::Rejected | ChanEnd::Closed => Outcome::Closed,
    }
}

async fn link_task(
    sess: Arc<Session>,
    peer: String,
    gen: u64,
    mode: Mode,
    kill: Receiver<()>,
    ans: Receiver<String>,
    active: Rc<Cell<usize>>,
) {
    let opened = Cell::new(false);
    let (ice, udp) = sess.ice();
    let out = match Link::new(ice, udp).await {
        Err(_) => Outcome::Closed,
        Ok(link) => {
            let o = or(
                or(
                    drive(&sess, &link, &peer, gen, mode, &ans, &opened),
                    monitor(&link.state, &opened),
                ),
                async {
                    let _ = kill.recv().await; // a message or the sender dropping: both mean stop
                    Outcome::Killed
                },
            )
            .await;
            let _ = link.pc.close().await;
            o
        }
    };
    match out {
        Outcome::Killed => {}
        Outcome::Closed => sess.link_ended(&peer, gen, false),
        Outcome::Failed { nat } => sess.link_ended(&peer, gen, nat),
    }
    active.set(active.get() - 1);
}

enum Ev {
    Cmd(Cmd),
    Tick,
    Closed,
}

/// Thread body: serve signalling commands until the session stops.
pub fn run(sess: Arc<Session>, rx: Receiver<Cmd>) {
    let ex = Rc::new(LocalExecutor::new());
    smol::block_on(ex.run(main_loop(sess, rx, ex.clone())));
}

async fn main_loop(sess: Arc<Session>, rx: Receiver<Cmd>, ex: Rc<LocalExecutor<'static>>) {
    let active = Rc::new(Cell::new(0usize));
    let mut links: HashMap<String, LinkCtl> = HashMap::new();

    let spawn = |links: &mut HashMap<String, LinkCtl>, peer: String, gen: u64, mode: Mode| {
        let (kill_tx, kill_rx) = async_channel::bounded::<()>(1);
        let (ans_tx, ans_rx) = async_channel::bounded::<String>(1);
        active.set(active.get() + 1);
        ex.spawn(link_task(sess.clone(), peer.clone(), gen, mode, kill_rx, ans_rx, active.clone()))
            .detach();
        // Replacing an entry drops the old kill switch, which stops the old link (glare).
        links.insert(peer, LinkCtl { gen, _kill: kill_tx, ans: ans_tx });
    };

    while !sess.stopped() {
        let ev = or(
            async {
                match rx.recv().await {
                    Ok(c) => Ev::Cmd(c),
                    Err(_) => Ev::Closed,
                }
            },
            async {
                Timer::after(Duration::from_millis(100)).await;
                Ev::Tick
            },
        )
        .await;
        match ev {
            Ev::Closed => break,
            Ev::Tick => {}
            Ev::Cmd(Cmd::Offer { peer, offer_id, gen }) => spawn(&mut links, peer, gen, Mode::Offer(offer_id)),
            Ev::Cmd(Cmd::Answer { peer, offer_id, gen, sdp }) => {
                spawn(&mut links, peer, gen, Mode::Answer(offer_id, sdp))
            }
            Ev::Cmd(Cmd::Accept { peer, gen, sdp }) => {
                if let Some(l) = links.get(&peer).filter(|l| l.gen == gen) {
                    let _ = l.ans.try_send(sdp);
                }
            }
        }
        links.retain(|_, l| !l.ans.is_closed()); // finished tasks dropped their receivers
    }

    links.clear(); // kill every link; each closes its peer connection
    let end = Instant::now() + Duration::from_secs(2);
    while active.get() > 0 && Instant::now() < end {
        Timer::after(Duration::from_millis(20)).await;
    }
}
