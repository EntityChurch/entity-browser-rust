//! **The cross-impl consume check against a LIVE `entity-core-go` origin on
//! another host** — C-7 / `COHORT-OPEN-ITEMS` §1b, the half arch assigned to go
//! and the half that is ours to run.
//!
//! ## What this is, and how it differs from the two gates beside it
//!
//! | Gate | Impls | Hosts | What it drives |
//! |---|---|---|---|
//! | [`super::crossimpl_go`] | **two** (workbench-go emits) | one, off disk | the full signed-root walk |
//! | `make e2e-federation` | **one** (ours both ends) | **two containers** | name → binding → two signed roots |
//! | **this** | **two** (core-go emits) | **two containers** | the full signed-root walk, live |
//!
//! §1b's clause is the intersection nobody had: *a real publisher serving a
//! signed root over `http-poll` at an origin · a separate consumer resolving
//! name → binding → `transports` → fetch · hash-verify **and** signature-verify
//! at the consumer · the two on different hosts.*
//!
//! ## State: GREEN as of core-go `dabd076`
//!
//! Our reader enters at their advertised manifest, verifies the two-hop
//! signature against the key their peer-id carries, walks their CHAMP trie from
//! the *signed* root hash, and gets all three authored bodies back — over a real
//! TCP hop, sharing no process and no filesystem with the publisher.
//!
//! **It was red twice first, and the history is the reason the gates are shaped
//! the way they are — do not flatten it into "core-go had a bug".**
//!
//! 1. `cd8564a` signed a literal `root_hash` (`digest[i] = 0xC0 + i`) that the
//!    origin 404'd → `IncompleteWalk`. The fixture said so in its own comment; it
//!    was built for the Amendment-5 wire face and got assigned to a row whose
//!    first clause is *"a real publisher serving a signed root."*
//! 2. `ec30e96` served a real, verifying root that committed to **nothing** —
//!    `BuildTrieForPrefix` was handed the raw location index while the entries
//!    were written through a namespaced one, and that function lists with the
//!    unqualified prefix while trimming with the qualified one, so it returned an
//!    empty CHAMP node with no error. Every key was `Absent`.
//!
//! **Both were invisible to go's own probe, and the second was invisible to the
//! probe written for the first.** Their consumer resolves leaves through
//! location-index `system/hash` pointers and never walks the trie; the fix for
//! (1) then checked *"root fetched by hash → 200, old hash → 404"*, which an
//! empty root passes perfectly. The rule that survives both:
//! **a self-check must exercise the consumer's terminal operation — resolve a
//! key — not the last hop you added.** A negative control on the hop you just
//! fixed proves that hop and says nothing about the one after it.
//!
//! It is also why the pointer path and the signed-root path must not be spoken of
//! as one thing. The pointer path proves a body is the canonical pre-image of the
//! hash it was fetched *under*, which a lying origin satisfies trivially because
//! it supplied the pointer too. Only the trie closure closes that, and only (2)'s
//! fix made this origin's closure real.
//!
//! ## The layout is supplied by hand, and that is recorded rather than assumed
//!
//! go's poll handler serves no `transport-profile`
//! ([`a_live_go_origin_serves_no_endpoint_document`] pins the 404), so there is
//! no profile to discover — and §6.5.3 v1.8 forbids deriving one. `EXTENSION-NETWORK`
//! §6.5.4 makes profile discovery out-of-band in v1, so a consumer *handed* a
//! layout is conformant; a consumer that *guessed* it would not be. It is spelled
//! out in [`their_layout`] so nobody later mistakes it for a default. Note the
//! two places our own convention would have been wrong about them: content is
//! **flat**, not `sharded-2-4`, and the manifest is terminal at `/manifest`.
//!
//! ## Running it
//!
//! ```text
//! make crossimpl-go          # stand go's publisher up, run these, tear it down
//! ```
//!
//! The publisher is `entity-core-go`'s own `scripts/federation-publish.sh` —
//! their published interface, invoked, not reimplemented.
//!
//! **`#[ignore]`d with a reason, and that is the loud half.** An env-gated early
//! return prints to stderr, which `cargo test` swallows on a pass — so in
//! `make test` it would read as four more green tests rather than as four that
//! did not run, which is exactly the silent-skip shape this repo keeps finding at
//! the bottom of an unrun phase. Ignored, they appear in the count *with the
//! reason*; the env gate stays as the second belt, for a run that passes
//! `--include-ignored` without the rig.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::http_poll::{BinSource, Freshness, PollError};
use super::publish_layout::{ContentLayout, PublishLayout};
use super::signed_fetch::{PinnedPublisher, SignedFetchError, SignedSession};

/// **Their whole published corpus**, as their own startup contract prints it:
/// key (peer-relative to the declared `system/` prefix), authored body, and the
/// leaf **content hash they advertise** — 64-char, no algorithm byte, exactly as
/// `entry[i].content_hash` writes it.
///
/// **All three, not one.** A single-key check passes against a trie holding one
/// binding, which is a state one bad `List` away from where they just were; and
/// their fixture exists to have several keys at several depths.
///
/// The hashes are pinned because their fixture is **deterministic by
/// construction** (fixed seed, fixed tree, hand-rolled canonical CBOR so
/// byte-equality does not ride a Go-only serializer). Verified here across a
/// full stop/start of the publisher container: the entire contract, root hash
/// included, came back byte-identical. If a value below ever moves, their
/// emission changed — which is a thing worth being told rather than absorbing.
const THEIR_PAGES: [(&str, &str, &str); 3] = [
    (
        "blog/post/entry-1",
        "hello",
        "d20663fce170dc9c2fd970d765b11d1f077fac49a46e18532acc8305ffa7fc6a",
    ),
    (
        "blog/post/entry-2",
        "world",
        "e1f0e4d46fe870259f207e1427e47890e81d511055bc94babdaa349bb4ce1308",
    ),
    (
        "blog/post/entry-3",
        "fin",
        "5e53c3dd00cef1ff28e7063cce622ae8e77a80b2c9077010016b5e86479aa756",
    ),
];

/// The first of [`THEIR_PAGES`] — the single key the signature gate drives, since
/// it only needs to get far enough to prove the root verified.
const THEIR_KEY: &str = THEIR_PAGES[0].0;
const THEIR_BODY: &str = THEIR_PAGES[0].1;

// ---------------------------------------------------------------------------
// The target
// ---------------------------------------------------------------------------

/// The go publisher for this run — **out of band, exactly as a consumer gets
/// it.** An origin and a peer-id, the two strings arch's §1b hands over; nothing
/// is read off the publisher's filesystem, because sharing one is the shortcut
/// this whole gate exists to remove.
struct GoTarget {
    origin: String,
    peer_id: String,
}

/// `None` — with a reason on stderr — when the rig is not standing.
///
/// A silent skip is what this repo keeps finding at the bottom of a
/// permanently-unrun phase, so the reason is printed and the caller says which
/// case it took.
fn go_target() -> Option<GoTarget> {
    let origin = std::env::var("GO_FED_ORIGIN").ok()?;
    let peer_id = std::env::var("GO_FED_PEER_ID").ok()?;
    let origin = origin.trim().trim_end_matches('/').to_string();
    let peer_id = peer_id.trim().to_string();
    if origin.is_empty() || peer_id.is_empty() {
        return None;
    }
    // The same control the multi-host gate runs, for the same reason: a loopback
    // origin would pass every assertion below while proving nothing about a
    // second host. §11.5.1's blindness class.
    let host = host_of(&origin);
    let loopback = host == "localhost"
        || host == "::1"
        || host.parse::<std::net::Ipv4Addr>().map(|a| a.is_loopback()).unwrap_or(false);
    assert!(
        !loopback,
        "GO_FED_ORIGIN points at {host:?}, which is loopback — this is the cross-impl \
         MULTI-HOST leg, and a loopback origin makes it a slower copy of the disk fixture"
    );
    Some(GoTarget { origin, peer_id })
}

fn skipped(what: &str) {
    eprintln!(
        "SKIP {what}: GO_FED_ORIGIN / GO_FED_PEER_ID unset — this gate needs \
         entity-core-go's publisher standing on a bridge. `make crossimpl-go`."
    );
}

/// **Their endpoint, supplied out of band** (§6.5.4), field by field, because
/// every one of these differs from what our convention would have produced and
/// a silent default is how hop 0 fails.
fn their_layout(t: &GoTarget) -> PublishLayout {
    PublishLayout {
        // Terminal, and NOT `{origin}/{peer}/system/peer/published-root`. Both
        // are conformant; only the advertised one is findable, which is the
        // entire content of the v1.8 MUST.
        manifest_url: format!("{}/manifest", t.origin),
        content_url_prefix: format!("{}/content", t.origin),
        // FLAT. Their poll handler routes `content/{hex}` and 404s a sharded
        // path — a wrong guess here 404s every blob, which reads as a
        // withholding origin rather than as a layout mistake.
        content_layout: ContentLayout::Flat,
        // Peer-rooted, so `prefix_carries_peer` is true and the signature lands
        // at `{origin}/{peer}/system/signature/{hex}.bin`.
        tree_url_prefix: format!("{}/{}", t.origin, t.peer_id),
        tree_leaf_suffix: ".bin".to_string(),
    }
}

// ---------------------------------------------------------------------------
// The gates
// ---------------------------------------------------------------------------

/// **The cross-impl positive, and it is the half that is genuinely discharged.**
///
/// Their manifest decodes as a published-root entity, their two-hop signature
/// resolves, and it verifies against the key **their peer-id carries** — with no
/// key distribution, no shared process, no shared filesystem, and a real TCP hop
/// between two network namespaces.
///
/// It is asserted *through the outcome*, which is the tight part: every arm
/// below except `Verify` is reachable **only after** `fetch_root` has verified
/// the signature — `PublishedRootClient::resolve` (core-rust
/// `core/peer/src/published_root.rs`) opens with `let root = self.fetch_root()?;`
/// and `SignedSession::resolve` returns [`SignedFetchError::Verify`] terminally.
/// So one match carries both facts and cannot be satisfied by an origin whose
/// signature is wrong.
///
/// **`Absent` belongs on the verified side, and the first version of this test
/// put it on the wrong one.** It was written when the only way past the
/// signature was a withheld closure, so `IncompleteWalk` got its own arm and
/// everything else fell to a `panic!` that says *"did not verify"*. The moment go
/// published a **real but empty** root the walk started ending `Absent`, and this
/// gate blamed their signature for a tree that had verified perfectly. A
/// catch-all arm that names one cause is a diagnosis with no evidence behind it —
/// it survives exactly until the state it never considered shows up.
#[test]
#[ignore = "needs entity-core-go's publisher standing on a podman bridge — `make crossimpl-go`"]
fn a_live_go_origin_is_signed_by_the_key_its_peer_id_carries() {
    let Some(t) = go_target() else { return skipped("cross-impl go signature") };

    let pin = PinnedPublisher::with_layout(&t.peer_id, their_layout(&t))
        .expect("their peer-id is Ed25519 canonical form, so it carries its own verifying key");
    let session = SignedSession::new(pin);
    let src = LiveOrigin::new(&t.origin);

    let got = block_on(session.resolve(&src, THEIR_KEY));

    match &got {
        Ok(entity) => {
            let text = String::from_utf8_lossy(&entity.data);
            assert!(text.contains(THEIR_BODY), "{THEIR_KEY:?} came back with a body we did not author");
            eprintln!("cross-impl live: signature verified and the key resolved");
        }
        // The root verified; the origin then failed to produce a blob that root
        // declares. Terminal, and about the origin's completeness, not its key.
        Err(SignedFetchError::IncompleteWalk(what)) => {
            eprintln!("cross-impl live: signature verified; walk stopped at {what}");
        }
        // The root verified and the trie was walked to a conclusion — the key is
        // simply not in it. Says nothing about the signature, which is the whole
        // reason it is not in the panic arm.
        Err(SignedFetchError::Absent) => {
            eprintln!(
                "cross-impl live: signature verified; the walk completed and {THEIR_KEY:?} is \
                 not in their signed tree (see the sibling gate for what it does commit to)"
            );
        }
        Err(e @ SignedFetchError::Verify(_)) => panic!(
            "the go arm's signed root did not verify against the key its peer-id carries: {e}\n\
             This is a real cross-impl divergence — the signature, the two-hop keying, or the \
             bare-hashable body form."
        ),
        Err(e) => panic!(
            "the rig, not the arms: {e}\n\
             (`Transport` means the origin was unreachable from this container — check that the \
             test container is ON their bridge; `Budget` means the walk did not converge.)"
        ),
    }

    let log = src.log();
    assert!(
        log.iter().any(|u| u == &format!("{}/manifest", t.origin)),
        "the consumer must have entered at THEIR advertised manifest; fetched {log:?}"
    );
    assert!(
        log.iter().any(|u| u.contains("/system/signature/")),
        "the signature is a SECOND hop keyed on the published-root entity hash — a chain that \
         never fetched one verified nothing; fetched {log:?}"
    );
    eprintln!("cross-impl live: {} fetch(es) against {}", log.len(), t.origin);
}

/// **§1b's own clause: a consumer resolves a published key out of the signed
/// root.** Everything else in this file is a hop on the way here.
///
/// **It reports what the root DOES commit to when it fails**, via
/// [`SignedSession::enumerate`] — the keys are in the nodes (`EXTENSION-TREE`
/// §3.1's leaf is `[key, value_hash]`), so a failed resolve can always say
/// whether the tree is short, differently keyed, or empty. Without that, three
/// unrelated states arrive as one word, `Absent`, and the reader guesses. Note
/// the enumeration rides the same pump and the same verification, so it is not a
/// back door around the checks — it is the same walk asked a different question.
///
/// **This gate has been red twice, for two different reasons, and the second one
/// is why the failure text is now derived rather than written.**
///
/// 1. `cd8564a` published `root_hash = 0xC0,0xC1,…` as a literal, so the
///    declared root 404'd → [`SignedFetchError::IncompleteWalk`]. Fixed in
///    `ec30e96`.
/// 2. `ec30e96` publishes a **real, served, verifying root that commits to
///    nothing** — `BuildTrieForPrefix(cs, li, peerID, "system/")` lists the
///    **un-namespaced** index for `system/` while the entries were written
///    through a `NamespacedIndex` at `/{peer}/system/…`, so the build sees zero
///    bindings and emits an empty CHAMP node (`map: 00000000`, `data: []`).
///    Every key is `Absent`.
///
/// The through-line worth keeping: **their probe checks the hop they added, not
/// the operation a consumer performs.** The first fix's own verification was
/// *"root fetched by hash → 200, old fake hash → 404"* — which dereferences the
/// root and never asks it for a key, so an empty root passes it perfectly. That
/// is the same shape as the blind spot it was written to close, one step
/// further along.
///
/// **Left failing on purpose.** A comment saying go owes a walkable root is a
/// claim; a gate that goes green the day they land one is a measurement. It is
/// not in `make test` — it needs their rig — so it cannot mask a regression in
/// the everyday suite, which is the standing objection to a known-red gate.
#[test]
#[ignore = "needs entity-core-go's publisher standing on a podman bridge — `make crossimpl-go`"]
fn a_live_go_origin_resolves_a_published_key_from_its_signed_root() {
    let Some(t) = go_target() else { return skipped("cross-impl go resolve") };

    let pin = PinnedPublisher::with_layout(&t.peer_id, their_layout(&t)).expect("pins");
    let session = SignedSession::new(pin);
    let src = LiveOrigin::new(&t.origin);

    for (key, body, declared_hex) in THEIR_PAGES {
        let entity = resolve_or_explain(&session, &src, key);
        let text = String::from_utf8_lossy(&entity.data);
        assert!(
            text.contains(body),
            "{key:?} came back with a body we did not author: {text:?}"
        );
        // **The leaf their TRIE points at is the leaf their CONTRACT advertises.**
        // Our resolve already hash-verifies the body against whatever the trie
        // committed to, so this is not a second integrity check — it adjudicates
        // between two of *their* surfaces, which is a thing only a third party
        // holding both can do. The hash rides in the URL because it came out of
        // the trie, so the fetch log is the evidence.
        let want = format!("/content/00{declared_hex}");
        assert!(
            src.log().iter().any(|u| u.ends_with(&want)),
            "their trie routed {key:?} to a leaf other than the one entry[].content_hash \
             advertises ({declared_hex}) — their contract and their trie disagree; fetched {:?}",
            src.log()
        );
    }

    // **The key set, not just the keys we knew to ask for.** `enumerate` walks
    // the same verified root and FAILS on a declared child it cannot fetch
    // (arch's D9), so an exact count is a real statement about their emission:
    // three bindings, no more, none hidden behind an unresolvable link. Nothing
    // in this arc had ever asked a *foreign* root what it holds.
    let keys = block_on(session.enumerate(&src, "")).expect("their signed root enumerates");
    let mut expected: Vec<&str> = THEIR_PAGES.iter().map(|(k, _, _)| *k).collect();
    expected.sort_unstable();
    let mut got: Vec<&str> = keys.iter().map(String::as_str).collect();
    got.sort_unstable();
    assert_eq!(
        got, expected,
        "their signed root commits to a different key set than their contract prints"
    );

    eprintln!(
        "cross-impl live: {} key(s) resolved and enumerated from their signed root, \
         each at the leaf hash their contract advertises",
        keys.len()
    );
}

/// **A key their tree genuinely lacks is `Absent`, cross-impl and live.**
///
/// The distinction earns its keep exactly here: their origin is up and
/// answering, the walk completes, and the key simply is not in the signed tree.
/// A consumer that reported "the origin is down" would send an operator to the
/// wrong machine — and until `dabd076` **every** key on this origin produced this
/// same value for an entirely different reason, which is what makes it worth a
/// gate of its own rather than an assertion inside another one.
#[test]
#[ignore = "needs entity-core-go's publisher standing on a podman bridge — `make crossimpl-go`"]
fn a_key_absent_from_their_live_tree_is_absent_not_a_broken_origin() {
    let Some(t) = go_target() else { return skipped("cross-impl go absent") };

    let pin = PinnedPublisher::with_layout(&t.peer_id, their_layout(&t)).expect("pins");
    let session = SignedSession::new(pin);
    let src = LiveOrigin::new(&t.origin);

    // The control first: this gate is only meaningful if the tree is non-empty,
    // because an empty root answers `Absent` to everything.
    let keys = block_on(session.enumerate(&src, "")).expect("their signed root enumerates");
    assert!(
        !keys.is_empty(),
        "their root commits to nothing, so `Absent` here would prove nothing about the key"
    );

    let got = block_on(session.resolve(&src, "blog/post/no-such-entry"));
    assert!(
        matches!(got, Err(SignedFetchError::Absent)),
        "an absent key must not read as a broken or withholding origin; got {got:?}"
    );
}

/// Resolve, or fail with the root's own contents in the message. Shared by the
/// gates above so a red run always says *what the root does hold* — see the
/// module note on why `Absent` alone is three states wearing one word.
fn resolve_or_explain(
    session: &SignedSession,
    src: &LiveOrigin,
    key: &str,
) -> entity_entity::Entity {
    block_on(session.resolve(src, key)).unwrap_or_else(|e| {
        // Ask the root what it holds. `""` is every key under the declared
        // prefix — an empty answer is a fact about their emission, not a
        // failure of ours.
        let committed = match block_on(session.enumerate(src, "")) {
            Ok(keys) if keys.is_empty() => {
                "NOTHING — the root is an empty CHAMP node, so every key is `Absent`".to_string()
            }
            Ok(keys) => format!("{} key(s): {keys:?}", keys.len()),
            Err(e) => format!("(could not be enumerated either: {e})"),
        };
        panic!(
            "C-7 §1b is NOT discharged: resolving {key:?} gave `{e}`.\n\
             \n\
             Their signed root verifies against the key their peer-id carries (the sibling gate \
             passes), and it commits to {committed}.\n\
             \n\
             §1b needs a consumer to resolve a published key out of that root. A root that \
             verifies but holds no bindings satisfies every check a publisher can run on itself \
             and is unusable to a consumer in any language."
        )
    })
}

/// **Why the layout above is hand-written.** Their origin serves no endpoint
/// document, so there is nothing to discover — and deriving one is the v1.8
/// violation, so the alternative to supplying it is not consuming them at all.
///
/// Pinned as a 404 rather than left in prose: if they later ship a profile, this
/// goes red and the next seat replaces [`their_layout`] with a real decode
/// instead of maintaining a second copy of their endpoint by hand.
#[test]
#[ignore = "needs entity-core-go's publisher standing on a podman bridge — `make crossimpl-go`"]
fn a_live_go_origin_serves_no_endpoint_document() {
    let Some(t) = go_target() else { return skipped("cross-impl go profile") };

    let (status, _) = http_get(&format!("{}/transport-profile", t.origin))
        .expect("their origin answers; a connect failure here is the rig, not the arm");
    assert_eq!(
        status, 404,
        "their origin now serves a transport-profile — stop hand-writing `their_layout` and \
         decode it with `PublishLayout::from_profile_artifact`, which is the whole point of \
         §6.5.3 v1.8"
    );
}

// ---------------------------------------------------------------------------
// The transport — a real socket, from a vantage the consumer actually has
// ---------------------------------------------------------------------------

/// A [`BinSource`] over their live origin.
///
/// **This process must be ON their bridge.** Under rootless podman the host has
/// no route into a bridge network's address space — a probe from the host fails
/// against a demonstrably healthy server, which is one of the two topology
/// lessons the multi-host rig paid for. `make crossimpl-go` runs the test
/// container with `--network`, so the socket below is the consumer's own vantage
/// rather than an orchestrator's.
struct LiveOrigin {
    origin: String,
    fetched: RefCell<Vec<String>>,
}

impl LiveOrigin {
    fn new(origin: &str) -> Self {
        Self { origin: origin.to_string(), fetched: RefCell::new(Vec::new()) }
    }
    fn log(&self) -> Vec<String> {
        self.fetched.borrow().clone()
    }
}

impl BinSource for LiveOrigin {
    fn get(
        &self,
        url: String,
        _freshness: Freshness,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, PollError>>>> {
        self.fetched.borrow_mut().push(url.clone());
        // A URL under any other origin is refused rather than fetched — pinning
        // is worth nothing if the consumer will follow a URL off-publisher.
        if PublishLayout::relative_to_origin(&url, &self.origin).is_none() {
            return Box::pin(std::future::ready(Err(PollError::NotFound(404))));
        }
        let r = match http_get(&url) {
            // 404/410 is the origin CHOOSING; everything else stays retryable.
            // Collapsing them is how "withheld" and "unreachable" arrive as one
            // value, which is the seam this arc keeps meeting.
            Ok((200, body)) => Ok(body),
            Ok((s, _)) if s == 404 || s == 410 => Err(PollError::NotFound(s)),
            Ok((s, _)) => Err(PollError::Decode(format!("HTTP {s}"))),
            Err(e) => Err(PollError::Decode(e)),
        };
        Box::pin(std::future::ready(r))
    }
}

/// A one-shot HTTP/1.1 GET over a raw socket — no client crate, because this
/// tree has none on the native side and a cross-impl gate should not acquire a
/// dependency to make one request shape.
///
/// `Connection: close` so the body is "everything until EOF"; a chunked reply is
/// reported rather than mis-parsed, since silently truncating a trie node would
/// present as a hash mismatch and send the next reader after a lying origin.
fn http_get(url: &str) -> Result<(u16, Vec<u8>), String> {
    let rest = url.strip_prefix("http://").ok_or_else(|| format!("not a plain-http URL: {url}"))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };

    let mut stream = TcpStream::connect(authority).map_err(|e| format!("connect {authority}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(20))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(20))).ok();
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).map_err(|e| format!("write {url}: {e}"))?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| format!("read {url}: {e}"))?;

    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| format!("no header terminator in the reply to {url}"))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let body = raw[split + 4..].to_vec();

    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("unparseable status line from {url}: {head:?}"))?;

    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        return Err(format!(
            "{url} answered chunked; this reader does not de-chunk, and quietly returning the \
             framing bytes as a body would surface as a hash mismatch"
        ));
    }
    Ok((status, body))
}

/// Minimal executor — the consumer's futures are `!Send` by design (a wasm
/// `JsFuture` is), so no runtime is involved. Every future here is already
/// resolved when it is polled.
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

/// `http://10.89.3.2:8099` → `10.89.3.2`.
fn host_of(base: &str) -> String {
    base.trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit_once(':')
        .map(|(h, _)| h.to_string())
        .unwrap_or_else(|| base.trim_start_matches("http://").to_string())
}
