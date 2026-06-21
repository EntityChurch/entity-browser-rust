//! Tree-backed registry of the peers this app hosts.
//!
//! One entity per hosted peer at
//! `/{system_peer}/app/entity-browser/system/peers/{peer_id}`, in the
//! system (primary) peer's tree. The roster *is* the system peer's
//! state, matching the "open a peer window = the system peer's window"
//! model.
//!
//! This replaced the old content-free peer-registry *signal bump*.
//! That signal could only wake subscribers ("something changed");
//! this carries the actual roster, so peer-aware windows render from
//! the tree like every other entity-backed window instead of
//! re-scanning `Peers` directly.
//!
//! The registry is **derived, not authoritative**: [`PeerRegistry::sync`]
//! reconciles it from the authoritative live `Peers` (which peers are
//! hosted) plus SDK metadata. It is idempotent — a peer's entity is
//! only rewritten when its derived content actually changes, so
//! syncing on every lifecycle event (and re-syncing at boot) is free
//! when nothing moved and self-heals a stale roster.
//!
//! Only **public** identity goes here (peer_id, label, classification,
//! role). Private key seed material stays in `persistence.rs`.
//!
//! Both arms supported via [`WriterHandle`]; no per-arm boilerplate.

use std::collections::HashMap;

use entity_ecf::{cbor_map, to_ecf};
use entity_entity::Entity;

use crate::app_paths;
use crate::peer_display::{self, resolve_role};
use crate::peer_mode::PeerMode;
use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

/// Entity type name for hosted-peer registry records.
pub const REGISTRY_ENTRY_TYPE: &str = "app/entity-browser/peer-registry-entry";

/// One hosted peer's public, app-derived facts. The renderer-neutral
/// shape every peer-aware window will consume (Phase 2+).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRecord {
    pub peer_id: String,
    /// User-supplied label only — `None` when the peer has no label
    /// (consumers fall back to a short peer id). Kept faithful so the
    /// UI doesn't render a truncated pid as if it were a label.
    pub label: Option<String>,
    /// Structural classification tag: `primary` | `local` | `remote`
    /// (which SDK hosts it — a runtime fact). Drives badge color, not
    /// the mode label.
    pub display: String,
    /// Truthful human role: `system` | `frontend` | `backend (memory)`
    /// | `backend (opfs)`. Resolved from the authoritative persisted
    /// mode (not the old `persisted`-flag proxy that mislabeled every
    /// backend peer as "memory").
    pub role: String,
    /// Glyph paired with `role` (`★`/`●`/`◆`/`◆⛁`).
    pub glyph: String,
    /// Whether the peer's identity is saved to the persisted store
    /// (authoritative — membership in the persisted-mode map, not the
    /// unreliable `PeerMetadata.persisted` flag).
    pub persisted: bool,
    pub is_primary: bool,
    /// Peer has a local `PeerContext` (drives the "Tree" button).
    pub has_context: bool,
    /// App policy: user may delete this peer (non-primary).
    pub deletable: bool,
    pub listen_addresses: Vec<String>,
}

impl PeerRecord {
    /// Derive a record for `peer_id` from authoritative `Peers` state +
    /// the authoritative persisted-mode map. Pure read; no tree writes.
    pub fn derive(peers: &Peers, peer_id: &str, modes: &HashMap<String, PeerMode>) -> Self {
        let meta = peers.peer_metadata(peer_id);
        let label = meta.as_ref().and_then(|m| m.label.clone());
        let listen_addresses = meta
            .as_ref()
            .map(|m| m.listen_addresses.clone())
            .unwrap_or_default();
        let (kind, glyph, role) = resolve_role(peers, peer_id, modes);
        Self {
            peer_id: peer_id.to_string(),
            label,
            display: kind.as_str().to_string(),
            role: role.to_string(),
            glyph: glyph.to_string(),
            // "saved" iff the identity is in the persisted store.
            persisted: modes.contains_key(peer_id),
            is_primary: peer_id == peers.primary_peer_id(),
            has_context: peers.has_peer_context(peer_id),
            deletable: peer_display::is_user_deletable(peers, peer_id),
            listen_addresses,
        }
    }

    pub fn to_entity(&self) -> Entity {
        let data = to_ecf(&cbor_map! {
            "peer_id" => entity_ecf::text(&self.peer_id),
            "label" => entity_ecf::text(self.label.as_deref().unwrap_or("")),
            "display" => entity_ecf::text(&self.display),
            "role" => entity_ecf::text(&self.role),
            "glyph" => entity_ecf::text(&self.glyph),
            "persisted" => entity_ecf::bool_val(self.persisted),
            "is_primary" => entity_ecf::bool_val(self.is_primary),
            "has_context" => entity_ecf::bool_val(self.has_context),
            "deletable" => entity_ecf::bool_val(self.deletable),
            "listen_addresses" => entity_ecf::text(self.listen_addresses.join(","))
        });
        Entity::new(REGISTRY_ENTRY_TYPE, data)
            .expect("peer registry entity construction is infallible")
    }

    #[allow(dead_code)] // Phase 2: peer-aware windows decode the roster here
    pub fn from_entity(entity: &Entity) -> Option<Self> {
        let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
        let map = value.as_map()?;

        let mut peer_id = String::new();
        let mut label: Option<String> = None;
        let mut display = String::new();
        let mut role = String::new();
        let mut glyph = String::new();
        let mut persisted = false;
        let mut is_primary = false;
        let mut has_context = false;
        let mut deletable = false;
        let mut listen_addresses = Vec::new();

        for (k, v) in map {
            match k.as_text() {
                Some("peer_id") => {
                    if let Some(s) = v.as_text() {
                        peer_id = s.to_string();
                    }
                }
                Some("label") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            label = Some(s.to_string());
                        }
                    }
                }
                Some("display") => {
                    if let Some(s) = v.as_text() {
                        display = s.to_string();
                    }
                }
                Some("role") => {
                    if let Some(s) = v.as_text() {
                        role = s.to_string();
                    }
                }
                Some("glyph") => {
                    if let Some(s) = v.as_text() {
                        glyph = s.to_string();
                    }
                }
                Some("persisted") => {
                    if let Some(b) = v.as_bool() {
                        persisted = b;
                    }
                }
                Some("is_primary") => {
                    if let Some(b) = v.as_bool() {
                        is_primary = b;
                    }
                }
                Some("has_context") => {
                    if let Some(b) = v.as_bool() {
                        has_context = b;
                    }
                }
                Some("deletable") => {
                    if let Some(b) = v.as_bool() {
                        deletable = b;
                    }
                }
                Some("listen_addresses") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            listen_addresses = s.split(',').map(|x| x.to_string()).collect();
                        }
                    }
                }
                _ => {}
            }
        }

        if peer_id.is_empty() {
            return None;
        }
        Some(Self {
            peer_id,
            label,
            display,
            role,
            glyph,
            persisted,
            is_primary,
            has_context,
            deletable,
            listen_addresses,
        })
    }
}

/// Registry writer. Held by `EntityApp`; `sync` is called once per
/// frame (and at boot) to reconcile the tree roster against the live
/// `Peers`.
///
/// Idempotence is tracked **in memory** (`written`: peer_id →
/// last-written content hash), not by reading the tree back. This
/// matters in Worker mode: the consumer-side tree is a subscription
/// mirror, so a read-back would miss until a window subscribes — the
/// old design re-put every peer every frame whenever no peer window
/// was open. The in-memory map makes `sync` correct and cheap
/// regardless of subscription state, and makes prune exact (the
/// registry only ever touches entries it wrote).
pub struct PeerRegistry {
    system_peer_id: String,
    handle: Option<WriterHandle>,
    written: HashMap<String, entity_hash::Hash>,
}

impl PeerRegistry {
    pub fn new(peers: &Peers) -> Self {
        Self {
            system_peer_id: peers.system_peer_id().to_string(),
            handle: peers.writer_handle(),
            written: HashMap::new(),
        }
    }

    /// Reconcile the tree roster against the authoritative live `Peers`
    /// and persisted-mode map. Upserts a record for every hosted peer
    /// (writing only when its derived content changed) and removes
    /// records for peers no longer hosted. Idempotent and self-healing.
    pub fn sync(&mut self, peers: &Peers) {
        let Some(handle) = self.handle.clone() else {
            tracing::trace!("peer_registry: no writer handle");
            return;
        };

        // One authoritative-mode read per sync (not per peer).
        let modes = crate::persistence::peer_modes();
        let desired = peers.peer_ids();

        for pid in &desired {
            let entity = PeerRecord::derive(peers, pid, &modes).to_entity();
            let hash = entity.content_hash;
            if self.written.get(pid) == Some(&hash) {
                continue; // unchanged — no write, no churn
            }
            let path =
                app_paths::peer_registry_entry_path(app_paths::APP_ID, &self.system_peer_id, pid);
            handle.put(path, entity);
            self.written.insert(pid.clone(), hash);
        }

        // Prune entries the registry wrote for peers no longer hosted.
        let stale: Vec<String> = self
            .written
            .keys()
            .filter(|pid| !desired.iter().any(|d| d == *pid))
            .cloned()
            .collect();
        for pid in stale {
            let path =
                app_paths::peer_registry_entry_path(app_paths::APP_ID, &self.system_peer_id, &pid);
            handle.remove(path);
            self.written.remove(&pid);
        }
    }
}

/// Read the hosted-peer roster from the system peer's tree, sorted by
/// peer id. The consumer surface windows migrate onto in Phase 2+.
#[allow(dead_code)] // Phase 2: Peers/Connections/Key Manager read via this
pub fn read_registry(peers: &Peers) -> Vec<PeerRecord> {
    let sys = peers.system_peer_id();
    let prefix = app_paths::peers_registry_prefix(app_paths::APP_ID, sys);
    let mut entries = peers.tree_listing(sys, &prefix);
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    // Reconcile against the authoritative live host set. The registry is
    // **derived, not authoritative** (see module docs): `peers.peer_ids()`
    // is ground truth for which peers are hosted. A registry entry with no
    // matching hosted peer is a GHOST — most importantly the row left behind
    // when a backend peer is deleted: tearing down its dedicated Worker SDK
    // drops it from `peer_ids()` immediately, but the Worker-arm subscription
    // mirror does NOT reflect the registry-entry removal (the worker removes
    // the entity from its store — confirmed `removed=true` — yet the deletion
    // is not broadcast to subscribers, so `tree_listing` keeps returning the
    // stale entry). Filtering here makes the read self-healing — exactly the
    // property the module claims — so the deleted peer's row vanishes at once
    // regardless of the mirror lag. The underlying
    // Worker-arm delete-reflection gap is tracked as the open upstream
    // finding; this reconciliation is the app-tier guard, not its fix).
    let hosted: std::collections::HashSet<String> = peers.peer_ids().into_iter().collect();

    entries
        .into_iter()
        .filter_map(|e| {
            peers
                .get_entity(sys, &e.path)
                .and_then(|ent| PeerRecord::from_entity(&ent))
        })
        .filter(|rec| hosted.contains(&rec.peer_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_writes_primary_record_truthfully() {
        let pm = Peers::new_direct();
        let mut reg = PeerRegistry::new(&pm);
        reg.sync(&pm);

        let recs = read_registry(&pm);
        assert_eq!(recs.len(), 1, "exactly the primary peer");
        assert_eq!(recs[0].peer_id, pm.primary_peer_id());
        assert!(recs[0].is_primary);
        assert_eq!(recs[0].display, "primary");
        assert_eq!(recs[0].role, "system", "primary renders as system");
        assert_eq!(recs[0].glyph, "★");
        assert!(!recs[0].deletable, "primary peer is never user-deletable");
    }

    #[test]
    fn sync_is_content_idempotent() {
        let pm = Peers::new_direct();
        let mut reg = PeerRegistry::new(&pm);
        let sys = pm.primary_peer_id().to_string();
        let path = app_paths::peer_registry_entry_path(app_paths::APP_ID, &sys, &sys);

        reg.sync(&pm);
        let h1 = pm.get_entity(&sys, &path).unwrap().content_hash;
        reg.sync(&pm);
        let h2 = pm.get_entity(&sys, &path).unwrap().content_hash;
        assert_eq!(h1, h2, "re-sync with no change must not churn the entity");
    }

    #[test]
    fn sync_tracks_a_created_peer() {
        let mut pm = Peers::new_direct();
        let mut reg = PeerRegistry::new(&pm);
        reg.sync(&pm);
        assert_eq!(read_registry(&pm).len(), 1);

        // §4.1b: create_new_peer is now async-shaped (uniform across
        // arms). Direct's future is already-ready, so a current-thread
        // runtime resolves it without scheduling anything.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(pm.create_new_peer(Some("alice".into())))
            .expect("direct create_new_peer must succeed");
        reg.sync(&pm);

        let recs = read_registry(&pm);
        assert_eq!(recs.len(), 2, "primary + created peer");
        assert_eq!(recs.iter().filter(|r| r.is_primary).count(), 1);
    }

    /// The registry only manages entries it wrote: a foreign entity
    /// planted under the prefix is left untouched. (The old design
    /// scanned the whole prefix and would clobber anything there;
    /// the in-memory `written` map makes prune exact and scoped.)
    /// Prune-on-delete of registry-owned entries is exercised
    /// end-to-end by the worker e2e delete phase.
    #[test]
    fn sync_leaves_foreign_prefix_entities_untouched() {
        let pm = Peers::new_direct();
        let mut reg = PeerRegistry::new(&pm);
        let sys = pm.primary_peer_id().to_string();

        let foreign = app_paths::peer_registry_entry_path(app_paths::APP_ID, &sys, "FOREIGN");
        pm.put_entity(
            &sys,
            &foreign,
            PeerRecord {
                peer_id: "FOREIGN".into(),
                label: None,
                display: "remote".into(),
                role: "backend (memory)".into(),
                glyph: "◆".into(),
                persisted: false,
                is_primary: false,
                has_context: false,
                deletable: true,
                listen_addresses: vec![],
            }
            .to_entity(),
        );

        reg.sync(&pm);

        assert!(
            pm.get_entity(&sys, &foreign).is_some(),
            "sync must not touch entities it did not write"
        );
    }

    /// `read_registry` reconciles against the live host set: a registry
    /// entry whose peer is no longer hosted (a GHOST — e.g. the stale row
    /// a backend-peer delete leaves in the Worker-arm cache mirror because
    /// the deletion isn't broadcast to subscribers) is filtered out, while
    /// genuinely-hosted peers survive. This is the app-tier self-heal that
    /// makes a deleted peer's row vanish at once.
    #[test]
    fn read_registry_filters_ghost_entries_for_unhosted_peers() {
        let pm = Peers::new_direct();
        let mut reg = PeerRegistry::new(&pm);
        let sys = pm.primary_peer_id().to_string();

        // Seed the legitimate primary record.
        reg.sync(&pm);
        assert_eq!(read_registry(&pm).len(), 1, "primary present pre-ghost");

        // Plant a registry entry for a peer that is NOT hosted (mimics a
        // stale mirror entry surviving a delete). The path is registry-
        // owned, so it lands in `read_registry`'s scan — but its peer is
        // absent from `peers.peer_ids()`.
        let ghost_path = app_paths::peer_registry_entry_path(app_paths::APP_ID, &sys, "GHOST");
        pm.put_entity(
            &sys,
            &ghost_path,
            PeerRecord {
                peer_id: "GHOST".into(),
                label: None,
                display: "remote".into(),
                role: "backend (memory)".into(),
                glyph: "◆".into(),
                persisted: false,
                is_primary: false,
                has_context: false,
                deletable: true,
                listen_addresses: vec![],
            }
            .to_entity(),
        );

        // The entity is physically present in the tree...
        assert!(pm.get_entity(&sys, &ghost_path).is_some(), "ghost entity written");
        // ...but read_registry must NOT surface it (peer not hosted).
        let recs = read_registry(&pm);
        assert_eq!(recs.len(), 1, "ghost filtered; only the hosted primary remains");
        assert_eq!(recs[0].peer_id, sys, "the surviving record is the primary");
        assert!(
            !recs.iter().any(|r| r.peer_id == "GHOST"),
            "a registry entry for an unhosted peer must be reconciled away"
        );
    }

    #[test]
    fn record_entity_roundtrips() {
        let rec = PeerRecord {
            peer_id: "PEERAAAA".into(),
            label: Some("Alice".into()),
            display: "remote".into(),
            role: "backend (opfs)".into(),
            glyph: "◆⛁".into(),
            persisted: true,
            is_primary: false,
            has_context: true,
            deletable: true,
            listen_addresses: vec!["ws://127.0.0.1:4042".into()],
        };
        let back = PeerRecord::from_entity(&rec.to_entity()).expect("decodes");
        assert_eq!(rec, back);
    }

    #[test]
    fn record_entity_roundtrips_without_label() {
        let rec = PeerRecord {
            peer_id: "PEERBBBB".into(),
            label: None,
            display: "remote".into(),
            role: "backend (memory)".into(),
            glyph: "◆".into(),
            persisted: false,
            is_primary: false,
            has_context: false,
            deletable: true,
            listen_addresses: vec![],
        };
        let back = PeerRecord::from_entity(&rec.to_entity()).expect("decodes");
        assert_eq!(rec, back, "empty-string label must round-trip back to None");
    }
}
