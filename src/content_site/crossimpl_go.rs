//! **The cross-implementation consume check** — a site published by
//! `entity-workbench-go`, walked by our reader, with no shared code on the path.
//!
//! ADR-0012's distinction is the whole reason this file exists: a signed-root
//! result verified only by the same language's reader is **cohort-consistent,
//! not independent convergence**. Every other gate in this tree publishes with
//! our emitter and consumes with our reader, so it can only prove we agree with
//! ourselves.
//!
//! ## The fixture is a real Go emission, vendored
//!
//! `tests/fixtures/crossimpl-go-site/` is the byte-for-byte output of
//! workbench-go's `publish/cmd/crossimpl-fixture` at workbench-go `d940ce0`
//! (core-go `7593618`). It is deterministic — a pinned Ed25519 seed
//! (`FixtureSeed`) and a pinned publish instant (`FixtureInstant`,
//! 2026-08-18T00:00:00Z) — so it can be vendored and re-cut:
//!
//! ```text
//! cd ../entity-workbench-go/publish && go run ./cmd/crossimpl-fixture -out /tmp/go-site
//! ```
//!
//! The instant had to be pinned: `published_at` is a field **of** the
//! published-root entity, so a fresh clock moves the root's content hash, the
//! `system/signature/{root_hex}.bin` binding named after it, two content shards
//! and `{out}/manifest` — every artifact a consumer enters through. That was
//! their self-reported defect, fixed in `d940ce0`; before it, no expected head
//! hash could have been pinned against this fixture at all.
//!
//! The site deliberately carries `private/not-published` **outside** the
//! declared prefix, and one page nested two levels deep so the trie has interior
//! nodes — a flat site can pass a walk that a nested one fails.

use std::path::PathBuf;

use super::http_poll::{BinSource, Freshness, PollError};
use super::publish_layout::PublishLayout;
use super::signed_fetch::{PinnedPublisher, SignedFetchError, SignedSession};
use super::signed_root::DirFetcher;
use entity_peer::published_root::PublishedRootClient;

/// The fixture's publisher. Pinned by `FixtureSeed`; changing it on their side
/// invalidates every hash below, which is why they treat the seed as a wire
/// constant.
const PEER_ID: &str = "2KLv2nhwtPrLFd4BZFQuNK1ujtE74q8cVg7y8cYdcZZ5BL";

/// The published-root **entity** hash, 66-char wire hex (leading algorithm
/// byte). The signature binding is named after this.
const PUBLISHED_ROOT_HEX: &str =
    "00e0138dbeb37469ffb6da6d26906a8a7c52880fe76cb8f7859968091aa2dfff0c";

/// The CHAMP trie root the signed root commits to.
const TRIE_ROOT_HEX: &str =
    "00f567bfbd1bb19b89a9d37881352fed1ac13278860f63c1320b9eec29f4503b89";

/// The four published pages and their authored bodies, as the publisher bound
/// them. What `resolve` takes is whatever is relative to the root's **declared
/// prefix**, which the gate reads off the root rather than assuming.
const PAGES: [(&str, &str); 4] = [
    ("docs/index", "authored bytes from the Go arm"),
    ("docs/intro", "second page"),
    ("docs/deep/one", "nested one level"),
    ("docs/deep/two/leaf", "nested two levels"),
];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/crossimpl-go-site")
}

/// The pin, derived from nothing but the peer-id — for Ed25519 canonical form
/// the peer-id embeds the 32 public-key bytes, so a consumer needs no key
/// distribution to hold this publisher.
fn client(fetcher: DirFetcher) -> PublishedRootClient<DirFetcher> {
    let pid = entity_crypto::PeerId::from(PEER_ID.to_string());
    let (pubkey, kt_byte) = pid
        .derive_public_key()
        .expect("the fixture peer-id is Ed25519 canonical form, so it carries its own key");
    let key_type = entity_crypto::KeyType::from_byte(kt_byte).expect("a key type we can verify");
    PublishedRootClient::new(fetcher, pubkey, key_type, Some(PEER_ID.to_string()))
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// **The cross-impl walk.** Their bytes, our reader, our verification: fetch the
/// signed root, check it against the key the peer-id carries, walk the HAMT from
/// the *signed* hash, and get the authored bodies back byte-exact.
///
/// Everything asserted here is a fact about **their** emission that only our
/// code decides: the manifest decodes, the signature verifies against the pin,
/// the root commits to the trie root they published, and the walk reaches leaves
/// at four different depths.
#[test]
fn a_go_published_site_is_walked_and_verified_by_our_reader() {
    let c = client(DirFetcher::new(fixture_dir(), PEER_ID));

    let root = c
        .fetch_root()
        .expect("the Go arm's signed root verifies against the key its peer-id carries");

    assert_eq!(root.peer_id, PEER_ID);
    assert_eq!(
        root.root_hash.to_hex(),
        TRIE_ROOT_HEX,
        "the signed root commits to the trie root they published"
    );
    eprintln!(
        "cross-impl root: seq={} prefix={:?} trie={}",
        root.seq,
        root.prefix,
        root.root_hash.to_hex()
    );

    for (key, body) in PAGES {
        let rel = key
            .strip_prefix(root.prefix.trim_start_matches('/'))
            .unwrap_or(key);
        let entity = c
            .resolve(rel)
            .unwrap_or_else(|e| panic!("resolve {rel:?} (bound at {key:?}): {e}"))
            .unwrap_or_else(|| panic!("{rel:?} is absent from their signed tree"));
        let text = String::from_utf8_lossy(&entity.data).to_string();
        assert!(
            text.contains(body),
            "{rel:?} came back with a body we did not author: {text:?}"
        );
    }
}

/// **Their §3.3a extent claim matches what they actually emitted.** The site
/// carries `private/not-published` outside the declared prefix, and it must not
/// be reachable through the signed root.
///
/// Stated precisely, because the weaker reading is the tempting one: this is a
/// fact about **their emission** that our reader can check — the path is not in
/// the trie, so it reads as absent. It is *not* evidence that our reader would
/// refuse a key a publisher wrongly committed outside its own declared prefix;
/// nothing here exercises that, and no gate in this tree does.
#[test]
fn a_path_outside_their_declared_prefix_is_not_in_the_signed_tree() {
    let c = client(DirFetcher::new(fixture_dir(), PEER_ID));
    let root = c.fetch_root().expect("root verifies");

    for key in ["private/not-published", "../private/not-published"] {
        let got = c.resolve(key);
        assert!(
            !matches!(got, Ok(Some(_))),
            "{key:?} sits outside the declared prefix {:?} and must not resolve; got {got:?}",
            root.prefix
        );
    }
}

// ---------------------------------------------------------------------------
// The shipped path — `SignedSession` over their advertised endpoint
// ---------------------------------------------------------------------------

/// The origin their fixture was published for (`-origin`, and what every URL in
/// their `transport-profile` is rooted at).
const THEIR_ORIGIN: &str = "https://go-arm.example";

/// **The gate that says the browser can consume a foreign publisher.**
///
/// The three gates above drive `DirFetcher`, which is our CLI's reader. This one
/// drives [`SignedSession`] — the *shipped* consumer, the same type
/// `session_cache::session_for_layout` hands the `name open` verb — pinned at
/// the layout their profile advertises, over a `BinSource` that is their origin
/// with the network taken out.
///
/// **It would have failed before this session and not for a subtle reason:**
/// `PinnedPublisher` built `manifest_url` from our own convention, so the
/// browser asked for `{origin}/{peer}/system/peer/published-root` and their root
/// is at `{origin}/manifest`. Hop 0, every time.
#[test]
fn the_shipped_consumer_resolves_a_go_published_page_over_their_advertised_layout() {
    let layout = their_layout();
    assert_eq!(
        layout.manifest_url,
        format!("{THEIR_ORIGIN}/manifest"),
        "read from their profile, not derived — deriving is the v1.8 violation"
    );

    let pin = PinnedPublisher::with_layout(PEER_ID, layout).expect("their peer-id carries its key");
    let session = SignedSession::new(pin);
    let src = OriginDir::new(fixture_dir(), THEIR_ORIGIN);

    for (key, body) in PAGES {
        let rel = key.strip_prefix("docs/").expect("their declared prefix");
        let entity = block_on(session.resolve(&src, rel))
            .unwrap_or_else(|e| panic!("shipped consumer resolving {rel:?}: {e}"));
        assert!(
            String::from_utf8_lossy(&entity.data).contains(body),
            "{rel:?} came back with a body we did not author"
        );
    }
    eprintln!(
        "cross-impl over the shipped consumer: {} fetch(es) for {} page(s)",
        src.count(),
        PAGES.len()
    );
}

/// **A key that is genuinely absent is `Absent`, not a transport failure** —
/// across implementations, which is where the distinction earns its keep. Their
/// origin is intact and answering; the key simply is not in the signed tree, and
/// a consumer that reported "the origin is down" here would send an operator
/// looking at the wrong machine.
#[test]
fn a_key_absent_from_their_tree_is_absent_not_a_transport_error() {
    let pin = PinnedPublisher::with_layout(PEER_ID, their_layout()).expect("pins");
    let session = SignedSession::new(pin);
    let src = OriginDir::new(fixture_dir(), THEIR_ORIGIN);

    let got = block_on(session.resolve(&src, "no/such/page"));
    assert!(
        matches!(got, Err(SignedFetchError::Absent)),
        "an absent key must not read as a broken origin; got {got:?}"
    );
}

/// Their emitted endpoint, decoded from the artifact they ship beside the site.
fn their_layout() -> PublishLayout {
    let bytes = std::fs::read(fixture_dir().join("transport-profile"))
        .expect("their publish ships a transport-profile beside the site");
    PublishLayout::from_profile_artifact(&bytes).expect("it decodes as an http-poll endpoint")
}

/// A [`BinSource`] over their projected directory: **their origin with the
/// network removed.** URLs arrive absolute (that is the point — they are the
/// publisher's, not ours), so the origin is stripped to get a file. A URL under
/// any *other* origin is a 404 rather than a silent read, because a consumer
/// fetching cross-origin bytes it did not mean to is exactly the failure the
/// pinning is for.
struct OriginDir {
    root: PathBuf,
    origin: String,
    fetched: std::cell::RefCell<usize>,
}

impl OriginDir {
    fn new(root: PathBuf, origin: &str) -> Self {
        Self { root, origin: origin.to_string(), fetched: std::cell::RefCell::new(0) }
    }
    fn count(&self) -> usize {
        *self.fetched.borrow()
    }
}

impl BinSource for OriginDir {
    fn get(
        &self,
        url: String,
        _freshness: Freshness,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, PollError>>>> {
        *self.fetched.borrow_mut() += 1;
        let Some(rel) = PublishLayout::relative_to_origin(&url, &self.origin) else {
            return Box::pin(std::future::ready(Err(PollError::NotFound(404))));
        };
        let r = std::fs::read(self.root.join(rel))
            .map_err(|e| crate::content_site::http_poll::poll_error_for_io(rel, &e));
        Box::pin(std::future::ready(r))
    }
}

/// Minimal executor — these futures are `!Send` by design (a wasm `JsFuture`
/// is), so no runtime is involved.
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

/// **The signature is keyed on the published-root entity hash, and both arms
/// agree which hash that is.** Its own gate because it is the one place a
/// two-hop pointer convention could silently key on the trie root instead — a
/// publisher who owned both ends would never notice.
#[test]
fn the_signature_binding_is_named_after_the_published_root_entity() {
    let path = fixture_dir()
        .join(PEER_ID)
        .join(format!("system/signature/{PUBLISHED_ROOT_HEX}.bin"));
    assert!(
        path.exists(),
        "their signature binding is at {}, keyed on the published-root entity hash",
        path.display()
    );
}
