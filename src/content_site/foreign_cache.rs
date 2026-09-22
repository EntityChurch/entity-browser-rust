//! **The one entry point for reading someone else's bytes.**
//!
//! Every durable copy this client keeps of an artifact fetched from another
//! peer's origin — a site manifest, an app catalog, an app bundle — lives in our
//! own tree at that peer's natural path (`/{me}/{foreign}/…`). The durable tree
//! is the single source of truth *for state we own*; the moment a foreign
//! artifact is written into it, **it is a cache**, and it needs a freshness
//! model like any other. This module is that model.
//!
//! ## Why this module exists rather than a rule in a review checklist
//!
//! Four production incidents in one class (AP30's cache shape), and the audit
//! that enumerated them (`reviews/AUDIT-2026-08-28-CACHE-FRESHNESS-EVERY-COPY-OF-SOMEONE-ELSES-BYTES.md`)
//! found the cause is structural, not carelessness:
//!
//! 1. **The presence check short-circuits the layers that are correct.**
//!    [`Freshness::Mutable`](super::http_poll::Freshness) → `cache: no-store` is
//!    exactly right, and its comment names our exact symptom — but it never runs,
//!    because when the store already holds a copy **no request is issued at all**.
//! 2. **With no shared entry point owning the trigger, it is re-decided at every
//!    call site**, and six consumers disagreed six ways (once-per-open,
//!    every-boot, in-memory-miss, if-absent, if-absent, never).
//! 3. **The wrong answer is the natural one to write.** `if cached.is_none() {
//!    fetch() }` is correct for immutable content, and nothing about the store
//!    signals that this particular path holds someone else's *mutable* artifact.
//!
//! So the trigger moves to the layer that already knows `Mutable` from
//! `Immutable`, and [`Currency`] deliberately **has no variant meaning "I
//! already had one, so I did not look."**
//!
//! ## The mechanism — we did not have to invent change detection
//!
//! The substrate is content-addressed and the two-hop already carries the
//! answer. Hop 1 is a **pointer** at a stable tree path, fetched `no-store`, 58
//! bytes, whose content changes **if and only if** the entity changed. Hop 2 is
//! the body, addressed by that hash. So:
//!
//! ```text
//! H_remote = fetch_pointer(bin_url)         # hop 1 — exact, one conditional GET
//! H_local  = the content hash of the copy we hold
//! equal → our copy IS the current bytes. Stop. No body fetch, no write.
//! ```
//!
//! `H_local` costs nothing: an [`Entity`] carries its own `content_hash`, and it
//! is the same canonical `Hash::compute(type, data)` the pointer holds and the
//! two-hop verifies. **The pin is already in the tree** — which is also why
//! `CacheProvenance::pinned_root_hash` (a second, hand-rolled copy of the same
//! fact, which nothing ever compared) is redundant rather than merely unwired.
//!
//! ## The failure direction, and why it is safe on both arms
//!
//! A missing `H_local` means "fetch both hops and write" — costing a body
//! download, never serving a stale byte. That asymmetry is load-bearing on the
//! **Worker arm**, where the sync read answers from a per-subscription mirror
//! that fills asynchronously and may not hold a copy the durable store has.
//! **Do not "optimize" a mirror miss into a trust.**
//!
//! And when nothing is heard at all — offline origin, dead host, an expired
//! deadline — the answer is [`Currency::Unavailable`] and the held copy stands
//! **untouched**. Absence of evidence is never evidence (AP30 corollary (a)); a
//! cache that deletes what it cannot re-verify turns a brief outage into a
//! missing app.

use entity_entity::Entity;
use entity_hash::Hash;

use super::http_poll::{self, BinSource, PollError};
use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

/// A foreign artifact we keep a durable copy of: what it is, where it lives on
/// the origin, and where it lands in our tree.
///
/// A **closed set** on purpose. Adding a kind here is the moment to ask whether
/// the new thing needs a currency trigger — which is the question that went
/// unasked six times.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForeignArtifact {
    /// A content site's manifest — the site's mutable ref (title, nav, root).
    Manifest { peer: String, site: String },
    /// An app set's catalog — the launcher index for `{peer}/{set}`.
    AppCatalog { peer: String, set: String },
    /// An app's bundle — **the app's actual code**. The artifact that wedged
    /// every returning visitor, because it is the only one that moves when a
    /// publish ships new app code (`AppEntry` carries no hash and no version, so
    /// the catalog above it does not).
    AppBundle { peer: String, set: String, id: String },
    /// An app's asset-bundle index (`crate::apps::assets`) — the bundle's one
    /// mutable pointer. **Only the index needs currency**: every file it names
    /// is fetched by content hash through [`ensure_content`], which cannot go
    /// stale. A republished bundle moves exactly this pointer.
    AppAssetIndex { peer: String, set: String, id: String, bundle: String },
}

impl ForeignArtifact {
    /// The `.bin` pointer URL on the publisher's origin — hop 1's target.
    pub fn bin_url(&self, origin: &str) -> String {
        match self {
            Self::Manifest { peer, site } => http_poll::manifest_bin_url(origin, peer, site),
            Self::AppCatalog { peer, set } => http_poll::app_catalog_bin_url(origin, peer, set),
            Self::AppBundle { peer, set, id } => {
                http_poll::app_bundle_bin_url(origin, peer, set, id)
            }
            Self::AppAssetIndex { peer, set, id, bundle } => {
                http_poll::app_asset_index_bin_url(origin, peer, set, id, bundle)
            }
        }
    }

    /// Where the copy lives in **my** tree: the foreign peer's natural path, the
    /// same cache-at-natural-path shape the resolver's write-through uses.
    pub fn store_path(&self) -> String {
        match self {
            Self::Manifest { peer, site } => super::paths::manifest_path(peer, site),
            Self::AppCatalog { peer, set } => crate::apps::paths::catalog_path(peer, set),
            Self::AppBundle { peer, set, id } => crate::apps::paths::bundle_path(peer, set, id),
            Self::AppAssetIndex { peer, set, id, bundle } => {
                crate::apps::paths::asset_index_path(peer, set, id, bundle)
            }
        }
    }

    /// Whose bytes these are — the peer the origin was asked about.
    ///
    /// This is the join key for the health section's check 2 (*"the origin
    /// answers, and yet everything under one peer comes back 'not here'"*), so
    /// it must be the **publisher**, never the peer doing the caching.
    pub fn peer(&self) -> &str {
        match self {
            Self::Manifest { peer, .. }
            | Self::AppCatalog { peer, .. }
            | Self::AppBundle { peer, .. }
            | Self::AppAssetIndex { peer, .. } => peer,
        }
    }

    /// How this artifact is named in the refresh ledger, and from there in the
    /// health section's report.
    ///
    /// **Identifiers, not prose.** The sentence around them is composed in
    /// `doctor.rs`, which keeps every user-facing string on this path in one
    /// file — what makes translating that surface one extraction rather than a
    /// hunt.
    pub fn ledger_name(&self) -> String {
        match self {
            Self::Manifest { site, .. } => site.clone(),
            Self::AppCatalog { set, .. } => set.clone(),
            Self::AppBundle { set, id, .. } => format!("{set}/{id}"),
            Self::AppAssetIndex { set, id, bundle, .. } => format!("{set}/{id}/assets/{bundle}"),
        }
    }
}

/// The content hash of the copy we currently hold, if any.
///
/// A newtype, and the only way to build one is [`held_hash`] — so a consumer
/// cannot hand [`ensure_current`] a hand-rolled "I think this is current"
/// judgement, which is the entire defect this module exists to remove.
///
/// It is captured **synchronously, before the spawn**, because the app-tier
/// fetch paths deliberately borrow nothing across an await (they take a
/// `'static` writer handle first). That is a real constraint of this codebase,
/// not a design preference.
#[derive(Clone, Copy, Debug)]
pub struct HeldHash(Option<Hash>);

impl HeldHash {
    /// Nothing held — every fetch proceeds. Use when the caller has no `Peers`
    /// in hand (a fixture, a path we have deliberately never cached).
    pub fn none() -> Self {
        Self(None)
    }

    /// The held hash as hex, for a caller that keeps its own record beside the
    /// artifact (the site provenance ledger). **Never for deciding freshness** —
    /// that decision belongs to [`ensure_current`], which is the whole point of
    /// this module.
    pub fn hex(&self) -> Option<String> {
        self.0.as_ref().map(|h| h.to_hex())
    }
}

/// Read the content hash of the copy in **my** store, if it is there.
///
/// Sync, one L0/mirror read, no hashing: an [`Entity`] already carries its
/// canonical `content_hash`.
pub fn held_hash(peers: &Peers, me: &str, what: &ForeignArtifact) -> HeldHash {
    HeldHash(peers.get_entity(me, &what.store_path()).map(|e| e.content_hash))
}

/// The pins for **many** artifacts, taken in one synchronous pass.
///
/// The boot sweeps ([`crate::app::EntityApp::precache_origin_sites`],
/// [`super::discovery::warm_peer_sites`]) discover *which* sites exist only
/// after an await, and they may not borrow `Peers` across it. So they snapshot
/// what they hold up front — exactly as the sweep already did to build its
/// skip-set — and carry the pins into the spawned task.
///
/// **This replaces a presence set, and the difference is the whole fix.** The
/// old snapshot answered "do I have one of these?" and was used to skip the
/// fetch entirely; this one answers "which bytes do I have?" and is used to skip
/// only the *body download*. The request that says whether it moved is always
/// issued.
#[derive(Default, Clone)]
pub struct HeldSet(std::collections::BTreeMap<String, Hash>);

impl HeldSet {
    /// The pin for one artifact — `HeldHash::none()` when we hold nothing,
    /// which fetches. Never the other way around.
    pub fn get(&self, what: &ForeignArtifact) -> HeldHash {
        HeldHash(self.0.get(&what.store_path()).copied())
    }
}

/// Snapshot the pins for `artifacts`, synchronously.
pub fn held_set<'a>(
    peers: &Peers,
    me: &str,
    artifacts: impl IntoIterator<Item = &'a ForeignArtifact>,
) -> HeldSet {
    let mut out = std::collections::BTreeMap::new();
    for what in artifacts {
        let path = what.store_path();
        if let Some(e) = peers.get_entity(me, &path) {
            out.insert(path, e.content_hash);
        }
    }
    HeldSet(out)
}

/// What [`ensure_current`] did. **There is no "did not look" variant.**
#[derive(Debug)]
pub enum Currency {
    /// The source moved (or we held nothing): the body was fetched, verified,
    /// and written durably at the artifact's natural path.
    Fetched(Entity),
    /// Hop 1 says our copy is the current bytes. No body fetch, no write.
    Unchanged,
    /// Nothing was heard. **The held copy is untouched** — see the module docs.
    Unavailable(PollError),
}

impl Currency {
    /// Whether the durable copy changed as a result of this call — i.e. whether
    /// a surface reading it needs to re-render. `Unchanged` and `Unavailable`
    /// are both "nothing moved", which is what keeps this off the render loop.
    pub fn changed(&self) -> bool {
        matches!(self, Self::Fetched(_))
    }
}

/// **Ask the origin whether our copy is current, and make it current if it is
/// not.** Consumers call this unconditionally.
///
/// Hop 1 always. Hop 2 only when the pointer moved. One durable write, at the
/// artifact's natural path, only on a real change.
pub async fn ensure_current(
    src: &dyn BinSource,
    writer: &WriterHandle,
    held: HeldHash,
    origin: &str,
    what: &ForeignArtifact,
) -> Currency {
    let bin_url = what.bin_url(origin);
    // Hop 1 — 58 bytes, `no-store`. This is the request that four incidents
    // never issued.
    let remote = match http_poll::fetch_pointer(src, &bin_url).await {
        Ok(h) => h,
        Err(e) => return record(what, Currency::Unavailable(e)),
    };
    if held.0 == Some(remote) {
        return record(what, Currency::Unchanged);
    }
    // Hop 2 — content-addressed and hash-verified by `fetch_content`.
    match http_poll::fetch_content(src, origin, &remote).await {
        Ok(entity) => {
            writer.put(what.store_path(), entity.clone());
            record(what, Currency::Fetched(entity))
        }
        Err(e) => record(what, Currency::Unavailable(e)),
    }
}

/// **Make content-addressed bytes present in my store: fetch what is missing,
/// by hash, from `origin`.**
///
/// The one fetch in this module with **no currency check, and that is not an
/// exception to D24 — it is the case D24 exempts.** A body addressed by its own
/// hash cannot be stale: a different body has a different address, and
/// `fetch_content` verifies the bytes against the hash before anything is
/// written. So *"I hold it"* **is** *"I hold the current one"*, which is exactly
/// the inference this module exists to forbid for a mutable pointer, and
/// exactly the one that is sound here. What makes it safe is that the caller
/// can only name a hash, never a path.
///
/// Walks a `system/content/blob`'s declared chunks when the blob arrives or is
/// already held, so one call brings a whole file's closure in. Writes land in
/// `writer`'s content store; on the Worker arm that store has no put verb
/// (`WriterHandle::content_put`), so this returns `Unavailable`-shaped errors
/// rather than claiming success.
///
/// Not recorded in the refresh ledger: the ledger is about whether a
/// *publisher* answers for a *named* artifact, and a content fetch always
/// follows an index fetch that already recorded that fact.
pub async fn ensure_content(
    src: &dyn BinSource,
    writer: &WriterHandle,
    origin: &str,
    blob_hash: &Hash,
) -> Result<(), PollError> {
    let blob = match writer.content_get(blob_hash) {
        Some(b) => b,
        None => {
            let b = http_poll::fetch_content(src, origin, blob_hash).await?;
            writer.content_put(b.clone());
            b
        }
    };
    let chunks = crate::content_site::asset_store::chunk_hashes_of(&blob)
        .map_err(PollError::Decode)?;
    for ch in chunks {
        if writer.content_get(&ch).is_none() {
            let e = http_poll::fetch_content(src, origin, &ch).await?;
            writer.content_put(e);
        }
    }
    if writer.content_get(blob_hash).is_none() {
        return Err(PollError::Decode(
            "content fetched and not held afterwards — this arm cannot store content".into(),
        ));
    }
    Ok(())
}

/// **Every outcome lands in the refresh ledger, on the way out.**
///
/// This used to be three `refresh_ledger::record` calls in the Apps window —
/// one of the **three** consumers of `ensure_current`. The boot content-site
/// sweep (`app.rs`) and the discovery sweep (`content_site::discovery`) recorded
/// nothing, so Doctor's check 2 — documented as *"the signature from incident
/// A"* — could not see incident A's own fetches. It saw only the launcher, which
/// is incident **B**'s surface, and its `Agrees` line claimed *"every publisher
/// asked has served something"* over a set that was not the set that was asked.
///
/// Recording here is a **witness, not a notification** (AP44): this function is
/// the only legal way to fetch a foreign artifact — `tools/foreign-cache-lint.sh`
/// enforces that from the other side — so a consumer added tomorrow is covered
/// without knowing the ledger exists.
///
/// Note what is *not* recorded: nothing at all, ever, is skipped. `Unchanged` is
/// recorded as `Current` because *our copy is the current bytes* is the same
/// fact about the world as *we just fetched them* — a steady-state session where
/// nothing moves must not look like a session that never asked.
fn record(what: &ForeignArtifact, outcome: Currency) -> Currency {
    use crate::refresh_ledger::RefreshOutcome;
    let ledger_outcome = match &outcome {
        Currency::Fetched(_) | Currency::Unchanged => RefreshOutcome::Current,
        // The 404/network split is `PollError`'s, not ours, and it is carried
        // rather than flattened: waiting fixes one and never the other, so the
        // health section gives different advice.
        Currency::Unavailable(PollError::NotFound(_)) => RefreshOutcome::Withheld,
        Currency::Unavailable(e) => RefreshOutcome::Unreachable(e.to_string()),
    };
    crate::refresh_ledger::record(what.peer(), &what.ledger_name(), ledger_outcome);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::format::AppBundle;
    use crate::content_site::http_poll::fixture::PublishedFixture;
    use crate::content_site::http_poll::Freshness;
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use std::rc::Rc;

    const ORIGIN: &str = "http://publisher.example";

    /// A **real** publisher peer-id, generated.
    ///
    /// Not a placeholder string, and that is not fussiness: a write to a
    /// foreign-qualified path whose peer segment is not a well-formed peer-id is
    /// **silently dropped** by the tree. A test using `"PEERPUB"` therefore
    /// exercises the whole mechanism against a store that never kept anything —
    /// every read comes back `None`, `Unchanged` is unreachable, and the suite
    /// reports failures that have nothing to do with the code under test.
    /// (Measured, by writing it that way first.)
    fn foreign_peer() -> String {
        Peers::new_direct().primary_peer_id().to_string()
    }

    /// A [`BinSource`] over a [`PublishedFixture`] that **records every URL it
    /// is asked for**.
    ///
    /// The request log is not decoration: "unchanged costs no body download" is
    /// a claim about requests that were *not* made, and there is no way to
    /// assert it from the returned value. Without it, an implementation that
    /// re-downloaded the body every time and compared bytes afterwards would
    /// pass every other test in this module.
    struct RecordingSource {
        fx: PublishedFixture,
        seen: Rc<RefCell<Vec<String>>>,
        /// Fail every request — the offline origin.
        dead: bool,
    }

    impl BinSource for RecordingSource {
        fn get(
            &self,
            url: String,
            _freshness: Freshness,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>> {
            self.seen.borrow_mut().push(url.clone());
            if self.dead {
                // The exact shape a browser network failure takes in production
                // (`fetch_bytes` maps a rejected `fetch()` to `Decode`), not a
                // variant invented for the test.
                return Box::pin(std::future::ready(Err(PollError::Decode(
                    "fetch failed: origin is offline".into(),
                ))));
            }
            let r = self
                .fx
                .get(&url)
                .map(<[u8]>::to_vec)
                .ok_or(PollError::NotFound(404));
            Box::pin(std::future::ready(r))
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
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

    fn bundle_artifact(foreign: &str) -> ForeignArtifact {
        ForeignArtifact::AppBundle {
            peer: foreign.to_string(),
            set: "apps".into(),
            id: "marker-app".into(),
        }
    }

    /// A published origin serving one app bundle whose body carries `marker`.
    fn origin_serving(foreign: &str, marker: &str) -> PublishedFixture {
        let mut fx = PublishedFixture::new(ORIGIN);
        publish_bundle(&mut fx, foreign, marker);
        fx
    }

    fn publish_bundle(fx: &mut PublishedFixture, foreign: &str, marker: &str) {
        let bundle = AppBundle {
            html: format!("<!doctype html><body><h1>{marker}</h1></body>"),
        };
        fx.publish(&format!("{foreign}/apps/apps/bundles/marker-app.bin"), &bundle.to_entity());
    }

    /// A peer whose store we can read and write, plus its writer handle.
    fn me_with_writer() -> (Peers, String, WriterHandle) {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let writer = peers.writer_handle_for(&me).expect("direct arm always has a writer");
        (peers, me, writer)
    }

    fn body_requests(seen: &Rc<RefCell<Vec<String>>>) -> usize {
        seen.borrow().iter().filter(|u| u.contains("/content/")).count()
    }

    fn pointer_requests(seen: &Rc<RefCell<Vec<String>>>) -> usize {
        seen.borrow().iter().filter(|u| u.ends_with(".bin")).count()
    }

    // ── P — the positive case ─────────────────────────────────────────────

    #[test]
    fn an_artifact_we_do_not_hold_is_fetched_and_written() {
        let (peers, me, writer) = me_with_writer();
        let foreign = foreign_peer();
        let what = bundle_artifact(&foreign);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let src =
            RecordingSource { fx: origin_serving(&foreign, "V1"), seen: seen.clone(), dead: false };

        let out = block_on(ensure_current(
            &src,
            &writer,
            held_hash(&peers, &me, &what),
            ORIGIN,
            &what,
        ));

        assert!(matches!(out, Currency::Fetched(_)), "nothing held → fetch. Got {out:?}");
        let stored = peers
            .get_entity(&me, &what.store_path())
            .expect("the fetched bundle is written at the foreign peer's natural path");
        assert!(AppBundle::from_entity(&stored).html.contains("V1"));
    }

    #[test]
    fn a_republished_bundle_replaces_the_copy_we_hold() {
        // The production incident, in one test: same identity, same path, new
        // bytes behind a moved pointer.
        let (peers, me, writer) = me_with_writer();
        let foreign = foreign_peer();
        let what = bundle_artifact(&foreign);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut src =
            RecordingSource { fx: origin_serving(&foreign, "V1"), seen: seen.clone(), dead: false };
        block_on(ensure_current(&src, &writer, held_hash(&peers, &me, &what), ORIGIN, &what));

        // The publisher ships new app code. The old body stays served (a content
        // hotfix does not prune) — which is why the stale copy never 404s.
        publish_bundle(&mut src.fx, &foreign, "V2");
        let out = block_on(ensure_current(
            &src,
            &writer,
            held_hash(&peers, &me, &what),
            ORIGIN,
            &what,
        ));

        assert!(matches!(out, Currency::Fetched(_)), "the pointer moved → fetch. Got {out:?}");
        let stored = peers.get_entity(&me, &what.store_path()).expect("still cached");
        let html = AppBundle::from_entity(&stored).html;
        assert!(html.contains("V2"), "the returning read must be the NEW code: {html:?}");
        assert!(!html.contains("V1"), "the old bundle must not survive the republish");
    }

    // ── N1 — no-churn ─────────────────────────────────────────────────────

    #[test]
    fn an_unchanged_artifact_costs_one_pointer_and_no_body() {
        let (peers, me, writer) = me_with_writer();
        let foreign = foreign_peer();
        let what = bundle_artifact(&foreign);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let src =
            RecordingSource { fx: origin_serving(&foreign, "V1"), seen: seen.clone(), dead: false };
        block_on(ensure_current(&src, &writer, held_hash(&peers, &me, &what), ORIGIN, &what));
        assert_eq!(body_requests(&seen), 1, "the first call must download the body");
        seen.borrow_mut().clear();

        let out = block_on(ensure_current(
            &src,
            &writer,
            held_hash(&peers, &me, &what),
            ORIGIN,
            &what,
        ));

        assert!(matches!(out, Currency::Unchanged), "same pointer → Unchanged. Got {out:?}");
        assert_eq!(pointer_requests(&seen), 1, "hop 1 is ALWAYS issued — that is the point");
        assert_eq!(
            body_requests(&seen),
            0,
            "an unchanged artifact must not re-download its body: {:?}",
            seen.borrow()
        );
        assert!(!out.changed(), "Unchanged must not flip a surface dirty");
    }

    // ── N2 — absence of evidence ──────────────────────────────────────────

    #[test]
    fn an_offline_origin_leaves_the_held_copy_untouched() {
        let (peers, me, writer) = me_with_writer();
        let foreign = foreign_peer();
        let what = bundle_artifact(&foreign);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let src =
            RecordingSource { fx: origin_serving(&foreign, "V1"), seen: seen.clone(), dead: false };
        block_on(ensure_current(&src, &writer, held_hash(&peers, &me, &what), ORIGIN, &what));
        let before = peers.get_entity(&me, &what.store_path()).expect("cached");

        let dead = RecordingSource {
            fx: origin_serving(&foreign, "V1"),
            seen: Rc::new(RefCell::new(Vec::new())),
            dead: true,
        };
        let out = block_on(ensure_current(
            &dead,
            &writer,
            held_hash(&peers, &me, &what),
            ORIGIN,
            &what,
        ));

        assert!(matches!(out, Currency::Unavailable(_)), "nothing heard. Got {out:?}");
        let after = peers.get_entity(&me, &what.store_path()).expect(
            "an unreachable origin must NEVER remove a working cached copy — that would \
             turn a brief outage into a missing app",
        );
        assert_eq!(before.content_hash, after.content_hash, "the held copy must be byte-identical");
    }

    #[test]
    fn a_pointer_that_is_not_a_pointer_is_unavailable_never_changed() {
        // A truncated / garbage hop-1 body must not read as "it changed" — that
        // would let a broken origin overwrite good state with nothing.
        let (peers, me, writer) = me_with_writer();
        let foreign = foreign_peer();
        let what = bundle_artifact(&foreign);
        let mut fx = origin_serving(&foreign, "V1");
        fx.put(format!("{ORIGIN}/{foreign}/apps/apps/bundles/marker-app.bin"), vec![0xff, 0x00]);
        let src = RecordingSource { fx, seen: Rc::new(RefCell::new(Vec::new())), dead: false };

        let out = block_on(ensure_current(
            &src,
            &writer,
            held_hash(&peers, &me, &what),
            ORIGIN,
            &what,
        ));

        assert!(matches!(out, Currency::Unavailable(_)), "garbage hop 1 → Unavailable. Got {out:?}");
        assert!(
            peers.get_entity(&me, &what.store_path()).is_none(),
            "nothing was written from an unreadable pointer"
        );
    }

    // ── The pin itself ────────────────────────────────────────────────────

    #[test]
    fn the_held_hash_is_the_hash_the_pointer_carries() {
        // The claim the whole mechanism rests on: what the store holds and what
        // the origin publishes are the SAME canonical hash, so comparing them is
        // exact rather than a heuristic. If these ever diverge, `Unchanged`
        // becomes unreachable and every read silently re-downloads.
        let (peers, me, writer) = me_with_writer();
        let foreign = foreign_peer();
        let what = bundle_artifact(&foreign);
        let src = RecordingSource {
            fx: origin_serving(&foreign, "V1"),
            seen: Rc::new(RefCell::new(Vec::new())),
            dead: false,
        };
        block_on(ensure_current(&src, &writer, HeldHash::none(), ORIGIN, &what));

        let remote = block_on(http_poll::fetch_pointer(&src, &what.bin_url(ORIGIN)))
            .expect("the fixture serves a pointer");
        let held = held_hash(&peers, &me, &what);
        assert_eq!(held.0, Some(remote), "the store's content hash IS the published pointer");
    }

    // ── The OTHER artifact kind, because "same code path" is a claim ──────
    //
    // Every test above uses [`ForeignArtifact::AppBundle`], because that is the
    // artifact that wedged production. But A3 — the change that deleted
    // `precache_origin_sites`' "skip manifests I already hold" — moves the
    // **manifest** through this same entry point, and until these existed the
    // `Manifest` arm was exercised by exactly one assertion: the shape of its
    // URL. The two arms differ in `bin_url` / `store_path` and in nothing else,
    // which is precisely the kind of "obviously identical" that a typo in a
    // path builder makes false while every bundle test stays green.
    //
    // A stale manifest is a smaller blast radius than a stale bundle and worth
    // stating exactly: page bodies do NOT go stale (`resolve_closure_via` is a
    // pure-network two-hop with no store read), so what rots is the **directory
    // listing** — which sites a peer appears to have, and their titles.

    fn manifest_artifact(foreign: &str) -> ForeignArtifact {
        ForeignArtifact::Manifest { peer: foreign.to_string(), site: "labs".into() }
    }

    fn publish_manifest(fx: &mut PublishedFixture, foreign: &str, title: &str) {
        let manifest = crate::content_site::format::SiteManifest::new(
            "labs",
            title,
            "index",
            Vec::new(),
        );
        fx.publish(&format!("{foreign}/sites/labs/manifest.bin"), &manifest.to_entity());
    }

    fn origin_serving_manifest(foreign: &str, title: &str) -> PublishedFixture {
        let mut fx = PublishedFixture::new(ORIGIN);
        publish_manifest(&mut fx, foreign, title);
        fx
    }

    #[test]
    fn a_republished_manifest_replaces_the_copy_we_hold() {
        // **P for A3.** A publisher renames a site (or edits its nav) under a
        // stable identity. Before A1 this was a copy the boot sweep held forever
        // — `if already.contains(...) { continue; }` — so a returning profile's
        // directory rail showed a title nobody had used for weeks.
        let (peers, me, writer) = me_with_writer();
        let foreign = foreign_peer();
        let what = manifest_artifact(&foreign);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut src = RecordingSource {
            fx: origin_serving_manifest(&foreign, "Labs"),
            seen: seen.clone(),
            dead: false,
        };
        block_on(ensure_current(&src, &writer, held_hash(&peers, &me, &what), ORIGIN, &what));

        publish_manifest(&mut src.fx, &foreign, "Labs — Renamed");
        let out = block_on(ensure_current(
            &src,
            &writer,
            held_hash(&peers, &me, &what),
            ORIGIN,
            &what,
        ));

        assert!(matches!(out, Currency::Fetched(_)), "the pointer moved → fetch. Got {out:?}");
        let stored = peers.get_entity(&me, &what.store_path()).expect("still cached");
        assert_eq!(
            crate::content_site::format::SiteManifest::from_entity(&stored).title,
            "Labs — Renamed",
            "the directory listing must carry the title the publisher is serving NOW"
        );
    }

    #[test]
    fn an_unchanged_manifest_costs_one_pointer_and_no_body() {
        // **N1 for A3, and it is the half the boot sweeps pay for.** Both sweeps
        // now ask about every manifest on every boot; if an unchanged one cost a
        // body download, A3 would have traded a stale rail for a slower boot.
        let (peers, me, writer) = me_with_writer();
        let foreign = foreign_peer();
        let what = manifest_artifact(&foreign);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let src = RecordingSource {
            fx: origin_serving_manifest(&foreign, "Labs"),
            seen: seen.clone(),
            dead: false,
        };
        block_on(ensure_current(&src, &writer, held_hash(&peers, &me, &what), ORIGIN, &what));
        assert_eq!(body_requests(&seen), 1, "the first call must download the body");
        seen.borrow_mut().clear();

        let out = block_on(ensure_current(
            &src,
            &writer,
            held_hash(&peers, &me, &what),
            ORIGIN,
            &what,
        ));

        assert!(matches!(out, Currency::Unchanged), "same pointer → Unchanged. Got {out:?}");
        assert_eq!(pointer_requests(&seen), 1, "hop 1 is ALWAYS issued");
        assert_eq!(body_requests(&seen), 0, "an unchanged manifest re-downloaded its body");
    }

    #[test]
    fn an_offline_origin_leaves_the_held_manifest_untouched() {
        // **N2 for A3.** A cache that drops what it cannot re-verify turns a
        // brief outage into an EMPTY directory rail — the peer looks like it
        // hosts nothing, which reads as "gone" rather than "unreachable".
        let (peers, me, writer) = me_with_writer();
        let foreign = foreign_peer();
        let what = manifest_artifact(&foreign);
        let src = RecordingSource {
            fx: origin_serving_manifest(&foreign, "Labs"),
            seen: Rc::new(RefCell::new(Vec::new())),
            dead: false,
        };
        block_on(ensure_current(&src, &writer, held_hash(&peers, &me, &what), ORIGIN, &what));
        let before = peers.get_entity(&me, &what.store_path()).expect("cached");

        let dead = RecordingSource {
            fx: origin_serving_manifest(&foreign, "Labs"),
            seen: Rc::new(RefCell::new(Vec::new())),
            dead: true,
        };
        let out = block_on(ensure_current(
            &dead,
            &writer,
            held_hash(&peers, &me, &what),
            ORIGIN,
            &what,
        ));

        assert!(matches!(out, Currency::Unavailable(_)), "nothing heard. Got {out:?}");
        let after = peers.get_entity(&me, &what.store_path()).expect(
            "an unreachable origin must never empty the directory rail — that reads as \
             'this peer hosts nothing' rather than 'we could not ask'",
        );
        assert_eq!(before.content_hash, after.content_hash);
    }

    #[test]
    fn artifact_urls_and_paths_agree_with_the_publishers_layout() {
        let m = ForeignArtifact::Manifest { peer: "P".into(), site: "labs".into() };
        assert_eq!(m.bin_url("http://o"), "http://o/P/sites/labs/manifest.bin");
        assert_eq!(m.store_path(), "/P/sites/labs/manifest");

        let c = ForeignArtifact::AppCatalog { peer: "P".into(), set: "games".into() };
        assert_eq!(c.bin_url("http://o"), "http://o/P/apps/games/catalog.bin");
        assert_eq!(c.store_path(), "/P/apps/games/catalog");

        let b = bundle_artifact("P");
        assert_eq!(b.bin_url("http://o"), "http://o/P/apps/apps/bundles/marker-app.bin");
        assert_eq!(b.store_path(), "/P/apps/apps/bundles/marker-app");
    }
}

