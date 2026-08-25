//! ChatDelivery — the cross-peer delivery layer (the §1.4 "cache the others'
//! messages" step), proven end-to-end in `peers.rs`'s memory-transport tests.
//!
//! A conversation is a set of per-author, append-only, signed message logs
//! (`model.rs`). Our own log we author directly; the *other* participants' logs
//! we must pull in and cache under their prefix in our own store so the union
//! read (`ChatModel::load_messages`) sees them. That pull has two discovery
//! mechanisms feeding one fetch→cache pipeline:
//!
//! - **subscribe (reactive, Direct arm):** `subscribe_at` each remote
//!   participant → a notification (path+hash) streams back on every write.
//!   Needs a main-thread `PeerContext`, so Direct arm only.
//! - **poll (arm- and transport-agnostic):** every so often, `execute
//!   tree:get` the remote's messages prefix (a directory listing) and enqueue
//!   the message paths. `execute` routes over the connection pool on **either**
//!   arm and **any** transport, so this is what makes delivery work in Worker
//!   mode (`?worker=1`) — and thus over the **worker-only WebRTC** channel,
//!   where the reactive `subscribe_at` is unavailable. Its `execute` to the
//!   remote also *triggers* lazy WebRTC establishment.
//!
//! Both feed `notified_tx`; `pump` fetches each new path (`tree:get`) and caches
//! it under `/{author}/…` in our own store (`dispatch_write` — the L1 path the
//! union read reflects). A `seen` set makes it fetch-once, so the redundant
//! poll/subscribe overlap on the Direct arm costs nothing.
//!
//! **No transport-specific code lives here** — delivery rides the connection
//! pool, so WebSocket, WebRTC, and the in-process memory transport are identical.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use entity_capability::ResourceTarget;
use entity_entity::Entity;
use entity_handler::ExecuteOptions;
use entity_sdk::subscription::L1SubscriptionHandle;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use super::model::conversation_messages_prefix;
use crate::peers::Peers;

/// Pumps between poll-list passes. `pump` runs once per frame (~60/s); polling
/// every frame would hammer, so throttle to a fresh listing a few times a
/// second. The reactive subscribe path (Direct arm) is instant regardless.
const POLL_EVERY: u32 = 12;

/// Drives delivery for one bound conversation on one local peer.
pub struct ChatDelivery {
    local_pid: String,
    conversation_id: String,
    /// The remote participants whose logs we pull in (everyone but us).
    remote: Vec<String>,
    /// Message paths discovered (by subscribe notification OR poll listing),
    /// awaiting a fetch.
    notified_tx: UnboundedSender<String>,
    notified_rx: UnboundedReceiver<String>,
    /// Fetch outcomes, awaiting a local cache write. `Some` = a 200 fetch to
    /// cache; `None` = the fetch failed (non-200 / transport error) and the path
    /// must be released so a later poll can retry it.
    fetched_tx: UnboundedSender<(String, Option<Entity>)>,
    fetched_rx: UnboundedReceiver<(String, Option<Entity>)>,
    /// Live subscription handles — dropping the delivery unsubscribes. Shared so
    /// the wasm [`start`](ChatDelivery::start) path can land a handle from the
    /// spawned subscribe task; the callback captures only `Send + Sync` senders,
    /// never this `Rc`.
    subs: Rc<RefCell<Vec<L1SubscriptionHandle>>>,
    /// Paths successfully cached — never re-fetched. Marked only on a *completed*
    /// cache write, so a failed fetch (e.g. the first one, which is what triggers
    /// lazy WebRTC establishment and often is not yet 200) does NOT poison the
    /// path; the poll re-lists it and it retries.
    seen: HashSet<String>,
    /// Paths with a fetch currently in flight — a dispatch guard so a repeated
    /// notification/poll does not spawn a second concurrent fetch. Cleared on
    /// completion (success or failure).
    in_flight: HashSet<String>,
    /// Frame counter for the poll throttle.
    poll_tick: u32,
}

#[allow(dead_code)] // some methods are arm- or test-specific
impl ChatDelivery {
    pub fn new(local_pid: String, conversation_id: String, participants: Vec<String>) -> Self {
        let (notified_tx, notified_rx) = unbounded_channel();
        let (fetched_tx, fetched_rx) = unbounded_channel();
        let remote = participants
            .into_iter()
            .filter(|p| p != &local_pid)
            .collect();
        Self {
            local_pid,
            conversation_id,
            remote,
            notified_tx,
            notified_rx,
            fetched_tx,
            fetched_rx,
            subs: Rc::new(RefCell::new(Vec::new())),
            seen: HashSet::new(),
            in_flight: HashSet::new(),
            poll_tick: 0,
        }
    }

    /// Build the subscribe callback for one participant — captures only the
    /// `Send + Sync` notify sender (never the `Rc` subs), so it satisfies
    /// `subscribe_at`'s `Fn + Send + Sync` bound.
    fn notify_cb(&self) -> impl Fn(entity_sdk::subscription::L1SubscriptionEvent) + Send + Sync + 'static {
        let tx = self.notified_tx.clone();
        move |ev| {
            let _ = tx.send(ev.path);
        }
    }

    /// Subscribe to every remote participant's messages prefix, awaiting each.
    /// For test / awaitable contexts; the live window uses [`start`](Self::start).
    /// Direct arm only: on the Worker arm `direct_peer_context` yields
    /// `WorkerArm` and we skip (poll covers delivery there).
    pub async fn subscribe(&self, peers: &Peers) {
        for participant in &self.remote {
            let ctx = match peers.direct_peer_context(&self.local_pid) {
                Ok(ctx) => ctx,
                Err(_) => return,
            };
            let prefix = conversation_messages_prefix(participant, &self.conversation_id);
            match ctx
                .subscribe_at(participant.clone(), format!("{prefix}*"), self.notify_cb())
                .await
            {
                Ok(handle) => self.subs.borrow_mut().push(handle),
                Err(e) => {
                    tracing::warn!(peer = %participant, error = %e, "chat delivery: subscribe_at failed")
                }
            }
        }
    }

    /// Start reactive delivery from the frame loop (wasm, Direct arm): spawn each
    /// participant's subscribe off the loop and land the handle in the shared
    /// `subs`. No-op on the Worker arm — [`pump`](Self::pump)'s poll covers it.
    #[cfg(target_arch = "wasm32")]
    pub fn start(&self, peers: &Peers) {
        for participant in &self.remote {
            let ctx = match peers.direct_peer_context(&self.local_pid) {
                Ok(ctx) => ctx,
                Err(_) => return,
            };
            let prefix = conversation_messages_prefix(participant, &self.conversation_id);
            let fut = ctx.subscribe_at(participant.clone(), format!("{prefix}*"), self.notify_cb());
            let subs = self.subs.clone();
            let participant = participant.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fut.await {
                    Ok(handle) => subs.borrow_mut().push(handle),
                    Err(e) => {
                        tracing::warn!(peer = %participant, error = %e, "chat delivery: subscribe_at failed")
                    }
                }
            });
        }
    }

    /// One delivery tick — call each frame. (a) throttled poll-list of each
    /// remote's messages prefix, (b) fetch each newly-discovered path, (c) cache
    /// completed fetches into our own store. Cheap when idle. Idempotent: a path
    /// is fetched at most once (`seen`), and the cache write is content-addressed.
    pub fn pump(&mut self, peers: &Peers) {
        // (a) throttled poll: list each remote's prefix over the connection
        // (works on both arms / any transport). Discovered paths join the same
        // queue the subscribe notifications use.
        self.poll_tick = self.poll_tick.wrapping_add(1);
        if self.poll_tick >= POLL_EVERY {
            self.poll_tick = 0;
            for participant in &self.remote {
                let prefix = conversation_messages_prefix(participant, &self.conversation_id);
                let list = peers.execute(
                    &self.local_pid,
                    format!("entity://{participant}/system/tree"),
                    "get".to_string(),
                    empty_params(),
                    resource_opts(&prefix),
                );
                let sink = self.notified_tx.clone();
                spawn(async move {
                    if let Ok(hr) = list.await {
                        if hr.status == 200 {
                            for name in listing_child_names(&hr.result) {
                                let _ = sink.send(format!("{prefix}{name}"));
                            }
                        }
                    }
                });
            }
        }

        // (b) drain discovered paths → spawn a cross-peer fetch for each new one.
        // A path already cached (`seen`) or already being fetched (`in_flight`)
        // is skipped; everything else spawns a fetch that reports back success OR
        // failure, so a transient failure releases the path for a poll retry.
        while let Ok(path) = self.notified_rx.try_recv() {
            if self.seen.contains(&path) || !self.in_flight.insert(path.clone()) {
                continue;
            }
            let Some(author) = author_of(&path) else {
                self.in_flight.remove(&path);
                continue;
            };
            let fetch = peers.execute(
                &self.local_pid,
                format!("entity://{author}/system/tree"),
                "get".to_string(),
                empty_params(),
                resource_opts(&path),
            );
            let sink = self.fetched_tx.clone();
            spawn(async move {
                match fetch.await {
                    Ok(hr) if hr.status == 200 => {
                        let _ = sink.send((path, Some(hr.result)));
                    }
                    // Non-200 or transport error: report failure so `pump`
                    // releases the in-flight guard and the poll can retry.
                    _ => {
                        let _ = sink.send((path, None));
                    }
                }
            });
        }

        // (c) drain fetch outcomes. Release the in-flight guard either way; a
        // success caches under /{author}/… in OUR store and is marked `seen`
        // (fetch-once); a failure leaves the path unseen so the poll re-lists it.
        while let Ok((path, result)) = self.fetched_rx.try_recv() {
            self.in_flight.remove(&path);
            if let Some(entity) = result {
                if self.seen.insert(path.clone()) {
                    peers.dispatch_write(&self.local_pid, path, entity);
                }
            }
        }
    }
}

/// The authoring peer-id of a message path `/{author}/app/chat/{conv}/messages/…`.
fn author_of(path: &str) -> Option<String> {
    path.strip_prefix('/')?.split('/').next().map(str::to_string)
}

/// Empty params entity for a `system/tree:get` (the path travels in the resource).
fn empty_params() -> Entity {
    Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null)).unwrap()
}

/// `ExecuteOptions` targeting a single path/prefix via the resource.
fn resource_opts(target: &str) -> ExecuteOptions {
    ExecuteOptions {
        resource: Some(ResourceTarget {
            targets: vec![target.to_string()],
            exclude: vec![],
        }),
        ..Default::default()
    }
}

/// The child names (message-hash leaf segments) of a `system/tree` listing
/// entity (`{count, entries: {name: {..}}, offset, path}`). Empty on any
/// non-listing / malformed entity — never panics.
fn listing_child_names(entity: &Entity) -> Vec<String> {
    let Ok(value) = ciborium::from_reader::<ciborium::Value, _>(entity.data.as_slice()) else {
        return Vec::new();
    };
    let Some(map) = value.as_map() else {
        return Vec::new();
    };
    for (k, v) in map {
        if k.as_text() == Some("entries") {
            if let Some(entries) = v.as_map() {
                return entries
                    .iter()
                    .filter_map(|(name, _)| name.as_text().map(str::to_string))
                    .collect();
            }
        }
    }
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    tokio::spawn(f);
}

#[cfg(target_arch = "wasm32")]
fn spawn<F: std::future::Future<Output = ()> + 'static>(f: F) {
    wasm_bindgen_futures::spawn_local(f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_of_extracts_the_leading_peer_segment() {
        assert_eq!(
            author_of("/2KAlice/app/chat/conv1/messages/ecfv1-sha256:beef").as_deref(),
            Some("2KAlice")
        );
        assert_eq!(author_of("no-leading-slash").as_deref(), None);
    }

    #[test]
    fn listing_child_names_parses_a_tree_listing() {
        // The shape core/tree's handle_listing emits.
        use entity_ecf::{integer, text, to_ecf, Value};
        let data = to_ecf(&Value::Map(vec![
            (text("count"), integer(2)),
            (
                text("entries"),
                Value::Map(vec![
                    (text("ecfv1-sha256:aaaa"), Value::Map(vec![])),
                    (text("ecfv1-sha256:bbbb"), Value::Map(vec![])),
                ]),
            ),
            (text("offset"), integer(0)),
            (text("path"), text("/p/app/chat/c/messages/")),
        ]));
        let entity = Entity::new("system/tree/listing", data).unwrap();
        let mut names = listing_child_names(&entity);
        names.sort();
        assert_eq!(names, vec!["ecfv1-sha256:aaaa", "ecfv1-sha256:bbbb"]);

        assert!(listing_child_names(&Entity::new("x", vec![0xff]).unwrap()).is_empty());
    }
}
