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
/// The keyed registry read, **unresolved** — the primitive both layers sit on.
///
/// Private and it must stay that way: this is the operation the 2026-09-05
/// incident consisted of. It also exists to break a cycle that a first cut of
/// the fix walked straight into — [`get_origin`] routes through
/// [`list_origins`], and [`list_origins_raw`] used to build its rows by calling
/// `get_origin`, so defining one in terms of the other recursed until the stack
/// died. Caught on the first run of `get_origin_and_list_origins_agree_after_a_rekey`,
/// which is the entire argument for writing the agreement test rather than
/// reasoning that two functions "obviously" agree.
fn raw_origin(peers: &Peers, our_peer_id: &str, target_peer_id: &str) -> Option<String> {
    let path = app_paths::site_origin_path(APP_ID, our_peer_id, target_peer_id);
    let entity = peers.get_entity(our_peer_id, &path)?;
    decode_origin(&entity)
}

/// How many registry rows the supersession resolve **changed**, and to whom —
/// a boot-time D13 report, not a per-frame one.
///
/// The 2026-09-05 incident healed silently once fixed, and a silent heal is how
/// the next one gets missed: nothing anywhere said that a stored registration
/// named a publisher that no longer exists. [`list_origins`] runs every frame
/// and must stay quiet, so the report is taken **once**, by boot, through this.
///
/// Returns `(rows_before, rows_after, resolved_pairs)`. `resolved_pairs` is
/// empty on every boot but the one after a re-key, which is the point: a boot
/// that reports nothing is a boot where nothing moved.
pub fn supersession_effect(
    peers: &Peers,
    our_peer_id: &str,
) -> (usize, usize, Vec<(String, String)>) {
    let raw = list_origins_raw(peers, our_peer_id);
    let resolved: Vec<(String, String)> = raw
        .iter()
        .filter_map(|(peer, _)| {
            let live = crate::peer_supersession::resolve(peer);
            (live != *peer).then(|| (peer.clone(), live))
        })
        .collect();
    let after = apply_supersession(raw.clone(), crate::peer_supersession::resolve).len();
    (raw.len(), after, resolved)
}

/// **Defined in terms of [`list_origins`], deliberately — ONE expression of
/// "which publisher is live", not two that can drift (C15).**
///
/// The 2026-09-05 incident was a registry read that had not been resolved
/// against supersession. Fixing only `list_origins` would have left this
/// function as a second reader of the same registry answering a different
/// question, and the two disagree in *both* directions after a re-key: this one
/// would hand back the retired publisher's origin, and — the case a
/// resolve-then-lookup version still gets wrong — it would answer `None` for a
/// live successor whose origin is only reachable via the retired row that
/// carries it. Routing through the listing makes the three arms of
/// [`apply_supersession`] the single place that decision is made.
///
/// **Cost, stated:** a full listing per call rather than one keyed read. The
/// registry holds one row per publisher a deployment declares (one or two in
/// practice, `dist-federation`'s widest case is a handful) and every read is
/// in-memory on both arms, so this is a few map lookups. If that ever stops
/// being true, cache the listing per frame — do **not** reintroduce a second
/// path to the answer.
pub fn get_origin(peers: &Peers, our_peer_id: &str, target_peer_id: &str) -> Option<String> {
    let live = crate::peer_supersession::resolve(target_peer_id);
    list_origins(peers, our_peer_id)
        .into_iter()
        .find(|(peer, _)| *peer == live)
        .map(|(_, origin)| origin)
}

/// List **every** registered `(target_peer_id, origin)` under `our_peer_id`'s
/// registry — the canonical "what does this peer/domain host & reach" roster
/// the design's §8 browse-all front-door reads. Reads the durable **registry**
/// (not the raw deployment config), so it reflects every source that fed it:
/// the deployment-config `origins` map, `ENTITY_HOME_ORIGIN`, the e2e fixture,
/// and any returning-user override. One level (the immediate `{target}` keys),
/// sorted by peer-id, deduped. Arm-aware via [`Peers::tree_listing`] (Worker
/// arm: the reader must watch [`app_paths::site_origins_prefix`]).
/// **Superseded registrations are resolved here, and this is the chokepoint on
/// purpose (AP44).** A re-key leaves the retired publisher's registration in the
/// registry — nothing sweeps it, because the adoption path only ever *adds* —
/// so every caller of this function would otherwise have to remember to ask
/// `peer_supersession` itself. Seven do today and none did: the resolve had
/// exactly **two** call sites, both in `views/content_site/model.rs`, so the
/// content-site surface healed after a re-key and the Apps surface never did.
/// Found in production on 2026-09-05 by devops, on a real re-keyed deployment,
/// against a build whose own health checks were diagnosing the fault correctly
/// on the same boot (`fetch-failure-by-peer: diverges`, naming the dead peer)
/// while the resolver forty lines earlier could not see it.
///
/// **Read-time resolve, never a stored sweep** — the same stance
/// `peer_supersession` documents for the content-site call sites. The registry
/// keeps what it was told; what we *act on* is derived per read. Writing the
/// resolution down would be a durable record of a fact the live document owns
/// (AP30), and revalidation already exists to drop a record the domain
/// contradicts.
pub fn list_origins(peers: &Peers, our_peer_id: &str) -> Vec<(String, String)> {
    apply_supersession(
        list_origins_raw(peers, our_peer_id),
        crate::peer_supersession::resolve,
    )
}

/// [`list_origins`] **without** the supersession resolve — what is literally in
/// the registry.
///
/// **PRIVATE, and that is the enforcement point (AP44).** The incident this
/// module was changed for is a surface reading rows nobody had resolved, so the
/// fix must not be a convention that the next surface has to know about. There
/// is no way to obtain an unresolved listing from outside this module; if you
/// find yourself wanting one, you want [`list_origins`] and a reason written
/// down. Keeping [`apply_supersession`] pure is what makes the decision
/// testable without this function being reachable.
fn list_origins_raw(peers: &Peers, our_peer_id: &str) -> Vec<(String, String)> {
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
        if let Some(origin) = raw_origin(peers, our_peer_id, target) {
            out.insert(target.to_string(), origin);
        }
    }
    out.into_iter().collect()
}

/// Rewrite a raw registry listing so it names **live** publishers.
///
/// Pure, and takes the resolver as an argument rather than reaching for the
/// thread-local, so the whole decision is gated by `make test` on both arms —
/// `peer_supersession`'s own map is a thread-local and the cases that matter
/// here are *hypothetical* maps, which is the same reason `resolve_in` was split
/// out from `resolve`.
///
/// Three cases, and the middle one is the one a naive version gets wrong:
///
/// - **Not superseded** → kept verbatim. A peer nothing replaced resolves to
///   itself, so this is every row on every boot but the one after a re-key.
/// - **Superseded, successor is separately registered** → the retired row is
///   **dropped**. The successor's own registration was written by
///   `adopt_deployment_origin` from the live deployment document, which is
///   authoritative over anything we inferred; carrying the old origin forward
///   could only overwrite a better answer with a worse one.
/// - **Superseded, successor NOT registered** → the origin is **carried
///   forward** under the successor's id. Dropping here would be the tempting,
///   tidy choice and it is wrong: a re-key is normally the same publisher at the
///   same origin, so dropping the only row that names that origin turns a
///   recoverable re-key into an unreachable publisher — availability lost to
///   hygiene.
///
/// Two retired peers collapsing onto one unregistered successor is resolved by
/// first-wins over the sorted raw list. Deterministic, and it cannot arise from
/// the single-publisher recovery this mechanism supports (see `AGENTS.md` on why
/// re-key recovery is single-publisher).
fn apply_supersession(
    raw: Vec<(String, String)>,
    resolve: impl Fn(&str) -> String,
) -> Vec<(String, String)> {
    let registered: std::collections::BTreeSet<&str> =
        raw.iter().map(|(p, _)| p.as_str()).collect();
    let mut out: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for (peer, origin) in &raw {
        let live = resolve(peer);
        if live == *peer {
            out.insert(peer.clone(), origin.clone());
        } else if !registered.contains(live.as_str()) {
            out.entry(live).or_insert_with(|| origin.clone());
        }
        // else: superseded AND the successor speaks for itself — drop.
    }
    out.into_iter().collect()
}

/// One registry row the live deployment document has stopped naming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Withdrawn {
    pub peer_id: String,
    pub origin: String,
}

/// Which deployment-written registry rows the live document has **withdrawn** —
/// `DESIGN-RESILIENCE…` §1.1f **item 3**, the *un-name before you remove* rule
/// run in the other direction.
///
/// The adoption path only ever *adds*: `adopt_deployment_origin` writes a row per
/// declared origin and nothing has ever removed one, so a peer a domain stops
/// hosting keeps a registered origin that 404s forever. It shows up in the
/// browse-all roster [`list_origins`] feeds, it burns the retry ladder on every
/// visit, and Doctor's check 2 reports it as `fetch-failure-by-peer`.
///
/// **Pure, and it takes the whole listing rather than one row**, for the same
/// reason `apply_supersession` and `decide_home` are pure: this decides what to
/// *delete*, so every combination has to be gated by `make test` on both arms
/// instead of only through a boot that happens to produce it.
///
/// Four rules, and three of them are refusals:
///
/// - **An empty `declared` withdraws NOTHING.** `DeploymentConfig` parses
///   `origins` into a `BTreeMap`, so an **absent** `origins` key and an explicit
///   `origins: {}` arrive identically — the AP40 collapse, one field over from
///   the one `superseded` avoids by never emitting an empty key. Until the parse
///   can tell them apart, the only safe reading is the weaker one: a document
///   that names no origins is declining to say which peers it hosts, not
///   withdrawing every one of them. **Stated bound: a deployer cannot express
///   "I host nobody"**, and that is the direction we can afford to be wrong in.
/// - **A `user`-marked row is never withdrawn.** The Registry Browser's *Open in
///   Site Browser* registers peers the reader went and got, which no deployment
///   document has any claim over — the same line `adopt_deployment_origin` draws
///   with [`Adoption::KeptUserOverride`], drawn again here because a delete path
///   that honoured the mark only on writes would silently undo every override.
/// - **The home peer's row is never withdrawn**, even undeclared. A home the
///   *user* chose (D25) may name a peer this document does not host, and taking
///   its origin away turns "your home is a bit stale" into "your home cannot be
///   fetched at all". Pass the **resolved** home, not the document's.
/// - Everything else deployment-written and undeclared is **un-named**.
///
/// **A superseded row needs no special case, and the reason is worth keeping.**
/// Where a retired peer's entry is deliberately preserved through a re-key
/// transition it is still *in* `declared` (`HomeClaim::Takes` keeps sibling
/// origins), so it is kept by rule 3 and the carry-forward arm of
/// [`apply_supersession`] still has its row. Once the deployer drops it, the
/// successor has a declared row of its own and the retired one is already inert
/// at read time. That is the one distinction from `apply_supersession`'s
/// "availability lost to hygiene" warning: **that** function is acting on an
/// inference with no document in hand, and this one is acting on the live
/// document explicitly not naming the peer.
///
/// Un-naming is **reversible by the next publish** — a document that declares
/// the origin again re-seeds it as [`Adoption::Seeded`] on the next boot (E1) —
/// and it does **not** touch the peer's cached content under `/{foreign}/…`,
/// which stays readable exactly as a retired publisher's catalogs do (D24: a
/// cache that drops what it cannot re-verify turns an outage into a missing app).
/// We remove the *name*, never the bytes.
pub fn withdrawn_rows(
    rows: &[(String, String, String)],
    declared: &std::collections::BTreeMap<String, String>,
    home_peer: &str,
) -> Vec<Withdrawn> {
    if declared.is_empty() {
        return Vec::new();
    }
    rows.iter()
        .filter(|(peer, _, source)| {
            source != SOURCE_USER && peer != home_peer && !declared.contains_key(peer.as_str())
        })
        .map(|(peer, origin, _)| Withdrawn { peer_id: peer.clone(), origin: origin.clone() })
        .collect()
}

/// What one [`unname_withdrawn_origins`] pass did.
///
/// `unreadable` is its own fact rather than an empty `unnamed`: *we could not
/// list the registry* and *the registry holds nothing withdrawn* decide
/// different things, and only the first means the next boot still owes the sweep
/// (AP40 / AP30 corollary (a) — a read that cannot answer never changes state).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UnnameReport {
    pub examined: usize,
    pub unnamed: Vec<Withdrawn>,
    /// Removals the store refused. The row is still registered and will be
    /// re-offered next boot; counted so a persistent refusal is nameable.
    pub failed: usize,
    pub unreadable: bool,
}

/// Un-name every row [`withdrawn_rows`] identifies, durably.
///
/// **Call only with a document actually read this boot** — the same precondition
/// `peer_supersession::revalidate` carries, and for the same reason: no document
/// means no change, and D23's bounded fetch makes "no document" *more* common,
/// not less. A truncated document must never be able to empty the registry.
///
/// Runs **after** the adopt loop, so the rows the document does declare are
/// already written and the listing it judges is final.
pub async fn unname_withdrawn_origins(
    peers: &Peers,
    our_peer_id: &str,
    declared: &std::collections::BTreeMap<String, String>,
    home_peer: &str,
    timeout_ms: u32,
) -> UnnameReport {
    // NOTE: no `declared.is_empty()` short-circuit here, deliberately. The rule
    // lives in `withdrawn_rows` and nowhere else (C15) — a second copy would be
    // the cheaper code and an untestable guard, since the pure one already
    // returns nothing and the duplicate could be deleted with every test still
    // green. The cost is one listing read on a document that declares no
    // origins, which is a boot-path map walk.
    let mut report = UnnameReport::default();
    let rows = match list_origins_async(peers, our_peer_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "site-origins: could not list the registry — nothing un-named this boot"
            );
            report.unreadable = true;
            return report;
        }
    };
    report.examined = rows.len();
    for row in withdrawn_rows(&rows, declared, home_peer) {
        let path = origin_path(our_peer_id, &row.peer_id);
        match peers.remove_and_wait(our_peer_id, &path, timeout_ms).await {
            Ok(_) => {
                tracing::info!(
                    target_peer = %row.peer_id,
                    origin = %row.origin,
                    "site-origins: the deployment no longer declares this publisher — \
                     UN-NAMED its origin (its cached content is untouched, and a document \
                     that declares it again re-registers it on the next boot)"
                );
                report.unnamed.push(row);
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    target_peer = %row.peer_id,
                    "site-origins: could not un-name a withdrawn origin — it stays registered \
                     and will 404 until the next boot retries"
                );
                report.failed += 1;
            }
        }
    }
    if !report.unnamed.is_empty() || report.failed > 0 {
        tracing::info!(
            examined = report.examined,
            unnamed = report.unnamed.len(),
            failed = report.failed,
            declared = declared.len(),
            "site-origins: un-named the publishers the deployment stopped declaring"
        );
    }
    report
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

    /// `apply_supersession`'s three cases, as a pure function so every
    /// combination is gated by `make test` rather than only through a browser.
    /// The map is passed in, which is the same reason `resolve_in` exists.
    #[test]
    fn a_superseded_registration_resolves_to_the_live_publisher() {
        let map: std::collections::BTreeMap<&str, &str> = [("old", "new")].into_iter().collect();
        let resolve = |p: &str| map.get(p).map(|s| s.to_string()).unwrap_or_else(|| p.to_string());

        // 1. Successor separately registered → the retired row is DROPPED and
        //    the successor's own origin (from the live document) survives.
        assert_eq!(
            apply_supersession(
                vec![
                    ("old".into(), "http://old.example".into()),
                    ("new".into(), "http://new.example".into()),
                ],
                &resolve
            ),
            vec![("new".to_string(), "http://new.example".to_string())],
        );

        // 2. Successor NOT registered → the origin is CARRIED FORWARD. Dropping
        //    here is the tidy-looking answer and it loses the only row naming
        //    the origin the new publisher is actually served from.
        assert_eq!(
            apply_supersession(vec![("old".into(), "http://shared.example".into())], &resolve),
            vec![("new".to_string(), "http://shared.example".to_string())],
            "a re-key at the same origin must stay reachable, not be tidied away"
        );

        // 3. Nothing superseded → verbatim, which is every boot but one.
        assert_eq!(
            apply_supersession(vec![("other".into(), "http://other.example".into())], &resolve),
            vec![("other".to_string(), "http://other.example".to_string())],
        );
    }

    /// A carried-forward row must never overwrite a real registration, whichever
    /// order the raw list arrives in — the successor's own row was written from
    /// the live deployment document and outranks anything we inferred.
    #[test]
    fn a_carried_forward_origin_never_outranks_the_successors_own() {
        let map: std::collections::BTreeMap<&str, &str> = [("zzz", "aaa")].into_iter().collect();
        let resolve = |p: &str| map.get(p).map(|s| s.to_string()).unwrap_or_else(|| p.to_string());
        // `zzz` (retired) sorts AFTER `aaa` (its successor), so the successor is
        // inserted first and the retired row is reached second.
        assert_eq!(
            apply_supersession(
                vec![
                    ("aaa".into(), "http://authoritative.example".into()),
                    ("zzz".into(), "http://stale.example".into()),
                ],
                &resolve
            ),
            vec![("aaa".to_string(), "http://authoritative.example".to_string())],
        );
    }

    /// The other half of the 2026-09-05 fault: nothing swept the retired
    /// registration, so it survived every boot. Asserts the resolve happens at
    /// the chokepoint — a surface added tomorrow inherits it without knowing.
    #[test]
    fn a_retired_publishers_registration_does_not_survive_the_listing() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        set_origin(&peers, &me, "AAAold", "http://old.example");
        set_origin(&peers, &me, "ZZZnew", "http://new.example");

        assert_eq!(
            list_origins_raw(&peers, &me).len(),
            2,
            "the registry itself still holds both rows — the resolve is read-time, \
             not a stored sweep, so revalidation can still drop a false record"
        );

        crate::peer_supersession::set_all(
            [("AAAold".to_string(), "ZZZnew".to_string())].into_iter().collect(),
        );
        let live = list_origins(&peers, &me);
        crate::peer_supersession::set_all(Default::default());

        assert_eq!(
            live,
            vec![("ZZZnew".to_string(), "http://new.example".to_string())],
            "a retired publisher is still listed as reachable"
        );
    }

    /// `get_origin` and `list_origins` must never disagree about who is live —
    /// they are one expression now (C15), and this pins it in **both**
    /// directions, because a resolve-then-keyed-read version passes the first
    /// and fails the second.
    #[test]
    fn get_origin_and_list_origins_agree_after_a_rekey() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();

        // Only the RETIRED peer is registered — the successor has no row of its
        // own, so its origin is reachable only via the carried-forward one.
        set_origin(&peers, &me, "OLD", "http://shared.example");
        crate::peer_supersession::set_all(
            [("OLD".to_string(), "NEW".to_string())].into_iter().collect(),
        );

        let listed = list_origins(&peers, &me);
        let asked_new = get_origin(&peers, &me, "NEW");
        let asked_old = get_origin(&peers, &me, "OLD");
        crate::peer_supersession::set_all(Default::default());

        assert_eq!(listed, vec![("NEW".to_string(), "http://shared.example".to_string())]);
        assert_eq!(
            asked_new.as_deref(),
            Some("http://shared.example"),
            "the listing says NEW is reachable at this origin; asking about NEW directly \
             must not answer 'nowhere'"
        );
        assert_eq!(
            asked_old.as_deref(),
            Some("http://shared.example"),
            "a caller still holding the retired id is resolved forward, not refused"
        );
    }

    /// The boot report must be SILENT on an ordinary boot and specific on the
    /// one that matters — a report that fires every time is one nobody reads.
    #[test]
    fn the_supersession_report_is_silent_until_something_moves() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        set_origin(&peers, &me, "OLD", "http://old.example");
        set_origin(&peers, &me, "LIVE", "http://live.example");

        let (before, after, resolved) = supersession_effect(&peers, &me);
        assert_eq!((before, after), (2, 2));
        assert!(resolved.is_empty(), "nothing is superseded — this boot must log nothing");

        crate::peer_supersession::set_all(
            [("OLD".to_string(), "LIVE".to_string())].into_iter().collect(),
        );
        let (before, after, resolved) = supersession_effect(&peers, &me);
        crate::peer_supersession::set_all(Default::default());

        assert_eq!(before, 2, "the registry itself is untouched — read-time resolve");
        assert_eq!(after, 1, "the retired row collapses onto its successor");
        assert_eq!(
            resolved,
            vec![("OLD".to_string(), "LIVE".to_string())],
            "the report must name WHICH publisher was retired and WHO replaced it — \
             a count alone cannot be acted on"
        );
    }

    // ── §1.1f item 3 — un-naming a withdrawn publisher ────────────────────
    //
    // `withdrawn_rows` decides what to DELETE, so every arm gets a test here
    // rather than only the one a boot happens to produce. The shape is
    // `apply_supersession`'s: pure, whole-listing, no store.

    fn row(peer: &str, origin: &str, source: &str) -> (String, String, String) {
        (peer.to_string(), origin.to_string(), source.to_string())
    }

    fn declares(peers: &[&str]) -> std::collections::BTreeMap<String, String> {
        peers.iter().map(|p| (p.to_string(), format!("https://{p}.example"))).collect()
    }

    /// The plain case the item exists for: the domain stopped hosting a peer,
    /// so its registration stops naming one.
    #[test]
    fn a_publisher_the_document_stopped_declaring_is_un_named() {
        let rows = [
            row("kept", "https://kept.example", SOURCE_DEPLOYMENT),
            row("gone", "https://gone.example", SOURCE_DEPLOYMENT),
        ];
        let out = withdrawn_rows(&rows, &declares(&["kept"]), "kept");
        assert_eq!(
            out,
            vec![Withdrawn {
                peer_id: "gone".into(),
                origin: "https://gone.example".into()
            }],
            "only the undeclared row is withdrawn, and the report names its origin"
        );
    }

    /// **The guard that decides whether this feature is safe to ship.**
    /// `DeploymentConfig` parses `origins` into a `BTreeMap`, so an absent
    /// `origins` key and an explicit `origins: {}` are indistinguishable by the
    /// time they reach here — and `dist/` has no deployment document at all
    /// while a minimal one (`{"surface":"site"}`) names no origins. Sweeping on
    /// an empty declared set would empty the registry of every profile that
    /// booted against either.
    #[test]
    fn a_document_that_names_no_origins_withdraws_nothing() {
        let rows = [row("a", "https://a.example", SOURCE_DEPLOYMENT)];
        assert!(
            withdrawn_rows(&rows, &Default::default(), "").is_empty(),
            "declaring nothing is declining to say which peers are hosted — never a \
             withdrawal of all of them"
        );
    }

    /// A deployment document has no claim over a peer the reader went and got.
    /// The same line `adopt_deployment_origin` draws on the write path, drawn
    /// again on the delete path — honouring the mark only on writes would let a
    /// sweep silently undo every override.
    #[test]
    fn a_user_registered_origin_is_never_withdrawn_by_a_document() {
        let rows = [
            row("mine", "https://mine.example", SOURCE_USER),
            row("theirs", "https://theirs.example", SOURCE_DEPLOYMENT),
        ];
        let out = withdrawn_rows(&rows, &declares(&["other"]), "other");
        assert_eq!(
            out.iter().map(|w| w.peer_id.as_str()).collect::<Vec<_>>(),
            vec!["theirs"],
            "the user's row survives a document that declares neither"
        );
    }

    /// A home the **user** chose (D25) may name a peer this document does not
    /// host. Un-naming its origin turns "your home is stale" into "your home
    /// cannot be fetched", which is strictly worse than leaving a row that a
    /// later document can correct.
    #[test]
    fn the_resolved_home_peers_origin_survives_a_document_that_does_not_declare_it() {
        let rows = [row("myhome", "https://myhome.example", SOURCE_DEPLOYMENT)];
        assert!(
            withdrawn_rows(&rows, &declares(&["theirhome"]), "myhome").is_empty(),
            "the home peer is exempt even when the document declares someone else"
        );
        assert_eq!(
            withdrawn_rows(&rows, &declares(&["theirhome"]), "theirhome").len(),
            1,
            "and the exemption is the HOME, not a blanket pass — one bit apart"
        );
    }

    /// **The re-key transition, which is where a tidy version of this does
    /// damage.** After a re-key the document names the new peer as home and
    /// deliberately keeps the retired one's `origins` entry so old URLs resolve
    /// while visitors roll over. That entry is still *declared*, so the sweep
    /// must not touch it — `apply_supersession`'s carry-forward arm still needs
    /// the row it carries.
    #[test]
    fn a_retired_peer_the_document_still_routes_to_keeps_its_registration() {
        let rows = [
            row("2KOld", "https://cdn.example", SOURCE_DEPLOYMENT),
            row("2KNew", "https://cdn.example", SOURCE_DEPLOYMENT),
        ];
        assert!(
            withdrawn_rows(&rows, &declares(&["2KOld", "2KNew"]), "2KNew").is_empty(),
            "a routed retired peer is not a withdrawn one — this is the case the \
             affirmation rule exists for, one layer down"
        );
        // …and once the deployer tidies it away, it goes — the successor has a
        // declared row of its own, so nothing loses reachability.
        let out = withdrawn_rows(&rows, &declares(&["2KNew"]), "2KNew");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].peer_id, "2KOld");
    }

    /// The report separates *nothing to do* from *could not tell* (AP40). The
    /// pure half cannot produce `unreadable`, so this pins the default it is
    /// distinguished from.
    #[test]
    fn an_empty_pass_is_not_an_unreadable_one() {
        let report = UnnameReport::default();
        assert!(report.unnamed.is_empty());
        assert!(!report.unreadable, "a pass that found nothing has READ the registry");
    }

    /// The wiring, through a real store: the decision above is worth nothing if
    /// the row is still readable afterwards. Asserts the **listing**, not the
    /// return value — the removal is the subject, and `remove_and_wait` reporting
    /// success is not evidence that a reader stopped seeing the row (the
    /// recovery console's witness rule, one subsystem over).
    #[test]
    fn an_un_named_origin_is_gone_from_the_registry_and_its_neighbours_are_not() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        // `set_origin` marks USER, so write the two deployment rows the way boot
        // does — through the adopt path, which is the only writer of the
        // deployment mark.
        assert_eq!(adopt(&peers, &me, "kept", "https://kept.example"), Adoption::Seeded);
        assert_eq!(adopt(&peers, &me, "gone", "https://gone.example"), Adoption::Seeded);
        set_origin(&peers, &me, "mine", "https://mine.example");
        assert_eq!(list_origins(&peers, &me).len(), 3, "all three registered");

        let report = block_on(unname_withdrawn_origins(
            &peers,
            &me,
            &declares(&["kept"]),
            "kept",
            5_000,
        ));

        assert_eq!(report.examined, 3);
        assert_eq!(report.failed, 0);
        assert!(!report.unreadable);
        assert_eq!(
            report.unnamed.iter().map(|w| w.peer_id.as_str()).collect::<Vec<_>>(),
            vec!["gone"]
        );
        let after: Vec<String> =
            list_origins(&peers, &me).into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            after,
            vec!["kept".to_string(), "mine".to_string()],
            "the withdrawn row is gone from what a reader sees; the declared one and \
             the user's own are untouched"
        );
    }

    /// **The consequence, not the bookkeeping — the sibling of AP54's Apps
    /// gate, with withdrawal as the trigger instead of a re-key.**
    ///
    /// Registry rows are not rendered anywhere (measured: `list_origins` has
    /// seven consumers and every one of them either subscribes a prefix or picks
    /// a fetch target — **there is no surface that displays the registry**, which
    /// is most of why the 2026-09-05 incident was invisible). So the honest
    /// question is not *is the row gone* but *does a surface stop acting on it*,
    /// and `app_source` is where that is observable.
    ///
    /// **The PRECONDITION is the gate**, exactly as it is for the re-key twin: a
    /// profile holding the withdrawn publisher's catalog is what makes
    /// `app_source`'s "prefer the origin whose catalog we already hold" arm pick
    /// it. Without the warmed catalog the first loop finds nothing, the fallback
    /// takes "the first foreign origin", and the answer is decided by sort order
    /// — a pass with the defect fully present. The withdrawn peer is chosen to
    /// sort FIRST so even that fallback would hand it back.
    ///
    /// **Registered through the ADOPT path, not `set_origin`**, because
    /// `set_origin` marks `user` and a user-marked row is exempt by design. Using
    /// it here would make the sweep a no-op and the test vacuous.
    ///
    /// Falsified: neuter the removal in `unname_withdrawn_origins` and this reds
    /// with `apps_peer == the withdrawn publisher`.
    #[test]
    fn a_withdrawn_publisher_stops_serving_apps_to_a_returning_profile() {
        use crate::apps::format::AppCatalog;
        use crate::apps::paths;
        use crate::views::games::app_source;
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();

        // Real generated peer ids — `catalog_path` is peer-qualified, so a
        // placeholder string makes the warming `put` a silent no-op and the
        // precondition unreachable.
        let a = Peers::new_direct().primary_peer_id().to_string();
        let b = Peers::new_direct().primary_peer_id().to_string();
        let (withdrawn, kept) = if a < b { (a, b) } else { (b, a) };
        let (withdrawn, kept) = (withdrawn.as_str(), kept.as_str());

        assert_eq!(adopt(&peers, &me, withdrawn, "http://old.example"), Adoption::Seeded);
        assert_eq!(adopt(&peers, &me, kept, "http://new.example"), Adoption::Seeded);
        peers
            .writer_handle_for(&me)
            .expect("direct arm always has a writer")
            .put(
                paths::catalog_path(withdrawn, paths::APPS_SET),
                AppCatalog::default().to_entity(),
            );
        assert_eq!(
            app_source(&peers, &me, paths::APPS_SET).0,
            withdrawn,
            "PRECONDITION: the profile must be sourcing apps from the peer about to be \
             withdrawn, or this test cannot tell the fix from the defect"
        );

        // The deployer republishes declaring only the surviving peer.
        let declared: std::collections::BTreeMap<String, String> =
            [(kept.to_string(), "http://new.example".to_string())].into_iter().collect();
        let report =
            block_on(unname_withdrawn_origins(&peers, &me, &declared, kept, 5_000));
        assert_eq!(report.unnamed.len(), 1, "report: {report:?}");

        assert_eq!(
            app_source(&peers, &me, paths::APPS_SET).0,
            kept,
            "the Apps surface is still sourcing from a publisher the deployment has \
             STOPPED DECLARING. Its catalogs 404, and the preference for an origin we \
             already hold a catalog for is self-reinforcing — it can never recover on \
             its own (AP54, one trigger over)"
        );
        assert!(
            peers
                .get_entity(&me, &paths::catalog_path(withdrawn, paths::APPS_SET))
                .is_some(),
            "the withdrawn peer's CACHED CONTENT must be untouched — we un-name, we do \
             not sweep bytes (D24: a cache that drops what it cannot re-verify turns an \
             outage into a missing app, and there is still no export path)"
        );
    }

    /// The precondition, exercised rather than commented: with nothing declared
    /// the pass must not read as "everyone was withdrawn". Neutering the guard
    /// in [`withdrawn_rows`] empties this registry.
    #[test]
    fn a_pass_with_nothing_declared_leaves_the_registry_whole() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        assert_eq!(adopt(&peers, &me, "a", "https://a.example"), Adoption::Seeded);
        assert_eq!(adopt(&peers, &me, "b", "https://b.example"), Adoption::Seeded);

        let report =
            block_on(unname_withdrawn_origins(&peers, &me, &Default::default(), "a", 5_000));

        assert!(report.unnamed.is_empty());
        assert_eq!(list_origins(&peers, &me).len(), 2, "both rows survive");
    }

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
