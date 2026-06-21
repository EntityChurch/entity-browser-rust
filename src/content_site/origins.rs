//! The site-origin registry — `target_peer_id → static HTTP origin`.
//!
//! When a link names a site on **another** peer
//! (`entity://{peer}/sites/...`), the [`MultiResolver`] needs to
//! know *where* to fetch that peer's published artifacts. That mapping is
//! this registry: a small, reactive, reload-surviving key→value store
//! under OUR peer's app namespace
//! (`app/entity-browser/site-origins/{target}`).
//!
//! **This is a bootstrap/override cache, NOT canonical discovery.** The
//! canonical "where is peer X reachable" answer is X's *advertised
//! transport profile* (the `endpoint.url` in its signed `/manifest`); the
//! petname/registry substrate resolves *names → peer_ids*, a different
//! axis (see the registry review). The one genuinely
//! out-of-band fact is the *first* origin URL — seeded here for the demo
//! or set by the user; once a manifest is fetched the origin can flow
//! from there. Kept in the tree (not a Rust field) so a write fires the
//! window watch → re-render, and it survives reload (closure design §5).
//!
//! **Worker-arm note:** `get_origin` reads via the cache mirror, which is
//! fed only for *subscribed* prefixes — any surface that reads this must
//! also watch [`app_paths::site_origins_prefix`]
//! (`[[feedback_worker_cache_get_needs_subscription]]`).

#![allow(dead_code)] // consumers (the overlay/router wiring) land alongside

use entity_entity::Entity;

use crate::app_paths::{self, APP_ID};
use crate::peers::Peers;

/// Entity type for a registry entry (frontend app state).
const ORIGIN_TYPE: &str = "app/state/site_origin";

/// Record `target_peer_id`'s HTTP origin under `our_peer_id`'s registry.
/// The write fires the registry-prefix watch (reactive). `origin` is a
/// scheme-qualified base URL, e.g. `http://localhost:8083` (no trailing
/// slash needed — callers trim).
pub fn set_origin(peers: &Peers, our_peer_id: &str, target_peer_id: &str, origin: &str) {
    let path = origin_path(our_peer_id, target_peer_id);
    // Arm-aware seed write via the blessed router method: Direct → sync L0
    // (readable in the same pass; sync tests + immediate boot-seed depend
    // on it), Worker → async `dispatch_write`. No L0 hatch reach-through.
    peers.seed_write(our_peer_id, path, origin_entity(origin));
}

/// Tree path of `target_peer_id`'s origin entry under `our_peer_id`'s
/// registry. Exposed so the owned boot-load step can seed it durably via
/// [`Peers::put_if_absent`](crate::peers::Peers::put_if_absent) instead of
/// the fire-and-forget [`set_origin`].
pub fn origin_path(our_peer_id: &str, target_peer_id: &str) -> String {
    app_paths::site_origin_path(APP_ID, our_peer_id, target_peer_id)
}

/// Build the origin registry entity for `origin` (trailing slash trimmed).
pub fn origin_entity(origin: &str) -> Entity {
    let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
        "origin" => entity_ecf::text(origin.trim_end_matches('/'))
    });
    Entity::new(ORIGIN_TYPE, data).unwrap()
}

/// Look up `target_peer_id`'s registered HTTP origin from `our_peer_id`'s
/// registry, or `None` if unregistered (→ the resolver falls back to a
/// local read). Reads L1 via the router so it works on both arms (subject
/// to the Worker-arm subscription note above).
pub fn get_origin(peers: &Peers, our_peer_id: &str, target_peer_id: &str) -> Option<String> {
    let path = app_paths::site_origin_path(APP_ID, our_peer_id, target_peer_id);
    let entity = peers.get_entity(our_peer_id, &path)?;
    decode_origin(&entity)
}

/// List **every** registered `(target_peer_id, origin)` under `our_peer_id`'s
/// registry — the canonical "what does this peer/domain host & reach" roster
/// the design's §8 browse-all front-door reads. Reads the durable **registry**
/// (not the raw deployment config), so it reflects every source that fed it:
/// the deployment-config `origins` map, `ENTITY_HOME_ORIGIN`, the e2e fixture,
/// and any returning-user override. One level (the immediate `{target}` keys),
/// sorted by peer-id, deduped. Arm-aware via [`Peers::tree_listing`] (Worker
/// arm: the reader must watch [`app_paths::site_origins_prefix`]).
pub fn list_origins(peers: &Peers, our_peer_id: &str) -> Vec<(String, String)> {
    let prefix = app_paths::site_origins_prefix(APP_ID, our_peer_id);
    let mut out: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for entry in peers.tree_listing(our_peer_id, &prefix) {
        let Some(rest) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        let target = rest.trim_start_matches('/');
        // The registry is one level — `{prefix}/{target}`. Skip empties and
        // any deeper path (defensive; the registry never nests today).
        if target.is_empty() || target.contains('/') {
            continue;
        }
        if let Some(origin) = get_origin(peers, our_peer_id, target) {
            out.insert(target.to_string(), origin);
        }
    }
    out.into_iter().collect()
}

fn decode_origin(entity: &Entity) -> Option<String> {
    let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let origin = value.as_map()?.iter().find_map(|(k, v)| match k.as_text() {
        Some("origin") => v.as_text().map(str::to_string),
        _ => None,
    })?;
    if origin.is_empty() {
        None
    } else {
        Some(origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_an_origin_through_the_tree() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();

        assert_eq!(get_origin(&peers, &pid, "PEERB"), None, "unregistered → None");

        set_origin(&peers, &pid, "PEERB", "http://localhost:8083/");
        assert_eq!(
            get_origin(&peers, &pid, "PEERB").as_deref(),
            Some("http://localhost:8083"),
            "trailing slash trimmed on store"
        );

        // A different target is independent.
        assert_eq!(get_origin(&peers, &pid, "PEERC"), None);
        set_origin(&peers, &pid, "PEERC", "https://labs.example");
        assert_eq!(get_origin(&peers, &pid, "PEERC").as_deref(), Some("https://labs.example"));
    }

    #[test]
    fn list_origins_returns_the_whole_registered_roster() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();

        // Empty registry → empty roster.
        assert!(list_origins(&peers, &pid).is_empty());

        // Register a few hosted peers from "different sources".
        set_origin(&peers, &pid, "PEERB", "http://b.example/");
        set_origin(&peers, &pid, "PEERA", "https://a.example");
        set_origin(&peers, &pid, "PEERC", "http://localhost:9/alice");

        // The roster is the full set, sorted by peer-id, trailing slash trimmed.
        let roster = list_origins(&peers, &pid);
        assert_eq!(
            roster,
            vec![
                ("PEERA".to_string(), "https://a.example".to_string()),
                ("PEERB".to_string(), "http://b.example".to_string()),
                ("PEERC".to_string(), "http://localhost:9/alice".to_string()),
            ]
        );
    }
}
