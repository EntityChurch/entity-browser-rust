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
    /// The chain does not hold: bad signature, wrong key, or a body that does
    /// not hash to its address. **Terminal** — never retry.
    ///
    /// **This is the PUBLISHER's defect**, and the line above used to end
    /// *"…, `seq` rollback, …"*, which is a different party's — see
    /// [`Declined`](Self::Declined).
    Verify(String),
    /// ⭐ **We refused it, and everything about it was fine.**
    ///
    /// The signature verified, the bytes hash to their address, the chain
    /// holds — and this reader declined the root on **its own** policy. Today
    /// there is exactly one such policy and it is the anti-rollback floor: a
    /// published root whose `seq` went backwards is a correctly-signed root
    /// being replayed, so the refusal is deliberately made *after* verification
    /// and cannot be made before it.
    ///
    /// **Split out of [`Verify`](Self::Verify) on 2026-09-11**, which had been
    /// enumerating the collapse in its own doc comment. The cost was not the
    /// merged value, it was the sentence: a reader told *"verification failed:
    /// seq rollback"* has been handed **the publisher's defect** for something
    /// the publisher did nothing wrong in — and the case that produces it is
    /// the publisher's **own second machine** (`multi_device_sequence`, measured
    /// 2026-09-09: two out-dirs under one keypair are two independent sequences,
    /// and a consumer that read one refuses the other). *Their laptop is not an
    /// attack, and the report said it was.*
    ///
    /// **Terminal like `Verify`, and terminal for the opposite reason.** There
    /// the origin failed to prove itself; here it proved itself and we said no.
    /// Retrying changes neither, and it is the destination that differs: a
    /// verification failure sends a person to the publisher, this sends them to
    /// their own reader's floor.
    Declined(String),
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

/// Whose defect a terminal published-root failure is.
///
/// **One expression, because there are two call sites** and they had the
/// identical `SignedFetchError::Verify(e.to_string())` — the shape where one of
/// them grows a case and the other does not (C15). It is also the only place
/// the kernel's error taxonomy is read for *attribution* rather than for
/// control flow, so a new kernel variant lands here and nowhere else.
fn classify_root_error(e: PublishedRootError) -> SignedFetchError {
    match e {
        // ⭐ **The reader-owned one.** Everything verified and we said no.
        PublishedRootError::SeqRollback { .. } => SignedFetchError::Declined(e.to_string()),
        other => SignedFetchError::Verify(other.to_string()),
    }
}

impl std::fmt::Display for SignedFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "transport: {e}"),
            Self::Verify(e) => write!(f, "verification failed: {e}"),
            Self::Declined(e) => write!(
                f,
                "this reader declined a root that verified correctly: {e} — \
                 a policy of ours, not a fault of theirs"
            ),
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

    /// **The prefix this publisher's signed root DECLARES** (§3.3a) — the thing
    /// [`Self::resolve`]'s keys are relative to.
    ///
    /// ## ⚠ Why a caller ever needs this, and it is a live cross-impl trap
    ///
    /// [`Self::resolve`] takes a key **relative to the declared prefix**, and
    /// several conventions pin their addresses as **absolute peer-relative
    /// paths**: `APP-CONVENTION-FEED` §4.2 pins `{peer}/app/feed/index` by hand.
    /// The two agree exactly while a publisher declares the universal tree, and
    /// **our own emitter always does**, so nothing in this repo could notice
    /// they are different questions.
    ///
    /// Against a publisher who declared `app/feed/`, the committed key is
    /// `index` and a reader asking for the pinned `app/feed/index` misses every
    /// time. `entity-workbench-go` filed this as a reader's-mistakes entry and
    /// arch has taken it into `GUIDE-APPLICATION-DEVELOPMENT`; their framing is
    /// the part that transfers — *getting it wrong yields an empty feed with a
    /// valid signature over it, which is the most confident wrong answer
    /// available.* Measured here 2026-09-15 against their corridor fixture.
    ///
    /// ⭐ **It is the source's job, not the reader's.** A convention pins one
    /// address; what varies is the transport the bytes arrived over, and a live
    /// peer has no declared prefix at all. Putting the strip here keeps
    /// [`crate::feed_read`] spelling §4.2's address the way §4.2 spells it.
    ///
    /// Costs one manifest fetch. Callers that resolve many keys should ask once
    /// and hold it for the walk — the prefix is a property of the root they are
    /// already pinned to, not of the key.
    pub async fn declared_prefix<S: BinSource + ?Sized>(
        &self,
        src: &S,
    ) -> Result<String, SignedFetchError> {
        let bytes = src
            .get(self.pin.manifest_url(), Freshness::Mutable)
            .await
            .map_err(|e| SignedFetchError::Transport(format!("manifest: {e}")))?;
        match self.manifest.lock() {
            Ok(mut m) => *m = bytes,
            Err(_) => return Err(SignedFetchError::Transport("manifest slot poisoned".into())),
        }
        // The root carries its own detached signature, so reading it needs the
        // same async hop `resolve` makes — `fetch_root` is sync and reports the
        // miss rather than fetching it. One round is enough in practice (there
        // is exactly one signature to want); the bound is `resolve`'s, for the
        // same reason.
        for _ in 0..MAX_ROUNDS {
            match self.client.fetch_root() {
                Ok(root) => return Ok(root.prefix),
                Err(e) if !matches!(e, PublishedRootError::Fetch(_)) => {
                    return Err(classify_root_error(e))
                }
                _ => {}
            }
            let wanted = drain(&self.state.signature_misses);
            if wanted.is_empty() {
                // A fetch error with nothing outstanding is the origin failing
                // to serve its own manifest, not a key that is absent.
                return Err(SignedFetchError::Transport(
                    "the root could not be read and nothing was outstanding".into(),
                ));
            }
            self.pump_signatures(src, wanted).await?;
        }
        Err(SignedFetchError::Transport("the root did not settle".into()))
    }

    /// The signature half of the pump's async hop, shared by
    /// [`Self::declared_prefix`] and [`Self::resolve`] so there is one of it.
    async fn pump_signatures<S: BinSource + ?Sized>(
        &self,
        src: &S,
        wanted: Vec<Hash>,
    ) -> Result<(), SignedFetchError> {
        for target in wanted {
            let bytes = fetch_signature(src, &self.pin, &target).await?;
            if let Ok(mut c) = self.state.signatures.lock() {
                c.insert(target, bytes);
            }
        }
        Ok(())
    }

    /// Map an address a **convention** pins — absolute and peer-relative — onto
    /// the key space *this* publisher's trie was built over.
    ///
    /// Pure, and separate from [`Self::declared_prefix`] so the rule is gated
    /// without a publish: `/` (the universal tree) and `""` both mean *no
    /// prefix*, and a declared prefix that the pinned path does not start with
    /// is left alone rather than mangled — that publisher simply does not carry
    /// this key, and reporting it as absent is the honest answer.
    pub fn key_under_prefix(pinned_peer_relative: &str, declared_prefix: &str) -> String {
        let p = declared_prefix.trim_start_matches('/');
        if p.is_empty() {
            return pinned_peer_relative.to_string();
        }
        let p = if p.ends_with('/') { p.to_string() } else { format!("{p}/") };
        pinned_peer_relative.strip_prefix(&p).unwrap_or(pinned_peer_relative).to_string()
    }

    /// Resolve `relative_key` from the signed tree.
    ///
    /// `relative_key` is peer-relative — `sites/{site}/pages/{slug}`, the same
    /// key space the projection's trie was built over.
    ///
    /// ⚠ **Relative to the publisher's DECLARED PREFIX**, which is the same
    /// thing only while that prefix is the universal tree. If your key came
    /// from a convention that pins an absolute address, put it through
    /// [`Self::key_under_prefix`] first — see [`Self::declared_prefix`].
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
                    return Err(classify_root_error(e))
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
            self.pump_signatures(src, signature_wanted).await?;
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
                // than assumed.
                //
                // **Fixed upstream too** (`core/peer` `302b7f4`): the store
                // latches the mismatch and `resolve` checks it before believing
                // a `None`, so every consumer of a signed root gets the honest
                // answer, not just us. This check stays as the nearer half —
                // it refuses the bytes at the moment we would cache them, and
                // names the hash the origin failed to produce.
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

    /// **Resolve a hash this publisher's own signed data REFERENCED** — the
    /// by-hash counterpart to [`Self::resolve`]'s by-key walk.
    ///
    /// `EXTENSION-REGISTRY` v1.21 D8 makes a binding's `transports` a list of
    /// bare `system/hash` naming `system/peer/transport/*` entities, and D8a
    /// makes serving them a **MUST** on the publishing registry. So a consumer
    /// needs a way to follow a reference *out of* a verified body, which the
    /// trie walk cannot express — the hash is in the binding, not in the trie.
    ///
    /// Three properties, and each of them is a rule this repo has already paid
    /// for at least once:
    ///
    /// 1. **Verify before believing.** The bytes must hash to the address we
    ///    asked for, or an origin can answer any reference with anything. Same
    ///    check, same reason, same place in the sequence as [`Self::resolve`]'s.
    /// 2. **A missing referent is `IncompleteWalk`, not `Transport`.** The
    ///    reference came out of a body the signed root commits to, so a 404 is
    ///    the origin refusing its own declared closure — terminal, because
    ///    retrying grants a withholding origin unbounded attempts. Arch's own
    ///    reason for making D8a a MUST is that *"a missing referent and a
    ///    withheld one are byte-identical at the consumer"*; `declared_fetch_error`
    ///    is the single place that decision lives, so this call site cannot
    ///    re-collapse it by reaching for `Transport` out of habit.
    /// 3. **It shares the session cache**, so a profile referenced by several
    ///    bindings is fetched once and the `seq` floor still governs the session
    ///    it was fetched under.
    ///
    /// It deliberately does **not** consult the trie: a reference is not a key,
    /// and pretending otherwise would make an unreferenced-but-published entity
    /// resolvable, which is a different (looser) claim than the one D8a makes.
    pub async fn content<S: BinSource + ?Sized>(
        &self,
        src: &S,
        hash: &Hash,
    ) -> Result<Entity, SignedFetchError> {
        if let Ok(c) = self.state.content.lock() {
            if let Some(bytes) = c.get(hash) {
                return crate::content_site::http_poll::verify_and_decode(bytes, hash).map_err(|e| {
                    SignedFetchError::Verify(format!("cached content {}: {e:?}", hash.to_hex()))
                });
            }
        }
        let bytes = src
            .get(self.pin.layout.content_url(hash), Freshness::Immutable)
            .await
            .map_err(|e| declared_fetch_error(&hash.to_hex(), e))?;
        let entity =
            crate::content_site::http_poll::verify_and_decode(&bytes, hash).map_err(|e| {
                SignedFetchError::Verify(format!(
                    "referenced content {}: {e:?} — the origin served bytes that do not hash to \
                     the address its own published binding named",
                    hash.to_hex()
                ))
            })?;
        if let Ok(mut c) = self.state.content.lock() {
            c.insert(*hash, bytes);
        }
        Ok(entity)
    }

    /// **Enumerate the keys the signed root commits to — and fail rather than
    /// return a short list.**
    ///
    /// The keys are *in* the nodes: `EXTENSION-TREE` §3.1's leaf is
    /// `[key, value_hash]`, so a HAMT walk from the verified `root_hash`
    /// recovers the whole reachable key set with no new mechanism and nothing
    /// host-served. Passing `prefix: "system/registry/binding/by-name/"` makes
    /// the trie's key set *be* the name set.
    ///
    /// **Why this is written here instead of calling upstream.**
    /// `entity_tree::trie::collect_all_bindings` (§3.5) is the obvious reach and
    /// is *silently short*: it skips a missing `Entry::Link` with a bare
    /// `if let Some(..)` and returns a `BTreeMap`, not a `Result`. Measured on a
    /// 24-name registry, withholding one interior node hid **1 name of 24** with
    /// no error and a root hash that still verified — so a browse surface built
    /// on it makes a shortened list indistinguishable from a small registry,
    /// while the hidden name stays resolvable for anyone who already knows it.
    /// Arch's D9 says a walk MUST NOT be shortened without failing; this is that
    /// walk.
    ///
    /// Three properties it holds, each of which is the difference between an
    /// enumeration and a rumour:
    ///
    /// 1. **A declared child that does not resolve is `IncompleteWalk`**, never
    ///    the end of a branch. A trie node *declares* its children, so absence is
    ///    structural and checkable — the same reasoning that closed the
    ///    publisher's F8, applied to the consumer.
    /// 2. **Every node is hash-verified before it is believed** (`verify_and_decode`
    ///    on the way into the cache, exactly as [`Self::resolve`] does), so a
    ///    tampered interior node is a verification failure and not a shorter
    ///    answer.
    /// 3. **A node that does not decode is a failure**, not an empty node. A
    ///    corrupted body that happens to hash correctly cannot occur, but a body
    ///    of the wrong *type* can, and treating it as childless is how a walk
    ///    silently shortens.
    ///
    /// This does **not** make the listing authoritative about what the publisher
    /// *knows* — only about what this root commits to. That is the honest bound,
    /// and it is strictly stronger than the host-served `.list` artifacts, which
    /// commit to nothing at all.
    ///
    /// **Bounded** — see [`Self::enumerate_bounded`], which this calls with
    /// [`DEFAULT_ENUMERATION_BUDGET`]. A registry is not obliged to be small, and
    /// walking one is O(the whole trie): every interior node is a fetch.
    pub async fn enumerate<S: BinSource + ?Sized>(
        &self,
        src: &S,
        prefix: &str,
    ) -> Result<Vec<String>, SignedFetchError> {
        self.enumerate_bounded(src, prefix, DEFAULT_ENUMERATION_BUDGET)
            .await
            .map(|e| e.keys)
    }

    /// [`Self::enumerate`] with an explicit budget, reporting **whether it
    /// finished**.
    ///
    /// **A registry is allowed to be big, and a browser must not download one to
    /// find that out.** Walking a signed root costs a fetch per interior node, so
    /// enumeration is bounded on both axes a hostile *or merely large* origin can
    /// grow: nodes fetched and keys collected.
    ///
    /// **The bound reports itself, and that is the whole design.** A truncated
    /// list returned as a plain `Vec` is the same defect as the silently-short
    /// walk this function exists to prevent — the caller cannot tell "this
    /// registry has 40 names" from "this registry has 40,000 and I stopped".
    /// [`Enumeration::complete`] is `false` in the second case and a surface
    /// **must** say so rather than render a list that looks whole.
    ///
    /// Truncation is deliberately **not** an error: a partial listing is useful
    /// (it is a real prefix of a real key set, every key of it verified), where a
    /// failure would leave a large registry entirely unbrowsable. An error is
    /// reserved for the origin failing to produce what its root declares.
    ///
    /// The real answer for large registries is paged/prefix-scoped navigation —
    /// descend by trie position rather than collecting the whole key set. That is
    /// not built; this bound is what keeps its absence honest instead of slow.
    pub async fn enumerate_bounded<S: BinSource + ?Sized>(
        &self,
        src: &S,
        prefix: &str,
        budget: EnumerationBudget,
    ) -> Result<Enumeration, SignedFetchError> {
        // Same refresh-per-call contract as `resolve`: the manifest is the one
        // mutable artifact, and re-fetching it is what keeps the `seq` floor
        // meaningful.
        let bytes = src
            .get(self.pin.manifest_url(), Freshness::Mutable)
            .await
            .map_err(|e| SignedFetchError::Transport(format!("manifest: {e}")))?;
        match self.manifest.lock() {
            Ok(mut m) => *m = bytes,
            Err(_) => return Err(SignedFetchError::Transport("manifest slot poisoned".into())),
        }

        for _ in 0..MAX_ROUNDS {
            // `fetch_root` verifies the signature and enforces the `seq` floor,
            // so enumeration inherits the rollback defence rather than opening a
            // second door around it.
            let root = match self.client.fetch_root() {
                Ok(r) => r,
                Err(PublishedRootError::Fetch(_)) => {
                    // The manifest or its signature is not cached yet — drive the
                    // pump and come back.
                    self.pump_once(src).await?;
                    continue;
                }
                Err(e) => return Err(classify_root_error(e)),
            };

            match self.walk_keys(root.root_hash, prefix, budget)? {
                WalkOutcome::Complete(e) => return Ok(e),
                WalkOutcome::NeedsFetch => self.pump_once(src).await?,
            }
        }
        Err(SignedFetchError::Budget)
    }

    /// One structural pass over the trie against whatever is cached.
    ///
    /// Returns [`WalkOutcome::NeedsFetch`] the moment a declared child is not in
    /// hand (its hash is already in the miss log, put there by `PumpFetcher`),
    /// and only reports `Complete` when **every** declared child was reached.
    fn walk_keys(
        &self,
        root: Hash,
        prefix: &str,
        budget: EnumerationBudget,
    ) -> Result<WalkOutcome, SignedFetchError> {
        let mut keys: Vec<String> = Vec::new();
        let mut queue = vec![root];
        let mut seen: BTreeSet<Hash> = BTreeSet::new();
        let mut needs_fetch = false;
        let mut truncated = false;
        // **Nodes actually DECODED this pass** — what `max_nodes` bounds, and
        // deliberately not `seen.len()`. See the miss arm below.
        let mut read = 0usize;
        // Nodes this pass asked the pump for. Bounded separately so one round
        // cannot queue the whole trie for fetching.
        let mut wanted = 0usize;

        while let Some(h) = queue.pop() {
            // **Stop before the fetch, not after.** Checking the bound after
            // draining the queue would still have walked the whole trie; the
            // point is to not download a large registry, so an over-budget walk
            // must stop *asking*. `truncated` then rides out on the result and
            // the surface says so — a shorter list that does not announce itself
            // is the defect this whole module is built against.
            if read >= budget.max_nodes || keys.len() >= budget.max_keys {
                truncated = true;
                break;
            }
            if !seen.insert(h) {
                continue;
            }
            let Some(bytes) = self.state.content.lock().ok().and_then(|c| c.get(&h).cloned())
            else {
                // ⭐ **A miss is a fetch that has NOT happened yet, not a node we
                // walked — and counting it against `max_nodes` is what broke the
                // publication probe.**
                //
                // `seen` resets every round while the cache only grows, so the
                // walk converges by re-descending and reading one more level
                // each pass. Charging the read budget for a miss meant a cold
                // round spent the entire budget on nodes it had not read: the
                // root decodes, its ~32 children all miss, `seen` hits the cap,
                // and `truncated` is set — which then **short-circuits
                // `needs_fetch` below**, so the walk reports itself DONE having
                // decoded exactly one node. Measured on a 46-node trie whose
                // every key was under the prefix: `max_nodes: 32` returned
                // `walked=32, keys=0`, while `max_nodes: 64` returned
                // `walked=4, keys=64`. The budget was not too small; it was
                // being spent on nodes nobody read.
                //
                // Bounding `wanted` separately is what keeps the fetch cost
                // honest: the queue holds the children of nodes we read, so an
                // unbounded miss log would be up to 32×`max_nodes` fetches to
                // read `max_nodes` nodes. Hitting this cap is **not** truncation
                // — it is *come back next round*, and `needs_fetch` carries that,
                // so the walk never reports `complete` over a subtree it silently
                // declined to ask about.
                if wanted < budget.max_nodes {
                    wanted += 1;
                    // Record the miss the same way the client's fetcher would, so
                    // the pump has something to fetch on the next round.
                    if let Ok(mut m) = self.state.content_misses.lock() {
                        m.insert(h);
                    }
                    needs_fetch = true;
                }
                continue;
            };
            read += 1;
            // Already hash-verified on the way into the cache; decoding here is
            // about STRUCTURE.
            let entity = crate::content_site::http_poll::verify_and_decode(&bytes, &h)
                .map_err(|e| SignedFetchError::Verify(format!("node {}: {e:?}", h.to_hex())))?;
            if entity.entity_type != entity_tree::trie::TYPE_TREE_SNAPSHOT_NODE {
                // A leaf VALUE, not a node. Reached only when a bucket's value
                // hash was enqueued, which this walk does not do — so this is a
                // malformed tree rather than a normal terminus.
                return Err(SignedFetchError::Verify(format!(
                    "node {} is {}, not a trie node — this tree does not have the shape its \
                     root claims",
                    h.to_hex(),
                    entity.entity_type
                )));
            }
            let Some(node) = entity_tree::trie::SnapshotNodeData::from_cbor(&entity.data) else {
                // **Not "an empty node".** Treating an undecodable node as
                // childless is precisely how a walk shortens without failing.
                return Err(SignedFetchError::Verify(format!(
                    "trie node {} does not decode — a walk that treated this as childless would \
                     silently drop every key beneath it",
                    h.to_hex()
                )));
            };
            for entry in &node.data {
                match entry {
                    entity_tree::trie::Entry::Bucket(b) => {
                        for (k, _value_hash) in b {
                            // **The key bound applies INSIDE a bucket too.** Checked
                            // only between nodes, it does not bind at all on a trie
                            // whose keys live in one fat bucket — every key lands in
                            // a single visit and the walk reports `complete`. That
                            // is what the first version did, and the gate caught it:
                            // a bound that a common shape slips past is worse than
                            // none, because it reads as enforced.
                            if keys.len() >= budget.max_keys {
                                truncated = true;
                                break;
                            }
                            if k.starts_with(prefix) {
                                keys.push(k.clone());
                            }
                        }
                    }
                    // A link is a DECLARED child. If it is not fetchable the walk
                    // is incomplete — that is `IncompleteWalk`, raised by the pump
                    // when the fetch 404s, never a shorter list here.
                    entity_tree::trie::Entry::Link(sub) => queue.push(*sub),
                }
            }
        }

        // A truncated walk is DONE, not waiting: outstanding misses below the
        // cutoff are ours to abandon, not the origin's to answer. Returning
        // `NeedsFetch` here would fetch exactly what the bound exists to avoid.
        if needs_fetch && !truncated {
            return Ok(WalkOutcome::NeedsFetch);
        }
        keys.sort();
        keys.dedup();
        Ok(WalkOutcome::Complete(Enumeration {
            keys,
            complete: !truncated,
            // **Nodes read, not nodes touched.** A surface renders this as how
            // much of the tree we saw; a miss is a node we did not see.
            nodes_walked: read,
        }))
    }

    /// Drain the miss logs and fetch what they name.
    ///
    /// Shared by `enumerate`'s two pump sites. A content miss came out of a
    /// structure the signed root commits to, so a 404 is the origin refusing its
    /// own committed closure — [`declared_fetch_error`] is what keeps that
    /// terminal while a 5xx stays retryable.
    async fn pump_once<S: BinSource + ?Sized>(
        &self,
        src: &S,
    ) -> Result<(), SignedFetchError> {
        for target in drain(&self.state.signature_misses) {
            let bytes = fetch_signature(src, &self.pin, &target).await?;
            if let Ok(mut c) = self.state.signatures.lock() {
                c.insert(target, bytes);
            }
        }
        for want in drain(&self.state.content_misses) {
            let bytes = src
                .get(self.pin.layout.content_url(&want), Freshness::Immutable)
                .await
                .map_err(|e| declared_fetch_error(&want.to_hex(), e))?;
            if let Err(e) = crate::content_site::http_poll::verify_and_decode(&bytes, &want) {
                return Err(SignedFetchError::Verify(format!(
                    "content {}: {e:?} — the origin served bytes that do not hash to the address \
                     its own signed root committed to",
                    want.to_hex()
                )));
            }
            if let Ok(mut c) = self.state.content.lock() {
                c.insert(want, bytes);
            }
        }
        Ok(())
    }
}

/// Whether a structural pass finished or is waiting on bytes.
enum WalkOutcome {
    Complete(Enumeration),
    NeedsFetch,
}

/// How much of a signed root an enumeration may walk.
///
/// Both axes matter and they bound different things: `max_nodes` bounds the
/// **fetches** (one per interior node — the cost a large registry imposes on a
/// browser), `max_keys` bounds the **result** (the cost it imposes on a surface
/// that is about to render it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumerationBudget {
    pub max_nodes: usize,
    pub max_keys: usize,
}

/// Enough for the registries we publish today, small enough that pointing the
/// browser at a large one is slow-and-honest rather than a download.
///
/// Deliberately not tuned to a measurement: there is no defensible constant, and
/// the number is a stopgap for paged navigation rather than a claim about how
/// big a registry should be. Raise it when a surface has a reason, and prefer
/// giving that surface its own budget over moving everyone's.
pub const DEFAULT_ENUMERATION_BUDGET: EnumerationBudget =
    EnumerationBudget { max_nodes: 256, max_keys: 2048 };

/// The result of a bounded enumeration.
///
/// **Read `complete` before you render `keys`.** A partial list is a real prefix
/// of a real key set — every key in it was recovered from the signed root and
/// nothing host-served — but it is *not* the answer to "what does this registry
/// carry", and presenting it as one rebuilds the exact confusion this module
/// exists to prevent: a short list that cannot be told from a small registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enumeration {
    /// Keys under the requested prefix, sorted and deduped.
    pub keys: Vec<String>,
    /// `true` when the walk visited every node the root declares. `false` means
    /// the budget stopped it and there are more keys than these.
    pub complete: bool,
    /// Interior nodes visited — the fetch cost, and the useful thing to show an
    /// operator wondering why a big registry is slow.
    pub nodes_walked: usize,
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
            content: Vec::new(),
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
        // ⭐ **`Declined`, NOT `Verify`** — and the change of variant is the
        // whole point rather than a rename. Everything about v1 is correct: the
        // key is right, the signature verifies, every body hashes to its
        // address. What refused it is **this reader's** monotonicity floor, and
        // it necessarily runs *after* verification because a rollback is a
        // correctly-signed root being replayed. Reporting it as *"verification
        // failed"* hands a publisher's defect to a publisher who has none — and
        // the case that produces it in the field is their **own second machine**
        // (`multi_device_sequence`).
        assert!(
            matches!(&rolled_back, Err(SignedFetchError::Declined(e)) if e.contains("rollback")),
            "a session must refuse an older seq, and must not blame the publisher \
             for it: {rolled_back:?}"
        );

        // And a genuinely bad chain still lands on `Verify`, so the split is a
        // discrimination and not a relabelling of everything terminal.
        assert!(
            matches!(
                classify_root_error(PublishedRootError::SignatureInvalid),
                SignedFetchError::Verify(_)
            ),
            "every terminal outcome became the reader's own — then nothing was split"
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
            content: Vec::new(),
        }
    }

    /// **Enumeration is safe now: a withheld interior node FAILS the walk
    /// instead of shortening it.**
    ///
    /// This is the consumer half of arch's D9 (*the absence of a node is never an
    /// answer*) for the one operation that did not satisfy it. Resolution has
    /// been conformant for a while — a miss re-drives the pump and a withheld
    /// blob becomes `IncompleteWalk`. **Enumeration was the genuinely open half**,
    /// because the obvious implementation is upstream `collect_all_bindings`,
    /// which skips a missing `Entry::Link` with a bare `if let Some(..)` and
    /// returns a `BTreeMap` with nowhere to report the miss. Measured next door
    /// in `registry_publish`: 1 name of 24 hidden, silently, root hash still
    /// verifying.
    ///
    /// So a browse panel built on that walk shows a short list that is
    /// indistinguishable from a small registry, while the hidden name stays
    /// resolvable for anyone who already knows it — the browse and the resolve
    /// disagree and neither complains.
    ///
    /// Both directions, and the control is the half that keeps it honest:
    /// - **complete** — every key the root commits to comes back;
    /// - **withheld** — removing any single blob the enumeration declared makes
    ///   it fail, rather than return fewer keys.
    ///
    /// The victim is taken from *this* walk's own fetch log, one at a time, for
    /// the reason the sibling test records: under hash-keyed addressing "the
    /// first node" is not a stable referent, and a victim picked by position
    /// proves one path while reporting on all of them.
    #[test]
    fn enumerating_a_signed_root_fails_on_a_withheld_node_rather_than_shortening() {
        let dir = tempfile::tempdir().unwrap();
        let pin = publish_into(dir.path(), 0x3c, BODY);

        // (a) COMPLETE — the keys the signed root commits to.
        let warm = DirSource::new(dir.path());
        let session = SignedSession::new(pin.clone());
        let keys = block_on(session.enumerate(&warm, "sites/")).expect("a clean tree enumerates");
        assert!(
            keys.iter().any(|k| k.contains("pages/index")),
            "the enumeration must recover real page keys, got {keys:?}"
        );
        let complete = keys.len();
        assert!(complete >= 2, "the fixture publishes more than one page, got {keys:?}");

        // Which blobs did the enumeration actually declare? Ask the walk, don't
        // guess at the HAMT.
        let declared: Vec<String> = warm
            .fetched
            .borrow()
            .iter()
            .filter(|u| u.contains("/content/"))
            .cloned()
            .collect();
        assert!(!declared.is_empty(), "the walk must fetch content blobs to withhold one");

        // (b) WITHHELD — each declared blob in turn. None may produce a shorter
        // list; every one must produce an error.
        for victim_url in &declared {
            let victim_path = dir.path().join(victim_url.trim_start_matches('/'));
            if !victim_path.exists() {
                continue;
            }
            let withheld = std::fs::read(&victim_path).unwrap();
            std::fs::remove_file(&victim_path).unwrap();

            let src = DirSource::new(dir.path());
            let starved = SignedSession::new(pin.clone());
            let got = block_on(starved.enumerate(&src, "sites/"));

            std::fs::write(&victim_path, &withheld).unwrap();

            match got {
                Err(_) => {}
                Ok(short) => panic!(
                    "withholding {victim_url} returned {} of {} keys instead of failing — this is \
                     exactly the silent shortening the walk exists to prevent",
                    short.len(),
                    complete
                ),
            }
        }

        // Control: with everything restored it enumerates the same set again, so
        // the failures above were the withholding and not the loop.
        let again = SignedSession::new(pin);
        let re = block_on(again.enumerate(&DirSource::new(dir.path()), "sites/"))
            .expect("restored tree enumerates");
        assert_eq!(re.len(), complete, "the control must recover the original key set");
    }

    /// **A bounded walk says it was bounded.**
    ///
    /// Registries are allowed to be big, and a browser must not download one to
    /// discover that. The bound is the easy half; the half that matters is that
    /// truncation is *reported*, because a short list returned as a plain `Vec`
    /// is the same defect as the silently-short walk next door — the caller
    /// cannot tell "40 names" from "40,000, and I stopped at 40".
    ///
    /// Also pins that truncation is **not** an error: a partial listing is a real
    /// prefix of a real key set and is useful, where failing would make a large
    /// registry entirely unbrowsable. Errors stay reserved for the origin failing
    /// to produce what its root declares.
    #[test]
    fn a_bounded_enumeration_reports_that_it_was_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let pin = publish_into(dir.path(), 0x4d, BODY);

        // Unbounded: the honest full answer, and the baseline to compare against.
        let full = block_on(
            SignedSession::new(pin.clone())
                .enumerate_bounded(&DirSource::new(dir.path()), "sites/", DEFAULT_ENUMERATION_BUDGET),
        )
        .expect("a clean tree enumerates");
        assert!(full.complete, "the default budget must cover the fixture");
        assert!(full.keys.len() >= 2, "fixture must publish enough keys to truncate, got {:?}", full.keys);

        // Bounded to one key: fewer keys, and `complete` must say so.
        let tight = EnumerationBudget { max_nodes: 256, max_keys: 1 };
        let cut = block_on(
            SignedSession::new(pin.clone())
                .enumerate_bounded(&DirSource::new(dir.path()), "sites/", tight),
        )
        .expect("truncation is not an error — a partial listing is still useful");
        assert!(
            !cut.complete,
            "a walk stopped by its budget MUST report complete=false, or a surface renders a \
             partial list as if it were the whole registry"
        );
        assert!(
            cut.keys.len() < full.keys.len(),
            "the bound must actually bind: got {} of {}",
            cut.keys.len(),
            full.keys.len()
        );

        // Bounding NODES stops the fetching, which is the point — the cost a big
        // registry imposes is one fetch per interior node.
        let one_node = EnumerationBudget { max_nodes: 1, max_keys: 2048 };
        let shallow = block_on(
            SignedSession::new(pin)
                .enumerate_bounded(&DirSource::new(dir.path()), "sites/", one_node),
        )
        .expect("a node-bounded walk still returns what it saw");
        assert!(
            shallow.nodes_walked <= 1,
            "the node bound must stop the walk asking for more, walked {}",
            shallow.nodes_walked
        );
        // **`complete` is asserted against the fixture's real shape, not assumed.**
        // This fixture's trie fits in ONE node, so a 1-node budget genuinely IS a
        // complete walk and demanding `complete == false` would be asserting a
        // lie — the first draft did exactly that and this caught it. The bound is
        // only observable as incomplete when there was a second node to refuse.
        if full.nodes_walked > 1 {
            assert!(
                !shallow.complete,
                "with {} nodes in the tree, a 1-node budget must report incomplete",
                full.nodes_walked
            );
        } else {
            assert!(
                shallow.complete,
                "a single-node trie walked within budget is complete — reporting otherwise \
                 would cry wolf on every small registry"
            );
        }
    }

    /// ⭐⭐ **A cold walk must not spend its node budget on cache misses** — the
    /// defect that withheld *Open in Site Browser* from a live publisher,
    /// pinned here at the walk rather than at the surface that noticed it.
    ///
    /// `walk_keys` converges by re-descending: `seen` resets every round while
    /// the content cache only grows, so each pass reads one level deeper. The
    /// bound was checked against `seen.len()`, which counts nodes **popped** —
    /// including the ones that were not in the cache and had therefore not been
    /// *read*. On a cold start that is fatal in one step: the root decodes, its
    /// ~32 children all miss, `seen` hits the cap, `truncated` is set, and
    /// `truncated` short-circuits `needs_fetch` — so the walk reports itself
    /// **finished** having decoded exactly one node, with no keys and
    /// `complete: false`. A prefix probe reads that as *"could not tell"* and a
    /// browse surface renders an empty list for a publisher with thousands.
    ///
    /// **The budget is deliberately explicit and tight rather than
    /// `PROBE_BUDGET`.** This is a property of the walk at *any* bound; tying it
    /// to a constant that lives in another module would make this gate go quiet
    /// the day that constant is raised for an unrelated reason — which is
    /// exactly what happened to the first version of it.
    ///
    /// The fixture's node count is **asserted**, because a tree that fits inside
    /// the budget cannot exhibit the bug and would pass for the wrong reason.
    #[test]
    fn a_cold_walk_does_not_spend_its_node_budget_on_cache_misses() {
        let dir = tempfile::tempdir().unwrap();
        let mut root = RootProjector::new(
            entity_crypto::Keypair::from_seed([0x5e; 32]),
        )
        .unwrap();
        let peer_id = root.peer_id().to_string();
        let site = OwnedSite {
            peer_id: peer_id.clone(),
            site_id: "wide".into(),
            manifest: SiteManifest::new("wide", "Wide", "index", vec![NavItem::new("Home", "/index")]),
            pages: (0..900)
                .map(|i| (format!("p{i}"), SitePage::markdown("Page", "# a page")))
                .collect(),
            assets: Vec::new(),
            content: Vec::new(),
        };
        emit_owned_sites(dir.path(), std::slice::from_ref(&site), "", Some(&mut root)).unwrap();
        let nodes = root.finish(dir.path()).unwrap().trie_nodes;

        let budget = EnumerationBudget { max_nodes: 32, max_keys: 64 };
        assert!(
            nodes > budget.max_nodes,
            "a {nodes}-node trie fits inside a {}-node budget and cannot exhibit the defect",
            budget.max_nodes
        );

        let pin = PublishLayout::conventional("", &peer_id);
        let kp = entity_crypto::Keypair::from_seed([0x5e; 32]);
        let pin = PinnedPublisher {
            origin: String::new(),
            peer_id,
            pubkey: kp.public_key_bytes().to_vec(),
            key_type: kp.key_type(),
            layout: pin,
        };
        let got = block_on(
            SignedSession::new(pin).enumerate_bounded(&DirSource::new(dir.path()), "sites/", budget),
        )
        .expect("a clean tree enumerates");

        assert!(
            !got.keys.is_empty(),
            "a {nodes}-node trie with 900 keys under `sites/` returned NOTHING at a {}-node \
             budget after walking {} node(s) — the budget was spent on nodes that were never \
             read, and the caller cannot tell this from a publisher who has none",
            budget.max_nodes,
            got.nodes_walked
        );
        // `nodes_walked` must mean *read*, not *touched*: a surface renders it
        // as how much of the tree we saw. Finding 64 keys takes a handful of
        // nodes, so a figure at the cap is the miss-accounting coming back.
        assert!(
            got.nodes_walked < budget.max_nodes,
            "walked {} of a {}-node budget to find {} key(s) — reads should be far under the \
             ceiling once misses stop consuming it",
            got.nodes_walked,
            budget.max_nodes,
            got.keys.len()
        );
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
