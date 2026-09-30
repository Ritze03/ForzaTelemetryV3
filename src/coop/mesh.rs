//! Room logic of the Trystero transport: who to connect to (Nostr announces/offers/answers),
//! and how a connected peer's data-channel traffic lands in the shared co-op `Inner` state.
//!
//! Mesh, not star: every peer in a room is equal and each pair has one data channel. Each side
//! sends its own telemetry to every peer, so there is no host to lose and nothing to relay.
//! Why: with no server, a "host" would just be a random player whose PC has to forward everyone's
//! packets and whose disconnect kills the room; rooms are small (a handful of friends), so the
//! O(n²) links are cheap.
//!
//! Lock order: `Inner` before `SigState` (only `refresh` takes both); never take `Inner` while
//! holding `SigState`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::Message;
use uuid::Uuid;

use super::nostr::{self, OfferAction};
use super::rtc::Cmd;
use super::{Control, Inner, PlayerInfo, RemoteBuf, Role, ID_LEN, WIRE_LEN};
use crate::packet::ForzaPacket;

pub const NAT_ERROR: &str = "Couldn't reach a player directly (NAT). Try the Cloudflare option.";
const NO_RELAY_ERROR: &str = "No relay reachable";
/// Upper bound on simultaneous peers (live + handshaking): a rogue announce flood can't make
/// us spin up unbounded peer connections.
const MAX_PEERS: usize = 24;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ── shared-state handlers (no signalling; unit-testable on a bare `Inner`) ─────

/// Our identity as a `Control::Peer` frame, sent on channel open and on identity change.
pub fn peer_msg(inner: &Inner) -> Message {
    let c = Control::Peer { id: inner.my_id.clone(), name: inner.my_name.clone(), hue: inner.my_hue };
    Message::Text(serde_json::to_string(&c).unwrap_or_default())
}

fn drop_player(g: &mut Inner, id: &str) {
    g.roster.retain(|p| p.id != id);
    g.remote.remove(id);
    g.waypoints.remove(id);
}

/// A data channel to `key` (the remote's signalling peer id) opened: register its outgoing
/// queue so `Inner::broadcast` reaches it, and introduce ourselves first.
pub fn on_open(inner: &Mutex<Inner>, stop: &AtomicBool, key: &str, tx: SyncSender<Message>) -> bool {
    let mut g = lock(inner);
    if stop.load(Ordering::Relaxed) {
        return false;
    }
    let _ = tx.try_send(peer_msg(&g)); // queued before anything else can be
    g.clients.retain(|(k, _)| k != key);
    g.clients.push((key.to_string(), tx));
    true
}

/// A frame arrived on `key`'s channel.
///
/// Anti-spoof: the player id is what *that channel* announced in its `Peer` frame (a channel
/// can only ever speak for one id, and at most one channel holds an id at a time); the 16-byte
/// id prefix inside binary telemetry frames is ignored, exactly like the host does for WS clients.
///
/// Why "latest channel wins" when a second channel claims an already-bound id: a player who
/// left and rejoined may show up on a new channel before we noticed the old link die, and `Peer`
/// is sent only once per channel, so rejecting it would leave them invisible to us forever.
/// The older channel is evicted (its queue is dropped, which ends its pump and closes the link).
pub fn on_message(inner: &Mutex<Inner>, stop: &AtomicBool, key: &str, msg: Message) {
    let mut g = lock(inner);
    if stop.load(Ordering::Relaxed) || !g.mesh {
        return;
    }
    match msg {
        Message::Binary(d) if d.len() >= ID_LEN + WIRE_LEN => {
            let Some(id) = g.mesh_bound.get(key).cloned() else { return };
            if let Some(pkt) = ForzaPacket::from_bytes(&d[ID_LEN..]) {
                if let Some(buf) = g.remote.get_mut(&id) {
                    buf.push(pkt);
                }
            }
        }
        Message::Text(t) => match serde_json::from_str::<Control>(&t) {
            Ok(Control::Peer { id, name, hue }) => {
                if Uuid::parse_str(&id).is_err() || id == g.my_id {
                    return;
                }
                let name: String = name.chars().take(32).collect();
                let stale: Vec<String> = g
                    .mesh_bound
                    .iter()
                    .filter(|(k, v)| **v == id && *k != key)
                    .map(|(k, _)| k.clone())
                    .collect();
                for k in stale {
                    g.mesh_bound.remove(&k);
                    g.clients.retain(|(c, _)| *c != k);
                    g.waypoints.remove(&id); // the old session's ping is stale
                }
                if let Some(old) = g.mesh_bound.insert(key.to_string(), id.clone()) {
                    if old != id {
                        drop_player(&mut g, &old);
                    }
                }
                match g.roster.iter_mut().find(|p| p.id == id) {
                    Some(p) => {
                        p.name = name;
                        p.hue = hue;
                    }
                    None => g.roster.push(PlayerInfo { id: id.clone(), name, hue }),
                }
                g.remote.entry(id).or_insert_with(RemoteBuf::new);
            }
            Ok(Control::Waypoint { pos, hue, .. }) => {
                // Keyed by the channel's bound id, never the message's.
                let Some(id) = g.mesh_bound.get(key).cloned() else { return };
                match pos {
                    Some([x, z]) => {
                        g.waypoints.insert(id, (x, z, hue));
                    }
                    None => {
                        g.waypoints.remove(&id);
                    }
                }
            }
            _ => {}
        },
        _ => {}
    }
}

/// `key`'s channel is gone: mirror of the host's `cleanup_client`.
pub fn on_gone(inner: &Mutex<Inner>, key: &str) {
    let mut g = lock(inner);
    g.clients.retain(|(k, _)| k != key);
    if let Some(id) = g.mesh_bound.remove(key) {
        drop_player(&mut g, &id);
    }
}

// ── signalling session ─────────────────────────────────────────────

#[derive(PartialEq, Eq)]
enum Phase {
    /// We sent an offer with this id and await the answer.
    Offering(String),
    /// We answered an offer with this id; the connection is being established.
    Answering(String),
    Live,
}

struct Slot {
    phase: Phase,
    /// Distinguishes successive links to the same peer, so a replaced link's late cleanup
    /// can't tear down its successor.
    gen: u64,
}

#[derive(Default, Clone, Copy)]
struct RelayState {
    up: bool,
    failed: bool,
    retired: bool,
}

#[derive(Default)]
struct SigState {
    /// De-dupe of `peer|offerId|kind`: the same message arrives once per shared relay.
    seen: HashMap<String, Instant>,
    slots: HashMap<String, Slot>,
    relays: Vec<RelayState>,
    ever_up: bool,
    next_gen: u64,
    nat_failed: bool,
}

pub struct Session {
    self_id: String,
    room: String,
    root_topic: String,
    self_topic: String,
    key: [u8; 32],
    keys: nostr::Keys,
    stop: Arc<AtomicBool>,
    inner: Arc<Mutex<Inner>>,
    st: Mutex<SigState>,
    relay_q: Vec<mpsc::Sender<String>>,
    rtc_tx: async_channel::Sender<Cmd>,
    kick: mpsc::Sender<()>,
    ice: &'static [&'static str],
    udp: &'static str,
}

/// The receiving ends `build` hands back for the threads to own.
struct Rx {
    rtc: async_channel::Receiver<Cmd>,
    kick: mpsc::Receiver<()>,
    relays: Vec<mpsc::Receiver<String>>,
}

impl Session {
    /// Spawn the relay threads, the announcer and the WebRTC thread. Returns immediately.
    pub fn start(inner: Arc<Mutex<Inner>>, stop: Arc<AtomicBool>, room: &str) {
        let (sess, rx) = Self::build(inner, stop, room, super::rtc::ICE_SERVERS, "0.0.0.0:0");
        let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| {
            let _ = std::thread::Builder::new().name(name.to_string()).spawn(f);
        };
        let s = sess.clone();
        spawn("coop-rtc", Box::new(move || super::rtc::run(s, rx.rtc)));
        let s = sess.clone();
        spawn("coop-announce", Box::new(move || nostr::announce_loop(s, rx.kick)));
        for (idx, (url, rx)) in nostr::RELAYS.iter().zip(rx.relays).enumerate() {
            let s = sess.clone();
            spawn("coop-relay", Box::new(move || nostr::relay_loop(s, idx, url, rx)));
        }
    }

    /// Everything except the threads (tests drive a session by hand, with fake relays).
    fn build(
        inner: Arc<Mutex<Inner>>,
        stop: Arc<AtomicBool>,
        room: &str,
        ice: &'static [&'static str],
        udp: &'static str,
    ) -> (Arc<Session>, Rx) {
        let (rtc_tx, rtc) = async_channel::unbounded();
        let (kick, kick_rx) = mpsc::channel();
        let (mut relay_q, mut relays) = (Vec::new(), Vec::new());
        for _ in nostr::RELAYS {
            let (t, r) = mpsc::channel();
            relay_q.push(t);
            relays.push(r);
        }
        let self_id = nostr::rand_id(20);
        let sess = Arc::new(Session {
            root_topic: nostr::root_topic(room),
            self_topic: nostr::peer_topic(room, &self_id),
            key: nostr::room_key(room),
            keys: nostr::Keys::generate(),
            room: room.to_string(),
            self_id,
            stop,
            inner,
            st: Mutex::new(SigState {
                relays: vec![RelayState::default(); nostr::RELAYS.len()],
                ..Default::default()
            }),
            relay_q,
            rtc_tx,
            kick,
            ice,
            udp,
        });
        (sess, Rx { rtc, kick: kick_rx, relays })
    }

    /// STUN servers / local UDP bind address for new peer connections.
    pub fn ice(&self) -> (&'static [&'static str], &'static str) {
        (self.ice, self.udp)
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    // -- frames --

    pub fn req_frame(&self, sub_id: &str) -> String {
        nostr::req_frame(
            sub_id,
            &[&self.root_topic, &self.self_topic],
            nostr::now_secs().saturating_sub(nostr::SINCE_SLACK_SECS),
        )
    }

    /// A fresh announce (new nonce, so relays don't drop it as a duplicate id).
    pub fn announce_frame(&self) -> String {
        let content = json!({"peerId": self.self_id, "nonce": nostr::rand_id(8)}).to_string();
        nostr::event_frame(&self.keys, &self.root_topic, &content)
    }

    /// Queue a frame on every relay thread.
    pub fn broadcast(&self, frame: String) {
        for q in &self.relay_q {
            let _ = q.send(frame.clone());
        }
    }

    fn send_signal(&self, to: &str, payload: Value) {
        let topic = nostr::peer_topic(&self.room, to);
        self.broadcast(nostr::event_frame(&self.keys, &topic, &payload.to_string()));
    }

    pub fn send_offer(&self, to: &str, offer_id: &str, sdp: &str) {
        let enc = nostr::encrypt(&self.key, sdp);
        self.send_signal(to, json!({"peerId": self.self_id, "offerId": offer_id, "offer": enc}));
    }

    pub fn send_answer(&self, to: &str, offer_id: &str, sdp: &str) {
        let enc = nostr::encrypt(&self.key, sdp);
        self.send_signal(to, json!({"peerId": self.self_id, "offerId": offer_id, "answer": enc}));
    }

    // -- incoming signalling --

    pub fn handle_event(&self, topic: &str, content: &str) {
        let Ok(v) = serde_json::from_str::<Value>(content) else { return };
        let Some(peer) = v["peerId"].as_str() else { return };
        if peer == self.self_id || peer.is_empty() || peer.len() > 64 {
            return;
        }
        if topic == self.root_topic {
            self.on_announce(peer);
        } else if topic == self.self_topic {
            let Some(offer_id) = v["offerId"].as_str() else { return };
            if let Some(enc) = v["offer"].as_str() {
                self.on_offer(peer, offer_id, enc);
            } else if let Some(enc) = v["answer"].as_str() {
                self.on_answer(peer, offer_id, enc);
            }
        }
    }

    fn on_announce(&self, peer: &str) {
        let (offer_id, gen) = {
            let mut st = lock(&self.st);
            if st.slots.contains_key(peer) || st.slots.len() >= MAX_PEERS {
                return;
            }
            let (offer_id, gen) = (nostr::rand_id(12), st.next_gen);
            st.next_gen += 1;
            st.slots.insert(peer.to_string(), Slot { phase: Phase::Offering(offer_id.clone()), gen });
            (offer_id, gen)
        };
        let _ = self.rtc_tx.try_send(Cmd::Offer { peer: peer.to_string(), offer_id, gen });
        self.refresh();
    }

    /// First sighting of `(peer, offer_id, kind)`? (Same message via several relays → false.)
    fn first_seen(st: &mut SigState, peer: &str, offer_id: &str, kind: &str) -> bool {
        if st.seen.len() > 512 {
            st.seen.retain(|_, t| t.elapsed() < Duration::from_secs(120));
        }
        st.seen.insert(format!("{peer}|{offer_id}|{kind}"), Instant::now()).is_none()
    }

    fn on_offer(&self, peer: &str, offer_id: &str, enc: &str) {
        let gen = {
            let mut st = lock(&self.st);
            if !Self::first_seen(&mut st, peer, offer_id, "o") {
                return;
            }
            match st.slots.get(peer).map(|s| &s.phase) {
                Some(Phase::Live) => return,
                Some(Phase::Answering(id)) if id == offer_id => return,
                other => {
                    let outgoing = matches!(other, Some(Phase::Offering(_)));
                    if nostr::glare(outgoing, &self.self_id, peer) == OfferAction::IgnoreTheirs {
                        return;
                    }
                }
            }
            if st.slots.len() >= MAX_PEERS && !st.slots.contains_key(peer) {
                return;
            }
            let gen = st.next_gen;
            st.next_gen += 1;
            st.slots
                .insert(peer.to_string(), Slot { phase: Phase::Answering(offer_id.to_string()), gen });
            gen
        };
        // A forged or wrong-room offer fails AES-GCM here and is dropped.
        match nostr::decrypt(&self.key, enc) {
            Some(sdp) => {
                let _ = self.rtc_tx.try_send(Cmd::Answer {
                    peer: peer.to_string(),
                    offer_id: offer_id.to_string(),
                    gen,
                    sdp,
                });
            }
            None => {
                lock(&self.st).slots.remove(peer);
            }
        }
        self.refresh();
    }

    fn on_answer(&self, peer: &str, offer_id: &str, enc: &str) {
        let gen = {
            let mut st = lock(&self.st);
            if !Self::first_seen(&mut st, peer, offer_id, "a") {
                return;
            }
            match st.slots.get(peer) {
                Some(Slot { phase: Phase::Offering(id), gen }) if id == offer_id => *gen,
                _ => return,
            }
        };
        if let Some(sdp) = nostr::decrypt(&self.key, enc) {
            let _ = self.rtc_tx.try_send(Cmd::Accept { peer: peer.to_string(), gen, sdp });
        }
    }

    // -- link lifecycle (called from the WebRTC thread) --

    /// The channel to `peer` opened. False = the link was replaced/stopped: close it.
    pub fn link_open(&self, peer: &str, gen: u64, tx: SyncSender<Message>) -> bool {
        {
            let mut st = lock(&self.st);
            match st.slots.get_mut(peer) {
                Some(s) if s.gen == gen => s.phase = Phase::Live,
                _ => return false,
            }
            st.nat_failed = false;
        }
        let ok = on_open(&self.inner, &self.stop, peer, tx);
        self.refresh();
        ok
    }

    pub fn on_msg(&self, peer: &str, msg: Message) {
        let is_text = matches!(msg, Message::Text(_));
        on_message(&self.inner, &self.stop, peer, msg);
        if is_text {
            self.refresh(); // a Peer frame changes the roster (and the status with it)
        }
    }

    pub fn link_ended(&self, peer: &str, gen: u64, nat: bool) {
        {
            let mut st = lock(&self.st);
            if st.slots.get(peer).map(|s| s.gen) != Some(gen) {
                return; // already replaced by a newer link
            }
            st.slots.remove(peer);
            st.nat_failed |= nat;
        }
        if !self.stopped() {
            on_gone(&self.inner, peer);
        }
        self.refresh();
        // Let the peer find us again promptly (they may still be in the room).
        let _ = self.kick.send(());
    }

    // -- relay state --

    pub fn relay_up(&self, idx: usize) {
        {
            let mut st = lock(&self.st);
            st.relays[idx] = RelayState { up: true, failed: false, retired: false };
            st.ever_up = true;
        }
        self.refresh();
    }

    pub fn relay_down(&self, idx: usize, failed: bool, retired: bool) {
        {
            let mut st = lock(&self.st);
            let r = &mut st.relays[idx];
            r.up = false;
            r.failed |= failed;
            r.retired |= retired;
        }
        self.refresh();
    }

    /// Recompute `status`/`error` from the room state.
    fn refresh(&self) {
        let mut g = lock(&self.inner);
        if self.stopped() || !g.mesh {
            return;
        }
        let st = lock(&self.st);
        let up = st.relays.iter().filter(|r| r.up).count();
        let all_failed = st.relays.iter().all(|r| r.failed || r.retired);
        let pending = st.slots.values().filter(|s| s.phase != Phase::Live).count();
        let players = g.roster.len();
        g.status = if players > 1 {
            format!("{players} player(s)")
        } else if pending > 0 {
            "Negotiating…".into()
        } else if up == 0 {
            if st.ever_up { "Reconnecting…" } else { "Connecting to relays…" }.into()
        } else {
            "Waiting for players…".into()
        };
        // A failed handshake with one player says nothing once another link works (they may
        // simply have left mid-handshake): only warn about NAT while nobody is connected.
        let live = st.slots.values().any(|s| s.phase == Phase::Live);
        g.error = if st.nat_failed && !live {
            Some(NAT_ERROR.into())
        } else if up == 0 && all_failed && players <= 1 {
            Some(NO_RELAY_ERROR.into())
        } else {
            None
        };
    }
}

pub fn initial_role_state(g: &mut Inner, room: &str, name: &str, hue: f32) {
    g.role = Role::Client;
    g.mesh = true;
    g.my_name = name.to_string();
    g.my_hue = hue;
    g.words = Some(room.to_string());
    g.status = "Connecting to relays…".into();
    g.error = None;
    g.roster = vec![PlayerInfo { id: g.my_id.clone(), name: name.to_string(), hue }];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coop::{push_local, remote_players, tick};
    use crate::coop::rtc::{run_channel, timeout, Link};
    use std::cell::Cell;

    fn mesh_inner(name: &str) -> Arc<Mutex<Inner>> {
        let mut i = Inner::new(name, 10.0, 0);
        initial_role_state(&mut i, "room", name, 10.0);
        Arc::new(Mutex::new(i))
    }

    fn peer_frame(id: &str, name: &str) -> Message {
        Message::Text(
            serde_json::to_string(&Control::Peer { id: id.into(), name: name.into(), hue: 5.0 }).unwrap(),
        )
    }

    fn telemetry_frame(spoofed_id: [u8; ID_LEN], speed: f32) -> Message {
        let pkt = ForzaPacket { speed, ..Default::default() };
        let mut f = spoofed_id.to_vec();
        f.extend_from_slice(&pkt.to_bytes());
        Message::Binary(f)
    }

    #[test]
    fn peer_binding_roster_and_cleanup() {
        let inner = mesh_inner("Me");
        let stop = AtomicBool::new(false);
        let (tx, rx) = mpsc::sync_channel(8);
        assert!(on_open(&inner, &stop, "chanA", tx));
        // First frame queued for the peer is our identity.
        match rx.try_recv() {
            Ok(Message::Text(t)) => assert!(matches!(
                serde_json::from_str::<Control>(&t).unwrap(),
                Control::Peer { ref name, .. } if name == "Me"
            )),
            _ => panic!("expected our Peer frame first"),
        }

        // Telemetry before the channel announced who it is: dropped.
        let victim = Uuid::new_v4();
        on_message(&inner, &stop, "chanA", telemetry_frame(*victim.as_bytes(), 1.0));
        assert!(lock(&inner).remote.is_empty());

        let a = Uuid::new_v4().to_string();
        on_message(&inner, &stop, "chanA", peer_frame(&a, "Alice"));
        assert_eq!(lock(&inner).roster.len(), 2, "self + Alice");

        // The id prefix inside a binary frame is ignored (anti-spoof): it lands on Alice.
        on_message(&inner, &stop, "chanA", telemetry_frame(*victim.as_bytes(), 33.0));
        {
            let g = lock(&inner);
            assert!(!g.remote.contains_key(&victim.to_string()));
            assert_eq!(g.remote[&a].q.len(), 1);
        }
        // Nobody can claim our own id or a malformed one.
        let my_id = lock(&inner).my_id.clone();
        on_message(&inner, &stop, "chanB", peer_frame(&my_id, "Mallory"));
        on_message(&inner, &stop, "chanB", peer_frame("not-a-uuid", "Mallory"));
        assert_eq!(lock(&inner).roster.len(), 2);
        assert!(!lock(&inner).mesh_bound.contains_key("chanB"));

        // Waypoints are keyed by the channel's bound id, whatever the message says.
        let wp = |pos| {
            Message::Text(
                serde_json::to_string(&Control::Waypoint { id: "forged".into(), pos, hue: 90.0 }).unwrap(),
            )
        };
        on_message(&inner, &stop, "chanA", wp(Some([1.0, 2.0])));
        assert_eq!(lock(&inner).waypoints.get(&a), Some(&(1.0, 2.0, 90.0)));
        assert!(!lock(&inner).waypoints.contains_key("forged"));

        // Identity change re-sends Peer: name updates in place.
        on_message(&inner, &stop, "chanA", peer_frame(&a, "Alice2"));
        assert_eq!(lock(&inner).roster.iter().find(|p| p.id == a).unwrap().name, "Alice2");

        // Latest channel wins: Alice rejoined on chanB before chanA's link was noticed dead.
        // chanA is evicted (binding + outgoing queue); chanB takes over her roster entry.
        let (txb, _rxb) = mpsc::sync_channel(8);
        assert!(on_open(&inner, &stop, "chanB", txb));
        on_message(&inner, &stop, "chanB", peer_frame(&a, "Alice3"));
        {
            let g = lock(&inner);
            assert_eq!(g.roster.len(), 2, "still self + one Alice");
            assert_eq!(g.roster.iter().find(|p| p.id == a).unwrap().name, "Alice3");
            assert_eq!(g.mesh_bound.get("chanB"), Some(&a));
            assert!(!g.mesh_bound.contains_key("chanA"));
            assert_eq!(g.clients.len(), 1, "chanA's queue dropped");
            assert!(!g.waypoints.contains_key(&a), "stale ping cleared");
        }
        // Late frames / cleanup from the evicted channel change nothing.
        on_message(&inner, &stop, "chanA", telemetry_frame(*victim.as_bytes(), 5.0));
        on_gone(&inner, "chanA");
        assert_eq!(lock(&inner).roster.len(), 2);
        // The new channel now speaks for her.
        on_message(&inner, &stop, "chanB", telemetry_frame(*victim.as_bytes(), 44.0));
        assert_eq!(lock(&inner).remote[&a].q.len(), 2);

        // Channel closes: everything of Alice's goes, and her queue is deregistered.
        on_gone(&inner, "chanB");
        let g = lock(&inner);
        assert_eq!(g.roster.len(), 1);
        assert!(g.remote.is_empty() && g.waypoints.is_empty() && g.clients.is_empty());
        assert!(g.mesh_bound.is_empty());
    }

    #[test]
    fn nat_error_only_when_nobody_is_connected() {
        const NO_ICE: &[&str] = &[];
        let inner = mesh_inner("Me");
        let (sess, _rx) = Session::build(inner.clone(), Arc::new(AtomicBool::new(false)), "r", NO_ICE, "127.0.0.1:0");
        let slot = |phase, gen| Slot { phase, gen };
        let add = |k: &str, phase, gen| {
            lock(&sess.st).slots.insert(k.into(), slot(phase, gen));
        };
        // A stale link's failure (already replaced by a newer generation) is ignored.
        add("p1", Phase::Answering("o".into()), 5);
        sess.link_ended("p1", 4, true);
        assert!(!lock(&sess.st).nat_failed);
        assert_eq!(lock(&inner).error, None);
        // A real failure with nobody connected shows the NAT hint...
        sess.link_ended("p1", 5, true);
        assert_eq!(lock(&inner).error.as_deref(), Some(NAT_ERROR));
        // ...but not while another player's link is live.
        add("p2", Phase::Live, 6);
        sess.refresh();
        assert_eq!(lock(&inner).error, None);
        // Opening a link clears the flag for good.
        lock(&sess.st).slots.clear();
        add("p3", Phase::Offering("o".into()), 7);
        let (tx, _r) = mpsc::sync_channel(4);
        assert!(sess.link_open("p3", 7, tx));
        assert!(!lock(&sess.st).nat_failed);
    }

    #[test]
    fn stopped_session_ignores_late_traffic() {
        let inner = mesh_inner("Me");
        let stop = AtomicBool::new(true);
        let (tx, _rx) = mpsc::sync_channel(8);
        assert!(!on_open(&inner, &stop, "c", tx));
        on_message(&inner, &stop, "c", peer_frame(&Uuid::new_v4().to_string(), "X"));
        assert_eq!(lock(&inner).roster.len(), 1);
    }

    /// Full data path over two real WebRTC connections in this process, SDP passed by hand
    /// (no relays, no STUN: host candidates on loopback).
    #[test]
    fn loopback_data_channel_path() {
        let a = mesh_inner("A");
        let b = mesh_inner("B");
        let stop = AtomicBool::new(false);
        let opened = Cell::new(false);
        smol::block_on(async {
            let la = Link::new(&[], "127.0.0.1:0").await.unwrap();
            let lb = Link::new(&[], "127.0.0.1:0").await.unwrap();
            let (dc_a, offer) = la.offer().await.unwrap();
            let answer = lb.answer(offer).await.unwrap();
            la.accept(answer).await.unwrap();
            let dc_b = timeout(Duration::from_secs(5), lb.dc_in.recv()).await.unwrap().unwrap();

            let run_a = run_channel(
                dc_a,
                Duration::from_secs(10),
                |tx| on_open(&a, &stop, "B", tx),
                |m| on_message(&a, &stop, "B", m),
            );
            let run_b = run_channel(
                dc_b,
                Duration::from_secs(10),
                |tx| on_open(&b, &stop, "A", tx),
                |m| on_message(&b, &stop, "A", m),
            );
            let check = async {
                let deadline = Instant::now() + Duration::from_secs(10);
                let a_id = lock(&a).my_id.clone();
                let b_id = lock(&b).my_id.clone();
                // Peer frames cross: each roster learns the other player.
                loop {
                    let (ra, rb) = (lock(&a).roster.clone(), lock(&b).roster.clone());
                    if ra.iter().any(|p| p.id == b_id && p.name == "B")
                        && rb.iter().any(|p| p.id == a_id && p.name == "A")
                    {
                        break;
                    }
                    assert!(Instant::now() < deadline, "roster never converged");
                    smol::Timer::after(Duration::from_millis(20)).await;
                }
                // Telemetry from A shows up in B's remote_players via the jitter buffer.
                loop {
                    push_local(&a, &ForzaPacket { speed: 42.0, ..Default::default() });
                    tick(&b);
                    if let Some((info, pkt)) = remote_players(&b).first() {
                        assert_eq!(info.id, a_id);
                        assert_eq!(pkt.speed, 42.0);
                        break;
                    }
                    assert!(Instant::now() < deadline, "telemetry never arrived");
                    smol::Timer::after(Duration::from_millis(20)).await;
                }
                opened.set(true);
            };
            smol::future::or(
                async {
                    smol::future::zip(run_a, run_b).await;
                },
                check,
            )
            .await;
            let _ = (la.pc.close().await, lb.pc.close().await);
        });
        assert!(opened.get());
    }

    /// Two whole sessions (Nostr signalling + WebRTC actor) joined through a fake relay that
    /// just forwards frames between them: exercises announce → simultaneous offers → glare →
    /// answer → connect → Peer exchange → telemetry, and shutdown.
    #[test]
    fn two_sessions_meet_through_fake_relay() {
        const NO_ICE: &[&str] = &[];
        let room = "test-room-e2e";
        let stop = Arc::new(AtomicBool::new(false));
        let (ia, ib) = (mesh_inner("A"), mesh_inner("B"));
        let (sa, rxa) = Session::build(ia.clone(), stop.clone(), room, NO_ICE, "127.0.0.1:0");
        let (sb, rxb) = Session::build(ib.clone(), stop.clone(), room, NO_ICE, "127.0.0.1:0");
        let (rtc_a, rtc_b) = (rxa.rtc.clone(), rxb.rtc.clone());
        let ta = { let s = sa.clone(); std::thread::spawn(move || crate::coop::rtc::run(s, rtc_a)) };
        let tb = { let s = sb.clone(); std::thread::spawn(move || crate::coop::rtc::run(s, rtc_b)) };

        // The fake relay: everything a session publishes is delivered to the other one.
        let deliver = |from: &Rx, to: &Session| {
            for f in from.relays[0].try_iter() {
                let v: Value = serde_json::from_str(&f).unwrap();
                let ev = &v[1];
                to.handle_event(ev["tags"][0][1].as_str().unwrap(), ev["content"].as_str().unwrap());
            }
            for q in &from.relays[1..] {
                while q.try_recv().is_ok() {}
            }
        };
        sa.broadcast(sa.announce_frame());
        sb.broadcast(sb.announce_frame());

        let (a_id, b_id) = (lock(&ia).my_id.clone(), lock(&ib).my_id.clone());
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut got = false;
        while Instant::now() < deadline {
            deliver(&rxa, &sb);
            deliver(&rxb, &sa);
            push_local(&ia, &ForzaPacket { speed: 7.0, ..Default::default() });
            tick(&ib);
            if remote_players(&ib).iter().any(|(p, k)| p.id == a_id && k.speed == 7.0)
                && lock(&ia).roster.iter().any(|p| p.id == b_id)
            {
                got = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(got, "peers never connected; A status={:?}", lock(&ia).status);
        // Glare left exactly one live link each way, and the status says so.
        assert_eq!(lock(&ia).clients.len(), 1);
        assert_eq!(lock(&ib).clients.len(), 1);
        assert_eq!(lock(&ia).status, "2 player(s)");
        assert_eq!(lock(&ib).status, "2 player(s)");

        // Stopping tears everything down: the rtc threads end (closing their peer connections).
        stop.store(true, Ordering::Relaxed);
        ta.join().unwrap();
        tb.join().unwrap();
    }
}
