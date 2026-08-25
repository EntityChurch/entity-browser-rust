//! The **connector registry** — the durable, user-editable list of signaling
//! nodes this app may rendezvous through, and the selection among them.
//!
//! *"I run my own connector, or a community runs one; once I'm on it I connect
//! by peer id."* That intuition is `EXTENSION-SIGNALING` §2.2 and it is already
//! implemented upstream; what was missing at the app tier is somewhere to *put*
//! more than one node. Before this, [`crate::session_config::WebRtcProvisioning`]
//! held **exactly one** node, sourced from a build knob or a URL param — never
//! persisted, never user-editable (`REVIEW-CONNECTIVITY-LAYER-COHERENCE-2026-08-11`
//! §5.5).
//!
//! Durable record: one entity per node at
//! `/{peer}/app/entity-browser/connectors/{node_peer_id}`
//! ([`app_paths::connector_path`]), plus a single selection entity naming which
//! one provisioning should use. Same shape as any other peer — peer-id +
//! address — because §3 of that review is that a connector *is* just a peer we
//! know how to reach.
//!
//! ## Both halves or nothing (D3, fail closed)
//!
//! A node is `(peer_id, addr)` and **neither half is optional**: an address
//! without an id is an unauthenticated rendezvous, and an id without an address
//! has nowhere to dial. That rule already exists in
//! [`crate::session_config::resolve_webrtc_provisioning`] and this module
//! deliberately resolves *through* it rather than restating it, so there is one
//! place that can decide a provisioning is usable.
//!
//! ## Why there is a localStorage mirror
//!
//! Provisioning is consumed at **worker-Init time** — before any peer exists,
//! so before the tree can be read at all. (On the Worker arm a boot-time tree
//! read is doubly impossible: the cache mirror is only fed by subscriptions
//! that haven't been made yet.) The durable tree entity is the source of truth;
//! [`write_selection_mirror`] copies the resolved selection into localStorage so
//! the pre-peer boot path can honour it, exactly as `boot_fast_paint` does for
//! the fast-paint kill switch. The mirror is a **cache, never the authority** —
//! it is rewritten from the tree whenever the registry syncs.
//!
//! ## Precedence
//!
//! [`crate::session_config::webrtc_provisioning_from_query`] (URL) beats this,
//! this beats the build knob:
//!
//! ```text
//!   ?webrtc_node_*        dev / showcase / e2e — dynamic, never persisted
//!   > connector selection the user's choice, durable
//!   > ENTITY_WEBRTC_NODE_* the deployment default baked at build time
//! ```
//!
//! The URL staying on top is load-bearing for the harness: `make e2e-webrtc-chat`
//! hands each browser a signaling node on a **dynamic port**, which no durable
//! or build-time value can predict. The user's durable choice beating the build
//! knob is the actual point of the feature — otherwise "my connector" loses to
//! whatever the deployment was compiled with.

use entity_entity::Entity;

use crate::app_paths;
use crate::peers::Peers;
use crate::session_config::{resolve_webrtc_provisioning, WebRtcProvisioning};

/// Entity type for a persisted connector.
pub const CONNECTOR_TYPE: &str = "app/state/connector";

/// Entity type for the connector selection.
pub const CONNECTOR_SELECTION_TYPE: &str = "app/state/connector-selection";

/// localStorage key carrying the resolved selection for the pre-peer boot read.
/// Value is `"{node_peer_id}\u{1f}{node_addr}"` — the same `\x1f` packing the
/// app uses for multi-field values elsewhere, so it needs no JSON parser on the
/// boot path.
pub const SELECTION_MIRROR_KEY: &str = "entity_connector";

/// One connector: a signaling node the user has chosen to know about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
    /// The node's peer-id. Also its key in the registry.
    pub node_peer_id: String,
    /// The node's browser-reachable address (`ws://` / `wss://`). A browser
    /// cannot open a raw TCP socket, so a TCP-only node is unreachable here —
    /// the same constraint `WebRtcProvisioning::node_addr` documents.
    pub node_addr: String,
    /// Human label ("my box", "community node"). Free text, may be empty; the
    /// peer-id is the identity, this is only for the UI.
    pub label: String,
}

/// Reject a node peer-id that could not be a single safe path segment.
///
/// The registry keys entities by peer-id, so a value containing `/` would write
/// somewhere other than where the caller believes — path-shape validation, not
/// cryptographic validation. Emptiness and whitespace are rejected for the same
/// reason. We deliberately do NOT check Base58 or length: a peer-id format is
/// the kernel's business, and a stricter check here would reject a legitimate
/// id the moment the format widens.
pub fn validate_node_peer_id(id: &str) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("a connector needs a node peer-id".to_string());
    }
    if id.contains('/') || id.contains(char::is_whitespace) {
        return Err("a node peer-id cannot contain '/' or whitespace".to_string());
    }
    Ok(())
}

/// Encode a connector as its tree entity.
pub fn connector_to_entity(c: &Connector) -> Entity {
    let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
        "node_peer_id" => entity_ecf::text(&c.node_peer_id),
        "node_addr" => entity_ecf::text(&c.node_addr),
        "label" => entity_ecf::text(&c.label)
    });
    Entity::new(CONNECTOR_TYPE, data).unwrap()
}

/// Decode a persisted connector. `None` (caller warns) for anything malformed —
/// a bad row is skipped, never panics in frame.
pub fn connector_from_entity(entity: &Entity) -> Option<Connector> {
    let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let map = value.as_map()?;
    let field = |name: &str| -> Option<String> {
        map.iter()
            .find(|(k, _)| k.as_text() == Some(name))
            .and_then(|(_, v)| v.as_text())
            .map(str::to_string)
    };
    let node_peer_id = field("node_peer_id")?;
    let node_addr = field("node_addr")?;
    // A row whose id isn't path-safe can't have been written by `add`; treat it
    // as malformed rather than trusting it into a path.
    validate_node_peer_id(&node_peer_id).ok()?;
    Some(Connector {
        node_peer_id,
        node_addr,
        label: field("label").unwrap_or_default(),
    })
}

/// Encode the selection (which node peer-id provisioning should use).
fn selection_to_entity(node_peer_id: &str) -> Entity {
    let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
        "node_peer_id" => entity_ecf::text(node_peer_id)
    });
    Entity::new(CONNECTOR_SELECTION_TYPE, data).unwrap()
}

/// Decode the selection entity.
fn selection_from_entity(entity: &Entity) -> Option<String> {
    let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let map = value.as_map()?;
    map.iter()
        .find(|(k, _)| k.as_text() == Some("node_peer_id"))
        .and_then(|(_, v)| v.as_text())
        .map(str::to_string)
}

/// Every connector in the registry, sorted by peer-id (stable render order).
///
/// **Worker arm:** this is the sync mirror read, so a caller must have the
/// connectors prefix subscribed — [`ConnectorRegistry`] is what does that, and
/// is why the app holds one for its lifetime rather than listing on demand.
pub fn read_connectors(peers: &Peers, peer_id: &str) -> Vec<Connector> {
    let prefix = app_paths::connectors_prefix(app_paths::APP_ID, peer_id);
    let mut entries = peers.tree_listing(peer_id, &prefix);
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries
        .into_iter()
        .filter_map(|e| {
            let entity = peers.get_entity(peer_id, &e.path)?;
            match connector_from_entity(&entity) {
                Some(c) => Some(c),
                None => {
                    tracing::warn!(path = %e.path, "connectors: skipping malformed entry");
                    None
                }
            }
        })
        .collect()
}

/// The selected connector, or `None` when nothing is selected or the selection
/// names a node that is no longer in the registry.
///
/// A dangling selection resolves to `None` rather than to some other row: a
/// silent fallback to a *different* node is the "silent never-meet" failure
/// §2.2 warns about — both peers must land on the same node, so guessing one is
/// worse than having none.
pub fn selected_connector(peers: &Peers, peer_id: &str) -> Option<Connector> {
    let path = app_paths::connector_selection_path(app_paths::APP_ID, peer_id);
    let chosen = peers.get_entity(peer_id, &path).and_then(|e| selection_from_entity(&e))?;
    let found = read_connectors(peers, peer_id).into_iter().find(|c| c.node_peer_id == chosen);
    if found.is_none() {
        tracing::warn!(
            selected = %chosen,
            "connectors: selection names a node that is not in the registry — \
             treating as unselected rather than silently using another"
        );
    }
    found
}

/// Add (or overwrite) a connector. Keyed by node peer-id, so re-adding the same
/// node updates it instead of creating a duplicate pointing at one node.
///
/// Rejects a node with a missing half — see the module doc. Returns the reason
/// so a caller with a surface can report it (D13: a refusal must be sayable).
pub fn add_connector(peers: &Peers, peer_id: &str, c: &Connector) -> Result<(), String> {
    validate_node_peer_id(&c.node_peer_id)?;
    if c.node_addr.trim().is_empty() {
        return Err("a connector needs an address to dial".to_string());
    }
    let normalized = Connector {
        node_peer_id: c.node_peer_id.trim().to_string(),
        node_addr: c.node_addr.trim().to_string(),
        label: c.label.trim().to_string(),
    };
    let path = app_paths::connector_path(app_paths::APP_ID, peer_id, &normalized.node_peer_id);
    peers.dispatch_write(peer_id, path, connector_to_entity(&normalized));
    Ok(())
}

/// Remove a connector. Also clears the selection when it named this node — a
/// selection pointing at a deleted row is exactly the dangling case above, and
/// leaving it behind would keep warning on every read.
pub fn remove_connector(peers: &Peers, peer_id: &str, node_peer_id: &str) {
    let path = app_paths::connector_path(app_paths::APP_ID, peer_id, node_peer_id);
    peers.dispatch_remove(peer_id, path);
    let sel_path = app_paths::connector_selection_path(app_paths::APP_ID, peer_id);
    let selected_this = peers
        .get_entity(peer_id, &sel_path)
        .and_then(|e| selection_from_entity(&e))
        .is_some_and(|s| s == node_peer_id);
    if selected_this {
        peers.dispatch_remove(peer_id, sel_path);
    }
}

/// Select a connector by node peer-id. Selecting a node that isn't in the
/// registry is refused rather than written — a selection that resolves to
/// nothing is indistinguishable from none, so it should fail where the user can
/// see it.
pub fn select_connector(peers: &Peers, peer_id: &str, node_peer_id: &str) -> Result<(), String> {
    if !read_connectors(peers, peer_id).iter().any(|c| c.node_peer_id == node_peer_id) {
        return Err(format!("no connector with peer-id {node_peer_id} in the registry"));
    }
    let path = app_paths::connector_selection_path(app_paths::APP_ID, peer_id);
    peers.dispatch_write(peer_id, path, selection_to_entity(node_peer_id));
    Ok(())
}

/// The durable half of the provisioning precedence: the selected connector as a
/// [`WebRtcProvisioning`], or `None`.
///
/// Resolves through [`resolve_webrtc_provisioning`] so the both-halves-required
/// rule lives in exactly one place.
pub fn provisioning_from_registry(peers: &Peers, peer_id: &str) -> Option<WebRtcProvisioning> {
    let c = selected_connector(peers, peer_id)?;
    resolve_webrtc_provisioning(Some(&c.node_peer_id), Some(&c.node_addr))
}

/// Pack a selection for the localStorage mirror. Separated from the write so it
/// is testable natively — the write itself is wasm-only.
pub fn pack_mirror(p: &WebRtcProvisioning) -> String {
    format!("{}\u{1f}{}", p.node_peer_id, p.node_addr)
}

/// Parse a mirror value written by [`pack_mirror`]. Fails closed on anything
/// that isn't exactly two non-empty halves — a half-written mirror must not
/// become a half-provisioned rendezvous.
pub fn unpack_mirror(raw: &str) -> Option<WebRtcProvisioning> {
    let (id, addr) = raw.split_once('\u{1f}')?;
    resolve_webrtc_provisioning(Some(id), Some(addr))
}

// ---------------------------------------------------------------------------
// advertise() — asking a node what it actually serves
// ---------------------------------------------------------------------------

/// Ask a connector for its endpoint, limits, and lobby constant
/// (`system/signaling:advertise`).
///
/// **Why this is worth a round-trip rather than an assumption.** A node may
/// *override* the lobby constant; a peer that derives from `LOBBY_DEFAULT`
/// anyway lands in a bucket nobody else on that pool uses — and nothing errors.
/// Both peers simply never meet (`EXTENSION-SIGNALING` §2.2 Finding B: "per
/// deployment" without a named default is a bug, not a policy). So **any lobby
/// UI must call this first and derive from what the node returns**, and this is
/// that call.
///
/// Dispatched through the app's own cross-arm router rather than
/// `SignalingClient`, for two reasons: that client is native-only
/// (`#[cfg(not(target_arch = "wasm32"))]`, so it does not exist in the browser
/// — the surface this feature is for), and it takes a `&dyn Dispatcher`, which
/// a Worker-arm peer cannot hand the main thread. Calling a node is an ordinary
/// EXECUTE, which the router already does on both arms. The reply is decoded
/// with the extension's own `advertisement_from_params`, so the wire shape has
/// exactly one reader.
///
/// Returns an owned future (no borrow on `Peers`), so a caller can `spawn_local`
/// it from a sync handler — the pattern `delete_site`'s neighbours use.
///
/// `local_peer_id` is the peer whose connection pool and grants the dispatch
/// rides; the node is addressed by `entity://` URI, so it is reached the same
/// way any other remote peer is — never via the local peer registry.
pub fn advertise(
    peers: &Peers,
    local_peer_id: &str,
    node_peer_id: &str,
) -> impl std::future::Future<Output = Result<entity_signaling::Advertisement, String>> + 'static {
    // Same empty-map params the extension's own client sends.
    let params = Entity::new(
        "system/signaling/empty",
        entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![])),
    );
    let uri = format!("entity://{}/{}", node_peer_id, entity_signaling::PATTERN);
    let node = node_peer_id.to_string();
    let fut = params.map(|params| {
        peers.execute(
            local_peer_id,
            uri,
            entity_signaling::OP_ADVERTISE.to_string(),
            params,
            entity_handler::ExecuteOptions::default(),
        )
    });
    async move {
        let result = match fut {
            Ok(f) => f.await?,
            Err(e) => return Err(format!("advertise: encoding empty params failed: {e}")),
        };
        // A node that answers with a non-OK status is refusing, not advertising
        // — surface the status rather than trying to decode the body.
        if result.status != entity_handler::STATUS_OK {
            return Err(format!(
                "advertise: node {node} refused with status {}",
                result.status
            ));
        }
        entity_signaling::data::advertisement_from_params(&result.result.data)
            .map_err(|e| format!("advertise: node {node} sent an undecodable advertisement: {e}"))
    }
}

/// Make sure `local_peer_id` can actually reach `c`, dialing the node's
/// registered address when nothing is connected to it yet.
///
/// **Why every node call needs this.** A node is reached by `entity://` URI
/// like any other remote peer, so the dispatch ladder has to find a route:
/// rung 1 is a pooled connection and rung 2 a published transport profile.
/// A connector the user just added has neither — nothing has ever dialed it —
/// and rung 4 (live establishment) is circular for the rendezvous node itself.
/// So an EXECUTE against a freshly added connector fails with *no transport
/// profile for peer*, which reads as "the node is down" when the truth is that
/// we never called it. The address is sitting in the registry row; this dials
/// it. `connect_peer` publishes the profile on success, so later calls in the
/// session route without a second dial.
///
/// Skipped when the kernel liveness read-model already says `connected` — that
/// surface is authoritative for whether a link is up (Amendment 12), and
/// `connect_peer` establishes a genuinely new connection every time it is
/// called, so dialing an already-connected node would pile up pooled
/// connections one per button press.
pub fn reach_node(
    peers: &Peers,
    local_peer_id: &str,
    c: &Connector,
) -> impl std::future::Future<Output = Result<(), String>> + 'static {
    let connected = crate::peer_liveness::liveness_of(peers, &c.node_peer_id).is_connected();
    let dial = if connected {
        None
    } else {
        Some(peers.connect_peer(local_peer_id, c.node_addr.clone()))
    };
    let node = c.node_peer_id.clone();
    let addr = c.node_addr.clone();
    async move {
        let Some(dial) = dial else { return Ok(()) };
        match dial.await {
            Ok(reached) => {
                // The address is the identity we hold; a node answering under a
                // different peer-id is not the node the user selected, and
                // rendezvousing through it would put us in a bucket at a
                // stranger's pool. Report rather than proceed.
                if reached != node {
                    return Err(format!(
                        "connector: {addr} answered as {reached}, not the {node} in the registry"
                    ));
                }
                Ok(())
            }
            Err(e) => Err(format!("connector: dialing {addr} failed: {e}")),
        }
    }
}

/// The lobby constant to derive with at a node — the node's override when it
/// published one, otherwise the protocol default.
///
/// The whole point of [`advertise`]: deriving from `LOBBY_DEFAULT` at a node
/// that overrode it is the silent never-meet, so a lobby-mode connect must go
/// through here rather than reaching for the constant directly.
pub fn lobby_constant_for(ad: &entity_signaling::Advertisement) -> &str {
    ad.limits.lobby_constant.as_deref().unwrap_or(entity_signaling::LOBBY_DEFAULT)
}

// ---------------------------------------------------------------------------
// The pre-peer mirror + the registry watch
// ---------------------------------------------------------------------------

/// Copy the resolved selection into localStorage (or clear it when nothing is
/// selected), so the pre-peer boot path can provision from it. Same idiom as
/// `boot_fast_paint::write_enabled_mirror`.
///
/// Clearing on `None` matters: a stale mirror would keep provisioning a node
/// the user has already removed, and it would do so *silently*, since nothing
/// on the boot path can consult the tree to notice the disagreement.
#[cfg(target_arch = "wasm32")]
pub fn write_selection_mirror(p: Option<&WebRtcProvisioning>) {
    if let Some(Ok(Some(storage))) = web_sys::window().map(|w| w.local_storage()) {
        match p {
            Some(p) => {
                let _ = storage.set_item(SELECTION_MIRROR_KEY, &pack_mirror(p));
            }
            None => {
                let _ = storage.remove_item(SELECTION_MIRROR_KEY);
            }
        }
    }
}

/// Read the pre-peer mirror. `None` when absent or half-written.
#[cfg(target_arch = "wasm32")]
pub fn read_selection_mirror() -> Option<WebRtcProvisioning> {
    let storage = web_sys::window()?.local_storage().ok()??;
    let raw = storage.get_item(SELECTION_MIRROR_KEY).ok()??;
    unpack_mirror(&raw)
}

/// Resolve the §6.5 provisioning for this session, applying the module's
/// precedence: **URL > durable connector selection > build knob.**
///
/// One resolver, called from both provisioning sites (the Worker `InitParams`
/// and the Direct-arm establisher), because those two sourcing the node
/// differently is precisely the class of bug that makes one arm silently
/// rendezvous somewhere the other doesn't.
///
/// Logs which source won — with a node this is the only D13 surface for *why*
/// we are dialing the node we are dialing.
#[cfg(target_arch = "wasm32")]
pub fn resolve_provisioning(url_query: &str) -> Option<WebRtcProvisioning> {
    let (p, source) = resolve_provisioning_quietly(url_query)?;
    tracing::info!(node_peer_id = %p.node_peer_id, "webrtc: provisioning from {source}");
    Some(p)
}

/// The precedence itself, with **no logging** — so a surface may ask "what would
/// a reload resolve right now?" every frame without flooding the log.
///
/// Split out rather than duplicated deliberately: two expressions of this
/// precedence is the exact bug class [`resolve_provisioning`]'s doc warns about,
/// so there is still only one, and the logging wrapper is the only difference.
#[cfg(target_arch = "wasm32")]
pub fn resolve_provisioning_quietly(
    url_query: &str,
) -> Option<(WebRtcProvisioning, &'static str)> {
    if let Some(p) = crate::session_config::webrtc_provisioning_from_query(url_query) {
        return Some((p, "URL query"));
    }
    if let Some(p) = read_selection_mirror() {
        return Some((p, "the selected connector (durable registry)"));
    }
    let p = crate::session_config::webrtc_provisioning_default()?;
    Some((p, "the build knob"))
}

/// Has the provisioning a reload would use drifted from what this session
/// actually booted with?
///
/// **Why compare resolved values rather than watching the selection.** The
/// registry is only *one* of three sources, and the URL outranks it
/// ([`resolve_provisioning`]). A notice wired to "the selection changed" would
/// tell a user booted with `?webrtc_node=…` to reload to apply a choice that a
/// reload will keep ignoring — worse than silence, because it is a promise the
/// app cannot keep. Running both sides through the same resolver makes URL
/// precedence self-handling: when the URL wins, `now` never differs from
/// `booted`, and no notice appears without a line of code saying so.
///
/// It also generalizes past the node. The comparison is over the whole
/// [`WebRtcProvisioning`], so anything later added to it — ICE servers from a
/// node's §4.5.1 advertisement being the live case — inherits the notice
/// without touching this function.
///
/// Pure and native-testable; the wasm callers supply the two sides.
pub fn provisioning_drifted(
    booted: Option<&WebRtcProvisioning>,
    reload_would_use: Option<&WebRtcProvisioning>,
) -> bool {
    booted != reload_would_use
}

/// Keeps the localStorage mirror in step with the durable registry.
///
/// The app holds ONE of these for its lifetime. Its [`WindowWatch`] subscribes
/// the connectors prefix **and** the selection path — on the Worker arm that
/// subscription is also what feeds the cache mirror, so without it
/// [`read_connectors`] returns silently empty and the registry would look empty
/// to every reader (the worker-cache rule). [`ConnectorRegistry::sync`] runs each
/// frame but is a single atomic-bool check when nothing changed.
pub struct ConnectorRegistry {
    watch: crate::window_watch::WindowWatch,
    peer_id: String,
}

impl ConnectorRegistry {
    pub fn new(peers: &Peers) -> Self {
        let peer_id = peers.system_peer_id().to_string();
        let mut watch = crate::window_watch::WindowWatch::new();
        peers.watch_prefix(
            &mut watch,
            &peer_id,
            app_paths::connectors_prefix(app_paths::APP_ID, &peer_id),
        );
        // The selection lives outside the connectors prefix (so a listing never
        // has to filter it out), which means it needs its own watch — otherwise
        // choosing a different connector would update nothing until some
        // unrelated registry change happened to dirty this.
        peers.watch_prefix(
            &mut watch,
            &peer_id,
            app_paths::connector_selection_path(app_paths::APP_ID, &peer_id),
        );
        Self { watch, peer_id }
    }

    /// Reconcile the mirror from the tree when the registry or the selection
    /// changed. Cheap no-op otherwise.
    pub fn sync(&mut self, peers: &Peers) {
        if !self.watch.take_dirty() {
            return;
        }
        let resolved = provisioning_from_registry(peers, &self.peer_id);
        #[cfg(target_arch = "wasm32")]
        write_selection_mirror(resolved.as_ref());
        tracing::debug!(
            selected = resolved.as_ref().map(|p| p.node_peer_id.as_str()).unwrap_or("(none)"),
            "connectors: mirror reconciled from the durable registry"
        );
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn conn(id: &str, addr: &str) -> Connector {
        Connector { node_peer_id: id.to_string(), node_addr: addr.to_string(), label: String::new() }
    }

    /// Wait until `done` holds. Writes here go through `dispatch_write` (L1,
    /// the dispatched-and-authz-checked boundary — the same one
    /// `user_themes` uses in production), so they land a dispatch later rather
    /// than in-line.
    ///
    /// Polled rather than a fixed `sleep(30ms)`: a fixed sleep is a guess about
    /// how long a dispatch takes, which is the shape that fails on a loaded box
    /// and passes everywhere else. This returns on the first check in the
    /// healthy case and gives the failing assertion a real bound.
    async fn settle(mut done: impl FnMut() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    #[test]
    fn a_connector_round_trips_through_its_entity() {
        let c = Connector {
            node_peer_id: "2KNode".to_string(),
            node_addr: "ws://10.0.0.4:9000".to_string(),
            label: "my box".to_string(),
        };
        assert_eq!(connector_from_entity(&connector_to_entity(&c)), Some(c));
    }

    #[test]
    fn a_node_peer_id_that_is_not_a_safe_path_segment_is_refused() {
        // The registry keys entities by this value, so a `/` would write
        // somewhere other than the caller believes.
        assert!(validate_node_peer_id("a/b").is_err());
        assert!(validate_node_peer_id(" spaced id").is_err());
        assert!(validate_node_peer_id("").is_err());
        assert!(validate_node_peer_id("2KLegitNodeId").is_ok());
    }

    #[tokio::test]
    async fn add_list_select_and_remove_round_trip_through_the_tree() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        assert!(read_connectors(&peers, &me).is_empty());

        add_connector(&peers, &me, &conn("2KBeta", "ws://b:1")).unwrap();
        add_connector(&peers, &me, &conn("2KAlpha", "ws://a:1")).unwrap();
        settle(|| read_connectors(&peers, &me).len() == 2).await;
        let list = read_connectors(&peers, &me);
        // Sorted by peer-id, so render order is stable across reloads.
        assert_eq!(
            list.iter().map(|c| c.node_peer_id.as_str()).collect::<Vec<_>>(),
            vec!["2KAlpha", "2KBeta"]
        );

        assert!(selected_connector(&peers, &me).is_none(), "nothing selected yet");
        select_connector(&peers, &me, "2KBeta").unwrap();
        settle(|| selected_connector(&peers, &me).is_some()).await;
        assert_eq!(selected_connector(&peers, &me).unwrap().node_addr, "ws://b:1");

        remove_connector(&peers, &me, "2KBeta");
        settle(|| read_connectors(&peers, &me).len() == 1).await;
        assert_eq!(read_connectors(&peers, &me).len(), 1);
        settle(|| selected_connector(&peers, &me).is_none()).await;
        assert!(
            selected_connector(&peers, &me).is_none(),
            "removing the selected node clears the selection instead of leaving it dangling"
        );
    }

    #[tokio::test]
    async fn re_adding_the_same_node_updates_it_rather_than_duplicating() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        add_connector(&peers, &me, &conn("2KNode", "ws://old:1")).unwrap();
        add_connector(&peers, &me, &conn("2KNode", "ws://new:2")).unwrap();
        settle(|| {
            read_connectors(&peers, &me).first().is_some_and(|c| c.node_addr == "ws://new:2")
        })
        .await;
        let list = read_connectors(&peers, &me);
        assert_eq!(list.len(), 1, "one node, one row");
        assert_eq!(list[0].node_addr, "ws://new:2");
    }

    #[test]
    fn a_connector_missing_either_half_is_refused() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        assert!(add_connector(&peers, &me, &conn("2KNode", "")).is_err(), "no address");
        assert!(add_connector(&peers, &me, &conn("", "ws://a:1")).is_err(), "no peer-id");
        assert!(read_connectors(&peers, &me).is_empty(), "a refusal writes nothing");
    }

    #[test]
    fn selecting_an_unknown_node_is_refused_not_written() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        assert!(select_connector(&peers, &me, "2KGhost").is_err());
        assert!(selected_connector(&peers, &me).is_none());
    }

    /// A selection left pointing at a node that vanished must resolve to
    /// *nothing*, never to another row: both peers have to land on the same
    /// node, so silently substituting one is the §2.2 "silent never-meet".
    #[tokio::test]
    async fn a_dangling_selection_resolves_to_none_not_to_another_node() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        add_connector(&peers, &me, &conn("2KOnly", "ws://only:1")).unwrap();
        settle(|| !read_connectors(&peers, &me).is_empty()).await;
        select_connector(&peers, &me, "2KOnly").unwrap();
        settle(|| selected_connector(&peers, &me).is_some()).await;
        // Remove it the way something else in the tree would — straight at the
        // path, so the selection is left behind.
        peers.dispatch_remove(
            &me,
            app_paths::connector_path(app_paths::APP_ID, &me, "2KOnly"),
        );
        add_connector(&peers, &me, &conn("2KOther", "ws://other:1")).unwrap();
        settle(|| read_connectors(&peers, &me).iter().any(|c| c.node_peer_id == "2KOther")).await;
        assert!(selected_connector(&peers, &me).is_none());
        assert!(provisioning_from_registry(&peers, &me).is_none());
    }

    #[tokio::test]
    async fn the_registry_resolves_to_provisioning_through_the_shared_rule() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        add_connector(&peers, &me, &conn("2KNode", "ws://n:9")).unwrap();
        settle(|| !read_connectors(&peers, &me).is_empty()).await;
        select_connector(&peers, &me, "2KNode").unwrap();
        settle(|| provisioning_from_registry(&peers, &me).is_some()).await;
        let p = provisioning_from_registry(&peers, &me).unwrap();
        assert_eq!(p.node_peer_id, "2KNode");
        assert_eq!(p.node_addr, "ws://n:9");
        // Left inert here exactly as the build-knob path leaves them — an empty
        // ICE list means host-candidates-only, never "use a public default".
        assert!(p.ice_servers.is_empty());
    }

    /// Spawn a **real signaling node** on the memory transport: an
    /// `entity-peer` with the extension's own `SignalingHandler` mounted, an
    /// overridden lobby constant, and a listener. Returns its peer-id.
    ///
    /// A real node rather than a stub, because the thing worth proving is that
    /// our EXECUTE reaches the extension's handler and that its reply decodes —
    /// a stub would only prove we can talk to ourselves.
    pub(crate) fn spawn_signaling_node(
        registry: std::sync::Arc<entity_peer::transport::MemoryTransportRegistry>,
        lobby_override: &str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use entity_peer::transport::MemoryListener;

        let keypair =
            entity_crypto::IdentityKeypair::Ed25519(entity_crypto::Keypair::from_seed([7u8; 32]));
        let node_pid = keypair.peer_id().to_string();
        let core = std::sync::Arc::new(entity_signaling::SignalingCore::with_limits(
            "memory://node".to_string(),
            entity_signaling::Limits {
                lobby_constant: Some(lobby_override.to_string()),
                ..Default::default()
            },
        ));
        let peer = entity_peer::PeerBuilder::new()
            .identity_keypair(keypair)
            .config(entity_peer::PeerConfig {
                // The dispatch is cap-checked at the node; this test is about
                // the advertise round-trip, not about admission.
                debug_open_grants: true,
                ..Default::default()
            })
            .handler(std::sync::Arc::new(entity_signaling::SignalingHandler::new(
                core,
                node_pid.as_str(),
            )))
            .build()
            .expect("node peer builds");
        let shared = peer.shared();
        peer.start_engines(&shared);
        let listener = MemoryListener::bind(node_pid.clone(), registry).expect("node listens");
        let handle = tokio::spawn(async move {
            let _ = entity_peer::server::run(listener, shared).await;
        });
        (node_pid, handle)
    }

    /// `advertise` reaches a real node and decodes what it actually serves.
    ///
    /// The payoff assertion is the **lobby constant**: this node overrides it,
    /// and a peer that assumed `LOBBY_DEFAULT` would derive a rendezvous key
    /// nobody else on that pool uses — the §2.2 "silent never-meet", which
    /// produces no error anywhere. Asking is the only way to know.
    #[tokio::test]
    async fn advertise_reads_the_nodes_own_lobby_constant_rather_than_assuming() {
        use entity_peer::transport::{MemoryConnector, MemoryTransportRegistry};

        let registry = MemoryTransportRegistry::new();
        let (node_pid, node_handle) = spawn_signaling_node(registry.clone(), "pool-seven");

        let peers = Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(
            registry.clone(),
        )));
        let me = peers.primary_peer_id().to_string();
        // No `start_engines` on our side: this peer only dials out and
        // EXECUTEs. Starting them would mean reaching through
        // `direct_peer_shared`, the Direct-only L0 hatch that
        // `tests/escape_hatch_budget.rs` budgets — and the gate is right that
        // this file has no business using it. The *node* runs its engines; a
        // caller does not need them to dispatch.
        tokio::task::yield_now().await;

        peers
            .connect_peer(&me, format!("memory://{node_pid}"))
            .await
            .expect("dialing the node succeeds");

        let ad = advertise(&peers, &me, &node_pid).await.expect("the node advertises");

        assert_eq!(
            lobby_constant_for(&ad),
            "pool-seven",
            "the node's override must win over LOBBY_DEFAULT — deriving from the \
             default at a node that overrode it is the silent never-meet"
        );
        assert_ne!(lobby_constant_for(&ad), entity_signaling::LOBBY_DEFAULT);
        // The limits came from the node too, not from our assumptions.
        assert!(ad.limits.max_blob_bytes > 0);
        assert!(ad.limits.ttl_seconds > 0);

        node_handle.abort();
    }

    /// **A freshly added connector cannot be called until something dials it**
    /// — and [`reach_node`] is what does.
    ///
    /// The first half is the gap this test exists to pin: the node is up and
    /// listening, its row is in the registry, and `advertise` still fails,
    /// because the dispatch ladder has no route to a peer nobody has ever
    /// connected to. That failure reads as "the node is down"; it isn't. The
    /// second half is the fix, and the two halves must stay in one test — a
    /// `reach_node` that stopped dialing would leave the second passing on the
    /// pooled connection some earlier test left behind.
    #[tokio::test]
    async fn a_fresh_connector_is_unreachable_until_reach_node_dials_it() {
        use entity_peer::transport::{MemoryConnector, MemoryTransportRegistry};

        let registry = MemoryTransportRegistry::new();
        let (node_pid, node_handle) = spawn_signaling_node(registry.clone(), "pool-seven");

        let peers = Peers::new_direct_with_connector(std::sync::Arc::new(MemoryConnector::new(
            registry.clone(),
        )));
        let me = peers.primary_peer_id().to_string();
        tokio::task::yield_now().await;

        let c = Connector {
            node_peer_id: node_pid.clone(),
            node_addr: format!("memory://{node_pid}"),
            label: "the node".to_string(),
        };

        let no_route = advertise(&peers, &me, &node_pid).await;
        assert!(
            no_route.is_err(),
            "a connector nobody has dialed has no route — got {no_route:?}"
        );

        reach_node(&peers, &me, &c).await.expect("reach_node dials the registered address");
        let ad = advertise(&peers, &me, &node_pid).await;
        assert!(ad.is_ok(), "after reach_node the same call works — got {ad:?}");

        node_handle.abort();
    }

    /// A node that isn't there fails as an error the caller can report, rather
    /// than hanging or resolving to a default advertisement.
    #[tokio::test]
    async fn advertise_against_an_unreachable_node_is_an_error_not_a_default() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let err = advertise(&peers, &me, "2KNobodyHome").await;
        assert!(err.is_err(), "got {err:?}");
    }

    /// The reload notice fires when — and only when — a reload would actually
    /// change something.
    ///
    /// Each case below is a state a user can reach in about two clicks, and
    /// three of them are ways a naive "did the selection change?" check gets it
    /// wrong: nagging a session that never had a connector, staying silent when
    /// the *first* connector is chosen, and staying silent when the selected one
    /// is removed out from under a running establisher.
    #[test]
    fn the_reload_notice_fires_only_when_a_reload_would_change_something() {
        let node = |id: &str| WebRtcProvisioning {
            node_peer_id: id.to_string(),
            node_addr: "ws://n:9".to_string(),
            ice_servers: Vec::new(),
            poll_interval_ms: None,
            max_deadline_ms: None,
        };

        // Steady state, and the one that must stay quiet: nothing provisioned,
        // nothing selected. A notice here would be permanent furniture on every
        // default build — which is exactly what a static hint would have been.
        assert!(!provisioning_drifted(None, None), "no connector, no nag");

        // Running on the node we booted with.
        let a = node("2KNodeA");
        assert!(!provisioning_drifted(Some(&a), Some(&a)), "unchanged is quiet");

        // The main case: the user just picked their FIRST connector. Booted with
        // nothing, a reload would now resolve one. This is the state the whole
        // notice exists for, and a check that only watched for a *changed*
        // selection would miss it.
        assert!(provisioning_drifted(None, Some(&a)), "first selection is pending");

        // Switched nodes.
        let b = node("2KNodeB");
        assert!(provisioning_drifted(Some(&a), Some(&b)), "a different node is pending");

        // Removed the selected connector. The session keeps rendezvousing
        // through a node the registry no longer names — still a divergence
        // between what is running and what a reload would do, and still worth
        // saying.
        assert!(provisioning_drifted(Some(&a), None), "a removed selection is pending");

        // The generalization, asserted rather than asserted-in-prose: the
        // comparison is over the WHOLE provisioning, not the node id. When
        // EXTENSION-SIGNALING §4.5.1 reflection endpoints start landing in the
        // mirror, the same node with new ICE servers is a real change that a
        // reload applies — and this notice already covers it with no new code.
        let mut a_with_ice = a.clone();
        a_with_ice.ice_servers = vec![crate::session_config::IceServer {
            urls: vec!["stun:example:3478".to_string()],
            username: None,
            credential: None,
        }];
        assert!(
            provisioning_drifted(Some(&a), Some(&a_with_ice)),
            "same node, new ICE servers — a reload changes the session, so say so"
        );
    }

    #[test]
    fn the_mirror_round_trips_and_fails_closed_on_a_half() {
        let p = WebRtcProvisioning {
            node_peer_id: "2KNode".to_string(),
            node_addr: "ws://n:9".to_string(),
            ice_servers: Vec::new(),
            poll_interval_ms: None,
            max_deadline_ms: None,
        };
        assert_eq!(unpack_mirror(&pack_mirror(&p)), Some(p));
        assert!(unpack_mirror("").is_none());
        assert!(unpack_mirror("2KNodeOnly").is_none(), "an id with no address");
        assert!(unpack_mirror("2KNode\u{1f}").is_none(), "an address that is blank");
        assert!(unpack_mirror("\u{1f}ws://n:9").is_none(), "an address with no id");
    }
}
