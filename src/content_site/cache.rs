//! Foreign-site cache provenance — the SDK-tier freshness ledger, hand-rolled
//! at L5 (Option α).
//!
//! ## The field-split
//!
//! When this peer caches another peer's site, two *different* records appear,
//! and they have two different owners:
//!
//! - **Foreign authored content** (Category A) — the manifest + page entities,
//!   hash-verified — is cached at its **natural universal path**
//!   `/{foreign}/sites/{S}/...` in MY store (see [`super::paths`] /
//!   [`super::resolver`]). Byte-faithful to the foreign peer's tree; the store
//!   content-addresses + dedups internally.
//! - **Provenance** (this module) — the *temporal* facts about that cache
//!   (`last_reconciled`, `pinned_root_hash`, `source_transport`) — is **SDK-tier
//!   and uniform across every L5 app** that caches foreign content. It lives
//!   under MY peer at `/{me}/system/cache/{foreign}/sites/{S}/provenance`, a
//!   reserved SDK prefix (L0 bookkeeping per SDK-OPERATIONS §2.7 — records no
//!   peer will ever observe). It is **never synced**.
//!
//! App-tier *preferences* (`visit_count`, `bookmarked`, `is_home`) are the third
//! record; they are genuinely Content-Site-specific and live under
//! [`crate::app_paths::site_cache_pref_prefix`], not here.
//!
//! ## Why hand-rolled (Option α)
//!
//! The generic "fetch a remote entity, persist it at its universal path, record
//! provenance" primitive is a drafted-but-unratified SDK seam
//! (`resolve_remote` / `prefetch`). Until it lands we write provenance here, at
//! L5, under **the exact path-shape the SDK will own** — so the eventual lift is
//! an interface-level refactor (writes move into the SDK; readers point at the
//! read result's `provenance` instead of these L0 reads). The path-shape does
//! not change.
//!
//! > **Authorization note:** writing the cached content under `/{foreign}/...`
//! > is L0-legal (the store does not validate the path's peer-segment), but on
//! > the Worker arm every write is an L1 dispatch. It currently rides
//! > `debug_open_grants`; the durable answer is a scoped cross-namespace cache
//! > capability (tracked separately). Provenance writes under `/{me}/system/...`
//! > are within the owner cap's own namespace and need nothing.

use entity_entity::Entity;
use entity_hash::Hash;

use crate::peers::Peers;

/// Entity type for a provenance record (substrate-tier bookkeeping; the
/// `system/cache/` *path* is what makes it SDK-tier, not the type label).
const PROVENANCE_TYPE: &str = "app/state/cache_provenance";

/// SDK-tier provenance for one cached foreign site. The freshness subset the
/// design ratifies as v1 (not polish): without `last_reconciled` you cannot
/// answer "is my cache stale?" — `pinned_root_hash` proves *coherence* (you
/// have a consistent snapshot), not *currency* (it's the latest), because the
/// manifest is the mutable ref.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CacheProvenance {
    /// Wall-clock ms when this site was last verified-fresh from the authority.
    /// `0` = unknown (native has no clock; the Worker arm stamps `Date.now()`).
    pub last_reconciled: u64,
    /// The manifest hash this cached snapshot is anchored to (hex). The *ref*
    /// in the git model — the one mutable thing; page bodies are immutable by
    /// their own content hash. Revalidation (deferred) compares a freshly
    /// fetched manifest hash against this.
    pub pinned_root_hash: String,
    /// The http-poll origin we fetched from (the `source_transport`).
    pub source_transport: String,
}

impl CacheProvenance {
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let Some(map) = value.as_map() else {
            return Self::default();
        };
        let mut out = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("last_reconciled") => {
                    if let Some(i) = v.as_integer() {
                        out.last_reconciled = u64::try_from(i128::from(i)).unwrap_or(0);
                    }
                }
                Some("pinned_root_hash") => {
                    if let Some(s) = v.as_text() {
                        out.pinned_root_hash = s.to_string();
                    }
                }
                Some("source_transport") => {
                    if let Some(s) = v.as_text() {
                        out.source_transport = s.to_string();
                    }
                }
                _ => {}
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "last_reconciled" => entity_ecf::integer(self.last_reconciled as i64),
            "pinned_root_hash" => entity_ecf::text(&self.pinned_root_hash),
            "source_transport" => entity_ecf::text(&self.source_transport)
        });
        Entity::new(PROVENANCE_TYPE, data).unwrap()
    }
}

/// Prefix of MY whole foreign-cache provenance ledger. A Worker-arm surface
/// that reads provenance must observe this prefix (the cache mirror only feeds
/// subscribed prefixes — `feedback_worker_cache_get_needs_subscription`).
pub fn provenance_prefix(my_peer_id: &str) -> String {
    format!("/{}/system/cache/", my_peer_id)
}

/// Tree path of the provenance record for foreign `(peer, site)`, under MY
/// peer: `/{me}/system/cache/{foreign}/sites/{site}/provenance`. The
/// `{foreign}/sites/{site}` tail **mirrors** the natural foreign content path,
/// so the ledger reads as "my record of caching that subtree."
pub fn provenance_path(my_peer_id: &str, foreign_peer_id: &str, site_id: &str) -> String {
    format!(
        "/{}/system/cache/{}/sites/{}/provenance",
        my_peer_id, foreign_peer_id, site_id
    )
}

/// The pinned root hash (hex) for a manifest entity — `Hash::compute(type,
/// data)` over the manifest, exactly the integrity proof the two-hop fetch
/// verifies. This is what a later revalidation compares the re-fetched
/// manifest against to decide unchanged-vs-changed.
pub fn manifest_hash_hex(manifest: &Entity) -> String {
    Hash::compute(&manifest.entity_type, &manifest.data).to_hex()
}

/// Record (or refresh) the provenance for a cached foreign site. Arm-aware seed
/// write via [`Peers::seed_write`] (Direct → sync L0; Worker → `dispatch_write`).
/// Idempotent in the steady state (the store dedups an unchanged record).
pub fn write_provenance(
    peers: &Peers,
    my_peer_id: &str,
    foreign_peer_id: &str,
    site_id: &str,
    prov: &CacheProvenance,
) {
    peers.seed_write(
        my_peer_id,
        provenance_path(my_peer_id, foreign_peer_id, site_id),
        prov.to_entity(),
    );
}

/// Read the provenance for a cached foreign site, or `None` if never cached.
/// Reads MY store (selector = my peer; the ledger lives under my namespace).
pub fn read_provenance(
    peers: &Peers,
    my_peer_id: &str,
    foreign_peer_id: &str,
    site_id: &str,
) -> Option<CacheProvenance> {
    peers
        .get_entity(my_peer_id, &provenance_path(my_peer_id, foreign_peer_id, site_id))
        .map(|e| CacheProvenance::from_entity(&e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_round_trips_through_entity() {
        let p = CacheProvenance {
            last_reconciled: 1_700_000_000_123,
            pinned_root_hash: "abc123".into(),
            source_transport: "http://labs.example".into(),
        };
        assert_eq!(CacheProvenance::from_entity(&p.to_entity()), p);
    }

    #[test]
    fn default_provenance_round_trips() {
        let p = CacheProvenance::default();
        assert_eq!(CacheProvenance::from_entity(&p.to_entity()), p);
    }

    #[test]
    fn paths_are_under_my_system_cache_mirroring_the_foreign_tail() {
        assert_eq!(
            provenance_path("ME", "BOB", "blog"),
            "/ME/system/cache/BOB/sites/blog/provenance"
        );
        assert_eq!(provenance_prefix("ME"), "/ME/system/cache/");
        // The record lives under MY namespace (capability-trivial), never
        // under the foreign peer's namespace (that stays byte-faithful).
        assert!(provenance_path("ME", "BOB", "blog").starts_with("/ME/"));
    }

    #[test]
    fn manifest_hash_is_the_content_hash() {
        use crate::content_site::SiteManifest;
        let m = SiteManifest::new("blog", "Bob's Blog", "index", vec![]).to_entity();
        let h = manifest_hash_hex(&m);
        // Stable + equals a direct compute over (type, data).
        assert_eq!(h, Hash::compute(&m.entity_type, &m.data).to_hex());
        assert!(!h.is_empty());
    }

    #[test]
    fn write_then_read_provenance_round_trips_through_my_store() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        assert!(read_provenance(&peers, &me, "BOB", "blog").is_none(), "absent → None");

        let p = CacheProvenance {
            last_reconciled: 42,
            pinned_root_hash: "deadbeef".into(),
            source_transport: "http://b.example".into(),
        };
        write_provenance(&peers, &me, "BOB", "blog", &p);
        assert_eq!(read_provenance(&peers, &me, "BOB", "blog"), Some(p));
    }
}
