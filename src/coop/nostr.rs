//! Trystero-style signalling over public Nostr relays: the pure protocol pieces (topics, event
//! signing, room-key encryption, subscription frames) and the per-relay socket loop. The room
//! logic that reacts to events lives in `mesh.rs`.
//!
//! Why Nostr: relays are free, public and already speak WebSocket, so peers find each other by
//! a shared Room ID with no server of ours. Only the (encrypted) SDP handshake travels over
//! them; telemetry then flows peer-to-peer over WebRTC data channels.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use k256::schnorr::signature::hazmat::PrehashSigner;
use k256::schnorr::SigningKey;
use serde_json::{json, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tungstenite::Message;

use super::mesh::Session;
use super::{connect_ws, set_client_timeout};

/// Namespaces the topics so we never collide with other Trystero apps on the same relays.
pub const APP_ID: &str = "ForzaTelemetryV3";

/// Fixed relay list: both peers must subscribe to a common relay, so it is not user-tunable.
/// Picked (2026-09) for speed and for not demanding auth/PoW. Redundancy matters more than any
/// single relay: a room works while at least one shared relay is reachable.
pub const RELAYS: &[&str] = &[
    "wss://nos.lol",
    "wss://nostr.mad-social.net",
    "wss://nostr.purpura.cloud",
    "wss://nostr.stakey.net",
    "wss://relay.mappingbitcoin.com",
    "wss://offchain.pub",
];

/// Announce cadence after (re)start: fast at first so a newcomer is found within a second, then
/// a slow heartbeat as a fallback for missed discovery.
pub const ANNOUNCE_SCHEDULE_MS: [u64; 4] = [200, 500, 1300, 5300];
pub const ANNOUNCE_STEADY: Duration = Duration::from_secs(30);

const B36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// SHA-1 of `s`, each digest byte rendered as JS `byte.toString(36)` (lowercase, unpadded) and
/// concatenated. This is Trystero's topic hash; kept byte-compatible on purpose.
pub fn sha1_b36(s: &str) -> String {
    let mut out = String::new();
    for &b in Sha1::digest(s.as_bytes()).iter() {
        if b >= 36 {
            out.push(B36[(b / 36) as usize] as char);
        }
        out.push(B36[(b % 36) as usize] as char);
    }
    out
}

/// Event kind for a topic: `(sum of UTF-16 code units) % 10000 + 20000` (the ephemeral range,
/// so relays don't store it).
pub fn topic_kind(topic: &str) -> u32 {
    topic.encode_utf16().map(u32::from).sum::<u32>() % 10_000 + 20_000
}

pub fn root_topic_plain(room: &str) -> String {
    format!("Trystero@{APP_ID}@{room}")
}

pub fn root_topic(room: &str) -> String {
    sha1_b36(&root_topic_plain(room))
}

pub fn peer_topic(room: &str, peer_id: &str) -> String {
    sha1_b36(&format!("{}@{peer_id}", root_topic_plain(room)))
}

pub fn fill_random(buf: &mut [u8]) {
    // getrandom only fails on exotic platforms without an entropy source.
    getrandom::fill(buf).expect("OS random source");
}

/// `n` random chars from `[0-9A-Za-z]` (Trystero's `genId`).
pub fn rand_id(n: usize) -> String {
    const SET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut out = String::with_capacity(n);
    let mut buf = [0u8; 1];
    while out.len() < n {
        fill_random(&mut buf);
        if buf[0] < 248 {
            // 248 = 4 * 62: rejection keeps the distribution uniform.
            out.push(SET[(buf[0] % 62) as usize] as char);
        }
    }
    out
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Per-session Nostr identity. A throwaway key: it only has to make events valid, not be "us".
pub struct Keys {
    key: SigningKey,
    pub pubkey: String,
}

impl Keys {
    pub fn generate() -> Self {
        loop {
            let mut sk = [0u8; 32];
            fill_random(&mut sk);
            if let Ok(key) = SigningKey::from_bytes((&sk).into()) {
                let pubkey = hex(&key.verifying_key().to_bytes());
                return Self { key, pubkey };
            }
        }
    }
}

/// NIP-01 event: `id = sha256([0,pubkey,created_at,kind,tags,content])`, BIP-340 Schnorr `sig`.
pub fn make_event(keys: &Keys, topic: &str, content: &str, created_at: u64) -> Value {
    let kind = topic_kind(topic);
    let tags = json!([["x", topic]]);
    let ser = json!([0, keys.pubkey, created_at, kind, tags, content]).to_string();
    let id = Sha256::digest(ser.as_bytes());
    let sig: k256::schnorr::Signature = keys.key.sign_prehash(&id).expect("schnorr sign");
    json!({
        "id": hex(&id),
        "pubkey": keys.pubkey,
        "created_at": created_at,
        "kind": kind,
        "tags": tags,
        "content": content,
        "sig": hex(&sig.to_bytes()),
    })
}

/// The `["EVENT", ev]` wire frame publishing `content` to `topic`.
pub fn event_frame(keys: &Keys, topic: &str, content: &str) -> String {
    json!(["EVENT", make_event(keys, topic, content, now_secs())]).to_string()
}

/// `["REQ", sub, {kinds, since, "#x"}]` for the given topics.
pub fn req_frame(sub_id: &str, topics: &[&str], since: u64) -> String {
    let mut kinds: Vec<u32> = topics.iter().map(|t| topic_kind(t)).collect();
    kinds.sort_unstable();
    kinds.dedup();
    json!(["REQ", sub_id, {"kinds": kinds, "since": since, "#x": topics}]).to_string()
}

// ── room-key encryption ────────────────────────────────────────────

/// SHA-256(":ForzaTelemetryV3:<room>"): the Room ID is effectively the shared secret.
pub fn room_key(room: &str) -> [u8; 32] {
    Sha256::digest(format!(":{APP_ID}:{room}").as_bytes()).into()
}

/// AES-256-GCM, random 12-byte IV. Wire: `iv bytes joined by ","` + `$` + `base64(ciphertext)`.
/// (Trystero's shape, but with a 12-byte IV: we never interoperate with JS clients.)
pub fn encrypt(key: &[u8; 32], plain: &str) -> String {
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let mut iv = [0u8; 12];
    fill_random(&mut iv);
    let ct = cipher.encrypt(&Nonce::from(iv), plain.as_bytes()).expect("aes-gcm encrypt");
    let iv_s: Vec<String> = iv.iter().map(|b| b.to_string()).collect();
    format!("{}${}", iv_s.join(","), B64.encode(ct))
}

pub fn decrypt(key: &[u8; 32], wire: &str) -> Option<String> {
    let (iv_s, ct_s) = wire.split_once('$')?;
    let iv: Vec<u8> = iv_s.split(',').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let iv: [u8; 12] = iv.try_into().ok()?;
    let ct = B64.decode(ct_s).ok()?;
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let pt = cipher.decrypt(&Nonce::from(iv), ct.as_slice()).ok()?;
    String::from_utf8(pt).ok()
}

// ── glare ──────────────────────────────────────────────────────────

/// What to do with a peer's offer that arrives while we may have one of our own in flight.
#[derive(Debug, PartialEq, Eq)]
pub enum OfferAction {
    /// Drop any offer of ours and answer theirs.
    Answer,
    /// Ours wins: ignore theirs (they will answer ours).
    IgnoreTheirs,
}

/// Both sides see each other's announce and offer at once ("glare"). Deterministic tie-break so
/// exactly one offer survives: the lexicographically smaller peer id keeps its own offer.
pub fn glare(has_outgoing: bool, self_id: &str, peer_id: &str) -> OfferAction {
    if has_outgoing && self_id < peer_id {
        OfferAction::IgnoreTheirs
    } else {
        OfferAction::Answer
    }
}

// ── relay socket loop ──────────────────────────────────────────────

/// What a relay said that we care about.
#[derive(Debug, PartialEq, Eq)]
pub enum RelayMsg {
    /// `(x-tag topic, content)`
    Event(String, String),
    RateLimited,
    /// Terminal rejection: this relay will never take us (auth, PoW, blocked…).
    Retire,
    /// Subscription closed by the relay for a non-terminal reason: re-REQ.
    Resubscribe,
    Other,
}

pub fn parse_relay_msg(text: &str) -> RelayMsg {
    let Ok(Value::Array(a)) = serde_json::from_str::<Value>(text) else {
        return RelayMsg::Other;
    };
    let kind = a.first().and_then(Value::as_str).unwrap_or("");
    let reason = match kind {
        "OK" if a.get(2) == Some(&Value::Bool(false)) => a.get(3).and_then(Value::as_str),
        "CLOSED" => a.get(2).and_then(Value::as_str),
        "EVENT" => {
            let ev = a.get(2);
            let content = ev.and_then(|e| e["content"].as_str());
            let topic = ev
                .and_then(|e| e["tags"].as_array())
                .and_then(|t| t.iter().find(|t| t[0] == "x"))
                .and_then(|t| t[1].as_str());
            return match (topic, content) {
                (Some(t), Some(c)) => RelayMsg::Event(t.to_string(), c.to_string()),
                _ => RelayMsg::Other,
            };
        }
        "NOTICE" => a.get(1).and_then(Value::as_str).filter(|m| m.contains("rate-limit")),
        _ => None,
    };
    let Some(r) = reason else { return RelayMsg::Other };
    if r.starts_with("rate-limited:") || kind == "NOTICE" {
        RelayMsg::RateLimited
    } else if ["blocked:", "restricted:", "auth-required:", "pow:"].iter().any(|p| r.starts_with(p)) {
        RelayMsg::Retire
    } else if kind == "CLOSED" {
        RelayMsg::Resubscribe
    } else {
        RelayMsg::Other // "duplicate:" etc. — expected, we publish on several relays
    }
}

/// Sleep `d` in short slices so a stop request is honoured promptly. True = stopped.
fn sleep_or_stop(sess: &Session, d: Duration) -> bool {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if sess.stopped() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50).min(end - Instant::now()));
    }
    sess.stopped()
}

/// One relay's lifetime: connect, subscribe, announce, pump frames both ways; on any drop
/// reconnect with backoff (and re-REQ + re-announce). Returns when the session stops or the
/// relay is retired.
pub fn relay_loop(sess: Arc<Session>, idx: usize, url: &'static str, out_rx: Receiver<String>) {
    let mut backoff = Duration::from_secs(1);
    let sub_id = rand_id(64);
    loop {
        if sess.stopped() {
            return;
        }
        let mut ws = match connect_ws(url) {
            Ok(w) => w,
            Err(_) => {
                sess.relay_down(idx, true, false);
                if sleep_or_stop(&sess, backoff) {
                    return;
                }
                backoff = (backoff * 2).min(Duration::from_secs(60));
                continue;
            }
        };
        set_client_timeout(&mut ws, Some(Duration::from_millis(20)));
        while out_rx.try_recv().is_ok() {} // stale frames from before the reconnect
        let ok = ws.write(Message::Text(sess.req_frame(&sub_id))).is_ok()
            && ws.write(Message::Text(sess.announce_frame())).is_ok()
            && ws.flush().is_ok();
        if !ok {
            sess.relay_down(idx, true, false);
            if sleep_or_stop(&sess, backoff) {
                return;
            }
            backoff = (backoff * 2).min(Duration::from_secs(60));
            continue;
        }
        sess.relay_up(idx);
        backoff = Duration::from_secs(1);

        let mut hold_until: Option<Instant> = None;
        let mut retire = false;
        'session: loop {
            if sess.stopped() {
                let _ = ws.close(None);
                return;
            }
            if hold_until.map_or(true, |t| Instant::now() >= t) {
                hold_until = None;
                let mut wrote = false;
                while let Ok(f) = out_rx.try_recv() {
                    if ws.write(Message::Text(f)).is_err() {
                        break 'session;
                    }
                    wrote = true;
                }
                if wrote && ws.flush().is_err() {
                    break 'session;
                }
            }
            match ws.read() {
                Ok(Message::Text(t)) => match parse_relay_msg(&t) {
                    RelayMsg::Event(topic, content) => sess.handle_event(&topic, &content),
                    RelayMsg::RateLimited => {
                        hold_until = Some(Instant::now() + Duration::from_secs(10))
                    }
                    RelayMsg::Retire => {
                        retire = true;
                        break 'session;
                    }
                    RelayMsg::Resubscribe => {
                        if ws.send(Message::Text(sess.req_frame(&sub_id))).is_err() {
                            break 'session;
                        }
                    }
                    RelayMsg::Other => {}
                },
                Ok(Message::Close(_)) => break 'session,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break 'session,
            }
        }
        let _ = ws.close(None);
        if retire {
            sess.relay_down(idx, true, true);
            return;
        }
        sess.relay_down(idx, false, false);
        if sleep_or_stop(&sess, backoff) {
            return;
        }
    }
}

/// Announce thread: fast cadence at start (and after a kick), then a slow heartbeat.
pub fn announce_loop(sess: Arc<Session>, kick: Receiver<()>) {
    let mut step = 0usize;
    loop {
        let wait = ANNOUNCE_SCHEDULE_MS
            .get(step)
            .map_or(ANNOUNCE_STEADY, |&ms| Duration::from_millis(ms));
        let end = Instant::now() + wait;
        let mut kicked = false;
        while Instant::now() < end {
            if sess.stopped() {
                return;
            }
            match kick.recv_timeout(Duration::from_millis(100).min(end - Instant::now())) {
                Ok(()) => {
                    kicked = true;
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if kicked {
            while kick.try_recv().is_ok() {}
            step = 0; // re-announce soon, on the fast cadence
            continue;
        }
        sess.broadcast(sess.announce_frame());
        step += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k256::schnorr::signature::hazmat::PrehashVerifier;
    use k256::schnorr::{Signature, VerifyingKey};

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn sha1_b36_vectors() {
        // Computed independently: ''.join(base36(b) for b in hashlib.sha1(s).digest()), bytes
        // rendered unpadded like JS `b.toString(36)`.
        assert_eq!(sha1_b36(""), "621l4j6m2m2z23d1e2d5b6n452oo404v6079");
        assert_eq!(sha1_b36("abc"), "4p491q1i1z63l2y561q11353c285e304c5s604d");
        assert_eq!(root_topic("k7f2-9qzm-x4pd"), "g5y532e701h4cm5m466j40s6x4q5w2b3b3t6u");
        assert_eq!(
            peer_topic("k7f2-9qzm-x4pd", "AbCdEfGhIjKlMnOpQrSt"),
            "26f1v345e2n141d6t1j6hi504h3n1i376q2l5y"
        );
    }

    #[test]
    fn kind_vectors() {
        assert_eq!(topic_kind("g5y532e701h4cm5m466j40s6x4q5w2b3b3t6u"), 22837);
        assert_eq!(topic_kind("26f1v345e2n141d6t1j6hi504h3n1i376q2l5y"), 22805);
        // Always in the ephemeral range.
        assert!((20_000..30_000).contains(&topic_kind("anything at all")));
        assert_eq!(topic_kind(""), 20_000);
    }

    #[test]
    fn event_id_and_signature_verify() {
        let keys = Keys::generate();
        let topic = root_topic("room");
        let ev = make_event(&keys, &topic, "{\"peerId\":\"x\"}", 1_700_000_000);
        assert_eq!(ev["kind"], topic_kind(&topic));
        assert_eq!(ev["tags"], json!([["x", topic]]));

        // id is the sha256 of the NIP-01 serialisation…
        let ser = json!([0, keys.pubkey, 1_700_000_000u64, topic_kind(&topic), [["x", topic]], "{\"peerId\":\"x\"}"])
            .to_string();
        let id = Sha256::digest(ser.as_bytes());
        assert_eq!(ev["id"], hex(&id));

        // …and the BIP-340 signature verifies against the x-only pubkey.
        let pk: [u8; 32] = unhex(ev["pubkey"].as_str().unwrap()).try_into().unwrap();
        let vk = VerifyingKey::from_bytes((&pk).into()).unwrap();
        let sig = Signature::try_from(unhex(ev["sig"].as_str().unwrap()).as_slice()).unwrap();
        vk.verify_prehash(&id, &sig).expect("signature verifies");
        // A different id must not verify.
        assert!(vk.verify_prehash(&Sha256::digest(b"other"), &sig).is_err());
    }

    #[test]
    fn frames_are_well_formed() {
        let keys = Keys::generate();
        let f: Value = serde_json::from_str(&event_frame(&keys, "t", "c")).unwrap();
        assert_eq!(f[0], "EVENT");
        assert_eq!(f[1]["content"], "c");

        let root = root_topic("r");
        let me = peer_topic("r", "me");
        let req: Value = serde_json::from_str(&req_frame("sub", &[&root, &me], 42)).unwrap();
        assert_eq!(req[0], "REQ");
        assert_eq!(req[1], "sub");
        assert_eq!(req[2]["since"], 42);
        assert_eq!(req[2]["#x"], json!([root, me]));
        assert!(req[2]["kinds"].as_array().unwrap().contains(&json!(topic_kind(&root))));
    }

    #[test]
    fn aes_gcm_round_trip() {
        let key = room_key("k7f2-9qzm-x4pd");
        let sdp = "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\n…ünïcode";
        let wire = encrypt(&key, sdp);
        let (iv, ct) = wire.split_once('$').expect("iv$ciphertext");
        assert_eq!(iv.split(',').count(), 12);
        assert!(!ct.is_empty());
        assert_eq!(decrypt(&key, &wire).as_deref(), Some(sdp));
        // Fresh IV each time.
        assert_ne!(encrypt(&key, sdp), wire);
        // Wrong room → authentication fails; garbage → None, no panic.
        assert_eq!(decrypt(&room_key("other-room"), &wire), None);
        assert_eq!(decrypt(&key, "nonsense"), None);
        assert_eq!(decrypt(&key, "1,2,3$AAAA"), None);
        // Room key derivation (sha256 of ":ForzaTelemetryV3:<room>").
        assert_eq!(
            hex(&room_key("k7f2-9qzm-x4pd")),
            "029813428beaa3327674c51253d800217950325ed8fe0e31a6b3e3dce45cf95c"
        );
    }

    #[test]
    fn glare_rule() {
        // Nothing outgoing: always answer.
        assert_eq!(glare(false, "aaa", "bbb"), OfferAction::Answer);
        assert_eq!(glare(false, "bbb", "aaa"), OfferAction::Answer);
        // Both sent offers: the smaller id keeps its own, the larger answers.
        assert_eq!(glare(true, "aaa", "bbb"), OfferAction::IgnoreTheirs);
        assert_eq!(glare(true, "bbb", "aaa"), OfferAction::Answer);
    }

    #[test]
    fn relay_messages_classified() {
        let ev = r#"["EVENT","sub",{"content":"hi","tags":[["x","topic"]],"kind":20001}]"#;
        assert_eq!(parse_relay_msg(ev), RelayMsg::Event("topic".into(), "hi".into()));
        assert_eq!(parse_relay_msg(r#"["OK","id",true,""]"#), RelayMsg::Other);
        assert_eq!(parse_relay_msg(r#"["OK","id",false,"duplicate: have it"]"#), RelayMsg::Other);
        assert_eq!(parse_relay_msg(r#"["OK","id",false,"rate-limited: slow"]"#), RelayMsg::RateLimited);
        assert_eq!(parse_relay_msg(r#"["OK","id",false,"blocked: no"]"#), RelayMsg::Retire);
        assert_eq!(parse_relay_msg(r#"["OK","id",false,"pow: need 20"]"#), RelayMsg::Retire);
        assert_eq!(parse_relay_msg(r#"["CLOSED","sub","auth-required: login"]"#), RelayMsg::Retire);
        assert_eq!(parse_relay_msg(r#"["CLOSED","sub","error: shutting down"]"#), RelayMsg::Resubscribe);
        assert_eq!(parse_relay_msg(r#"["CLOSED","sub","rate-limited: x"]"#), RelayMsg::RateLimited);
        assert_eq!(parse_relay_msg(r#"["EOSE","sub"]"#), RelayMsg::Other);
        assert_eq!(parse_relay_msg(r#"["NOTICE","you are rate-limited"]"#), RelayMsg::RateLimited);
        assert_eq!(parse_relay_msg("not json"), RelayMsg::Other);
    }

    #[test]
    fn rand_id_shape() {
        let id = rand_id(20);
        assert_eq!(id.len(), 20);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(id, rand_id(20));
    }
}
