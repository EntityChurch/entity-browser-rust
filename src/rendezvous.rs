//! **Meeting at a name** — the `lobby` / `tag` / `secret` rendezvous modes at
//! the app tier.
//!
//! Until now this app could express exactly one thought about a remote peer:
//! *"I already know your 44-character peer id — meet me."* That is `pair` mode,
//! derived inside both browser establishers from the two peer-ids
//! (`main_thread_establish.rs`, `core/peer/src/worker_webrtc.rs`), and it is the
//! only one of the protocol's four rendezvous modes anything here reached.
//! `tag` / `secret` / `lobby` have been implemented upstream — and unreachable
//! from any surface — since the signaling extension landed
//! (`REVIEW-CONNECTIVITY-LAYER-COHERENCE-2026-08-11` §5.5, item 4). This module
//! is where they become reachable.
//!
//! ## What a meet is, and what it is not
//!
//! A meet is **discovery, not a second establishment path.** We post one
//! `connect-request` into a bucket at the selected connector, read what else is
//! in that bucket, and come away with **peer ids**. Connecting to one of those
//! peers afterwards is the connect this app already has: the ordinary dispatch
//! ladder, whose rung 4 runs the proven `pair`-mode WebRTC establishment
//! (`make e2e-webrtc-chat`). Rendezvous introduces; it never sits in the data
//! path, and it never authorizes — the resulting connection still runs the
//! ordinary handshake and capability flow.
//!
//! Two consequences worth stating plainly, because both are easy to assume away:
//!
//! - **A discovered peer-id is a claim until we connect.** A bare (unsealed)
//!   `connect-request` names its own author on the wire, so anyone can write any
//!   id there. We prefer the **verified signer** when the message arrived in a
//!   §6.3 container and mark the row accordingly ([`Discovered::verified`]) —
//!   but an unverified id is not a lie we can detect here. It is checked where
//!   it matters: at the handshake. So a meet **remembers nothing on its own**;
//!   it reports, and the user decides who to keep.
//! - **A `tag` is public by design and a weak `secret` is a `tag` in disguise.**
//!   `tag` is a discovery convenience, explicitly not access control; `secret`
//!   is only as strong as its entropy, and a memorable phrase is enumerable
//!   (`EXTENSION-SIGNALING` §2.2). Both surfaces say so where the user types one.
//!
//! ## Lobby mode must ask the node
//!
//! [`Mode::Lobby`] carries no user input: its input is *the pool's* lobby
//! constant, and a node may **override** it. A peer that derives from
//! `LOBBY_DEFAULT` at a node that overrode it lands in a bucket nobody else on
//! that pool uses, and **nothing errors** — the two peers simply never meet
//! (§2.2 Finding B). So [`resolve_key`] resolves a lobby key *through*
//! [`crate::connectors::advertise`], and there is no way to ask this module for
//! a lobby key without that round trip. That is deliberate: the alternative is
//! a one-line convenience whose failure mode is invisible.
//!
//! ## Why the session is frame-pumped rather than an async loop
//!
//! A meet is several round trips over minutes, and every one of them needs
//! `&Peers` to build — which a spawned task cannot hold across an `.await`
//! (`WriterHandle`'s module doc is the same problem for writes). So
//! [`MeetSession`] is a state machine pumped from the frame loop, exactly like
//! `ChatDelivery::pump` and `EntityApp::sync_maintained_peers`: each pump builds
//! at most one `'static` round trip from the `&Peers` it was handed, spawns it,
//! and lands the result in shared state. No timer primitive is involved, so the
//! same code drives the browser (rAF), the Worker arm, and a native test loop.
//!
//! ## Two things the poll loop must keep doing
//!
//! Both are upstream MUSTs that a naive implementation gets wrong (§3.2, and
//! `cmd/signaling-meet`'s module doc):
//!
//! - **Do not stop at the first answer.** A bucket is a *set with history*, not
//!   a queue: `tag` and `lobby` keys are stable by construction, so a bucket
//!   still holds earlier exchanges until the node's TTL reaps them. A session
//!   that returned at its first hit would report a stale peer and abandon the
//!   one actually waiting. We keep listening for the whole window and report
//!   everyone.
//! - **Skip nonces already answered**, or every poll re-answers the same request
//!   and fills the bucket against its `max_bucket_blobs` bound.
//!
//! ## Pacing: two people press a button at roughly the same time
//!
//! A meet is a **human-time** activity, and the shape of the wait is the whole
//! design. Two people who agreed on a name out of band press *Meet* seconds
//! apart, or one presses and then walks to the other machine — so the useful
//! window is minutes and the useful *cadence* is fast at the start and slow
//! afterwards. [`poll_interval_ms`] is that schedule; [`SEARCH_WINDOW`] is the
//! window.
//!
//! Three things it is worth knowing before changing either:
//!
//! - **Pacing out costs nothing in the common case and everything in the tail.**
//!   Whoever presses second finds the first one on their *first* poll, because
//!   the first one's `connect-request` is already sitting in the bucket. What
//!   the interval bounds is the *other* direction — how long the person who
//!   pressed first waits to notice — so the ceiling is a bound on the worst
//!   case for the person who is already waiting, which is why it stays in
//!   single-digit seconds rather than going to the TTL.
//! - **The interval may never approach the node's TTL.** A bucket entry expires;
//!   an interval longer than the TTL can straddle a deposit-and-expiry and miss
//!   a peer *with nothing erroring anywhere* — the same silent never-meet the
//!   lobby constant produces one section up. The TTL is **advertised**
//!   (`Limits::ttl_seconds`, §4.5), so [`poll_interval_ms`] takes it rather than
//!   assuming the 60 s default, and [`resolve_key`] hands back what the node
//!   said whenever it had to ask (lobby mode). For `tag`/`secret` we do not
//!   make the extra round trip and the protocol default stands in — stated
//!   rather than hidden, and narrowing-only: a node advertising a *longer* TTL
//!   never widens our interval, because the human's wait bounds it too.
//! - **The window is wall clock, not a poll count.** A frame-counted window
//!   means something different on a 60 fps tab, a backgrounded one (where rAF
//!   stops entirely) and a native test loop, and what this bound is about is
//!   *seconds a person waits*. Same argument as [`SETUP_BUDGET`], which was
//!   wall clock first.
//!
//! Answering is deliberately **not** paced: a queued answer is a peer already
//! waiting on us, so [`MeetSession::pump`] sends it on the next frame whatever
//! the poll schedule says.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use entity_entity::Entity;
use entity_signaling::coordination::{
    self, CollectedCoordination, CollectedMessage, ConnectRequest, ConnectResponse, Nonce,
};
use entity_signaling::data::CollectResult;
use entity_signaling::{key, CollectRequest, OfferRequest, RendezvousKey};

use crate::connectors::{self, Connector};
use crate::peers::Peers;

/// Owned future over a node round trip. `Send` on native (so `tokio::spawn`
/// takes it) and not on wasm (the Worker arm's proxy is `Rc`-backed) — the same
/// split every future alias in `peers.rs` uses.
#[cfg(not(target_arch = "wasm32"))]
pub type NodeFuture<T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>> + Send>>;
#[cfg(target_arch = "wasm32")]
pub type NodeFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, String>>>>;

#[cfg(not(target_arch = "wasm32"))]
fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        drop(rt.spawn(f));
    }
}

#[cfg(target_arch = "wasm32")]
fn spawn<F: std::future::Future<Output = ()> + 'static>(f: F) {
    wasm_bindgen_futures::spawn_local(f);
}

// ---------------------------------------------------------------------------
// The modes
// ---------------------------------------------------------------------------

/// A rendezvous mode with its input — the *name* two peers agree on out of band.
///
/// `pair` is deliberately absent: it needs no naming surface, because its input
/// is the two peer-ids and the establishers already derive it themselves. This
/// enum is the set of modes a **user** can express.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// "Just connect me to anyone here, right now." Input is the pool's lobby
    /// constant, which comes from the node — never from us.
    Lobby,
    /// "Anyone who knows the label meets." Public discovery, byte-exact UTF-8,
    /// **not** access control.
    Tag(String),
    /// "Anyone who knows the string meets", so knowing it is a lightweight
    /// admission gate — and only as strong as its entropy.
    Secret(String),
}

impl Mode {
    /// The mode names a surface accepts, in the order they should be offered.
    /// Spelled from the upstream constants so a rename there surfaces here.
    pub const NAMES: &'static [&'static str] = &[key::MODE_TAG, key::MODE_SECRET, key::MODE_LOBBY];

    /// Parse a `(mode, input)` pair from a surface.
    ///
    /// Rejects an input for `lobby` rather than ignoring it: a user typing
    /// `meet lobby chess` means the word to matter, and silently dropping it
    /// would put them in the pool's open bucket while they believed they were
    /// somewhere named — a never-meet they would have no way to see.
    pub fn parse(mode: &str, input: &str) -> Result<Self, String> {
        let input = input.trim();
        match mode.trim() {
            key::MODE_TAG if !input.is_empty() => Ok(Mode::Tag(input.to_string())),
            key::MODE_SECRET if !input.is_empty() => Ok(Mode::Secret(input.to_string())),
            key::MODE_TAG | key::MODE_SECRET => {
                Err(format!("{mode} needs something to meet at"))
            }
            key::MODE_LOBBY if input.is_empty() => Ok(Mode::Lobby),
            key::MODE_LOBBY => Err(
                "lobby takes no input — it meets at whatever constant the node publishes"
                    .to_string(),
            ),
            other => Err(format!(
                "unknown mode {other:?} (try: {})",
                Self::NAMES.join(", ")
            )),
        }
    }

    /// The upstream mode tag (`tag` / `secret` / `lobby`).
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Lobby => key::MODE_LOBBY,
            Mode::Tag(_) => key::MODE_TAG,
            Mode::Secret(_) => key::MODE_SECRET,
        }
    }

    /// The part of the input that may be shown. **A `secret`'s never is** — it
    /// is the admission gate, and echoing it into scrollback or a window
    /// someone is screen-sharing hands it to whoever is looking. `lobby` has no
    /// input of its own; its input belongs to the node.
    pub fn display_input(&self) -> &str {
        match self {
            Mode::Tag(label) => label,
            Mode::Lobby | Mode::Secret(_) => "",
        }
    }

    /// Mode and input as one line, for a surface with no room to compose them
    /// (the shell). Honours [`display_input`](Self::display_input).
    pub fn describe(&self) -> String {
        match self {
            Mode::Lobby => self.name().to_string(),
            Mode::Tag(_) => format!("{} {}", self.name(), self.display_input()),
            Mode::Secret(_) => format!("{} (hidden)", self.name()),
        }
    }
}

/// A peer-id learned from a bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub peer_id: String,
    /// `true` when the message carrying this id arrived in a §6.3 signed
    /// container whose signer matched the id it claimed. `false` means the id is
    /// an unverified claim — see the module doc: it is checked at the handshake,
    /// not here.
    pub verified: bool,
}

// ---------------------------------------------------------------------------
// Key derivation
// ---------------------------------------------------------------------------

/// Resolve the rendezvous key for `mode` at `node_peer_id`, plus the node's own
/// published TTL **when resolving had to ask it**.
///
/// `tag` and `secret` derive locally. **`lobby` asks the node** — see the module
/// doc; deriving it from `LOBBY_DEFAULT` at a node that overrode the constant is
/// the silent never-meet, so the round trip is not optional here.
///
/// The TTL rides along rather than being fetched separately: the advertisement
/// is already in hand on the one path that fetches it, and throwing it away
/// would leave [`poll_interval_ms`] pacing against a default we could have
/// replaced with a fact. `None` is *"we did not ask"*, never *"the node has no
/// TTL"* — the two would decide the same thing today and are different claims.
pub fn resolve_key(
    peers: &Peers,
    local_peer_id: &str,
    node_peer_id: &str,
    mode: &Mode,
) -> NodeFuture<(RendezvousKey, Option<u64>)> {
    match mode {
        Mode::Tag(label) => {
            let k = key::tag_key(label);
            Box::pin(async move { Ok((k, None)) })
        }
        Mode::Secret(secret) => {
            let k = key::secret_key(secret);
            Box::pin(async move { Ok((k, None)) })
        }
        Mode::Lobby => {
            let fut = connectors::advertise(peers, local_peer_id, node_peer_id);
            Box::pin(async move {
                let ad = fut.await?;
                Ok((key::lobby_key(connectors::lobby_constant_for(&ad)), Some(ad.limits.ttl_seconds)))
            })
        }
    }
}

// ---------------------------------------------------------------------------
// The three round trips
// ---------------------------------------------------------------------------

fn node_execute(
    peers: &Peers,
    local_peer_id: &str,
    node_peer_id: &str,
    operation: &str,
    params: Entity,
) -> NodeFuture<entity_handler::HandlerResult> {
    let uri = format!("entity://{}/{}", node_peer_id, entity_signaling::PATTERN);
    let fut = peers.execute(
        local_peer_id,
        uri,
        operation.to_string(),
        params,
        entity_handler::ExecuteOptions::default(),
    );
    let op = operation.to_string();
    let node = node_peer_id.to_string();
    Box::pin(async move {
        let result = fut.await?;
        if result.status != entity_handler::STATUS_OK {
            // The node's status, named with the operation: a refusal
            // (`bucket_full`, an ungranted peer) and an unreachable node are
            // different meets, and "it didn't work" hides which.
            return Err(format!(
                "{op}: node {node} answered with status {}",
                result.status
            ));
        }
        Ok(result)
    })
}

/// Put one coordination entity in the bucket (`offer`).
fn offer(
    peers: &Peers,
    local_peer_id: &str,
    node_peer_id: &str,
    rendezvous_key: RendezvousKey,
    message: &Entity,
) -> NodeFuture<()> {
    // Bare §6.2 framing, not a §6.3 sealed container — the same thing
    // `cmd/signaling-meet` (the canonical Rust participant) posts, and both are
    // accepted. Sealing would need the local peer's identity keypair on the main
    // thread, which the Worker arm cannot hand us; posting bare is the one shape
    // that behaves identically on both arms. The cost is exactly the one named
    // in the module doc: our own claim is unverified to a stranger, and theirs
    // to us. Introduction, not authorization.
    let params = match (OfferRequest {
        rendezvous_key,
        message: coordination::to_blob(message),
    })
    .to_entity()
    {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("offer: encoding failed: {e}");
            return Box::pin(async move { Err(msg) });
        }
    };
    let fut = node_execute(peers, local_peer_id, node_peer_id, entity_signaling::OP_OFFER, params);
    Box::pin(async move {
        fut.await?;
        Ok(())
    })
}

/// Announce ourselves in the bucket: a `connect-request` under our own peer-id.
///
/// **The candidate list is empty, and that is honest.** A browser peer has no
/// dialable address to advertise — its route is WebRTC established at dispatch
/// time — and a fabricated candidate is worse than none, because a candidate is
/// a claim another peer spends its crossing budget on (`punch_establisher`'s
/// `local_candidates` makes the same call). What this message carries that we
/// need is the peer-id.
pub fn announce(
    peers: &Peers,
    local_peer_id: &str,
    node_peer_id: &str,
    rendezvous_key: RendezvousKey,
    nonce: &Nonce,
) -> NodeFuture<()> {
    let request = ConnectRequest {
        initiator: local_peer_id.to_string(),
        candidates: Vec::new(),
        nonce: nonce.clone(),
    };
    match request.to_entity() {
        Ok(entity) => offer(peers, local_peer_id, node_peer_id, rendezvous_key, &entity),
        Err(e) => {
            let msg = format!("announce: encoding connect-request failed: {e}");
            Box::pin(async move { Err(msg) })
        }
    }
}

/// Answer someone else's `connect-request`, echoing their nonce.
///
/// Answering is not decoration. An initiator counts a meet only when its own
/// nonce comes back, so a peer that reads a bucket and never answers is
/// invisible to every counterpart that follows the standard exchange —
/// including the Go and Python participants. This is what makes us a
/// participant rather than an eavesdropper.
pub fn answer(
    peers: &Peers,
    local_peer_id: &str,
    node_peer_id: &str,
    rendezvous_key: RendezvousKey,
    nonce: &Nonce,
) -> NodeFuture<()> {
    let response = ConnectResponse {
        responder: local_peer_id.to_string(),
        candidates: Vec::new(),
        nonce: nonce.clone(),
    };
    match response.to_entity() {
        Ok(entity) => offer(peers, local_peer_id, node_peer_id, rendezvous_key, &entity),
        Err(e) => {
            let msg = format!("answer: encoding connect-response failed: {e}");
            Box::pin(async move { Err(msg) })
        }
    }
}

/// Read the bucket (`collect`), classified by the extension's own reader.
pub fn collect(
    peers: &Peers,
    local_peer_id: &str,
    node_peer_id: &str,
    rendezvous_key: RendezvousKey,
) -> NodeFuture<Vec<CollectedCoordination>> {
    let params = match (CollectRequest { rendezvous_key }).to_entity() {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("collect: encoding failed: {e}");
            return Box::pin(async move { Err(msg) });
        }
    };
    let fut =
        node_execute(peers, local_peer_id, node_peer_id, entity_signaling::OP_COLLECT, params);
    Box::pin(async move {
        let result = fut.await?;
        let collected = CollectResult::from_params(&result.result.data)
            .map_err(|e| format!("collect: undecodable result: {e}"))?;
        Ok(collected
            .messages
            .iter()
            .map(|blob| coordination::classify_collected(blob, &rendezvous_key))
            .collect())
    })
}

// ---------------------------------------------------------------------------
// Reading a bucket (pure)
// ---------------------------------------------------------------------------

/// Who a collected message is from: the **verified signer** when it arrived
/// sealed, the wire claim otherwise.
///
/// Mirrors upstream's private `author_of`, and for the same reason: written
/// against the claim, this code would keep looking correct while trusting
/// whatever a stranger typed. `Unknown` — an undecodable blob or a message type
/// a newer impl introduced — is a normal outcome, never an error.
fn author_of(c: &CollectedCoordination) -> Option<(&str, bool)> {
    let claimed = match &c.msg {
        CollectedMessage::Request(r) => &r.initiator,
        CollectedMessage::Response(r) => &r.responder,
        CollectedMessage::Sync(_) | CollectedMessage::Unknown => return None,
    };
    match c.signer.as_ref() {
        Some(s) => Some((s.peer_id(), true)),
        None => Some((claimed.as_str(), false)),
    }
}

/// Every peer-id in the bucket that is not us, deduped, sorted, a verified
/// sighting winning over an unverified one for the same id.
///
/// Our own messages are here on every poll — `collect` is non-destructive, so we
/// always re-read what we wrote — and a peer that "met itself" would report
/// success at meeting nobody.
pub fn discovered_in(messages: &[CollectedCoordination], my_peer_id: &str) -> Vec<Discovered> {
    let mut found: BTreeMap<String, bool> = BTreeMap::new();
    for c in messages {
        let Some((author, verified)) = author_of(c) else { continue };
        if author == my_peer_id || author.is_empty() {
            continue;
        }
        let entry = found.entry(author.to_string()).or_insert(verified);
        *entry = *entry || verified;
    }
    found
        .into_iter()
        .map(|(peer_id, verified)| Discovered { peer_id, verified })
        .collect()
}

/// The nonces of requests in the bucket that we should answer — anyone's but our
/// own. The caller skips the ones it has answered already (see the module doc).
pub fn requests_to_answer(messages: &[CollectedCoordination], my_peer_id: &str) -> Vec<Nonce> {
    messages
        .iter()
        .filter_map(|c| match &c.msg {
            CollectedMessage::Request(r) => {
                let author = author_of(c).map(|(a, _)| a).unwrap_or(r.initiator.as_str());
                (author != my_peer_id).then(|| r.nonce.clone())
            }
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

/// How long a session keeps listening. Bounded on purpose: an unbounded meet is
/// a background job nobody asked for, holding a bucket entry alive forever.
/// Restarting is one press.
///
/// **Two minutes, because the number people have to hit is each other, not a
/// deadline.** The previous window was ~30 s, which is shorter than "let me go
/// and tell them to press it too" — and it was *rendered as a counter climbing
/// to 60*, so the surface asked the user to watch a number race toward an
/// outcome they had no way to price. Four times the window at **half** the
/// requests is the trade [`poll_interval_ms`] buys.
pub const SEARCH_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);

/// The backoff schedule as `(listening for at least N ms, wait this long)`,
/// consulted by [`poll_interval_ms`]. Ascending by the first element; the last
/// entry that fits wins.
///
/// The first tier is what makes "we both pressed it just now" feel immediate;
/// the last is the steady state for someone who pressed and walked away.
const BACKOFF_MS: [(u64, u64); 3] = [(0, 1_000), (10_000, 3_000), (30_000, 8_000)];

/// The floor under the TTL-derived ceiling. A node advertising a nonsensically
/// short TTL must not turn the loop into a spin.
const MIN_INTERVAL_MS: u64 = 500;

/// How long to wait before the next `collect`, given how long this session has
/// been listening and the TTL the node published.
///
/// Pure, and native — the cadence is the part of a meet most likely to be
/// re-tuned by someone who cannot run a browser, and a schedule that can only be
/// observed through Selenium is one nobody checks.
///
/// The TTL **narrows and never widens**: a generous node does not license us to
/// leave a waiting person staring at nothing for a quarter of a minute.
pub fn poll_interval_ms(listening_ms: u64, ttl_seconds: u64) -> u64 {
    // A quarter of the TTL: an interval that straddles a deposit and its expiry
    // loses a peer silently, and a quarter leaves room for the answer exchange
    // inside one lifetime rather than merely clearing the edge.
    let ceiling = (ttl_seconds.saturating_mul(1000) / 4).max(MIN_INTERVAL_MS);
    let step = BACKOFF_MS
        .iter()
        .rev()
        .find(|(after, _)| listening_ms >= *after)
        .map(|(_, every)| *every)
        .unwrap_or(MIN_INTERVAL_MS);
    step.min(ceiling)
}

/// The TTL to pace against when the node never told us — the protocol's own
/// published default, read from the extension rather than typed in here, so a
/// §4.5 retune moves both together.
fn default_ttl_seconds() -> u64 {
    entity_signaling::Limits::default().ttl_seconds
}

/// Wall-clock budget for **getting to the bucket** — the dial, the key, the
/// announce. Not the search itself, which [`MAX_POLLS`] bounds.
///
/// It exists because a round trip can fail to *return at all*, and nothing else
/// here would ever end the session: the poll counter only advances once we are
/// listening, so a dial that neither resolves nor rejects would hold `busy`
/// forever and render "Searching…" indefinitely.
///
/// **Honest provenance:** no such hang has been observed. It was written after
/// a permanent-"Searching…" bug that looked like one and was not — the dial had
/// failed quickly and the *window* never repainted (see `Inner::changed`). The
/// budget stays because the failure it guards is real and unbounded-by-
/// construction, not because it was the fix; `a_setup_round_trip_that_never_
/// returns_still_ends_in_a_stated_failure` is what holds it.
///
/// Wall-clock, not a frame count: a native test pumps thousands of frames a
/// second and a loaded browser far fewer, and this bound is about *seconds a
/// person waits*.
const SETUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetPhase {
    /// Dialing the connector, if nothing has yet.
    Reaching,
    /// Deriving the key (a round trip in lobby mode).
    Deriving,
    /// Putting our own `connect-request` in the bucket.
    Announcing,
    /// Reading the bucket and answering what we find.
    Listening,
    /// The search window closed. Whatever was found is what there was.
    Done,
    /// The meet stopped early, with the reason the user needs to see.
    Failed(String),
}

impl MeetPhase {
    pub fn is_settled(&self) -> bool {
        matches!(self, MeetPhase::Done | MeetPhase::Failed(_))
    }
}

/// A snapshot for a surface to render. Cheap to take.
#[derive(Debug, Clone)]
pub struct MeetStatus {
    pub phase: MeetPhase,
    /// Mode and input on one line, safe to display ([`Mode::describe`]) — for a
    /// surface that cannot compose them itself.
    pub mode: String,
    /// The upstream mode tag (`tag` / `secret` / `lobby`), so a **localized**
    /// surface can render its own word for it rather than this English one.
    pub mode_name: &'static str,
    /// The input, empty whenever it must not be shown
    /// ([`Mode::display_input`]).
    pub mode_input: String,
    pub node_peer_id: String,
    pub found: Vec<Discovered>,
    /// Round trips spent reading the bucket. A diagnostic, **not** a progress
    /// bar: the window is wall clock, so this number does not divide into
    /// anything and a surface that renders it as `polls/max` is telling the user
    /// to watch a counter climb. Use [`remaining_ms`](Self::remaining_ms).
    pub polls: u32,
    /// Milliseconds left in the search window; `0` once settled.
    pub remaining_ms: u64,
}

#[derive(Debug)]
struct Inner {
    phase: MeetPhase,
    rendezvous_key: Option<RendezvousKey>,
    /// What the node published as its bucket TTL, when resolving the key had to
    /// ask (see [`resolve_key`]). Lands from a spawned round trip, which is why
    /// it lives here rather than on the session.
    ttl_seconds: Option<u64>,
    /// peer-id → was it ever seen verified.
    found: BTreeMap<String, bool>,
    answered: HashSet<Vec<u8>>,
    to_answer: VecDeque<Nonce>,
    polls: u32,
    /// One round trip at a time. Without this the pump would fire a fresh
    /// `collect` every frame while the first is still in flight.
    busy: bool,
    /// Something visible changed since a surface last looked.
    ///
    /// **A surface cannot detect this by diffing around `pump`.** Almost every
    /// change lands in a *spawned* round trip, i.e. between frames — so a
    /// caller comparing the status before and against after one `pump` sees
    /// them equal and concludes nothing happened. That is not hypothetical: the
    /// Peer Connections window did exactly that, and a meet that failed its dial
    /// kept rendering "Searching…" forever, because the window is repainted only
    /// when told and nothing told it (e2e Phase 14.7 caught it).
    changed: bool,
}

/// One meet, driven from the frame loop.
///
/// Owned by whichever surface started it — the Peer Connections window and the
/// Shell window each hold their own, because a meet is an action in progress
/// (like a dial marker), meaningless across a reload and not something to put in
/// the tree.
#[derive(Debug)]
pub struct MeetSession {
    local_peer_id: String,
    node: Connector,
    mode: Mode,
    nonce: Nonce,
    inner: Arc<Mutex<Inner>>,
    /// How long this session listens once it is in the bucket.
    window: std::time::Duration,
    /// When listening began. Taken on the first `Listening` pump rather than in
    /// the transition, because the transition lands in a spawned task and the
    /// clock that matters is the one the user is watching.
    listening_since: Option<web_time::Instant>,
    /// When the next `collect` is due. `None` until the first Listening pump,
    /// which polls immediately.
    next_poll_at: Option<web_time::Instant>,
    /// A fixed interval instead of [`poll_interval_ms`]'s schedule. **Tests
    /// only** — and the field itself is `cfg(test)`, so production has no slot
    /// for anyone to fill. An integration test that ran the real schedule would
    /// spend real seconds proving something [`poll_interval_ms`]'s own tests
    /// prove for free.
    #[cfg(test)]
    interval_override: Option<std::time::Duration>,
    /// When "getting to the bucket" stops being worth waiting for. `web_time`
    /// so the same code measures real seconds in the browser and on native.
    setup_deadline: web_time::Instant,
}

impl MeetSession {
    /// Start a meet at `node` in `mode`. Nothing happens until [`pump`](Self::pump).
    pub fn start(local_peer_id: &str, node: Connector, mode: Mode) -> Self {
        Self {
            local_peer_id: local_peer_id.to_string(),
            node,
            mode,
            nonce: Nonce::generate(),
            inner: Arc::new(Mutex::new(Inner {
                phase: MeetPhase::Reaching,
                rendezvous_key: None,
                ttl_seconds: None,
                found: BTreeMap::new(),
                answered: HashSet::new(),
                to_answer: VecDeque::new(),
                polls: 0,
                busy: false,
                changed: false,
            })),
            window: SEARCH_WINDOW,
            listening_since: None,
            next_poll_at: None,
            #[cfg(test)]
            interval_override: None,
            setup_deadline: web_time::Instant::now() + SETUP_BUDGET,
        }
    }

    /// Shorten the search window and fix the interval — for tests, which pump in
    /// a tight loop and would otherwise spend the real two minutes.
    #[cfg(test)]
    fn with_cadence(mut self, interval: std::time::Duration, window: std::time::Duration) -> Self {
        self.interval_override = Some(interval);
        self.window = window;
        self
    }

    /// The wait before the next `collect`.
    fn next_interval_ms(&self, listening_ms: u64, ttl_seconds: u64) -> u64 {
        #[cfg(test)]
        if let Some(fixed) = self.interval_override {
            return fixed.as_millis() as u64;
        }
        poll_interval_ms(listening_ms, ttl_seconds)
    }

    /// Milliseconds left in the search window. The full window until listening
    /// begins — the setup steps are not part of the time the user was promised,
    /// and counting them down would make a slow dial look like a short search.
    fn remaining_ms(&self) -> u64 {
        match self.listening_since {
            None => self.window.as_millis() as u64,
            Some(since) => self
                .window
                .saturating_sub(since.elapsed())
                .as_millis() as u64,
        }
    }

    /// Shorten the setup budget — for the test that proves a round trip which
    /// never returns still settles.
    #[cfg(test)]
    fn with_setup_budget(mut self, budget: std::time::Duration) -> Self {
        self.setup_deadline = web_time::Instant::now() + budget;
        self
    }

    pub fn status(&self) -> MeetStatus {
        let inner = match self.inner.lock() {
            Ok(i) => i,
            Err(_) => {
                return MeetStatus {
                    phase: MeetPhase::Failed("meet state poisoned".to_string()),
                    mode: self.mode.describe(),
                    mode_name: self.mode.name(),
                    mode_input: self.mode.display_input().to_string(),
                    node_peer_id: self.node.node_peer_id.clone(),
                    found: Vec::new(),
                    polls: 0,
                    remaining_ms: 0,
                }
            }
        };
        MeetStatus {
            phase: inner.phase.clone(),
            mode: self.mode.describe(),
            mode_name: self.mode.name(),
            mode_input: self.mode.display_input().to_string(),
            node_peer_id: self.node.node_peer_id.clone(),
            found: inner
                .found
                .iter()
                .map(|(peer_id, verified)| Discovered {
                    peer_id: peer_id.clone(),
                    verified: *verified,
                })
                .collect(),
            polls: inner.polls,
            // A settled session has no time left, whatever the clock says — the
            // window is the reason to keep waiting, and reporting one over a
            // search that already stopped is a countdown to nothing.
            remaining_ms: if inner.phase.is_settled() { 0 } else { self.remaining_ms() },
        }
    }

    /// End the search now, keeping what was found.
    pub fn stop(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            if !inner.phase.is_settled() {
                inner.phase = MeetPhase::Done;
                inner.changed = true;
            }
        }
    }

    /// Has anything visible changed since the last call? Clears the flag.
    ///
    /// A surface that repaints only when told (every window here) must ask this
    /// rather than diff the status around [`pump`](Self::pump) — see
    /// `Inner::changed`.
    pub fn take_changed(&self) -> bool {
        match self.inner.lock() {
            Ok(mut inner) => std::mem::take(&mut inner.changed),
            Err(_) => false,
        }
    }

    /// One frame of progress. Cheap when settled, when a round trip is in
    /// flight, or between polls.
    pub fn pump(&mut self, peers: &Peers) {
        let phase = {
            let Ok(mut inner) = self.inner.lock() else { return };
            if inner.phase.is_settled() {
                return;
            }
            // The setup budget is checked BEFORE `busy`, because the case it
            // exists for is precisely a round trip that never returns: with the
            // busy check first, a hung dial would keep the session out of this
            // branch forever. Listening is bounded by its poll count instead.
            if inner.phase != MeetPhase::Listening
                && web_time::Instant::now() > self.setup_deadline
            {
                inner.phase = MeetPhase::Failed(format!(
                    "{} did not answer within {}s",
                    crate::views::short_pid(&self.node.node_peer_id),
                    SETUP_BUDGET.as_secs()
                ));
                inner.changed = true;
                return;
            }
            if inner.busy {
                return;
            }
            inner.phase.clone()
        };
        let key = self.inner.lock().ok().and_then(|i| i.rendezvous_key);

        match phase {
            MeetPhase::Reaching => {
                let fut = connectors::reach_node(peers, &self.local_peer_id, &self.node);
                self.run(Box::pin(fut), |inner, _| inner.phase = MeetPhase::Deriving);
            }
            MeetPhase::Deriving => {
                let fut =
                    resolve_key(peers, &self.local_peer_id, &self.node.node_peer_id, &self.mode);
                self.run(fut, |inner, (k, ttl)| {
                    inner.rendezvous_key = Some(k);
                    inner.ttl_seconds = ttl;
                    inner.phase = MeetPhase::Announcing;
                });
            }
            MeetPhase::Announcing => {
                let Some(key) = key else { return };
                let fut = announce(
                    peers,
                    &self.local_peer_id,
                    &self.node.node_peer_id,
                    key,
                    &self.nonce,
                );
                self.run(fut, |inner, _| inner.phase = MeetPhase::Listening);
            }
            MeetPhase::Listening => {
                let Some(key) = key else { return };
                // Answer before polling again: a queued answer is a peer already
                // waiting on us, and it costs one round trip to stop being
                // invisible to them.
                let next_answer = self.inner.lock().ok().and_then(|mut i| i.to_answer.pop_front());
                if let Some(nonce) = next_answer {
                    let fut = answer(
                        peers,
                        &self.local_peer_id,
                        &self.node.node_peer_id,
                        key,
                        &nonce,
                    );
                    self.run(fut, |_, _| {});
                    return;
                }
                // The window starts when we are actually in the bucket, and the
                // first poll is due immediately: whoever pressed second finds
                // the first one right here, on this poll, with no wait at all.
                let now = web_time::Instant::now();
                let since = *self.listening_since.get_or_insert(now);
                if now.duration_since(since) >= self.window {
                    let Ok(mut inner) = self.inner.lock() else { return };
                    inner.phase = MeetPhase::Done;
                    inner.changed = true;
                    return;
                }
                if now < *self.next_poll_at.get_or_insert(now) {
                    return;
                }
                let ttl = self
                    .inner
                    .lock()
                    .ok()
                    .and_then(|i| i.ttl_seconds)
                    .unwrap_or_else(default_ttl_seconds);
                let wait =
                    self.next_interval_ms(now.duration_since(since).as_millis() as u64, ttl);
                self.next_poll_at = Some(now + std::time::Duration::from_millis(wait));
                {
                    let Ok(mut inner) = self.inner.lock() else { return };
                    inner.polls += 1;
                    // **This is also what keeps the countdown honest.** A window
                    // repaints only when told, and nothing else ticks — so the
                    // displayed time-left is only ever as fresh as the last
                    // poll. That is sound because the poll interval is capped in
                    // single-digit seconds while the display is coarse to the
                    // minute: the phrase cannot be stale by enough to be wrong.
                    // A finer display would need its own reason to repaint.
                    inner.changed = true;
                }
                let fut = collect(peers, &self.local_peer_id, &self.node.node_peer_id, key);
                let me = self.local_peer_id.clone();
                self.run(fut, move |inner, messages: Vec<CollectedCoordination>| {
                    for d in discovered_in(&messages, &me) {
                        let entry = inner.found.entry(d.peer_id).or_insert(d.verified);
                        *entry = *entry || d.verified;
                    }
                    for nonce in requests_to_answer(&messages, &me) {
                        // Answered-once, tracked by nonce: the bucket keeps
                        // handing us the same request every poll until its TTL.
                        if inner.answered.insert(nonce.as_bytes().to_vec()) {
                            inner.to_answer.push_back(nonce);
                        }
                    }
                });
            }
            MeetPhase::Done | MeetPhase::Failed(_) => {}
        }
    }

    /// Spawn one round trip, land its result, clear `busy`.
    ///
    /// A failure **fails the session** rather than being retried silently: every
    /// step here is a call to a node the user just named, and one that cannot be
    /// reached, refuses us, or answers undecodably is a fact they need — the
    /// alternative is a spinner that never resolves.
    fn run<T: 'static>(
        &self,
        fut: NodeFuture<T>,
        land: impl FnOnce(&mut Inner, T) + Send + 'static,
    ) {
        {
            let Ok(mut inner) = self.inner.lock() else { return };
            inner.busy = true;
        }
        let state = self.inner.clone();
        spawn(async move {
            let outcome = fut.await;
            let Ok(mut inner) = state.lock() else { return };
            inner.busy = false;
            // A round trip that lands after the session gave up must not
            // resurrect it: the user has already been told it failed, and a
            // status that un-fails itself is worse than either outcome.
            if inner.phase.is_settled() {
                return;
            }
            match outcome {
                Ok(value) => land(&mut inner, value),
                Err(e) => {
                    tracing::warn!(error = %e, "meet: step failed");
                    inner.phase = MeetPhase::Failed(e);
                }
            }
            // Every landing is a between-frames change, which is exactly what a
            // surface cannot see by diffing around `pump`.
            inner.changed = true;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::tests::spawn_signaling_node;
    use entity_peer::transport::{MemoryConnector, MemoryTransportRegistry};

    /// An app peer that dials over the shared memory transport, plus its
    /// connector row for `node`.
    fn app_peer(
        registry: std::sync::Arc<MemoryTransportRegistry>,
        node_pid: &str,
    ) -> (Peers, String, Connector) {
        let peers =
            Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(registry)));
        let me = peers.primary_peer_id().to_string();
        let connector = Connector {
            node_peer_id: node_pid.to_string(),
            node_addr: format!("memory://{node_pid}"),
            label: "test node".to_string(),
            ice: String::new(),
            ice_advertised: String::new(),
            relay: String::new(),
            relay_username: String::new(),
            relay_credential: String::new(),
        };
        (peers, me, connector)
    }

    /// Pump both sessions until `done`, or give up. Bounded, and returns on the
    /// first satisfied check — a fixed sleep here would encode a guess about how
    /// long a round trip takes, which is the shape that fails on a loaded box.
    async fn pump_until(
        a: (&mut MeetSession, &Peers),
        b: (&mut MeetSession, &Peers),
        mut done: impl FnMut(&MeetSession, &MeetSession) -> bool,
    ) {
        let (sa, pa) = a;
        let (sb, pb) = b;
        for _ in 0..600 {
            sa.pump(pa);
            sb.pump(pb);
            if done(sa, sb) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    #[test]
    fn a_mode_is_parsed_from_what_a_surface_can_offer() {
        assert_eq!(Mode::parse("tag", "chess"), Ok(Mode::Tag("chess".into())));
        assert_eq!(Mode::parse("secret", " s3cret "), Ok(Mode::Secret("s3cret".into())));
        assert_eq!(Mode::parse("lobby", ""), Ok(Mode::Lobby));
        assert!(Mode::parse("tag", "  ").is_err(), "a tag needs a label");
        assert!(
            Mode::parse("lobby", "chess").is_err(),
            "a lobby input must be refused, not dropped — dropping it puts the user \
             in the open bucket while they believe they are somewhere named"
        );
        assert!(Mode::parse("pair", "x").is_err(), "pair has no naming surface");
    }

    /// A secret must never be echoed back at the user's screen.
    #[test]
    fn a_secret_is_never_rendered_into_a_status_line() {
        let s = Mode::Secret("correct-horse-battery-staple".to_string()).describe();
        assert!(!s.contains("correct-horse"), "got {s:?}");
    }

    /// The cadence starts fast and settles, and **every step stays under the
    /// node's TTL** — an interval that can straddle a deposit and its expiry
    /// loses a peer with nothing erroring anywhere, which is the same silent
    /// never-meet the lobby constant produces.
    #[test]
    fn the_poll_cadence_backs_off_and_never_outruns_the_bucket() {
        let ttl = default_ttl_seconds();
        let first = poll_interval_ms(0, ttl);
        let middle = poll_interval_ms(15_000, ttl);
        let late = poll_interval_ms(90_000, ttl);

        assert!(first < middle && middle < late, "{first} {middle} {late}");
        assert!(
            first <= 1_000,
            "the first tier is what makes 'we both just pressed it' feel immediate; got {first}"
        );

        // Sampled across the whole window, at the *published* TTL and at a
        // node that publishes a much shorter one.
        for ttl_seconds in [ttl, 8, 4, 1] {
            let ttl_ms = ttl_seconds * 1000;
            let mut at = 0;
            while at <= SEARCH_WINDOW.as_millis() as u64 {
                let wait = poll_interval_ms(at, ttl_seconds);
                assert!(
                    wait < ttl_ms || wait == MIN_INTERVAL_MS,
                    "an interval of {wait}ms at ttl {ttl_seconds}s can outlive a bucket entry"
                );
                assert!(wait >= MIN_INTERVAL_MS, "a {wait}ms interval is a spin");
                at += wait;
            }
        }
    }

    /// A generous node does not license a slow surface. The TTL **narrows**:
    /// what bounds the interval at the top is the person waiting, not the bucket.
    #[test]
    fn a_longer_advertised_ttl_never_widens_the_interval() {
        let generous = poll_interval_ms(90_000, 3_600);
        let default = poll_interval_ms(90_000, default_ttl_seconds());
        assert_eq!(generous, default, "a one-hour TTL must not slow the surface down");
        assert!(generous <= 8_000, "the tail interval is the wait a person already sat through");
    }

    /// The pacing has to be **cheaper** than what it replaced, not just longer:
    /// the previous loop spent 60 round trips in 30 s, and the complaint it
    /// answers was about the wait, not about throughput.
    #[test]
    fn the_longer_window_costs_fewer_round_trips_than_the_old_one() {
        let ttl = default_ttl_seconds();
        let mut at = 0u64;
        let mut polls = 0u32;
        while at < SEARCH_WINDOW.as_millis() as u64 {
            at += poll_interval_ms(at, ttl);
            polls += 1;
        }
        assert!(
            polls < 60,
            "120 s of searching must cost fewer than the 60 polls the old 30 s window spent; \
             got {polls}"
        );
        assert!(polls > 10, "and it must still actually look; got {polls}");
    }

    /// The same input under two modes is two different buckets — domain
    /// separation is upstream's, but *passing the right mode* is ours, and a
    /// mix-up here is a silent never-meet with no error anywhere.
    #[test]
    fn tag_and_secret_with_the_same_input_do_not_share_a_bucket() {
        assert_ne!(key::tag_key("chess"), key::secret_key("chess"));
    }

    /// Our own messages are in every poll (collect is non-destructive), and a
    /// peer that counted itself would report meeting nobody as success.
    #[test]
    fn reading_a_bucket_never_reports_ourselves() {
        let k = key::tag_key("chess");
        let mine = ConnectRequest {
            initiator: "2KMe".to_string(),
            candidates: Vec::new(),
            nonce: Nonce::generate(),
        }
        .to_entity()
        .unwrap();
        let theirs = ConnectRequest {
            initiator: "2KThem".to_string(),
            candidates: Vec::new(),
            nonce: Nonce::generate(),
        }
        .to_entity()
        .unwrap();
        let messages: Vec<CollectedCoordination> = [mine, theirs]
            .iter()
            .map(|e| coordination::classify_collected(&coordination::to_blob(e), &k))
            .collect();

        let found = discovered_in(&messages, "2KMe");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].peer_id, "2KThem");
        assert!(
            !found[0].verified,
            "a bare message's author is a claim, and must not be reported as verified"
        );
        assert_eq!(
            requests_to_answer(&messages, "2KMe").len(),
            1,
            "we answer theirs and not our own"
        );
    }

    /// **The feature, end to end:** two peers who know each other's *nothing*
    /// meet at a label through a real node, and each comes away with the other's
    /// peer id.
    ///
    /// A real `entity-peer` node with the extension's own `SignalingHandler`,
    /// not a stub: what is worth proving is that our offer/collect round trips
    /// reach the real handler and that its replies decode. Neither side is
    /// given the other's id.
    #[tokio::test]
    async fn two_peers_meet_at_a_tag_and_learn_each_others_ids() {
        let registry = MemoryTransportRegistry::new();
        let (node_pid, node) = spawn_signaling_node(registry.clone(), "pool-seven");

        let (peers_a, pid_a, conn_a) = app_peer(registry.clone(), &node_pid);
        let (peers_b, pid_b, conn_b) = app_peer(registry.clone(), &node_pid);
        assert_ne!(pid_a, pid_b);
        tokio::task::yield_now().await;

        let mut a = MeetSession::start(&pid_a, conn_a, Mode::Tag("chess".into()))
            .with_cadence(std::time::Duration::ZERO, std::time::Duration::from_secs(5));
        let mut b = MeetSession::start(&pid_b, conn_b, Mode::Tag("chess".into()))
            .with_cadence(std::time::Duration::ZERO, std::time::Duration::from_secs(5));

        let (want_a, want_b) = (pid_b.clone(), pid_a.clone());
        pump_until(
            (&mut a, &peers_a),
            (&mut b, &peers_b),
            move |a, b| {
                a.status().found.iter().any(|d| d.peer_id == want_a)
                    && b.status().found.iter().any(|d| d.peer_id == want_b)
            },
        )
        .await;

        let (sa, sb) = (a.status(), b.status());
        assert!(
            !matches!(sa.phase, MeetPhase::Failed(_)) && !matches!(sb.phase, MeetPhase::Failed(_)),
            "a: {:?}  b: {:?}",
            sa.phase,
            sb.phase
        );
        assert!(
            sa.found.iter().any(|d| d.peer_id == pid_b),
            "A met nobody at the tag; found {:?}",
            sa.found
        );
        assert!(
            sb.found.iter().any(|d| d.peer_id == pid_a),
            "B met nobody at the tag; found {:?}",
            sb.found
        );

        node.abort();
    }

    /// **The mutation check for the one above:** two peers at *different* labels
    /// share a node and never meet. If they did, the meet would not be keyed by
    /// what the user typed at all, and the passing test above would be measuring
    /// nothing.
    #[tokio::test]
    async fn two_peers_at_different_tags_never_meet() {
        let registry = MemoryTransportRegistry::new();
        let (node_pid, node) = spawn_signaling_node(registry.clone(), "pool-seven");

        let (peers_a, pid_a, conn_a) = app_peer(registry.clone(), &node_pid);
        let (peers_b, pid_b, conn_b) = app_peer(registry.clone(), &node_pid);
        tokio::task::yield_now().await;

        let mut a =
            MeetSession::start(&pid_a, conn_a, Mode::Tag("chess".into())).with_cadence(std::time::Duration::ZERO, std::time::Duration::from_millis(400));
        let mut b =
            MeetSession::start(&pid_b, conn_b, Mode::Tag("checkers".into())).with_cadence(std::time::Duration::ZERO, std::time::Duration::from_millis(400));

        // Run both to the end of their (short) windows rather than to a hit.
        pump_until((&mut a, &peers_a), (&mut b, &peers_b), |a, b| {
            a.status().phase == MeetPhase::Done && b.status().phase == MeetPhase::Done
        })
        .await;

        assert!(a.status().found.is_empty(), "A found {:?}", a.status().found);
        assert!(b.status().found.is_empty(), "B found {:?}", b.status().found);

        node.abort();
    }

    /// Lobby mode derives from **the node's** constant, not from `LOBBY_DEFAULT`.
    ///
    /// This node overrides it. A peer that assumed the default would derive a
    /// key nobody else on this pool uses — the silent never-meet, which produces
    /// no error anywhere. The assertion is the derived key itself, because that
    /// is the only place the mistake would be visible.
    #[tokio::test]
    async fn a_lobby_key_comes_from_the_node_not_from_the_default() {
        let registry = MemoryTransportRegistry::new();
        let (node_pid, node) = spawn_signaling_node(registry.clone(), "pool-seven");
        let (peers, me, connector) = app_peer(registry.clone(), &node_pid);
        tokio::task::yield_now().await;

        connectors::reach_node(&peers, &me, &connector).await.expect("dial the node");
        let (k, ttl) = resolve_key(&peers, &me, &node_pid, &Mode::Lobby)
            .await
            .expect("the node advertises its lobby constant");

        assert_eq!(k, key::lobby_key("pool-seven"));
        // The advertisement was already in hand; dropping the TTL would leave
        // the poll cadence pacing against a default we could have replaced with
        // a fact about this node.
        assert_eq!(
            ttl,
            Some(default_ttl_seconds()),
            "a lobby resolve must carry back the TTL the node published"
        );
        assert_ne!(
            k,
            key::lobby_key(entity_signaling::LOBBY_DEFAULT),
            "deriving from LOBBY_DEFAULT at a node that overrode it is the never-meet"
        );

        node.abort();
    }

    /// **A round trip that never returns still settles.**
    ///
    /// The failure this pins was found in the browser, not here: a WebSocket to
    /// an unresolvable host neither connects nor errors within 30 s, so a
    /// session bounded only by its poll count sat in `Reaching` — one dial in
    /// flight, `busy` set, nothing else able to run — and rendered "Searching…"
    /// indefinitely. The setup budget is what ends it, and the assertion below
    /// is that it ends in a **stated failure**, not a quiet `Done`.
    ///
    /// Driven with a zero budget rather than a real dial, because the point is
    /// the deadline, and a test that waited 20 s to prove a timeout would be
    /// paid for on every run forever.
    #[tokio::test]
    async fn a_setup_round_trip_that_never_returns_still_ends_in_a_stated_failure() {
        let registry = MemoryTransportRegistry::new();
        let (peers, me, connector) = app_peer(registry, "2KNobodyHome");
        let mut s = MeetSession::start(&me, connector, Mode::Tag("chess".into()))
            .with_cadence(std::time::Duration::ZERO, std::time::Duration::from_secs(5))
            .with_setup_budget(std::time::Duration::ZERO);

        s.pump(&peers);
        match s.status().phase {
            MeetPhase::Failed(reason) => assert!(
                reason.contains("did not answer"),
                "the reason must say what we gave up on; got {reason:?}"
            ),
            other => panic!("a session past its setup budget must fail, got {other:?}"),
        }
    }

    /// A meet against a node that isn't there fails **visibly**, with the reason.
    /// A session that quietly kept polling would render as a spinner that never
    /// resolves — the dead-button disease wearing a progress indicator.
    #[tokio::test]
    async fn a_meet_at_an_unreachable_node_fails_with_a_reason() {
        let registry = MemoryTransportRegistry::new();
        let (peers, me, _) = app_peer(registry.clone(), "2KNobodyHome");
        let ghost = Connector {
            node_peer_id: "2KNobodyHome".to_string(),
            node_addr: "memory://2KNobodyHome".to_string(),
            label: String::new(),
            ice: String::new(),
            ice_advertised: String::new(),
            relay: String::new(),
            relay_username: String::new(),
            relay_credential: String::new(),
        };
        let mut s = MeetSession::start(&me, ghost, Mode::Tag("chess".into())).with_cadence(std::time::Duration::ZERO, std::time::Duration::from_millis(400));

        for _ in 0..200 {
            s.pump(&peers);
            if s.status().phase.is_settled() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        match s.status().phase {
            MeetPhase::Failed(reason) => assert!(!reason.is_empty()),
            other => panic!("expected a visible failure, got {other:?}"),
        }
    }
}
