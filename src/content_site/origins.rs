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

    // One decision point, shared with `mirror_origins` — see
    // `adoption_decision`. Two copies of this branch is how "kept the user's"
    // and "no change" drift apart between two writers of the same registry.
    let outcome = adoption_decision(existing.as_ref(), &origin);
    if matches!(outcome, Adoption::Seeded | Adoption::Updated { .. }) {
        peers
            .put_and_wait(
                our_peer_id,
                path,
                origin_entity_from(&origin, SOURCE_DEPLOYMENT),
                timeout_ms,
            )
            .await
            .map_err(|e| format!("origin write: {e}"))?;
    }
    Ok(outcome)
}

/// Decide what a mirror/adopt write should do, given whatever is already at the
/// destination. **Pure** — no I/O — because this branch is the whole safety
/// argument and it should be testable without a store.
///
/// The four outcomes are [`Adoption`]'s, deliberately reused rather than given a
/// parallel vocabulary: they mean exactly the same four things here, and two
/// enums for one decision is how a "kept the user's" turns into a "no change"
/// somewhere downstream (AP40).
pub fn adoption_decision(existing: Option<&Entity>, incoming_origin: &str) -> Adoption {
    let incoming = incoming_origin.trim_end_matches('/');
    let Some(entity) = existing else {
        return Adoption::Seeded;
    };
    let current = decode_origin(entity).unwrap_or_default();
    if current == incoming {
        return Adoption::Unchanged;
    }
    if decode_source(entity) == SOURCE_USER {
        return Adoption::KeptUserOverride { theirs: current };
    }
    Adoption::Updated { from: current }
}

/// What one [`mirror_origins`] pass did.
///
/// Counts rather than a per-target list: an incident wants *how many moved and
/// did anything refuse*, and the per-target detail is already on its own log
/// line. `source_unheard` is separate from `0 seeded` for the AP30 reason — *we
/// could not read the source* and *the source had nothing* are different facts,
/// and only one of them means the destination is now knowingly incomplete.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MirrorReport {
    pub seeded: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub kept_user_override: usize,
    /// The source peer's registry could not be listed. **Nothing was written**
    /// — a read that cannot answer must never be able to change state
    /// (AP30 corollary (a)).
    pub source_unheard: bool,
    /// Targets whose record at the **destination** could not be read, and which
    /// were therefore left alone.
    ///
    /// Its own counter rather than silence, for two reasons. It is a distinct
    /// fact (AP40) — the pass ran, the source was fine, and these specific
    /// targets are unresolved — and, bluntly, **without it this arm cannot be
    /// falsified**: a destination we cannot read is usually one we cannot write
    /// either, so "skipped the read" and "tried to seed and the write failed"
    /// leave every other count at zero and look identical. Measured: the first
    /// version of `an_unreadable_destination_skips_that_target_rather_than_seeding_over_it`
    /// stayed **green** with the guard neutered.
    pub unreadable_targets: usize,
    /// Source and destination are the same peer, so there was nothing to do.
    /// This is the shipped default (a Site Browser on the system peer) and it
    /// is named rather than folded into an all-zero report, because *"nothing
    /// to do"* and *"a pass that found nothing"* are not the same fact.
    pub same_peer: bool,
}

impl MirrorReport {
    fn record(&mut self, outcome: &Adoption) {
        match outcome {
            Adoption::Seeded => self.seeded += 1,
            Adoption::Updated { .. } => self.updated += 1,
            Adoption::Unchanged => self.unchanged += 1,
            Adoption::KeptUserOverride { .. } => self.kept_user_override += 1,
        }
    }

    /// Did this pass change anything the reader will see?
    pub fn wrote_anything(&self) -> bool {
        self.seeded > 0 || self.updated > 0
    }
}

/// Copy every origin registered under `from_peer` into `to_peer`'s registry.
///
/// # Why this exists
///
/// The origin registry is **per-reader-peer by design**, and that is not an
/// accident to be flattened: the peer whose registry a Site Browser reads is the
/// same peer whose tree caches the fetched content (`site_cache_prefix`,
/// `resolve_from_my_store`, `persist_to_cache`), which is what makes a reload
/// and an offline read work at all. Widening the *read* across peers would break
/// that pairing; the registry has to be where the cache is.
///
/// But both production writers — boot's `adopt_deployment_origin` and the
/// Registry Browser's *Open in Site Browser* — write under
/// [`Peers::system_peer_id`](crate::peers::Peers::system_peer_id) only. So a
/// Site Browser bound to any **other** local peer reads an empty registry: every
/// foreign site resolves `Unreachable`, and the deployment's own home origin is
/// invisible. That is reachable today — `open "Site Browser" @somepeer` in the
/// Shell binds a window to any peer the profile has — and it is *also* the
/// latent half of the coming system/user split, where `system_peer_id()` stops
/// aliasing `primary_peer_id` and **every** Site Browser lands on the empty side.
///
/// So: make the writers reach the readers. This runs at spawn, is a no-op when
/// the bound peer *is* the system peer (the shipped default), and re-runs each
/// spawn, which is what gives a later deployment change a way to propagate.
///
/// # What it will not do
///
/// **Never overwrite a `user`-marked record at the destination.** A user's
/// *Open in Site Browser* under `to_peer` outranks a copy of the deployment's
/// idea of the same peer, exactly as it does in `adopt_deployment_origin`, and
/// the divergence is reported rather than obeyed in either direction (AP30/D24).
/// **The source's own mark is preserved**, not restamped as `deployment`: a copy
/// of a user override is still a user override, and restamping it would let the
/// next boot silently replace it.
///
/// **`tree_listing_async` / `get_entity_async`, not their sync twins — AP41.**
/// The sync pair answers from the per-prefix cache mirror, which at spawn is not
/// primed for a peer nobody has subscribed. This is a spawn-path pass with no
/// frame budget to protect.
pub async fn mirror_origins(
    peers: &Peers,
    from_peer: &str,
    to_peer: &str,
    timeout_ms: u32,
) -> MirrorReport {
    let mut report = MirrorReport::default();
    if from_peer == to_peer {
        report.same_peer = true;
        return report;
    }
    let source = match list_origins_async(peers, from_peer).await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(
                from_peer = %from_peer,
                to_peer = %to_peer,
                error = %e,
                "site-origin mirror: source registry unreadable — copying nothing"
            );
            report.source_unheard = true;
            return report;
        }
    };
    for (target, origin, src_mark) in source {
        let path = origin_path(to_peer, &target);
        let existing = match peers.get_entity_async(to_peer, &path).await {
            Ok(e) => e,
            Err(e) => {
                // Same rule as the listing: an errored read is not an answer, so
                // this target is skipped rather than blindly overwritten.
                tracing::warn!(
                    to_peer = %to_peer,
                    target = %target,
                    error = %e,
                    "site-origin mirror: destination unreadable for this target — skipping it"
                );
                report.unreadable_targets += 1;
                continue;
            }
        };
        let outcome = adoption_decision(existing.as_ref(), &origin);
        if matches!(outcome, Adoption::Seeded | Adoption::Updated { .. }) {
            if let Err(e) = peers
                .put_and_wait(
                    to_peer,
                    path,
                    origin_entity_from(&origin, &src_mark),
                    timeout_ms,
                )
                .await
            {
                tracing::warn!(
                    to_peer = %to_peer, target = %target, error = %e,
                    "site-origin mirror: write failed"
                );
                continue;
            }
        }
        if let Adoption::KeptUserOverride { theirs } = &outcome {
            tracing::info!(
                to_peer = %to_peer, target = %target,
                theirs = %theirs, source_origin = %origin,
                "site-origin mirror: kept this peer's own override"
            );
        }
        report.record(&outcome);
    }
    report
}

/// How long a mirror write may take before we give up on that target. Matches
/// the boot seed's budget — same store, same op.
pub const MIRROR_TIMEOUT_MS: u32 = 5_000;

/// Mirror the system peer's registry onto **every other local peer**, and report
/// each pass. Called from `boot_load`, immediately after the deployment
/// document's own origins have been adopted — so the source is final before it
/// is copied.
///
/// # Why this is a boot step and not a window-spawn step
///
/// A spawn-time hook is where this belongs on paper, and it is not available:
/// `Peers` is a non-`Clone` router, so a task spawned from a window factory
/// cannot hold it across an await, and this pass is a *conversation* (list the
/// source, then read each destination record before deciding to write). The
/// blessed escape hatches do not fit either — `WriterHandle` is write-only and
/// `DispatchHandle` is single-peer with no read. `boot_load` is an `async fn`
/// on the app that already awaits `adopt_deployment_origin` in a loop, so it is
/// the one place the borrow lives long enough.
///
/// **The bound that costs, stated rather than discovered:** an origin registered
/// *mid-session* — the Registry Browser's *Open in Site Browser* — reaches other
/// local peers only on the next boot. For the window that click opens this is
/// immaterial, because that window is bound to the system peer by construction
/// (`RegistryBrowserModel::reader_peer`); it matters only for a Site Browser
/// already open on some other peer, which is the narrow residue of the case
/// this whole pass exists to close.
pub async fn mirror_to_all_local_peers(peers: &Peers) {
    let system = peers.system_peer_id().to_string();
    for reader in peers.peer_ids() {
        if reader == system {
            continue;
        }
        let report = mirror_origins(peers, &system, &reader, MIRROR_TIMEOUT_MS).await;
        report_mirror(&system, &reader, &report);
    }
}

/// The D13 line for one mirror pass — one per Site Browser spawn, whatever
/// happened, for the reason [`crate::window_hydration::report`] exists: the
/// outcome that logs nothing is the one an incident needs.
pub fn report_mirror(from_peer: &str, to_peer: &str, r: &MirrorReport) {
    tracing::info!(
        from_peer = %from_peer,
        to_peer = %to_peer,
        outcome = %mirror_label(r),
        seeded = r.seeded,
        updated = r.updated,
        unchanged = r.unchanged,
        kept_user_override = r.kept_user_override,
        "site-origin registry mirrored onto the reader's peer"
    );
}

/// One word for what a pass did. Five outcomes, five words, and
/// `every_mirror_outcome_has_its_own_word` asserts the count so a sixth cannot
/// quietly reuse one.
pub fn mirror_label(r: &MirrorReport) -> &'static str {
    if r.same_peer {
        "not-needed"
    } else if r.source_unheard {
        "source-unheard"
    } else if r.unreadable_targets > 0 {
        // Checked before the write outcomes on purpose: a pass that moved two
        // origins and could not read a third is **partial**, and reporting it as
        // "mirrored" is the collapse AP40 is about.
        "partial"
    } else if r.wrote_anything() {
        "mirrored"
    } else if r.kept_user_override > 0 {
        "kept-override"
    } else {
        "already-current"
    }
}

/// `list_origins`, but subscription-independent on both arms — AP41.
///
/// Returns `(target_peer_id, origin, source_mark)`. Errors rather than returning
/// a short list, because a partial source listing would silently under-mirror
/// and the caller's whole job is completeness.
pub async fn list_origins_async(
    peers: &Peers,
    our_peer_id: &str,
) -> Result<Vec<(String, String, String)>, String> {
    let prefix = app_paths::site_origins_prefix(APP_ID, our_peer_id);
    let listing = peers
        .tree_listing_async(our_peer_id, &prefix)
        .await
        .map_err(|e| format!("origin listing: {e}"))?;
    let mut out: std::collections::BTreeMap<String, (String, String)> =
        std::collections::BTreeMap::new();
    for entry in listing {
        let Some(rest) = entry.path.strip_prefix(&prefix) else {
            continue;
        };
        let target = rest.trim_start_matches('/');
        if target.is_empty() || target.contains('/') {
            continue;
        }
        let path = origin_path(our_peer_id, target);
        let entity = peers
            .get_entity_async(our_peer_id, &path)
            .await
            .map_err(|e| format!("origin read {target}: {e}"))?;
        let Some(entity) = entity else { continue };
        if let Some(origin) = decode_origin(&entity) {
            out.insert(target.to_string(), (origin, decode_source(&entity)));
        }
    }
    Ok(out
        .into_iter()
        .map(|(t, (o, s))| (t, o, s))
        .collect())
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

    // ---------------------------------------------------------------------
    // The mirror — making the origin writers reach a reader bound to another
    // local peer. See `mirror_origins` for why this cannot be a wider read.
    // ---------------------------------------------------------------------

    fn origin_record(origin: &str, source: &str) -> Entity {
        origin_entity_from(origin, source)
    }

    /// The safety branch, exhaustively, with no store in the way. Every arm of
    /// the mirror and of `adopt_deployment_origin` routes through this one
    /// function, so this is where the decision is pinned.
    #[test]
    fn the_adoption_decision_covers_all_four_outcomes() {
        assert_eq!(adoption_decision(None, "https://a.example"), Adoption::Seeded);

        let same = origin_record("https://a.example", SOURCE_DEPLOYMENT);
        assert_eq!(adoption_decision(Some(&same), "https://a.example"), Adoption::Unchanged);
        assert_eq!(
            adoption_decision(Some(&same), "https://a.example/"),
            Adoption::Unchanged,
            "a trailing slash is not a different origin — otherwise every boot rewrites"
        );

        let moved = origin_record("https://old.example", SOURCE_DEPLOYMENT);
        assert_eq!(
            adoption_decision(Some(&moved), "https://new.example"),
            Adoption::Updated { from: "https://old.example".into() }
        );

        let mine = origin_record("https://mine.example", SOURCE_USER);
        assert_eq!(
            adoption_decision(Some(&mine), "https://theirs.example"),
            Adoption::KeptUserOverride { theirs: "https://mine.example".into() },
            "a user's own Open in Site Browser outranks a copy of the deployment's idea"
        );
    }

    /// The shipped default: every Site Browser today is bound to the system
    /// peer, so the pass must do nothing at all — and say so in its own word
    /// rather than as an all-zero report that reads like a pass that ran and
    /// found nothing.
    #[test]
    fn mirroring_a_peer_onto_itself_is_named_not_needed_and_writes_nothing() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        set_origin(&peers, &pid, "PUB", "https://pub.example");

        let r = block_on(mirror_origins(&peers, &pid, &pid, 5_000));
        assert!(r.same_peer);
        assert_eq!(mirror_label(&r), "not-needed");
        assert!(!r.wrote_anything());
        assert_eq!(r.seeded + r.updated + r.unchanged + r.kept_user_override, 0);
    }

    /// AP30 corollary (a): a source we cannot read changes nothing, and says
    /// which of the two silences it is. `source-unheard` must never collapse
    /// into "the source had nothing", because only one of them means the
    /// destination is now knowingly incomplete.
    #[test]
    fn an_unreadable_source_registry_mirrors_nothing_and_says_so() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();

        let r = block_on(mirror_origins(&peers, "PEER-NOBODY-HOSTS", &pid, 5_000));
        assert!(r.source_unheard, "an unknown peer is not an empty registry");
        assert_eq!(mirror_label(&r), "source-unheard");
        assert!(!r.wrote_anything());
        assert!(
            list_origins(&peers, &pid).is_empty(),
            "a read that could not answer must not be able to write"
        );
    }

    /// A source that genuinely holds nothing is `already-current`, not
    /// `source-unheard` — the other half of the pair above.
    #[test]
    fn an_empty_source_registry_is_not_reported_as_unheard() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let r = block_on(mirror_origins(&peers, &pid, "OTHER", 5_000));
        assert!(!r.source_unheard);
        assert_eq!(mirror_label(&r), "already-current");
    }

    /// `list_origins_async` is the AP41-correct twin of `list_origins` and the
    /// mirror's only source of truth — it must see exactly what the sync one
    /// sees on the arm where both work, and carry the source mark the sync one
    /// throws away.
    #[test]
    fn the_async_listing_agrees_with_the_sync_one_and_carries_the_source_mark() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        set_origin(&peers, &pid, "PEERB", "http://b.example/");
        block_on(adopt_deployment_origin(&peers, &pid, "PEERA", "https://a.example", 5_000))
            .expect("seeds");

        let sync: Vec<(String, String)> = list_origins(&peers, &pid);
        let asy = block_on(list_origins_async(&peers, &pid)).expect("lists");

        assert_eq!(
            sync,
            asy.iter().map(|(t, o, _)| (t.clone(), o.clone())).collect::<Vec<_>>(),
            "the two listings must not disagree about the roster"
        );
        let marks: std::collections::BTreeMap<&str, &str> =
            asy.iter().map(|(t, _, s)| (t.as_str(), s.as_str())).collect();
        assert_eq!(marks["PEERB"], SOURCE_USER, "set_origin marks user");
        assert_eq!(marks["PEERA"], SOURCE_DEPLOYMENT, "the boot adopt marks deployment");
    }

    /// An unreadable **destination** skips that target rather than overwriting
    /// it blind — the same rule as the source listing, one level in. Pinned
    /// because the tempting shortcut (treat an errored read as "absent, so
    /// seed") is precisely how a user override gets destroyed by a network
    /// hiccup.
    #[test]
    fn an_unreadable_destination_skips_that_target_rather_than_seeding_over_it() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        set_origin(&peers, &pid, "PUB", "https://pub.example");

        let r = block_on(mirror_origins(&peers, &pid, "PEER-NOBODY-HOSTS", 5_000));
        assert!(!r.source_unheard, "the source was fine; the destination was not");
        assert_eq!(
            r.seeded + r.updated + r.unchanged + r.kept_user_override,
            0,
            "every target was skipped, none was written blind"
        );
        // **This is the assertion that makes the test falsifiable**, and the
        // three above are not. A destination we cannot read is one we cannot
        // write either, so removing the guard makes the code try to seed, fail
        // the write, and leave every count above at zero — identical. Only a
        // positive record of *why* the target was skipped tells the two apart.
        assert_eq!(
            r.unreadable_targets, 1,
            "the target must be recorded as skipped-because-unreadable, not merely absent"
        );
        assert_eq!(mirror_label(&r), "partial");
    }

    /// Five outcomes, five words, and the **count** asserted so a sixth cannot
    /// quietly reuse one — the `Hydration` / `IndexLoad` gate, third instance.
    #[test]
    fn every_mirror_outcome_has_its_own_word() {
        let all = [
            MirrorReport { same_peer: true, ..Default::default() },
            MirrorReport { source_unheard: true, ..Default::default() },
            MirrorReport { unreadable_targets: 1, ..Default::default() },
            MirrorReport { seeded: 1, ..Default::default() },
            MirrorReport { kept_user_override: 1, ..Default::default() },
            MirrorReport { unchanged: 1, ..Default::default() },
        ];
        let labels: std::collections::BTreeSet<&str> =
            all.iter().map(mirror_label).collect();
        assert_eq!(labels.len(), all.len(), "two mirror outcomes share a label");
        assert_eq!(all.len(), 6, "a new outcome needs its own word and this count");

        // A partial pass that ALSO wrote is still partial — the ordering in
        // `mirror_label` is the claim, so pin it rather than trusting it.
        assert_eq!(
            mirror_label(&MirrorReport { seeded: 2, unreadable_targets: 1, ..Default::default() }),
            "partial",
            "two moved and one unreadable is not `mirrored`"
        );
    }
}
