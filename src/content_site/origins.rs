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
/// **Marked [`SOURCE_USER`]**, because this is the out-of-band setter: the
/// Registry Browser's *Open in Site Browser* (a peer the reader went and got,
/// via a signed binding that peer published about itself) and test seeding. A
/// deployment document will not overwrite one of these — it reports the
/// divergence and leaves it. The deployment's own registrations go through
/// [`adopt_deployment_origin`], which is the only writer of [`SOURCE_DEPLOYMENT`].
pub fn set_origin(peers: &Peers, our_peer_id: &str, target_peer_id: &str, origin: &str) {
    let path = origin_path(our_peer_id, target_peer_id);
    // Arm-aware seed write via the blessed router method: Direct → sync L0
    // (readable in the same pass; sync tests + immediate boot-seed depend
    // on it), Worker → async `dispatch_write`. No L0 hatch reach-through.
    peers.seed_write(our_peer_id, path, origin_entity_from(origin, SOURCE_USER));
}

/// Tree path of `target_peer_id`'s origin entry under `our_peer_id`'s
/// registry. Exposed so the owned boot-load step can seed it durably via
/// [`Peers::put_if_absent`](crate::peers::Peers::put_if_absent) instead of
/// the fire-and-forget [`set_origin`].
pub fn origin_path(our_peer_id: &str, target_peer_id: &str) -> String {
    app_paths::site_origin_path(APP_ID, our_peer_id, target_peer_id)
}

/// Who put an origin record here.
///
/// **This exists because `put_if_absent` cannot tell the two apart, and that
/// conflation is a live risk.** Boot used to register a deployment-config origin
/// with `put_if_absent`, rationale *"a returning user's override wins"* — but an
/// absence check cannot distinguish *"the user overrode this"* from *"we wrote
/// it ourselves last boot"*. The consequence: **a domain that moves a peer's
/// origin (new CDN, new host, same publisher identity) leaves every returning
/// profile on the old origin forever.** R1 does not catch it — R1 compares
/// *identity*, and the identity did not change. Preserving a real override needs
/// a **marked** override, not an absence check. (AP30, one layer down from the
/// record shape: the same "we already asked once" defect.)
pub const SOURCE_DEPLOYMENT: &str = "deployment";
/// A peer the **user** went and opened (the Registry Browser's *Open in Site
/// Browser*). A deployment document does not overwrite one of these; it says so
/// and leaves it.
pub const SOURCE_USER: &str = "user";

/// Build the origin registry entity for `origin` (trailing slash trimmed),
/// marked as deployment-written. See [`origin_entity_from`].
pub fn origin_entity(origin: &str) -> Entity {
    origin_entity_from(origin, SOURCE_DEPLOYMENT)
}

/// Build the origin registry entity for `origin`, marked with who wrote it.
pub fn origin_entity_from(origin: &str, source: &str) -> Entity {
    let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
        "origin" => entity_ecf::text(origin.trim_end_matches('/')),
        "source" => entity_ecf::text(source)
    });
    Entity::new(ORIGIN_TYPE, data).unwrap()
}

/// Who wrote the record at this path — **an unmarked record reads as
/// deployment-written**, and that default is a decision, not an accident.
///
/// Every record written before this field existed came from the boot
/// registration; treating them as user overrides would freeze exactly the strand
/// this marker exists to prevent, on every profile that has ever booted. There
/// has never been a surface where a user types an origin by hand, so the
/// conservative-looking alternative protects nothing that exists.
pub fn origin_source(peers: &Peers, our_peer_id: &str, target_peer_id: &str) -> Option<String> {
    let path = app_paths::site_origin_path(APP_ID, our_peer_id, target_peer_id);
    let entity = peers.get_entity(our_peer_id, &path)?;
    Some(decode_source(&entity))
}

fn decode_source(entity: &Entity) -> String {
    let Ok(value) = ciborium::from_reader::<ciborium::Value, _>(entity.data.as_slice()) else {
        return SOURCE_DEPLOYMENT.to_string();
    };
    value
        .as_map()
        .and_then(|m| {
            m.iter().find_map(|(k, v)| match k.as_text() {
                Some("source") => v.as_text().map(str::to_string),
                _ => None,
            })
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| SOURCE_DEPLOYMENT.to_string())
}

/// What [`adopt_deployment_origin`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Adoption {
    /// No record here before — the document's origin is now registered.
    Seeded,
    /// A deployment-written record named a different origin. **Replaced**, which
    /// is the whole point: this is the CDN move that used to strand a profile
    /// forever.
    Updated { from: String },
    /// The registered origin already matches. No write.
    Unchanged,
    /// A **user**-marked record disagrees. Kept, and the divergence reported
    /// rather than silently obeyed in either direction.
    KeptUserOverride { theirs: String },
}

/// Register `origin` for `target_peer_id` **from a deployment document**,
/// replacing a stale deployment-written value and never a user's.
///
/// Reads authoritatively (`get_entity_async`, not the Worker cache mirror — at
/// boot the registry prefix is not subscribed yet, so the sync read would
/// answer `None` and every boot would look like a fresh seed).
pub async fn adopt_deployment_origin(
    peers: &Peers,
    our_peer_id: &str,
    target_peer_id: &str,
    origin: &str,
    timeout_ms: u32,
) -> Result<Adoption, String> {
    let path = origin_path(our_peer_id, target_peer_id);
    let origin = origin.trim_end_matches('/').to_string();
    let existing = peers
        .get_entity_async(our_peer_id, &path)
        .await
        .map_err(|e| format!("origin read: {e}"))?;

    if let Some(entity) = existing {
        let source = decode_source(&entity);
        let current = decode_origin(&entity).unwrap_or_default();
        if current == origin {
            return Ok(Adoption::Unchanged);
        }
        if source == SOURCE_USER {
            return Ok(Adoption::KeptUserOverride { theirs: current });
        }
        peers
            .put_and_wait(
                our_peer_id,
                path,
                origin_entity_from(&origin, SOURCE_DEPLOYMENT),
                timeout_ms,
            )
            .await
            .map_err(|e| format!("origin update: {e}"))?;
        return Ok(Adoption::Updated { from: current });
    }

    peers
        .put_and_wait(
            our_peer_id,
            path,
            origin_entity_from(&origin, SOURCE_DEPLOYMENT),
            timeout_ms,
        )
        .await
        .map_err(|e| format!("origin seed: {e}"))?;
    Ok(Adoption::Seeded)
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

    // ── A moved origin, which used to be permanent ────────────────────────
    //
    // These are native and synchronous only because the Direct arm resolves
    // `get_entity_async` / `put_and_wait` immediately. The behaviour they pin is
    // the one that matters on a real deploy: a domain moving a peer to a new CDN
    // under a stable publisher identity.

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
        fn raw() -> RawWaker {
            fn noop(_: *const ()) {}
            fn clone(_: *const ()) -> RawWaker {
                raw()
            }
            RawWaker::new(std::ptr::null(), &RawWakerVTable::new(clone, noop, noop, noop))
        }
        let waker = unsafe { Waker::from_raw(raw()) };
        let mut cx = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    fn adopt(peers: &Peers, pid: &str, target: &str, origin: &str) -> Adoption {
        block_on(adopt_deployment_origin(peers, pid, target, origin, 5_000))
            .expect("the Direct arm always resolves")
    }

    /// **P** — the CDN move. This is the scenario that used to strand every
    /// returning visitor on all six domains, with no client-side recovery and no
    /// signal, presenting exactly like a re-key.
    #[test]
    fn a_deployment_that_moves_an_origin_updates_a_returning_profile() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();

        assert_eq!(adopt(&peers, &pid, "PUB", "https://old-cdn.example"), Adoption::Seeded);
        assert_eq!(get_origin(&peers, &pid, "PUB").as_deref(), Some("https://old-cdn.example"));

        // Same publisher identity, new host — the case R1 cannot see, because it
        // compares identity and the identity did not change.
        assert_eq!(
            adopt(&peers, &pid, "PUB", "https://new-cdn.example"),
            Adoption::Updated { from: "https://old-cdn.example".into() }
        );
        assert_eq!(get_origin(&peers, &pid, "PUB").as_deref(), Some("https://new-cdn.example"));
    }

    /// **N1** — a document that repeats itself writes nothing. Every boot
    /// re-registers every origin; if that were a write, it would be a durable
    /// write per hosted peer per boot for the life of the profile.
    #[test]
    fn a_repeated_origin_is_unchanged_not_rewritten() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        assert_eq!(adopt(&peers, &pid, "PUB", "https://cdn.example/"), Adoption::Seeded);
        assert_eq!(adopt(&peers, &pid, "PUB", "https://cdn.example"), Adoption::Unchanged);
        // …and the trailing slash is not a difference, or every boot would
        // "move" an origin that never moved.
        assert_eq!(adopt(&peers, &pid, "PUB", "https://cdn.example/"), Adoption::Unchanged);
    }

    /// **The override the old rationale claimed to protect, now actually
    /// protected** — and reported rather than silently obeyed either way.
    #[test]
    fn a_user_marked_origin_is_kept_and_the_divergence_reported() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        set_origin(&peers, &pid, "PUB", "https://mine.example");
        assert_eq!(
            adopt(&peers, &pid, "PUB", "https://theirs.example"),
            Adoption::KeptUserOverride { theirs: "https://mine.example".into() }
        );
        assert_eq!(
            get_origin(&peers, &pid, "PUB").as_deref(),
            Some("https://mine.example"),
            "a deployment document must not overwrite a peer the reader went and got"
        );
    }

    /// **The legacy default, stated as a test because it is a decision.** A
    /// record written before the marker existed reads as deployment-written and
    /// is therefore updatable. Treating it as an override would freeze exactly
    /// the strand this change exists to prevent, on every profile that has ever
    /// booted.
    #[test]
    fn an_unmarked_legacy_record_is_treated_as_deployment_written() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Exactly what the old `origin_entity` wrote: an `origin` key, no source.
        let legacy = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "origin" => entity_ecf::text("https://old-cdn.example")
        });
        peers.seed_write(
            &pid,
            origin_path(&pid, "PUB"),
            Entity::new(ORIGIN_TYPE, legacy).unwrap(),
        );
        assert_eq!(origin_source(&peers, &pid, "PUB").as_deref(), Some(SOURCE_DEPLOYMENT));
        assert_eq!(
            adopt(&peers, &pid, "PUB", "https://new-cdn.example"),
            Adoption::Updated { from: "https://old-cdn.example".into() }
        );
    }
}
