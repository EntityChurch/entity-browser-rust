//! **B14/B15 design spike** — does a signed published root, projected as static
//! files, actually compose into an end-to-end authenticated fetch?
//!
//! This is the thing the routing packet asserted and nothing had run. It proves
//! the composition **and measures the projection**, which is what B14 needs
//! before a line of emit code is written.
//!
//! ## Why this is not a re-run of an upstream gate
//!
//! `core/peer/tests/publish_fetch_http_poll.rs` is the closest upstream test and
//! it deliberately does **not** drive `PublishedRootClient` (its divergence #2 —
//! it hand-rolls a Mechanism-A consumer so the wire shapes stay visible). So the
//! `PublishedRootClient` HAMT walk over a *static* origin — our exact B15 shape —
//! is unproven upstream. It is proven here.
//!
//! ## The measurement that sizes B14
//!
//! [`common::AskLog`] logs every hash the client asks for. That log **is the
//! projection spec**: exactly the blob set a static publish must write for the
//! signed root to be walkable. We do not guess the closure — we read it off a
//! successful walk.
//!
//! ## The finding this exists to pin
//!
//! Our publisher today (`src/content_site/publish_fixture.rs`) emits a hand-built
//! `path → hash` **pointer mirror** plus the authored bodies, and **no HAMT trie
//! nodes at all**. `the_signed_root_is_unwalkable_without_the_trie_nodes` is that
//! layout, and it fails — silently, as `Ok(None)`, indistinguishable from "this
//! page does not exist". That is the whole content of B14: the projection has to
//! grow the trie closure, not just gain a manifest file.
//!
//! The harness lives in `tests/common/mod.rs`; `registry_static_resolve.rs`
//! builds the naming layer on the same publisher.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use common::{text_body, Origin, Publisher, RecordingFetcher};
use entity_entity::Entity;
use entity_hash::Hash;
use entity_peer::published_root::{ContentFetcher, PublishedRootError};

/// A published page, as the site reader would hand it to the projector.
const PAGE_TYPE: &str = "app/content-site/page/v1";

/// The three site pages the spike publishes. Keys are **relative to the
/// declared prefix** `/{peer_id}/` — the peer-qualified §3.3 shape this
/// implementation publishes, and the form `PublishedRootClient::resolve` takes.
const PAGES: [(&str, &str); 3] = [
    ("sites/demo/pages/index", "the demo home page"),
    ("sites/demo/pages/about", "about the demo"),
    ("sites/entity-info/pages/index", "a second site, cross-linked"),
];

fn author(pages: &[(&str, &str)], p: &Publisher) {
    for (key, text) in pages {
        p.bind(
            key,
            Entity::new(PAGE_TYPE, text_body(text)).expect("authors"),
        );
    }
}

/// One publisher, one publish — the common case.
fn publish(seed: u8) -> common::Published {
    let p = Publisher::new(seed);
    author(&PAGES, &p);
    p.publish()
}

/// Today's projection: authored bodies + the pointer mirror, no trie nodes.
fn todays_projection(p: &common::Published) -> Origin {
    p.full.without_trie_nodes(&p.leaf_hashes)
}

// ---------------------------------------------------------------------------
// The gates
// ---------------------------------------------------------------------------

/// **The composition claim.** A pinned consumer fetches the signed root,
/// verifies it against the publisher key, walks the HAMT from the *signed*
/// hash, and gets back the authored bytes — over nothing but a static blob
/// store. No live peer, no path→hash index, no trust in the origin.
#[test]
fn a_pinned_consumer_walks_a_signed_root_over_a_static_origin() {
    let p = publish(7);
    let c = p.client(RecordingFetcher::new(p.full.clone()));

    let root = c.fetch_root().expect("signed root verifies against the pinned key");
    assert_eq!(root.peer_id, p.peer_id);
    assert_eq!(root.prefix, format!("/{}/", p.peer_id), "§3.3a declared prefix");
    eprintln!("first publish carries seq={}", root.seq);

    for (key, text) in PAGES {
        let entity = c
            .resolve(key)
            .unwrap_or_else(|e| panic!("resolve {key}: {e}"))
            .unwrap_or_else(|| panic!("{key} is absent from the signed tree"));
        assert_eq!(entity.entity_type, PAGE_TYPE);
        assert_eq!(entity.data, text_body(text), "{key} came back byte-exact");
    }
}

/// **The B14 finding, pinned.** Today's projection — authored bodies plus a
/// `path → hash` pointer mirror, and no trie nodes — cannot be walked from a
/// signed root. Note the failure mode: `Ok(None)`, not an error. A consumer
/// cannot tell "the publisher forgot to project its trie" from "that page does
/// not exist", which is why this has to be caught by the publisher.
#[test]
fn the_signed_root_is_unwalkable_without_the_trie_nodes() {
    let p = publish(8);
    let c = p.client(RecordingFetcher::new(todays_projection(&p)));

    // The root itself still verifies — signing is not the missing half.
    c.fetch_root().expect("the signed root verifies; only the walk is broken");

    for (key, _) in PAGES {
        assert!(
            matches!(c.resolve(key), Ok(None)),
            "{key} must read as ABSENT — the silent shape this test exists to name"
        );
    }
}

/// **The measurement that sizes B14.** Every hash a successful walk requested
/// is a file the static publish must write. Reported, not asserted on an exact
/// number — the HAMT's fan-out is upstream's business — but asserted to be
/// *strictly more* than the authored leaves, which is the claim that matters.
#[test]
fn the_walk_names_exactly_the_blobs_a_static_publish_must_project() {
    let p = publish(9);
    let fetcher = RecordingFetcher::new(p.full.clone());
    let log = fetcher.log();
    let c = p.client(fetcher);

    for (key, _) in PAGES {
        c.resolve(key).expect("resolves").expect("present");
    }

    let closure = log.closure();
    let leaves = PAGES.len();
    assert!(
        closure.len() > leaves,
        "a walk must touch interior trie nodes as well as the {leaves} leaves; \
         got {} — if this ever equals the leaf count the HAMT collapsed and the \
         measurement is meaningless",
        closure.len()
    );

    eprintln!(
        "B14 projection sizing: {} authored page(s) → {} blob(s) fetched by the walk \
         ({} interior node(s) our publisher does not emit today)",
        leaves,
        closure.len(),
        closure.len() - leaves
    );
}

/// **B14 sizing at production scale.** billslab publishes ~985 pages, so the
/// question that actually matters is how many *extra* files the trie closure
/// adds to a publish that already takes ~2 hours. Measured rather than
/// estimated, because a HAMT's node count is a function of fan-out and key
/// distribution, not something to reason about from the outside.
#[test]
fn the_trie_closure_is_measured_at_publication_scale() {
    const N: usize = 1000;
    let owned: Vec<(String, String)> = (0..N)
        .map(|i| {
            (
                format!("sites/big/pages/page-{i:04}"),
                format!("body of page {i}"),
            )
        })
        .collect();
    let pages: Vec<(&str, &str)> = owned
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let publisher = Publisher::new(14);
    author(&pages, &publisher);
    let p = publisher.publish();

    let total_blobs = p.full.blobs.len();
    let interior = total_blobs - N;

    assert!(interior > 0, "a {N}-key HAMT has interior nodes");
    assert!(
        interior < N,
        "interior nodes ({interior}) should be a fraction of {N} leaves, not a \
         multiple — if this ever inverts, the projection cost is the story"
    );

    let fetcher = RecordingFetcher::new(p.full.clone());
    let log = fetcher.log();
    let c = p.client(fetcher);
    c.resolve(pages[N / 2].0).expect("resolves").expect("present");
    let per_page = log.closure().len();

    eprintln!(
        "B14 at scale: {N} pages → {total_blobs} blobs ({interior} interior, \
         +{:.1}% over the leaves); one page costs {per_page} fetch(es) \
         (HAMT depth + the leaf)",
        (interior as f64 / N as f64) * 100.0,
    );
}

/// **Host-bytes-distrust (§1.2).** A hostile origin serving a body that does
/// not reproduce the hash it was asked for is refused — the property that makes
/// the origin untrusted, which is the whole point of the exercise.
#[test]
fn an_origin_that_swaps_one_page_for_another_is_refused() {
    let p = publish(10);

    // Locate the two authored leaf blobs by their bodies, then have the origin
    // answer a request for page 0 with page 1's bytes — a page swap, which is
    // the realistic attack on a static host and the one a `path → hash` index
    // could not defend against.
    let find = |text: &str| {
        let body = text_body(text);
        p.full
            .blobs
            .iter()
            .find(|(_, v)| v.windows(body.len()).any(|w| w == body.as_slice()))
            .map(|(k, v)| (*k, v.clone()))
            .unwrap_or_else(|| panic!("authored body for {text:?} is in the projection"))
    };
    let (victim, _) = find(PAGES[0].1);
    let (_, impostor_bytes) = find(PAGES[1].1);

    let c = p.client(RecordingFetcher::substituting(
        p.full.clone(),
        victim,
        impostor_bytes,
    ));

    let got = c.resolve(PAGES[0].0);
    assert!(
        matches!(got, Err(PublishedRootError::ContentHashMismatch)),
        "a swapped body decodes cleanly, so ONLY the hash recompute can catch \
         it — that is the property that makes the origin untrusted; got {got:?}"
    );
}

/// **The pin is what carries the trust.** A root signed by a different key is
/// refused even though it is internally well-formed — i.e. resolving a *name*
/// to the wrong peer-id cannot be rescued by the tree being valid.
#[test]
fn a_root_signed_by_another_key_is_refused_against_the_pin() {
    let real = publish(11);
    let impostor = publish(12);

    // The impostor's perfectly valid origin, offered under the real
    // publisher's pinned key.
    let c = real.client(RecordingFetcher::new(impostor.full.clone()));

    let got = c.fetch_root();
    assert!(
        matches!(
            got,
            Err(PublishedRootError::PeerIdMismatch { .. })
                | Err(PublishedRootError::SignatureInvalid)
        ),
        "a root from another key must not verify against the pin; got {got:?}"
    );
}

/// **Rollback rejection (§1.4), and the exact shape of the Q4 answer.** A
/// consumer that has seen `seq=N` refuses `seq<N` from the same origin. Note
/// what this does *not* buy, which is why the surface word is "verified as of
/// `published_at`": an origin that simply keeps serving `seq=N` forever is
/// indistinguishable from a publisher that has not published since.
#[test]
fn a_consumer_that_has_seen_a_newer_root_refuses_an_older_one() {
    let publisher = Publisher::new(13);
    author(&PAGES, &publisher);
    let v1 = publisher.publish();
    publisher.bind(
        "sites/demo/pages/index",
        Entity::new(PAGE_TYPE, text_body("the home page, edited")).expect("authors"),
    );
    let v2 = publisher.publish();

    assert!(
        v2.seq > v1.seq,
        "a second publish must advance seq ({} → {})",
        v1.seq,
        v2.seq
    );

    // v1 alone is validly signed — the rollback defence is *stateful*, not a
    // property of the older root, and this pins that distinction.
    let fresh = v2.client(RecordingFetcher::new(v1.full.clone()));
    fresh
        .fetch_root()
        .expect("v1 on its own verifies; nothing is wrong with the older root");

    // One client, an origin that serves the newer root and then reverts.
    let downgrade = v2.client(RollbackFetcher {
        newer: v2.full.clone(),
        older: v1.full.clone(),
        served: Mutex::new(0),
    });
    let seen = downgrade.fetch_root().expect("first fetch sees the newer root");
    assert_eq!(seen.seq, v2.seq);
    let got = downgrade.fetch_root();
    assert!(
        matches!(got, Err(PublishedRootError::SeqRollback { .. })),
        "an origin that reverts to an older signed root must be refused; got {got:?}"
    );
}

/// Serves the newer root once, then the older one — a downgrading origin.
/// Signatures are answered **by target**, so both roots verify on their own
/// and the only thing that can reject the second fetch is the `seq` check.
struct RollbackFetcher {
    newer: Origin,
    older: Origin,
    served: Mutex<usize>,
}

impl ContentFetcher for RollbackFetcher {
    fn manifest(&self) -> Result<Vec<u8>, String> {
        let mut n = self.served.lock().unwrap();
        *n += 1;
        Ok(if *n <= 1 {
            self.newer.manifest.clone()
        } else {
            self.older.manifest.clone()
        })
    }
    fn content(&self, hash: &Hash) -> Result<Vec<u8>, String> {
        self.newer
            .blobs
            .get(hash)
            .or_else(|| self.older.blobs.get(hash))
            .cloned()
            .ok_or_else(|| format!("404 {}", hash.to_hex()))
    }
    fn signature_for(&self, target: &Hash) -> Result<Option<Vec<u8>>, String> {
        let hex = target.to_hex();
        for o in [&self.newer, &self.older] {
            if o.head_hex == hex {
                return Ok(o.signature.clone());
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// B15 — driving the SYNC client from an ASYNC transport
// ---------------------------------------------------------------------------

/// The browser's problem in one struct: `ContentFetcher` is **sync** and
/// `fetch()` is **async**, and no async twin of `PublishedRootClient` exists
/// upstream (grepped: there is none). This fetcher serves only what is already
/// in a local cache and *records what it could not serve*, so the caller can
/// fetch those asynchronously and try again.
///
/// **A cache miss surfaces two different ways, and a pump must handle both.**
/// Measured here, not assumed:
///
/// - *Inside the HAMT walk*, `VerifyingFetchStore::get` swallows the fetch
///   error into `None`, so the miss arrives as `Ok(None)` — the same value a
///   genuinely absent key produces.
/// - *On the final leaf fetch*, `resolve` propagates it as
///   `Err(PublishedRootError::Fetch)` — the same value a real transport
///   failure produces.
///
/// So in **both** cases the miss is indistinguishable from a legitimate
/// outcome, in opposite directions. The miss log is what separates them, and
/// it is the pump's termination condition rather than an optimisation. A
/// browser implementation that only handled `Ok(None)` would fail on every
/// page — this test failed exactly that way before the leaf arm existed.
///
/// Second constraint the browser impl inherits: `PublishedRootClient` **owns**
/// its fetcher and exposes no accessor, so the cache and the miss log must be
/// `Arc`-shared handles held by the caller.
#[derive(Clone, Default)]
struct PumpState {
    cache: Arc<Mutex<BTreeMap<Hash, Vec<u8>>>>,
    misses: Arc<Mutex<BTreeSet<Hash>>>,
}

struct PumpFetcher {
    manifest: Vec<u8>,
    signature: Option<Vec<u8>>,
    state: PumpState,
}

impl ContentFetcher for PumpFetcher {
    fn manifest(&self) -> Result<Vec<u8>, String> {
        Ok(self.manifest.clone())
    }
    fn content(&self, hash: &Hash) -> Result<Vec<u8>, String> {
        let cached = self.state.cache.lock().unwrap().get(hash).cloned();
        match cached {
            Some(b) => Ok(b),
            None => {
                self.state.misses.lock().unwrap().insert(*hash);
                Err(format!("not cached: {}", hash.to_hex()))
            }
        }
    }
    fn signature_for(&self, _target: &Hash) -> Result<Option<Vec<u8>>, String> {
        Ok(self.signature.clone())
    }
}

/// **B15's design, validated.** A sync walk driven to completion over an async
/// transport, with no upstream change: resolve → collect misses → fetch them
/// (this is where `fetch().await` goes in the browser) → resolve again. It
/// terminates because each round either resolves or strictly grows the cache.
#[test]
fn the_sync_walk_can_be_driven_to_completion_over_an_async_transport() {
    let p = publish(15);
    let origin = p.full.clone();

    let state = PumpState::default();
    let client = p.client(PumpFetcher {
        manifest: origin.manifest.clone(),
        signature: origin.signature.clone(),
        state: state.clone(),
    });

    // The pump. In the browser each round's fetch is one `await`.
    let mut rounds = 0usize;
    let resolved = loop {
        rounds += 1;
        assert!(rounds < 32, "the pump must terminate; HAMT depth is small");

        let outcome = client.resolve(PAGES[0].0);
        // A verification failure is terminal — never retry it, or a hostile
        // origin gets an unbounded number of attempts to be believed.
        if let Err(e) = &outcome {
            assert!(
                matches!(e, PublishedRootError::Fetch(_)),
                "verification failure is terminal, not a cache miss: {e}"
            );
        }
        if let Ok(Some(entity)) = outcome {
            break entity;
        }

        // `Ok(None)` (miss inside the walk) or `Err(Fetch)` (miss on the leaf).
        // Only the miss log can say which of those is a miss at all.
        let outstanding: Vec<Hash> = {
            let mut m = state.misses.lock().unwrap();
            let took: Vec<Hash> = m.iter().copied().collect();
            m.clear();
            took
        };
        assert!(
            !outstanding.is_empty(),
            "nothing outstanding and no result ⇒ this is a real answer — the \
             key is absent, or the transport genuinely failed. The pump must \
             stop here rather than spin."
        );
        // ⟵ the async hop: in the browser this is `fetch(...).await`.
        for want in outstanding {
            let bytes = origin
                .blobs
                .get(&want)
                .unwrap_or_else(|| panic!("origin serves {}", want.to_hex()))
                .clone();
            state.cache.lock().unwrap().insert(want, bytes);
        }
    };

    assert_eq!(resolved.data, text_body(PAGES[0].1), "byte-exact after the pump");
    eprintln!(
        "B15 pump: resolved in {rounds} round(s) — one async hop per HAMT level \
         plus the leaf"
    );
}
