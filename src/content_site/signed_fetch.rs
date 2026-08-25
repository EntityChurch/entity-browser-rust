//! **B15** — consume a signed published root in the browser.
//!
//! B14 made a published tree self-authenticating; this is the half that
//! *believes* it. A consumer pins the publisher's key, fetches the signed root,
//! verifies it, and walks the HAMT from the **signed** hash — re-hashing every
//! blob the origin hands back. The origin becomes untrusted: a page swapped for
//! another *validly authored* page is refused on the recompute, which is exactly
//! what the plain two-hop pointer path ([`super::http_poll`]) cannot do, because
//! there the host chooses which hash answers a path.
//!
//! ## The structural problem, and the pump that answers it
//!
//! `PublishedRootClient::resolve` is **sync**; a browser `fetch()` is **async**,
//! and there is no async twin upstream. The answer is not to fork the walk — it
//! is to drive it:
//!
//! ```text
//! resolve  →  collect misses  →  await the fetches  →  resolve again
//! ```
//!
//! It terminates because each round either resolves or **strictly grows the
//! cache**, and the walk is HAMT-depth deep. One async hop per level, plus the
//! leaf. Validated before a line of this was written —
//! `tests/published_root_walk.rs::the_sync_walk_can_be_driven_to_completion_over_an_async_transport`.
//!
//! ## Three traps, all of them silent
//!
//! 1. **A cache miss surfaces two different ways, oppositely.** Inside the walk
//!    `VerifyingFetchStore::get` swallows the fetch error to `Ok(None)` — the
//!    same value as a key that genuinely is not in the tree. On the **final leaf
//!    fetch** it propagates `Err(Fetch)` — the same value as a real transport
//!    failure. In both directions the miss is indistinguishable from a
//!    legitimate outcome, so **the miss log is the termination condition, not an
//!    optimisation**. An implementation that handles only `Ok(None)` fails on
//!    every page.
//! 2. **`signature_for` must return `Err` on a miss, never `Ok(None)`.**
//!    `Ok(None)` reaches `verify_signed_root` as *"this origin serves no
//!    signature"* → `SignatureMissing`, which is **terminal** — so a pump built
//!    on `Ok(None)` would report an unsigned origin the first time the signature
//!    simply had not been fetched yet. `Err` maps to `Fetch`, which is
//!    retryable, and that is the difference between "not yet" and "never".
//! 3. **A verification failure is terminal and must never be retried.** Retrying
//!    it hands a hostile origin an unbounded number of attempts to be believed.
//!    Only [`PublishedRootError::Fetch`] re-enters the pump.
//!
//! `PublishedRootClient` owns its fetcher and exposes no accessor, so the cache
//! and the miss logs are `Arc` handles held by the caller — that is not a style
//! choice, it is the only way to see inside.
//!
//! ## The layout this undoes
//!
//! Three shape mismatches between our projection and upstream's `content_url`
//! publisher, all of them the fetcher's business (the trait is ours to
//! implement) and none of them needing an upstream change:
//!
//! | | ours | upstream |
//! |---|---|---|
//! | manifest | 3-key wire entity at `{peer}/system/peer/published-root.bin` | `manifest_url` |
//! | content | sharded `content/{aa}/{bb}/{hex}` | flat `content_url` |
//! | signature | **two-hop** — a `system/hash` pointer at `{peer}/system/signature/{hex}.bin` | direct leaf at `signature_url` |

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use entity_crypto::KeyType;
use entity_entity::Entity;
use entity_hash::Hash;
use entity_peer::published_root::{ContentFetcher, PublishedRootClient, PublishedRootError};

use super::http_poll::{content_url, crack_pointer, BinSource, Freshness, PollError};
use super::publish_layout::PublishLayout;

/// How many pump rounds before we give up. The walk is HAMT-depth deep and the
/// measured projection resolves a page in **3 fetches**, so this is a runaway
/// guard, not a budget anyone should reach.
const MAX_ROUNDS: usize = 32;

/// A publisher we hold the key for. **The pin is the key, not the origin** —
/// which is what lets the same tree be served from anywhere, and what makes a
/// registry binding survive a republish (it names a peer-id, never a root).
#[derive(Clone)]
pub struct PinnedPublisher {
    /// HTTP origin the files are served from (`""` = same-origin).
    ///
    /// **Nothing on the signed path derives a URL from this any more** — that is
    /// [`layout`](Self::layout)'s job, per §6.5.3 v1.8. It is kept because it is
    /// *where we pinned this publisher*, which the first-origin-wins note in
    /// `session_cache` is about, and because the transport-trusted `.list`
    /// convenience surfaces still take an origin.
    pub origin: String,
    pub peer_id: String,
    pub pubkey: Vec<u8>,
    pub key_type: KeyType,
    /// **Where this publisher's artifacts actually are**, as *they* advertised —
    /// not as we would have laid them out. See
    /// [`PublishLayout`](super::publish_layout::PublishLayout): deriving the
    /// manifest by convention is the hop-0 defect the cross-implementation run
    /// found, and the reason this field exists rather than a second `format!`.
    pub layout: PublishLayout,
}

impl PinnedPublisher {
    /// **Pin a publisher at an origin the user typed, with no profile to read.**
    ///
    /// Falls back to our own layout convention — legitimate *here and nowhere
    /// else*, because it is the absence of an endpoint document rather than a
    /// shortcut past one (`name pin <peer-id> <origin>` gives us a URL, not an
    /// endpoint). A publisher reached through a registry binding **has**
    /// advertised transports, and that path goes through [`Self::with_layout`];
    /// deriving there is the §6.5.3 v1.8 violation.
    pub fn from_peer_id(origin: impl Into<String>, peer_id: &str) -> Option<Self> {
        let origin = origin.into();
        let layout = PublishLayout::conventional(&origin, peer_id);
        Self::with_layout(peer_id, layout).map(|p| Self { origin, ..p })
    }

    /// **Pin a publisher at the layout it advertised** — the path every
    /// binding-resolved consumer takes.
    ///
    /// **Derives the key from the peer-id alone**, which is what makes the whole
    /// chain work from one pinned string. A registry binding names a *peer-id*,
    /// not a public key, and there is no key distribution problem to solve:
    /// for Ed25519 in canonical form the peer-id **embeds** the public key —
    /// `bs58(varint(key_type) || varint(hash_type) || digest)` with
    /// `hash_type = identity`, so the digest **is** the 32 key bytes
    /// (`PeerId::derive_public_key`). Decode it and you hold the pin.
    ///
    /// Returns `None` for a peer-id whose form does not carry its key — the
    /// SHA-256 legacy form (`derive_public_key` refuses any non-identity
    /// `hash_type`), or a key type we cannot verify with. Those need an
    /// out-of-band key, and saying so beats guessing one.
    pub fn with_layout(peer_id: &str, layout: PublishLayout) -> Option<Self> {
        let pid = entity_crypto::PeerId::from(peer_id.to_string());
        let (pubkey, key_type_byte) = pid.derive_public_key()?;
        let key_type = KeyType::from_byte(key_type_byte).ok()?;
        Some(Self {
            origin: layout.origin_for(peer_id),
            peer_id: peer_id.to_string(),
            pubkey,
            key_type,
            layout,
        })
    }

    fn manifest_url(&self) -> String {
        self.layout.manifest_url.clone()
    }

    fn signature_pointer_url(&self, target: &Hash) -> String {
        self.layout.signature_pointer_url(&self.peer_id, target)
    }
}

/// Why a verified fetch did not produce bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedFetchError {
    /// The origin could not be reached, or served nothing at a URL we need.
    /// Retryable.
    Transport(String),
    /// The chain does not hold: bad signature, wrong key, `seq` rollback, or a
    /// body that does not hash to its address. **Terminal** — never retry.
    Verify(String),
    /// The key is genuinely not in the signed tree. Not a failure of the
    /// origin; a fabricated binding cannot appear here, which is the point.
    Absent,
    /// The origin did not produce a blob **its own signed root declares** —
    /// `tree/incomplete-walk`. **Terminal**, per `EXTENSION-TREE` §3.3a as
    /// ruled in arch `ROUTING-2026-08-18-l` §5: an origin failing to serve its
    /// committed closure is not a transient fault, and retrying grants a
    /// hostile origin unbounded attempts to be believed while turning a
    /// withholding into a hang.
    ///
    /// **This is the discriminator [`Transport`] used to swallow.** A withheld
    /// blob and an unreachable origin both arrived as `Transport`, which is the
    /// fourth appearance in this arc of *"absent" and "withheld" reaching the
    /// caller as the same value* (B14's `Ok(None)`; `--verify` over an
    /// incomplete closure; the structural fix for that one). The signal exists
    /// at the fetcher — an origin that answers 404 has *chosen* — and it was
    /// simply being discarded one layer down.
    ///
    /// **Not reachable from the manifest fetch, deliberately.** Incompleteness
    /// is only definable against a root you already hold; a 404 on the manifest
    /// means this origin serves no published root for this publisher at all,
    /// which is an unreachable publisher, not a short walk.
    ///
    /// [`Transport`]: Self::Transport
    IncompleteWalk(String),
    /// The pump did not converge. A structural bug or a pathological tree —
    /// reported rather than spun on.
    Budget,
}

impl std::fmt::Display for SignedFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "transport: {e}"),
            Self::Verify(e) => write!(f, "verification failed: {e}"),
            Self::Absent => write!(f, "not in the signed tree"),
            Self::IncompleteWalk(e) => {
                write!(f, "the origin withheld an entity its signed root declares: {e}")
            }
            Self::Budget => write!(f, "the walk did not converge"),
        }
    }
}

/// The caller's view inside the client's fetcher.
#[derive(Clone, Default)]
struct PumpState {
    content: Arc<Mutex<BTreeMap<Hash, Vec<u8>>>>,
    signatures: Arc<Mutex<BTreeMap<Hash, Vec<u8>>>>,
    content_misses: Arc<Mutex<BTreeSet<Hash>>>,
    signature_misses: Arc<Mutex<BTreeSet<Hash>>>,
}

struct PumpFetcher {
    /// Shared, because the manifest is the one MUTABLE artifact — it advances
    /// with `seq` on every republish — and it is refreshed per resolve while the
    /// client that reads it lives for the whole session.
    manifest: Arc<Mutex<Vec<u8>>>,
    state: PumpState,
}

impl ContentFetcher for PumpFetcher {
    fn manifest(&self) -> Result<Vec<u8>, String> {
        self.manifest
            .lock()
            .map(|m| m.clone())
            .map_err(|_| "manifest slot poisoned".to_string())
    }

    fn content(&self, hash: &Hash) -> Result<Vec<u8>, String> {
        let cached = self.state.content.lock().ok().and_then(|c| c.get(hash).cloned());
        match cached {
            Some(b) => Ok(b),
            None => {
                if let Ok(mut m) = self.state.content_misses.lock() {
                    m.insert(*hash);
                }
                Err(format!("not cached: {}", hash.to_hex()))
            }
        }
    }

    fn signature_for(&self, target: &Hash) -> Result<Option<Vec<u8>>, String> {
        let cached = self.state.signatures.lock().ok().and_then(|c| c.get(target).cloned());
        match cached {
            Some(b) => Ok(Some(b)),
            // `Err`, NOT `Ok(None)` — see trap 2 in the module docs. `Ok(None)`
            // means "this origin serves no signature" and is terminal.
            None => {
                if let Ok(mut m) = self.state.signature_misses.lock() {
                    m.insert(*target);
                }
                Err(format!("signature not cached: {}", target.to_hex()))
            }
        }
    }
}

/// A live consumer of one pinned publisher.
///
/// **Hold this across page loads — do not build one per fetch.** Two things live
/// in it, and both are lost by a per-call client:
///
/// 1. **The `seq` floor.** `PublishedRootClient` enforces monotonicity against
///    the highest `seq` it has seen *in its own lifetime*. A fresh client per
///    page means an origin can serve root `seq 5` for one page and `seq 3` for
///    the next — a rollback to a previous publish, page by page — and nothing
///    notices. That is the whole point of `SeqRollback`, and a short-lived
///    client silently discards it.
/// 2. **The content cache.** Trie interior nodes are shared by every key in the
///    tree, so the second page of a site costs the leaf and little else.
///    Measured: **5 fetches for the first page, 2 for the next** on the same
///    site.
pub struct SignedSession {
    pin: PinnedPublisher,
    state: PumpState,
    manifest: Arc<Mutex<Vec<u8>>>,
    client: PublishedRootClient<PumpFetcher>,
}

impl SignedSession {
    pub fn new(pin: PinnedPublisher) -> Self {
        let state = PumpState::default();
        let manifest = Arc::new(Mutex::new(Vec::new()));
        let client = PublishedRootClient::new(
            PumpFetcher { manifest: Arc::clone(&manifest), state: state.clone() },
            pin.pubkey.clone(),
            pin.key_type,
            Some(pin.peer_id.clone()),
        );
        Self { pin, state, manifest, client }
    }

    pub fn pin(&self) -> &PinnedPublisher {
        &self.pin
    }

    /// Resolve `relative_key` from the signed tree.
    ///
    /// `relative_key` is peer-relative — `sites/{site}/pages/{slug}`, the same
    /// key space the projection's trie was built over.
    pub async fn resolve<S: BinSource + ?Sized>(
        &self,
        src: &S,
        relative_key: &str,
    ) -> Result<Entity, SignedFetchError> {
        // The manifest needs no hash and is MUTABLE, so it is re-fetched
        // `no-store` on every resolve rather than cached with the content. That
        // refresh is also what makes the `seq` floor meaningful: a stale
        // manifest can never roll back, because we never look at it twice.
        let bytes = src
            .get(self.pin.manifest_url(), Freshness::Mutable)
            .await
            .map_err(|e| SignedFetchError::Transport(format!("manifest: {e}")))?;
        match self.manifest.lock() {
            Ok(mut m) => *m = bytes,
            Err(_) => return Err(SignedFetchError::Transport("manifest slot poisoned".into())),
        }

        for _ in 0..MAX_ROUNDS {
            match self.client.resolve(relative_key) {
                Ok(Some(entity)) => return Ok(entity),
                // A verification failure is TERMINAL. Only a fetch error
                // re-enters the pump; anything else is the origin failing to
                // prove itself, and retrying gives it unbounded attempts.
                Err(e) if !matches!(e, PublishedRootError::Fetch(_)) => {
                    return Err(SignedFetchError::Verify(e.to_string()))
                }
                // `Ok(None)` (a miss inside the walk, or a genuine absence) and
                // `Err(Fetch)` (a miss on the leaf, or a genuine transport
                // failure) are BOTH ambiguous. Only the miss log separates them.
                _ => {}
            }

            let content_wanted = drain(&self.state.content_misses);
            let signature_wanted = drain(&self.state.signature_misses);
            if content_wanted.is_empty() && signature_wanted.is_empty() {
                // Nothing outstanding and no result ⇒ that was a real answer.
                return Err(SignedFetchError::Absent);
            }

            // ⟵ the async hop.
            for target in signature_wanted {
                let bytes = fetch_signature(src, &self.pin, &target).await?;
                if let Ok(mut c) = self.state.signatures.lock() {
                    c.insert(target, bytes);
                }
            }
            for want in content_wanted {
                // `want` came out of the miss log, which means a trie node the
                // signed root commits to NAMED this hash. So a 404 here is the
                // origin refusing its own committed closure — terminal — while
                // a 5xx or a dead socket is not. See
                // `SignedFetchError::IncompleteWalk`.
                let bytes = src
                    .get(self.pin.layout.content_url(&want), Freshness::Immutable)
                    .await
                    .map_err(|e| declared_fetch_error(&want.to_hex(), e))?;
                // **Verify before caching, or a corrupted INTERIOR node reads
                // as "that name is not bound".**
                //
                // The leaf is safe without this: upstream hashes it and returns
                // `ContentHashMismatch`, which is terminal here. An interior
                // node is not. `VerifyingFetchStore::get` (core/peer) does
                // `verify_content(..).ok()?` — a mismatch becomes `None`, the
                // trie walk reads that as "no such branch", `resolve` returns
                // `Ok(None)`, and with the bad bytes sitting happily in our
                // cache there is no outstanding miss to re-drive the pump. The
                // walk therefore ends as `Absent` → `NameError::NotBound`:
                // *nobody has claimed that name*, said about an origin that
                // just served bytes matching no hash it committed to.
                //
                // Measured on the shipped chain by S3
                // (`a_single_flipped_byte_in_a_served_body_is_refused_as_a_
                // verification_failure`), which is why the check is here rather
                // than assumed. Fixing it upstream would mean giving that store
                // a way to report a mismatch, which `ContentStore::get` has no
                // room for — and `core/*` is the kernel, not ours. Our cache is
                // ours, so the honest place is the moment we admit bytes to it.
                //
                // Fifth appearance of one seam: "absent" and "corrupt/withheld"
                // keep arriving as the same value.
                if let Err(e) = crate::content_site::http_poll::verify_and_decode(&bytes, &want) {
                    return Err(SignedFetchError::Verify(format!(
                        "content {}: {e:?} — the origin served bytes that do not hash to the \
                         address its own signed root committed to",
                        want.to_hex()
                    )));
                }
                if let Ok(mut c) = self.state.content.lock() {
                    c.insert(want, bytes);
                }
            }
        }
        Err(SignedFetchError::Budget)
    }
}

/// One-shot convenience. **Prefer [`SignedSession`]** for anything that fetches
/// more than once from the same publisher — see its docs for what a per-call
/// client throws away.
pub async fn resolve_signed<S: BinSource + ?Sized>(
    src: &S,
    pin: &PinnedPublisher,
    relative_key: &str,
) -> Result<Entity, SignedFetchError> {
    SignedSession::new(pin.clone()).resolve(src, relative_key).await
}

/// The signature is a **two-hop** in our layout: a `system/hash` pointer names
/// the signature entity's blob. Both hops happen here so the sync fetcher sees
/// one cached answer.
async fn fetch_signature<S: BinSource + ?Sized>(
    src: &S,
    pin: &PinnedPublisher,
    target: &Hash,
) -> Result<Vec<u8>, SignedFetchError> {
    let ptr = src
        .get(pin.signature_pointer_url(target), Freshness::Immutable)
        .await
        .map_err(|e| declared_fetch_error("signature pointer", e))?;
    // A malformed pointer is the ORIGIN's failure to serve a chain, not a
    // transport hiccup — retrying it would spin.
    let hash = crack_pointer(&ptr)
        .map_err(|e| SignedFetchError::Verify(format!("signature pointer: {e}")))?;
    src.get(pin.layout.content_url(&hash), Freshness::Immutable)
        .await
        .map_err(|e| declared_fetch_error("signature body", e))
}

/// Classify a fetch failure for an entity the signed root **declares**.
///
/// The one place the terminal/retryable split is decided, so a new call site
/// cannot re-collapse it by reaching for `Transport` out of habit. Only reach
/// for this where the hash or path came from inside the committed chain — for
/// anything the root has not named yet (the manifest), a failure is plain
/// transport.
fn declared_fetch_error(what: &str, e: PollError) -> SignedFetchError {
    match e {
        PollError::NotFound(_) => SignedFetchError::IncompleteWalk(format!("{what}: {e}")),
        _ => SignedFetchError::Transport(format!("{what}: {e}")),
    }
}

fn drain(set: &Arc<Mutex<BTreeSet<Hash>>>) -> Vec<Hash> {
    match set.lock() {
        Ok(mut s) => {
            let took: Vec<Hash> = s.iter().copied().collect();
            s.clear();
            took
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::content_site::format::{NavItem, SiteManifest, SitePage};
    use crate::content_site::http_poll::PollError;
    use crate::content_site::publish_fixture::emit_owned_sites;
    use crate::content_site::read::OwnedSite;
    use crate::content_site::signed_root::RootProjector;
    use std::cell::RefCell;
    use std::future::Future;
    use std::path::{Path, PathBuf};
    use std::pin::Pin;

    const KEY: &str = "sites/home/pages/index";
    const BODY: &str = "# the bytes a pinned consumer gets";

    /// A [`BinSource`] over an emitted publish directory — the real transport
    /// with the network taken out, which is the only part of B15 a native test
    /// cannot exercise. It **counts fetches**, because the pump's cost is one of
    /// the things worth pinning.
    struct DirSource {
        root: PathBuf,
        fetched: RefCell<Vec<String>>,
        /// URLs to answer with the bytes of a DIFFERENT (validly authored) blob.
        substitute: RefCell<BTreeMap<String, Vec<u8>>>,
        /// Substrings whose URLs fail as a **transport fault** rather than a
        /// 404 — a 5xx or a dead socket. The origin never says "absent", so a
        /// blob that is genuinely present is simply unreachable this attempt.
        unreachable: RefCell<Vec<String>>,
    }

    impl DirSource {
        fn new(root: &Path) -> Self {
            Self {
                root: root.to_path_buf(),
                fetched: RefCell::new(Vec::new()),
                substitute: RefCell::new(BTreeMap::new()),
                unreachable: RefCell::new(Vec::new()),
            }
        }
        /// Make every URL containing `needle` fail the way an overloaded origin
        /// fails: an answer that is not "absent".
        fn make_unreachable(&self, needle: &str) {
            self.unreachable.borrow_mut().push(needle.to_string());
        }
        fn count(&self) -> usize {
            self.fetched.borrow().len()
        }
    }

    impl BinSource for DirSource {
        fn get(
            &self,
            url: String,
            _freshness: Freshness,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>> {
            self.fetched.borrow_mut().push(url.clone());
            if let Some(bytes) = self.substitute.borrow().get(&url) {
                return Box::pin(std::future::ready(Ok(bytes.clone())));
            }
            if self.unreachable.borrow().iter().any(|n| url.contains(n.as_str())) {
                return Box::pin(std::future::ready(Err(PollError::Decode(
                    "HTTP 503 (origin unreachable)".into(),
                ))));
            }
            let rel = url.trim_start_matches('/');
            let r = std::fs::read(self.root.join(rel))
                .map_err(|e| crate::content_site::http_poll::poll_error_for_io(rel, &e));
            Box::pin(std::future::ready(r))
        }
    }

    /// Minimal executor — these futures are `!Send` by design (a wasm
    /// `JsFuture` is), so no runtime is involved. Mirrors the one
    /// `publish_fixture`'s tests use.
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

    fn publish_into(dir: &Path, seed: u8, body: &str) -> PinnedPublisher {
        let kp = entity_crypto::Keypair::from_seed([seed; 32]);
        let pubkey = kp.public_key_bytes().to_vec();
        let key_type = kp.key_type();
        let mut root = RootProjector::new(kp).unwrap();
        let peer_id = root.peer_id().to_string();
        let site = OwnedSite {
            peer_id: peer_id.clone(),
            site_id: "home".into(),
            manifest: SiteManifest::new("home", "Home", "index", vec![NavItem::new("Home", "/index")]),
            pages: vec![
                ("index".into(), SitePage::markdown("Home", body)),
                ("other".into(), SitePage::markdown("Other", "# a different authored page")),
            ],
            assets: vec![],
        };
        emit_owned_sites(dir, std::slice::from_ref(&site), "", Some(&mut root)).unwrap();
        root.finish(dir).unwrap();
        let layout = PublishLayout::conventional("", &peer_id);
        PinnedPublisher { origin: String::new(), peer_id, pubkey, key_type, layout }
    }

    /// **The B15 gate.** The sync walk driven to completion over an async
    /// transport: a pinned consumer fetches a page from a published tree it has
    /// never seen, and gets the authored bytes.
    #[test]
    fn a_pinned_consumer_resolves_a_page_over_the_async_pump() {
        let dir = tempfile::tempdir().unwrap();
        let pin = publish_into(dir.path(), 0x15, BODY);
        let src = DirSource::new(dir.path());

        let entity = block_on(resolve_signed(&src, &pin, KEY)).expect("resolves");
        let authored = SitePage::markdown("Home", BODY).to_entity();
        assert_eq!(entity.content_hash, authored.content_hash);
        assert_eq!(entity.data, authored.data);
        eprintln!("B15 pump: {} fetch(es) to resolve one page", src.count());
    }

    /// **The property the whole chain exists for.** The origin answers the
    /// content URL with a *different, validly authored* page — no forgery, the
    /// signature untouched. The walk refuses it. The plain two-hop pointer path
    /// cannot do this, because there the host picks the hash.
    #[test]
    fn a_substituted_page_is_refused_even_though_it_is_validly_authored() {
        let dir = tempfile::tempdir().unwrap();
        let pin = publish_into(dir.path(), 0x16, BODY);
        let src = DirSource::new(dir.path());

        let index = SitePage::markdown("Home", BODY).to_entity();
        let other = SitePage::markdown("Other", "# a different authored page").to_entity();
        src.substitute.borrow_mut().insert(
            content_url(&pin.origin, &index.content_hash),
            entity_ecf::ecf_for_hash(&other.entity_type, &other.data),
        );

        let got = block_on(resolve_signed(&src, &pin, KEY));
        assert!(
            !matches!(&got, Ok(e) if e.data == other.data),
            "a substituted page must never be served as the authored one: {got:?}"
        );
    }

    /// A key that is not in the signed tree is [`SignedFetchError::Absent`] —
    /// **not** a transport error and not a hang. This is the half of trap 1 that
    /// looks like a cache miss: `Ok(None)` with nothing outstanding.
    #[test]
    fn a_key_absent_from_the_signed_tree_is_absent_not_a_transport_error() {
        let dir = tempfile::tempdir().unwrap();
        let pin = publish_into(dir.path(), 0x17, BODY);
        let src = DirSource::new(dir.path());

        let got = block_on(resolve_signed(&src, &pin, "sites/home/pages/nope"));
        assert_eq!(got, Err(SignedFetchError::Absent), "got {got:?}");
    }

    /// A root signed by a key we did not pin is **terminal**, and the pump must
    /// not retry it — retrying a verification failure hands a hostile origin
    /// unbounded attempts. Checked by fetch count, not just by the error: a pump
    /// that spun would fetch the manifest once but re-verify 32 times.
    #[test]
    fn a_root_signed_by_another_key_fails_terminally_without_retrying() {
        let dir = tempfile::tempdir().unwrap();
        let mut pin = publish_into(dir.path(), 0x18, BODY);
        let impostor = entity_crypto::Keypair::from_seed([0x99; 32]);
        pin.pubkey = impostor.public_key_bytes().to_vec();

        let src = DirSource::new(dir.path());
        let got = block_on(resolve_signed(&src, &pin, KEY));
        assert!(matches!(got, Err(SignedFetchError::Verify(_))), "got {got:?}");
        assert!(
            src.count() <= 3,
            "a terminal failure must not be pumped: {} fetches",
            src.count()
        );
    }

    /// **The mutation check for trap 2.** `signature_for` returns `Err` on a
    /// miss precisely so the pump can fetch it; if it returned `Ok(None)` the
    /// client would see `SignatureMissing` — terminal — on the very first round,
    /// and every page on a correctly signed origin would fail. Proven by
    /// observing that the signature IS fetched, and that removing it from the
    /// origin surfaces as the origin's failure rather than as "unsigned".
    ///
    /// **The failure it surfaces as tightened when the 404 discriminator
    /// landed**: a deleted signature is an artifact the root *declares*, so it
    /// is [`SignedFetchError::IncompleteWalk`] — terminal — not the retryable
    /// `Transport` this asserted while every origin failure shared one variant.
    /// The property under test is unchanged and is the `assert_ne` below: it
    /// must never be `Verify`, because that would mean the pump concluded
    /// "unsigned" from a file it had not fetched yet.
    #[test]
    fn the_signature_is_fetched_by_the_pump_not_assumed_absent() {
        let dir = tempfile::tempdir().unwrap();
        let pin = publish_into(dir.path(), 0x19, BODY);
        let src = DirSource::new(dir.path());
        block_on(resolve_signed(&src, &pin, KEY)).expect("resolves");
        assert!(
            src.fetched.borrow().iter().any(|u| u.contains("system/signature/")),
            "the pump must fetch the signature: {:?}",
            src.fetched.borrow()
        );

        // With the signature gone, this is the ORIGIN withholding an artifact
        // its own root declares — not a claim that the publisher never signed.
        let sig_dir = dir.path().join(&pin.peer_id).join("system/signature");
        std::fs::remove_dir_all(&sig_dir).unwrap();
        let src2 = DirSource::new(dir.path());
        let got = block_on(resolve_signed(&src2, &pin, KEY));
        assert!(matches!(got, Err(SignedFetchError::IncompleteWalk(_))), "got {got:?}");
        assert!(
            !matches!(got, Err(SignedFetchError::Verify(_))),
            "a signature that was never fetched must not be read as unsigned: {got:?}"
        );
    }

    /// **The discriminator itself: the SAME declared blob, failing two ways,
    /// must classify two ways.** `EXTENSION-TREE` §3.3a (arch
    /// `ROUTING-2026-08-18-l` §5) rules an incomplete walk **terminal** and a
    /// transport failure **retryable**, and before this the two were one
    /// variant — a withheld blob and an unreachable origin both arrived as
    /// `Transport`.
    ///
    /// That collapse is the fourth appearance of one seam in this arc: *"absent"
    /// and "withheld" keep reaching the caller as the same value.* The signal
    /// was never missing — an origin that answers 404 has **chosen**, and an
    /// origin that drops the connection has not — it was being discarded one
    /// layer below `SignedFetchError`.
    ///
    /// **Mutation-checked both ways**, which is the only reason this is a gate
    /// and not a restatement: collapse `declared_fetch_error` to always-
    /// `Transport` and the first half fails; to always-`IncompleteWalk` and the
    /// second half fails. A single-arm version of this test passes under one of
    /// those mutations, which is how the collapse survived in the first place.
    #[test]
    fn a_withheld_blob_is_terminal_while_an_unreachable_origin_stays_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let pin = publish_into(dir.path(), 0x2b, BODY);

        // A clean resolve first, to learn which content blobs this walk
        // actually declares — picking one by position would be guessing at the
        // HAMT shape, and the shape is hash-keyed.
        let warm = DirSource::new(dir.path());
        block_on(resolve_signed(&warm, &pin, KEY)).expect("resolves clean");
        let declared: Vec<String> = warm
            .fetched
            .borrow()
            .iter()
            .filter(|u| u.contains("/content/"))
            .cloned()
            .collect();
        assert!(!declared.is_empty(), "the walk must fetch content blobs to withhold one");
        // (a) WITHHELD — EVERY declared blob in turn, not one. The walk fetches
        // a trie node, a page body and the signature body through the same
        // classifier, and picking one victim covers whichever the HAMT happened
        // to order first: the first draft of this test picked `declared[0]` and
        // silently only ever exercised the signature path.
        for victim_url in &declared {
            let victim_path = dir.path().join(victim_url.trim_start_matches('/'));
            assert!(victim_path.exists(), "victim must exist: {victim_path:?}");
            let withheld = std::fs::read(&victim_path).unwrap();
            std::fs::remove_file(&victim_path).unwrap();

            let src = DirSource::new(dir.path());
            let got = block_on(resolve_signed(&src, &pin, KEY));
            assert!(
                matches!(got, Err(SignedFetchError::IncompleteWalk(_))),
                "withholding {victim_url} must be tree/incomplete-walk — got {got:?}"
            );

            // (b) UNREACHABLE — that same blob, restored and correct, behind an
            // origin that is merely failing. Nothing was withheld; retrying is
            // legitimate. Same blob, same walk, opposite classification.
            std::fs::write(&victim_path, &withheld).unwrap();
            let src = DirSource::new(dir.path());
            src.make_unreachable(victim_url.trim_start_matches('/'));
            let got = block_on(resolve_signed(&src, &pin, KEY));
            assert!(
                matches!(got, Err(SignedFetchError::Transport(_))),
                "an unreachable origin serving {victim_url} must stay retryable — got {got:?}"
            );
        }
        println!("discriminator: {} declared blob(s), each terminal when withheld and retryable when unreachable", declared.len());
    }

    /// A tree with its trie closure stripped resolves nothing, and does so
    /// **without spinning** — the miss log empties and the pump stops. This is
    /// the pre-B14 projection.
    ///
    /// **This assertion used to accept `Transport(_) | Absent` and its doc said
    /// the two were "the same observable".** That is arch's §4 invariant stated
    /// backwards (`ROUTING-2026-08-18-j`): *the absence of a node is never an
    /// answer.* Tightened to `Transport` alone and **it passed unchanged**, so
    /// the tolerance was never describing our behaviour — it was describing the
    /// emitter-side ambiguity and quietly generalising it to the consumer.
    ///
    /// Why we already satisfy it: a miss inside the walk is recorded by
    /// `PumpFetcher`, the pump then *fetches* it, and a withheld blob makes that
    /// fetch fail → `Transport`. [`SignedFetchError::Absent`] is reachable only
    /// when the walk completes with **nothing outstanding** — every node on the
    /// path resolved and none held the key. Paired with
    /// `a_key_absent_from_the_signed_tree_is_absent_not_a_transport_error`,
    /// which pins the other half; neither test means much without the other.
    #[test]
    fn an_unwalkable_tree_terminates_rather_than_spinning() {
        let dir = tempfile::tempdir().unwrap();
        let pin = publish_into(dir.path(), 0x1a, BODY);
        // Delete every blob the pointer mirror does NOT name — the interior
        // nodes plus the head/signature bodies.
        let named: BTreeSet<String> = walk(&dir.path().join(&pin.peer_id))
            .into_iter()
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("bin"))
            .filter_map(|p| std::fs::read(&p).ok())
            .filter_map(|b| crack_pointer(&b).ok())
            .map(|h| h.to_hex())
            .collect();
        for blob in walk(&dir.path().join("content")) {
            let name = blob.file_name().unwrap().to_string_lossy().to_string();
            if !named.contains(&name) {
                let _ = std::fs::remove_file(&blob);
            }
        }

        let src = DirSource::new(dir.path());
        let got = block_on(resolve_signed(&src, &pin, KEY));
        // Tightened a second time. It first accepted `Transport | Absent`
        // (arch's invariant stated backwards); then `Transport` alone, which
        // passed unchanged and proved the tolerance never described us. Now the
        // withheld blob has its own terminal variant, so the assertion says the
        // specific true thing: the ORIGIN failed to produce its committed
        // closure. `Absent` remains unreachable here, which is the invariant.
        assert!(
            matches!(got, Err(SignedFetchError::IncompleteWalk(_))),
            "an unwalkable tree must be a FAILURE, never an absence — got {got:?}"
        );
        assert!(src.count() < 32, "the pump must not spin: {} fetches", src.count());
    }


    /// **The reason [`SignedSession`] exists.** A session refuses a root whose
    /// `seq` went backwards — an origin rolling a site back to a previous
    /// publish while every signature and every hash still checks out. A
    /// per-call client cannot see it: the floor lives for the lifetime of the
    /// client, so a fresh one has nothing to compare against.
    ///
    /// Mutation-checked in the direction that matters: driven through the
    /// one-shot `resolve_signed`, the same rollback is **accepted**. That is the
    /// hole, demonstrated rather than asserted.
    #[test]
    fn a_session_refuses_a_seq_rollback_that_a_per_call_client_accepts() {
        let v1 = tempfile::tempdir().unwrap();
        let v2 = tempfile::tempdir().unwrap();

        // ONE publisher, two publishes — which is the only way to get a real
        // `seq` ordering. A second `RootProjector` would be a fresh peer with no
        // prior head, and both publishes would be seq 0.
        let kp = entity_crypto::Keypair::from_seed([0x2b; 32]);
        let pubkey = kp.public_key_bytes().to_vec();
        let key_type = kp.key_type();
        let mut root = RootProjector::new(kp).unwrap();
        let peer_id = root.peer_id().to_string();

        let s1 = site(&peer_id, "# v1 bytes");
        emit_owned_sites(v1.path(), std::slice::from_ref(&s1), "", Some(&mut root)).unwrap();
        let r1 = root.finish(v1.path()).unwrap();

        let s2 = site(&peer_id, "# v2 bytes");
        emit_owned_sites(v2.path(), std::slice::from_ref(&s2), "", Some(&mut root)).unwrap();
        let r2 = root.finish(v2.path()).unwrap();
        assert!(r2.seq > r1.seq, "the republish must advance seq: {} then {}", r1.seq, r2.seq);

        let pin = PinnedPublisher {
            origin: String::new(),
            layout: PublishLayout::conventional("", &peer_id),
            peer_id,
            pubkey,
            key_type,
        };

        // A session that has seen v2 refuses v1 — same key, same peer-id, valid
        // signature, older `seq`.
        let session = SignedSession::new(pin.clone());
        let newer = DirSource::new(v2.path());
        let got_v2 = block_on(session.resolve(&newer, KEY)).expect("v2 resolves");
        assert!(String::from_utf8_lossy(&got_v2.data).contains("v2 bytes"));

        let older = DirSource::new(v1.path());
        let rolled_back = block_on(session.resolve(&older, KEY));
        assert!(
            matches!(&rolled_back, Err(SignedFetchError::Verify(e)) if e.contains("rollback")),
            "a session must refuse an older seq: {rolled_back:?}"
        );

        // The mutation: the one-shot path has no floor, so it takes v1 happily.
        // This is what a per-page client would do on every navigation.
        let fresh = DirSource::new(v1.path());
        let accepted = block_on(resolve_signed(&fresh, &pin, KEY))
            .expect("the one-shot path accepts the rollback — which is the hole");
        assert!(
            String::from_utf8_lossy(&accepted.data).contains("v1 bytes"),
            "the demonstration only holds if the older tree really was served"
        );
    }

    /// The cache is what makes a second page cheap — trie interior nodes are
    /// shared by every key in the tree.
    #[test]
    fn a_second_page_from_the_same_session_costs_far_less_than_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let pin = publish_into(dir.path(), 0x2c, BODY);
        let src = DirSource::new(dir.path());
        let session = SignedSession::new(pin);

        block_on(session.resolve(&src, KEY)).expect("first page");
        let first = src.count();
        block_on(session.resolve(&src, "sites/home/pages/other")).expect("second page");
        let second = src.count() - first;

        assert!(
            second < first,
            "the cache must make the second page cheaper: {first} then {second}"
        );
        eprintln!("B15 session: {first} fetch(es) for the first page, {second} for the second");
    }

    fn site(peer_id: &str, body: &str) -> OwnedSite {
        OwnedSite {
            peer_id: peer_id.to_string(),
            site_id: "home".into(),
            manifest: SiteManifest::new("home", "Home", "index", vec![NavItem::new("Home", "/index")]),
            pages: vec![
                ("index".into(), SitePage::markdown("Home", body)),
                ("other".into(), SitePage::markdown("Other", "# a different authored page")),
            ],
            assets: vec![],
        }
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else { return out };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
