//! The **connector registry** — the durable, user-editable list of signaling
//! nodes this app may rendezvous through, and the selection among them.
//!
//! **"Registry" here is local, and it is NOT `EXTENSION-REGISTRY`.** This file
//! is a list of signaling nodes kept on the system peer of *this* app; it
//! performs no `:resolve`, holds no signed service-advertisement set, and knows
//! nothing of §3b. Nothing in `src/` resolves an `EXTENSION-REGISTRY` name
//! registry at all — which is exactly why the connector-entered-by-URL path
//! (`SIGNALING` §13 item 5b) is the one the spec has to serve separately. The
//! two senses collided on that seam and cost arch a re-read
//! (`ROUTING-2026-08-16-d` §4); the name stays because it is right inside this
//! crate, so the disambiguation lives here instead.
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
/// Value is `"{node_peer_id}\u{1f}{node_addr}\u{1f}{ice}"` — the same `\x1f`
/// packing the app uses for multi-field values elsewhere, so it needs no JSON
/// parser on the boot path. The third field is optional on read: a mirror
/// written before reflectors existed is a host-only deployment, which is exactly
/// what it meant.
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
    /// The node's **reflectors** — comma/whitespace-separated `stun:` URIs, as
    /// typed. Empty means host-candidates-only, which is a legal LAN deployment
    /// and is what every build shipped before this field existed.
    ///
    /// **Why it lives on the connector and not in a global setting.** A
    /// reflector is deployment infrastructure belonging to the same operator as
    /// the signaling node: the peer you rendezvous through is the peer who knows
    /// which STUN server is near you. One connector, one place to configure a
    /// rendezvous, and switching nodes switches both halves together — a global
    /// setting would quietly outlive the node it was chosen for.
    ///
    /// Stored as raw text rather than parsed servers so what the user typed
    /// survives a round trip (and so a future URI form does not need a
    /// migration); parsed at every read by
    /// [`crate::session_config::parse_ice_urls`], and refused at
    /// [`add_connector`] so a malformed value never reaches the tree.
    ///
    /// This is the **manual** half. The automatic half is [`Self::ice_advertised`],
    /// and the two are merged by [`merge_reflectors`].
    pub ice: String,
    /// The node's **own** reflectors as it advertised them (`EXTENSION-SIGNALING`
    /// §4.5.1) — space-separated, **verbatim as published**, learned from
    /// [`advertise`] and refreshed on every successful call.
    ///
    /// **Stored, not merely read, because provisioning is consumed at worker
    /// `Init`** — before any node has been dialled. A value only obtainable by
    /// asking the node could not reach the establisher that needs it, so the
    /// answer is cached on the row that produced it and read at boot like the
    /// typed half. §4.5.1 makes this sound explicitly: §9.3 forbids a reflector
    /// requiring authentication, so **there is no credential here to expire**
    /// and a read at connect time has no staleness problem. That is exactly why
    /// the automatic half could ship while the TURN/rotation half waits on
    /// `PROPOSAL-SIGNALING-ICE-PROVISIONING-LIFETIME`.
    ///
    /// **Kept separate from `ice` rather than folded into it.** They have
    /// different owners: the user types one and a node publishes the other, so a
    /// re-advertise must be able to refresh the node's half without touching
    /// what the user chose, and a user editing their half must not silently
    /// inherit authorship of the node's. Merging happens at read.
    ///
    /// **Never a directory.** §4.5.1 is the node describing *its own* §9.3
    /// listener; a peer MUST NOT read it as a set of third-party STUN servers.
    /// Storing it per-connector is what keeps that true — these reflectors are
    /// scoped to the node that published them and die with its row.
    pub ice_advertised: String,
    /// The node operator's **relay** (TURN) URIs, as typed — a separate field
    /// from [`Self::ice`] on purpose.
    ///
    /// A reflector is a commodity: credential-free by spec (§9.3 forbids
    /// reflector authentication) and **node-advertisable**, which is what
    /// [`Self::ice_advertised`] is and why its dedup is defined over published
    /// bytes exactly. A relay is rented, carries credentials, forwards every
    /// packet, and `EXTENSION-REGISTRY` §3b has **no credential channel** — a
    /// node cannot advertise one, so this half is always the user's own.
    ///
    /// Parsed with its credentials by [`crate::session_config::parse_relay`],
    /// and refused at [`add_connector`] if the three fields disagree.
    pub relay: String,
    /// Username for [`Self::relay`]. Stored in this peer's tree in plaintext,
    /// like the rest of the app's configuration — stated rather than implied,
    /// because a TURN credential is usually a shared rotatable secret and this
    /// is not a secret store.
    pub relay_username: String,
    /// Credential for [`Self::relay`]. See [`Self::relay_username`] on storage.
    pub relay_credential: String,
}

/// Merge a node's advertised reflectors into the user's typed ones
/// (`EXTENSION-SIGNALING` §4.5.1: *"a consumer merges rather than replaces"*).
///
/// Typed entries come first, then advertised, **deduplicated by endpoint bytes
/// exactly as published** — no trimming into a different string, no case
/// folding, no default-port canonicalization. The dedup is defined over the
/// published bytes, so normalizing here would silently break it (and would break
/// it *identically* for `EXTENSION-REGISTRY` §3b, whose set shares this form).
///
/// **Merging is safe, not merely permitted:** §9.3 already requires consulting
/// several reflectors and requiring agreement, so more sources strictly improve
/// the NAT-type conclusion. A single reflector is advisory and never trusted,
/// whichever field it arrived in.
///
/// A malformed *advertised* entry is dropped individually and warned about — the
/// node is not the user's to correct, and discarding the user's own working
/// reflectors over a remote typo is the wrong failure. The typed half is passed
/// through untouched because [`add_connector`] already refused it whole.
pub fn merge_reflectors(typed: &str, advertised: &str) -> String {
    let split = |s: &str| -> Vec<String> {
        s.split(|c: char| c == ',' || c.is_whitespace())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    };
    let mut out: Vec<String> = split(typed);
    for u in split(advertised) {
        if let Err(e) = crate::session_config::validate_reflector_uri(&u) {
            tracing::warn!(
                error = %e,
                "connector: dropping one malformed advertised reflector — the node published \
                 an entry that is not the RFC 7064 form; the rest of the list still applies"
            );
            continue;
        }
        // Byte-exact dedup (§4.5.1) — `contains` over the published strings.
        if !out.iter().any(|e| e == &u) {
            out.push(u);
        }
    }
    out.join(" ")
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
        "label" => entity_ecf::text(&c.label),
        "ice" => entity_ecf::text(&c.ice),
        "ice_advertised" => entity_ecf::text(&c.ice_advertised),
        "relay" => entity_ecf::text(&c.relay),
        "relay_username" => entity_ecf::text(&c.relay_username),
        "relay_credential" => entity_ecf::text(&c.relay_credential)
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
        // Absent on every row written before reflectors existed — an empty
        // string is exactly what those deployments meant (host-only), so a
        // missing field is a default, never a malformed row.
        ice: field("ice").unwrap_or_default(),
        // Absent on every row written before §4.5.1 landed, and on every row
        // whose node has not been advertised-to yet. Empty means "this node has
        // told us nothing", which is the same posture as a node that serves no
        // reflection — so a missing field is a default, never a malformed row.
        ice_advertised: field("ice_advertised").unwrap_or_default(),
        // Absent on every row written before relays were configurable. Empty
        // means "no relay", which is what those deployments meant and is the
        // fail-closed reading — so a missing field is a default, never a
        // malformed row. Same rule as `ice` above.
        relay: field("relay").unwrap_or_default(),
        relay_username: field("relay_username").unwrap_or_default(),
        relay_credential: field("relay_credential").unwrap_or_default(),
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
/// The node-id the selection entity *names*, whether or not that node is still
/// in the registry.
///
/// Distinct from [`selected_connector`] on purpose, and the distinction is
/// load-bearing exactly once: a **dangling** selection resolves to no connector
/// and is still an expressed choice. [`add_connector`] must not treat it as
/// "nothing is selected" and quietly repoint the user at a node they have not
/// picked — that is the §2.2 silent-substitution this module refuses everywhere
/// else. So the auto-select asks this, and every reader that wants a usable
/// node asks the other one.
fn selection_marker(peers: &Peers, peer_id: &str) -> Option<String> {
    let path = app_paths::connector_selection_path(app_paths::APP_ID, peer_id);
    peers.get_entity(peer_id, &path).and_then(|e| selection_from_entity(&e))
}

/// **The node this session actually rendezvous through** — the registry row when
/// there is one, otherwise the provisioning in force, synthesized into a row.
///
/// # The bug this exists to close
///
/// Provisioning has three sources (`resolve_provisioning_quietly`: URL query >
/// selected connector > build knob) and **only the middle one writes a registry
/// row**. So a session provisioned by URL or by the build knob installs a
/// working §6.5 establisher and then refuses every operation that asks the
/// registry: `meet` answered *"no connector selected — `connector add …`"* while
/// `net`, one command earlier, printed `OK rendezvous` and `OK establisher`.
/// Two rows of the same report contradicting each other, and the person is told
/// to add the node they already have.
///
/// Reproduced in two real browsers against a real desktop (2026-08-21) and it
/// is not a corner: it is **every** `make pair-serve` build (the whole point of
/// which is that the node is baked in and nobody types one) and **every**
/// browser that loads the desktop's served URL. Both provision by a path that
/// leaves the registry empty.
///
/// # Why synthesize rather than write a row
///
/// A registry row is durable, user-owned state — the thing `connector rm` and
/// the connector list manage. URL provisioning is explicitly *"never persisted"*
/// and the build knob is a property of the binary; materializing either as a row
/// would put a connector the user cannot account for into their list, and
/// re-materialize it after they removed it. The synthesized row is derived,
/// lasts one call, and carries `label` empty so no surface claims the user named
/// it.
///
/// **Callers that manage the registry must keep using [`selected_connector`]** —
/// this is for callers that need *a node to talk to*.
#[cfg(target_arch = "wasm32")]
pub fn node_in_force(peers: &Peers, peer_id: &str) -> Option<Connector> {
    if let Some(c) = selected_connector(peers, peer_id) {
        return Some(c);
    }
    // No row: fall back to what this session BOOTED with, so the node we dial is
    // the node the establisher was actually installed with, by construction.
    //
    // `booted_snapshot`, not a fresh `resolve_provisioning_quietly` — a re-resolve
    // here has no URL to read (the verb holds only `&Peers`), so it would silently
    // skip the highest-precedence source and answer with the build knob or
    // nothing. It would also drift from the installed establisher the moment the
    // selection mirror changed, which is the same "what a reload would use"
    // vacuity `capture_booted` exists to prevent.
    let p = booted_snapshot()?;
    Some(Connector {
        node_peer_id: p.node_peer_id,
        node_addr: p.node_addr,
        label: String::new(),
        // The reflectors ride the provisioning too; they are already merged and
        // parsed there, and re-deriving them here would be a second expression
        // of the merge that could disagree with the one the agent got.
        ice: p
            .ice_servers
            .iter()
            .filter(|s| !s.is_relay())
            .flat_map(|s| s.urls.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "),
        ice_advertised: String::new(),
        relay: String::new(),
        relay_username: String::new(),
        relay_credential: String::new(),
    })
}

/// The native shadow: there is no URL and no localStorage mirror off-wasm, so
/// the registry row is the only source and this is exactly `selected_connector`.
#[cfg(not(target_arch = "wasm32"))]
pub fn node_in_force(peers: &Peers, peer_id: &str) -> Option<Connector> {
    selected_connector(peers, peer_id)
}

pub fn selected_connector(peers: &Peers, peer_id: &str) -> Option<Connector> {
    let chosen = selection_marker(peers, peer_id)?;
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

/// What [`add_connector`] did beyond writing the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddOutcome {
    /// The add also became the selection, because nothing was selected. The
    /// caller owes the user a word about it — a selection changing under you is
    /// exactly the kind of helpfulness that must be stated, not inferred
    /// [AP25].
    pub selected: bool,
}

/// Add (or overwrite) a connector. Keyed by node peer-id, so re-adding the same
/// node updates it instead of creating a duplicate pointing at one node.
///
/// Rejects a node with a missing half — see the module doc. Returns the reason
/// so a caller with a surface can report it (D13: a refusal must be sayable).
///
/// # It selects the row when nothing is selected, and that is not a convenience
///
/// A registry holding connectors with **no selection** resolves to no
/// provisioning at all: [`provisioning_from_registry`] starts at
/// [`selected_connector`]. So "I added my node" leaves an app that looks
/// configured, installs no establisher, and hands out peer ids nobody can reach
/// — the same both-halves-or-neither failure [AP22] that shipped once already
/// (a resolved node with no establisher), arriving through the registry instead
/// of through the install.
///
/// The fix is structural rather than a note in a doc: adding the first node
/// *is* choosing it, in one expression, so no surface can do one half. It never
/// overrides an existing choice — a second node is added and not selected,
/// because at that point the user has expressed one.
///
/// The selection is written directly rather than through [`select_connector`]
/// on purpose: that function validates against `read_connectors`, and the row
/// we just wrote is a *dispatched* write which has not landed yet on either arm
/// — so the validating path would refuse the row it is being asked about. Here
/// the row is known to exist because we are the one writing it.
pub fn add_connector(peers: &Peers, peer_id: &str, c: &Connector) -> Result<AddOutcome, String> {
    validate_node_peer_id(&c.node_peer_id)?;
    if c.node_addr.trim().is_empty() {
        return Err("a connector needs an address to dial".to_string());
    }
    // Refuse a malformed reflector list HERE — at the surface where it was
    // typed and can be corrected (D13). Downstream the same value only warns,
    // because by then there is no user to tell; that split is deliberate.
    crate::session_config::parse_ice_urls(&c.ice)?;
    // Same posture, one field along: refuse a half-configured relay HERE, where
    // the person who typed it can fix it. A relay URL with no credentials builds
    // an `RTCIceServer` that looks configured and gathers no relay candidates —
    // the silent-nothing failure this whole area keeps producing.
    crate::session_config::parse_relay(&c.relay, &c.relay_username, &c.relay_credential)?;
    // `ice_advertised` is preserved from the existing row and the caller's value
    // is IGNORED — `record_advertised_reflectors` is its only writer, and this
    // is what makes that true structurally rather than by everyone remembering.
    // Re-adding a node the user already knows (to fix a label or a moved
    // address) is the common case, and clearing what that node told us would
    // silently drop a provisioned session back to host-only until the next
    // successful advertise — a NAT regression from editing a label.
    let learned = read_connectors(peers, peer_id)
        .into_iter()
        .find(|e| e.node_peer_id == c.node_peer_id.trim())
        .map(|e| e.ice_advertised)
        .unwrap_or_default();
    let normalized = Connector {
        node_peer_id: c.node_peer_id.trim().to_string(),
        node_addr: c.node_addr.trim().to_string(),
        label: c.label.trim().to_string(),
        ice: c.ice.trim().to_string(),
        ice_advertised: learned,
        relay: c.relay.trim().to_string(),
        relay_username: c.relay_username.trim().to_string(),
        relay_credential: c.relay_credential.trim().to_string(),
    };
    // Read the pre-existing selection BEFORE writing, so the answer is about
    // what the user had chosen and not about the row going in. The *marker*,
    // not the resolved connector: a selection left dangling by a removed node
    // is still a choice, and repointing it at whatever gets added next is the
    // silent substitution `selected_connector` exists to refuse.
    let had_selection = selection_marker(peers, peer_id).is_some();
    let path = app_paths::connector_path(app_paths::APP_ID, peer_id, &normalized.node_peer_id);
    peers.dispatch_write(peer_id, path, connector_to_entity(&normalized));
    if !had_selection {
        let sel = app_paths::connector_selection_path(app_paths::APP_ID, peer_id);
        peers.dispatch_write(peer_id, sel, selection_to_entity(&normalized.node_peer_id));
    }
    Ok(AddOutcome { selected: !had_selection })
}

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

/// Ask `row`'s node what it serves and record its §4.5.1 reflectors — dial
/// first, because a connector nobody has called has no route.
///
/// **This is what makes the automatic half automatic.** Without it §4.5.1 is
/// still a manual step wearing a different hat: the node publishes its reflector
/// and nothing ever asks, so a user who adds a connector and meets by name gets
/// host candidates only and no indication why. Adding a connector is the moment
/// the app learns what that node is — the address, that it answers at all, and
/// its reflectors.
///
/// Fire-and-forget by design: the caller (`add`, a `Check` press) has already
/// succeeded at what the user asked for, and this is enrichment. A node that is
/// down means no reflectors learned, which is exactly the pre-§4.5.1 posture and
/// degrades to host-only rather than failing the add. It logs at debug, and the
/// visible surfaces (`Check`, `connector check`) report the same round trip
/// properly when a user asks for it explicitly.
pub fn learn_node_reflectors(peers: &Peers, peer_id: &str, row: &Connector) {
    let Some(writer) = peers.writer_handle() else {
        return;
    };
    let reach = reach_node(peers, peer_id, row);
    let fut = advertise(peers, peer_id, &row.node_peer_id);
    let owner = peer_id.to_string();
    let row = row.clone();
    spawn(async move {
        if let Err(e) = reach.await {
            tracing::debug!(node = %row.node_peer_id, error = %e, "connector: could not reach the node to learn its reflectors");
            return;
        }
        match fut.await {
            Ok(ad) => {
                if record_advertised_reflectors(&writer, &owner, &row, &ad.reflection_endpoints) {
                    tracing::info!(
                        node = %row.node_peer_id,
                        reflectors = %ad.reflection_endpoints.join(" "),
                        "connector: learned the node's own reflectors (§4.5.1)"
                    );
                }
            }
            Err(e) => {
                tracing::debug!(node = %row.node_peer_id, error = %e, "connector: the node did not advertise")
            }
        }
    });
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
    // §4.5.1: what the node advertised is ADDITIONAL to what the user typed.
    let ice = merge_reflectors(&c.ice, &c.ice_advertised);
    // Degrade LOUDLY, never silently — the same split `ice` already uses. A bad
    // value is *refused* where it is typed (`add_connector`); by the time it is
    // read back there is no user to tell, and losing the rendezvous over a relay
    // typo would cost more than losing the relay.
    let relay = match crate::session_config::parse_relay(
        &c.relay,
        &c.relay_username,
        &c.relay_credential,
    ) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "connector: ignoring a malformed relay — this session will not use a relay"
            );
            None
        }
    };
    resolve_webrtc_provisioning(Some(&c.node_peer_id), Some(&c.node_addr), Some(&ice))
        .map(|p| p.with_relay(relay))
}

/// What `row` should become once `node` has advertised `advertised` — or `None`
/// when nothing changed.
///
/// Pure, so the decision is native-testable on both arms. **`None` when
/// unchanged is the load-bearing half**, because every `meet` re-advertises: an
/// unconditional write would put a tree write (and a subscription wake, and a
/// re-render) on a repeating path for a value that almost never moves.
///
/// Stores the published bytes **joined and otherwise unaltered** — no
/// per-entry validation here, because §4.5.1's dedup is defined over exactly
/// those bytes and a value repaired on the way in would dedup against nothing.
/// Malformed entries are dropped at [`merge_reflectors`], per entry, at read.
pub fn advertised_update(row: &Connector, advertised: &[String]) -> Option<Connector> {
    let joined = advertised.join(" ");
    if row.ice_advertised == joined {
        return None;
    }
    Some(Connector { ice_advertised: joined, ..row.clone() })
}

/// Record what a node advertised as its own §9.3 reflectors, on that node's
/// connector row. The **only** writer of [`Connector::ice_advertised`].
///
/// Takes a [`WriterHandle`] and an already-resolved row rather than `&Peers`,
/// because every caller lands in a **spawned future** — `advertise()` is async
/// and its result arrives off-frame, where no `&Peers` can be held (and where
/// the Worker arm needs the handle's own transport anyway). Both call sites
/// already resolve the row before spawning, to dial it.
///
/// Returns `true` when it wrote. A node with no connector row is not this
/// function's problem: advertising to a node the user never added is legitimate
/// (a URL-param or build-knob session), there is no row to carry the answer, and
/// the typed half still provisions that session.
pub fn record_advertised_reflectors(
    writer: &crate::writer_handle::WriterHandle,
    peer_id: &str,
    row: &Connector,
    advertised: &[String],
) -> bool {
    let Some(updated) = advertised_update(row, advertised) else {
        return false;
    };
    let path = app_paths::connector_path(app_paths::APP_ID, peer_id, &row.node_peer_id);
    writer.put(path, connector_to_entity(&updated));
    true
}

/// Pack a selection for the localStorage mirror. Separated from the write so it
/// is testable natively — the write itself is wasm-only.
pub fn pack_mirror(p: &WebRtcProvisioning) -> String {
    // **Reflectors and the relay are packed SEPARATELY, and flattening them
    // together is a real bug, not an inefficiency.** The first version of this
    // joined every entry's urls into one list and dropped `username`/
    // `credential` on the floor. A relay survived the round trip as a bare
    // `turn:` url with no credentials — which `parse_ice_urls` then *refuses*
    // on the way back in, so `unpack_mirror` dropped the whole ICE list with a
    // warning and the session silently fell back to host-only. Measured: the
    // establisher installed with `ice_servers=0` against a connector row that
    // held a complete, valid relay.
    //
    // An entry is a relay iff it carries credentials — the same discriminator
    // `parse_relay` enforces on the way in, so the two cannot disagree.
    let (relays, reflectors): (Vec<_>, Vec<_>) =
        p.ice_servers.iter().partition(|s| s.is_relay());
    let join = |v: &[&crate::session_config::IceServer]| {
        v.iter()
            .flat_map(|s| s.urls.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join(",")
    };
    let ice = join(&reflectors);
    let relay = join(&relays);
    // One credential pair, from the first relay entry — `parse_relay` produces
    // exactly one, and a second would have to come from somewhere that does not
    // exist yet.
    let user = relays
        .first()
        .and_then(|s| s.username.clone())
        .unwrap_or_default();
    let cred = relays
        .first()
        .and_then(|s| s.credential.clone())
        .unwrap_or_default();
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        p.node_peer_id, p.node_addr, ice, relay, user, cred
    )
}

/// Parse a mirror value written by [`pack_mirror`]. Fails closed on anything
/// that isn't at least two non-empty halves — a half-written mirror must not
/// become a half-provisioned rendezvous.
///
/// **The third field is optional, and that is a compatibility requirement, not
/// laxity.** A mirror written before reflectors existed has two fields; reading
/// it as host-only is exactly what it meant. Rejecting it would strand every
/// returning user on the pre-peer boot path — which is precisely the read this
/// mirror exists to serve.
pub fn unpack_mirror(raw: &str) -> Option<WebRtcProvisioning> {
    let mut parts = raw.split('\u{1f}');
    let id = parts.next()?;
    let addr = parts.next()?;
    let ice = parts.next().unwrap_or("");
    // Fields 4-6 are absent on every mirror written before relays existed, and
    // absent means "no relay" — the same compatibility rule the reflector field
    // above already follows, and the fail-closed reading.
    let relay = parts.next().unwrap_or("");
    let user = parts.next().unwrap_or("");
    let cred = parts.next().unwrap_or("");
    // Degrade loudly, never silently — this is a read path with no user to
    // correct, exactly like the malformed-reflector case. Losing the rendezvous
    // over a bad mirror would cost far more than losing the relay.
    let relay = match crate::session_config::parse_relay(relay, user, cred) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "connector mirror: ignoring a malformed relay — this session will not use one"
            );
            None
        }
    };
    resolve_webrtc_provisioning(Some(id), Some(addr), Some(ice)).map(|p| p.with_relay(relay))
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

/// Resolve the provisioning for **this session** and remember it, in one
/// expression.
///
/// Two surfaces need the booted value: `EntityApp` (for the reload notice) and
/// [`crate::readiness`] (for the preflight report). They must be the *same*
/// value, and the capture is order-sensitive — `ConnectorRegistry::sync`
/// rewrites the localStorage mirror on its first dirty frame, so a second
/// resolve taken later would read the live selection and quietly become "what a
/// reload would use", making every comparison against it vacuous.
///
/// So there is one call, at boot, and it both returns and records. A caller
/// cannot take one half: the same shape as `signaling_node::mount` returning
/// the handler and its grant together [AP22], for the same reason — two
/// expressions of one decision is how the halves drift.
#[cfg(target_arch = "wasm32")]
pub fn capture_booted(url_query: &str) -> Option<WebRtcProvisioning> {
    // The *quiet* resolver: `webrtc_init_config` already logs which source won
    // during the same boot, and a second identical line reads as two
    // provisioning decisions having been taken.
    let booted = resolve_provisioning_quietly(url_query).map(|(p, _)| p);
    BOOTED.with(|b| *b.borrow_mut() = booted.clone());
    booted
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// What [`capture_booted`] resolved. Set once at boot and never again — the
    /// property that makes it safe for a render input to read without a dirty
    /// signal (the *moving* side is the mirror, which `ConnectorRegistry`
    /// watches).
    static BOOTED: std::cell::RefCell<Option<WebRtcProvisioning>> =
        const { std::cell::RefCell::new(None) };
}

/// What this session booted with, for surfaces that hold no `EntityApp`
/// reference (the Shell's verbs get `&Peers` and nothing else).
#[cfg(target_arch = "wasm32")]
pub fn booted_snapshot() -> Option<WebRtcProvisioning> {
    BOOTED.with(|b| b.borrow().clone())
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
        Connector {
            node_peer_id: id.to_string(),
            node_addr: addr.to_string(),
            label: String::new(),
            ice: String::new(),
            ice_advertised: String::new(),
            relay: String::new(),
            relay_username: String::new(),
            relay_credential: String::new(),
        }
    }

    // -----------------------------------------------------------------
    // §4.5.1 — the node-advertised half
    // -----------------------------------------------------------------

    /// The merge rule itself: advertised reflectors are ADDITIONAL, and the
    /// dedup is over published bytes exactly.
    #[test]
    fn advertised_reflectors_are_additional_and_dedup_on_exact_bytes() {
        // Union, typed first — a node's answer never displaces the user's.
        assert_eq!(
            merge_reflectors("stun:typed.example:3478", "stun:node.example:3478"),
            "stun:typed.example:3478 stun:node.example:3478"
        );
        // Either side alone is the whole answer.
        assert_eq!(merge_reflectors("", "stun:node.example:3478"), "stun:node.example:3478");
        assert_eq!(merge_reflectors("stun:typed.example:3478", ""), "stun:typed.example:3478");
        assert_eq!(merge_reflectors("", ""), "");
        // Byte-identical entries collapse to one.
        assert_eq!(merge_reflectors("stun:a.example:3478", "stun:a.example:3478"), "stun:a.example:3478");
        // ...and NON-identical bytes do NOT, however equivalent they look.
        // §4.5.1 pins dedup to the published bytes — no case folding, no
        // default-port canonicalization — because normalizing here would break
        // the same dedup for EXTENSION-REGISTRY §3b, which shares this form.
        // These three are the same reflector to a DNS resolver and three
        // distinct published strings to the rule.
        assert_eq!(
            merge_reflectors("stun:a.example:3478", "stun:A.example:3478 stun:a.example"),
            "stun:a.example:3478 stun:A.example:3478 stun:a.example",
            "normalizing to collapse these would silently break the pinned byte-exact dedup"
        );
    }

    /// One bad entry from a remote node must not cost the user the reflectors
    /// they configured themselves — the node is not theirs to correct.
    #[test]
    fn a_malformed_advertised_entry_is_dropped_alone_and_the_typed_half_survives() {
        let merged = merge_reflectors(
            "stun:mine.example:3478",
            "stun://bad.example:3478 stun:good.example:3478 turn:relay.example:3478 nonsense",
        );
        assert_eq!(
            merged, "stun:mine.example:3478 stun:good.example:3478",
            "the two malformed entries and the credential-less TURN go; nothing else does"
        );
        // The whole-list refusal still applies where a human typed it.
        assert!(
            crate::session_config::parse_ice_urls("stun:ok.example:3478 stun://bad").is_err(),
            "a TYPED list stays all-or-nothing — it is refused at add_connector, where it can be fixed"
        );
    }

    /// The no-write property. Every `meet` re-advertises, so an unconditional
    /// write would put a tree write + a wake + a re-render on a repeating path.
    #[test]
    fn recording_an_unchanged_advertisement_is_not_a_write() {
        let mut row = conn("2KNode", "ws://node.example:4040");
        row.ice_advertised = "stun:a.example:3478 stun:b.example:3478".to_string();

        assert!(
            advertised_update(&row, &[
                "stun:a.example:3478".to_string(),
                "stun:b.example:3478".to_string()
            ])
            .is_none(),
            "same set, same order ⇒ no write"
        );
        // A node that stops serving reflection IS a change, and must be recorded
        // — otherwise a session keeps offering a reflector that has gone away.
        let cleared = advertised_update(&row, &[]).expect("dropping to none is a change");
        assert_eq!(cleared.ice_advertised, "");
        // And so is a reorder, because the stored value is the published bytes.
        assert!(
            advertised_update(&row, &[
                "stun:b.example:3478".to_string(),
                "stun:a.example:3478".to_string()
            ])
            .is_some()
        );
    }

    /// A row written before §4.5.1 existed decodes as "this node has told us
    /// nothing" — a default, never a malformed row.
    #[test]
    fn a_pre_4_5_1_row_decodes_as_no_advertisement_and_the_field_round_trips() {
        let mut c = conn("2KNode", "ws://node.example:4040");
        c.ice = "stun:typed.example:3478".to_string();
        c.ice_advertised = "stun:node.example:3478".to_string();
        let back = connector_from_entity(&connector_to_entity(&c)).expect("round trips");
        assert_eq!(back, c);

        // The pre-v1.1 shape: same entity, no `ice_advertised` key at all.
        let legacy = Entity::new(
            CONNECTOR_TYPE,
            entity_ecf::to_ecf(&entity_ecf::cbor_map! {
                "node_peer_id" => entity_ecf::text("2KNode"),
                "node_addr" => entity_ecf::text("ws://node.example:4040"),
                "label" => entity_ecf::text(""),
                "ice" => entity_ecf::text("stun:typed.example:3478")
            }),
        )
        .unwrap();
        let decoded = connector_from_entity(&legacy).expect("a pre-4.5.1 row is not malformed");
        assert_eq!(decoded.ice_advertised, "");
        assert_eq!(decoded.ice, "stun:typed.example:3478", "and its typed half is untouched");
        // The same row also predates the relay fields, and reads as "no relay"
        // — the fail-closed default, and what that deployment actually meant.
        assert_eq!(decoded.relay, "");
        assert_eq!(decoded.relay_username, "");
        assert_eq!(decoded.relay_credential, "");
    }

    /// The relay survives the tree, credentials included.
    ///
    /// Worth its own test because the failure is silent in the worst way: a
    /// field dropped on **encode** leaves a relay that the user typed, the form
    /// accepted, and every surface shows — which then gathers no relay
    /// candidates, because the credential never came back. That is exactly the
    /// looks-configured-does-nothing shape this whole feature exists to refuse.
    #[test]
    fn a_relay_round_trips_through_the_tree_with_its_credentials() {
        let mut c = conn("2KNode", "ws://node.example:4040");
        c.ice = "stun:typed.example:3478".to_string();
        c.relay = "turn:relay.example:3478 turns:relay.example:5349".to_string();
        c.relay_username = "alice".to_string();
        c.relay_credential = "s3cret".to_string();

        let back = connector_from_entity(&connector_to_entity(&c)).expect("round trips");
        assert_eq!(back, c);
        // Named individually as well as by struct equality: a future `PartialEq`
        // that skipped a field would let the whole-struct assert pass.
        assert_eq!(back.relay_username, "alice");
        assert_eq!(back.relay_credential, "s3cret");
        assert_eq!(back.ice, "stun:typed.example:3478", "the reflector half is untouched");
    }

    /// `add_connector` refuses a half-configured relay at the surface where it
    /// was typed — the same posture the reflector list already has, and for a
    /// sharper reason: a relay URL with no credentials is accepted by
    /// `RTCPeerConnection` and gathers nothing.
    #[tokio::test]
    async fn a_relay_without_credentials_is_refused_where_it_was_typed() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();

        let mut c = conn("2KNode", "ws://node.example:4040");
        c.relay = "turn:relay.example:3478".to_string();
        let e = add_connector(&peers, &me, &c).expect_err("a bare relay is refused");
        assert!(
            e.contains("username") && e.contains("credential"),
            "the refusal must name what is missing: {e}"
        );

        // With both, it is accepted — the refusal is about the missing halves,
        // not about relays being unwelcome.
        c.relay_username = "alice".to_string();
        c.relay_credential = "s3cret".to_string();
        add_connector(&peers, &me, &c).expect("a complete relay is accepted");
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
            ice: "stun:stun.example.org:3478".to_string(),
            ice_advertised: String::new(),
            relay: String::new(),
            relay_username: String::new(),
            relay_credential: String::new(),
        };
        assert_eq!(connector_from_entity(&connector_to_entity(&c)), Some(c));
    }

    /// A row written before reflectors existed must still read — as host-only,
    /// which is exactly what it meant. Anything else strands every returning
    /// user's registry on upgrade.
    #[test]
    fn a_connector_written_before_reflectors_still_reads() {
        let legacy = Entity::new(
            CONNECTOR_TYPE,
            entity_ecf::to_ecf(&entity_ecf::cbor_map! {
                "node_peer_id" => entity_ecf::text("2KNode"),
                "node_addr" => entity_ecf::text("ws://10.0.0.4:9000"),
                "label" => entity_ecf::text("my box")
            }),
        )
        .unwrap();
        let c = connector_from_entity(&legacy).expect("a pre-reflector row is not malformed");
        assert_eq!(c.node_addr, "ws://10.0.0.4:9000");
        assert_eq!(c.ice, "", "absent means host-only, the posture it shipped with");
    }

    /// The refusal lands where the user typed it (D13) — and the row does not
    /// reach the tree, so the downstream warn-and-degrade path never sees it.
    #[tokio::test]
    async fn a_malformed_reflector_is_refused_at_the_surface() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let bad = Connector {
            node_peer_id: "2KNode".to_string(),
            node_addr: "ws://n:9".to_string(),
            label: String::new(),
            ice: "turn:relay.example:3478".to_string(),
            ice_advertised: String::new(),
            relay: String::new(),
            relay_username: String::new(),
            relay_credential: String::new(),
        };
        let err = add_connector(&peers, &me, &bad).expect_err("turn has no credential carrier");
        assert!(err.contains("username"), "the reason must be sayable: {err}");
        assert!(read_connectors(&peers, &me).is_empty(), "and nothing was written");
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
        // Let the first add land before the second. Every write here is
        // *dispatched*, so back-to-back adds read each other's pre-image — the
        // same property `add_connector`'s own `ice_advertised` lookup has. A
        // user adds one node at a time; a test that does not settle is testing
        // the race, not the rule.
        settle(|| selected_connector(&peers, &me).is_some()).await;
        add_connector(&peers, &me, &conn("2KAlpha", "ws://a:1")).unwrap();
        settle(|| read_connectors(&peers, &me).len() == 2).await;
        let list = read_connectors(&peers, &me);
        // Sorted by peer-id, so render order is stable across reloads.
        assert_eq!(
            list.iter().map(|c| c.node_peer_id.as_str()).collect::<Vec<_>>(),
            vec!["2KAlpha", "2KBeta"]
        );

        // The FIRST add is also the selection — a registry with rows and no
        // selection provisions nothing, so adding one node has to be enough to
        // have a node. The second add must NOT move it: by then the user has
        // expressed a choice.
        assert_eq!(
            selected_connector(&peers, &me).unwrap().node_peer_id,
            "2KBeta",
            "the first add selects; the second leaves the choice alone"
        );

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

    /// Adding the first node **is** choosing it, and the outcome says so.
    ///
    /// The state this closes is a registry with rows and no selection, which
    /// resolves to no provisioning at all — an app that looks configured,
    /// installs no establisher, and hands out ids nobody can reach [AP22]. It
    /// is only reachable through a surface that adds without selecting, which
    /// is what both of ours used to do.
    ///
    /// The reported `selected` flag is asserted as hard as the tree state,
    /// because a caller that cannot tell the two adds apart cannot tell the
    /// user which one still needs `connector use` [AP25].
    #[tokio::test]
    async fn the_first_connector_added_becomes_the_selection_and_later_ones_do_not() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();

        let first = add_connector(&peers, &me, &conn("2KFirst", "ws://first:1")).unwrap();
        assert!(first.selected, "an empty registry has no choice to respect");
        settle(|| selected_connector(&peers, &me).is_some()).await;
        assert_eq!(selected_connector(&peers, &me).unwrap().node_peer_id, "2KFirst");

        let second = add_connector(&peers, &me, &conn("2KSecond", "ws://second:1")).unwrap();
        assert!(!second.selected, "a second node must not steal an expressed choice");
        settle(|| read_connectors(&peers, &me).len() == 2).await;
        assert_eq!(
            selected_connector(&peers, &me).unwrap().node_peer_id,
            "2KFirst",
            "adding a node is not switching to it"
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

    /// Editing a label must not cost you NAT traversal.
    ///
    /// `add_connector` is the re-add path (keyed by peer-id, so fixing a label
    /// or a moved address overwrites the row), and the caller's `ice_advertised`
    /// is deliberately IGNORED in favour of what is already stored. Without
    /// that, every re-add would blank the node's own reflectors and silently
    /// drop the next session to host-candidates-only until something happened to
    /// advertise again — a traversal regression with no visible cause.
    ///
    /// Mutation check: make `add_connector` take `c.ice_advertised` and the
    /// final assertion fails (the surfaces all pass `String::new()`).
    #[tokio::test]
    async fn re_adding_a_connector_keeps_what_the_node_advertised() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        add_connector(&peers, &me, &conn("2KNode", "ws://node:1")).unwrap();
        settle(|| !read_connectors(&peers, &me).is_empty()).await;

        // The node tells us its reflector.
        let row = read_connectors(&peers, &me).into_iter().next().unwrap();
        let writer = peers.writer_handle().expect("a direct writer");
        assert!(record_advertised_reflectors(
            &writer,
            &me,
            &row,
            &["stun:node.example:3478".to_string()]
        ));
        settle(|| {
            read_connectors(&peers, &me)
                .first()
                .is_some_and(|c| c.ice_advertised == "stun:node.example:3478")
        })
        .await;

        // Now the user edits the label — the surfaces build a fresh Connector
        // with an empty `ice_advertised`, exactly as the UI does.
        let mut edited = conn("2KNode", "ws://node:1");
        edited.label = "my box".to_string();
        edited.ice = "stun:mine.example:3478".to_string();
        add_connector(&peers, &me, &edited).unwrap();
        settle(|| read_connectors(&peers, &me).first().is_some_and(|c| c.label == "my box")).await;

        let after = read_connectors(&peers, &me).into_iter().next().unwrap();
        assert_eq!(after.label, "my box", "the edit applied");
        assert_eq!(after.ice, "stun:mine.example:3478", "and the typed half is theirs");
        assert_eq!(
            after.ice_advertised, "stun:node.example:3478",
            "and the node's own advertisement survived the edit"
        );

        // End to end: the provisioning a reload would use carries BOTH halves,
        // merged, with the typed one first (§4.5.1).
        select_connector(&peers, &me, "2KNode").unwrap();
        settle(|| selected_connector(&peers, &me).is_some()).await;
        let p = provisioning_from_registry(&peers, &me).expect("both halves of the node are set");
        let urls: Vec<&str> =
            p.ice_servers.iter().flat_map(|s| s.urls.iter()).map(String::as_str).collect();
        assert_eq!(urls, vec!["stun:mine.example:3478", "stun:node.example:3478"]);
    }

    /// **The registry is not the only source of a rendezvous, and asking it as
    /// though it were refused every provisioned session that had no row.**
    ///
    /// Native can only pin the half it has — off wasm there is no URL and no
    /// boot snapshot, so `node_in_force` *is* `selected_connector` and the
    /// property to hold is that it agrees with it exactly. The half that matters
    /// (falling back to `booted_snapshot`) is `wasm32`-only and was measured in
    /// two real browsers instead: before, `meet` answered "no connector
    /// selected" one command after `net` printed `OK rendezvous`; after, both
    /// browsers met and learned each other's ids with nobody typing one.
    ///
    /// Recording the gap rather than implying coverage: a green `make test` says
    /// nothing about the fallback arm.
    #[tokio::test]
    async fn the_node_in_force_is_the_selected_row_when_there_is_one() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        assert!(node_in_force(&peers, &me).is_none(), "nothing selected, nothing provisioned");

        add_connector(&peers, &me, &conn("2KnodeA", "ws://a:4041")).unwrap();
        settle(|| node_in_force(&peers, &me).is_some()).await;
        let n = node_in_force(&peers, &me).expect("the first add selects itself");
        assert_eq!(n.node_peer_id, "2KnodeA");
        assert_eq!(
            n.node_peer_id,
            selected_connector(&peers, &me).unwrap().node_peer_id,
            "with a row present the two must never disagree",
        );
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
            ice: String::new(),
            ice_advertised: String::new(),
            relay: String::new(),
            relay_username: String::new(),
            relay_credential: String::new(),
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

        // The reflectors survive the boot mirror. Without this the pre-peer
        // boot path — the ONLY read that happens before a tree exists — would
        // provision the node and silently drop its ICE, so a returning user
        // would be host-only until something re-synced the registry.
        let with_ice = WebRtcProvisioning {
            node_peer_id: "2KNode".to_string(),
            node_addr: "ws://n:9".to_string(),
            ice_servers: vec![crate::session_config::IceServer {
                urls: vec!["stun:a.example:3478".to_string(), "stun:b.example:3478".to_string()],
                username: None,
                credential: None,
            }],
            poll_interval_ms: None,
            max_deadline_ms: None,
        };
        assert_eq!(unpack_mirror(&pack_mirror(&with_ice)), Some(with_ice));

        // A two-field mirror written before reflectors existed reads as
        // host-only rather than failing — same compatibility rule as the row.
        let legacy = unpack_mirror("2KNode\u{1f}ws://n:9").expect("a legacy mirror still reads");
        assert!(legacy.ice_servers.is_empty());

        // **The relay survives the boot mirror WITH its credentials, and this
        // is the assertion that was missing when the feature first "worked".**
        //
        // `pack_mirror` originally flattened every entry's urls into one list
        // and dropped `username`/`credential`. A relay came back as a bare
        // `turn:` url, which `parse_ice_urls` then REFUSES — so `unpack_mirror`
        // discarded the whole ICE list and the session fell back to host-only,
        // silently, against a connector row holding a perfectly good relay.
        // Measured through the shipped surface: the establisher installed with
        // `ice_servers=0`. Native tests on the tree round-trip all passed,
        // because the tree is not the path boot reads.
        let with_relay = WebRtcProvisioning {
            node_peer_id: "2KNode".to_string(),
            node_addr: "ws://n:9".to_string(),
            ice_servers: vec![
                crate::session_config::IceServer {
                    urls: vec!["stun:a.example:3478".to_string()],
                    username: None,
                    credential: None,
                },
                crate::session_config::IceServer {
                    urls: vec!["turn:r.example:3478".to_string()],
                    username: Some("alice".to_string()),
                    credential: Some("s3cret".to_string()),
                },
            ],
            poll_interval_ms: None,
            max_deadline_ms: None,
        };
        let back = unpack_mirror(&pack_mirror(&with_relay)).expect("round trips");
        assert_eq!(back, with_relay);
        assert_eq!(back.ice_servers.len(), 2, "reflector and relay stay separate entries");
        assert_eq!(back.ice_servers[1].credential.as_deref(), Some("s3cret"));

        // A relay with NO reflectors is the common shape for someone who was
        // handed only TURN credentials, and it must not collapse.
        let relay_only = WebRtcProvisioning {
            ice_servers: vec![crate::session_config::IceServer {
                urls: vec!["turn:r.example:3478".to_string()],
                username: Some("bob".to_string()),
                credential: Some("hunter2".to_string()),
            }],
            ..with_relay.clone()
        };
        assert_eq!(unpack_mirror(&pack_mirror(&relay_only)), Some(relay_only));

        assert!(unpack_mirror("").is_none());
        assert!(unpack_mirror("2KNodeOnly").is_none(), "an id with no address");
        assert!(unpack_mirror("2KNode\u{1f}").is_none(), "an address that is blank");
        assert!(unpack_mirror("\u{1f}ws://n:9").is_none(), "an address with no id");
    }
}
