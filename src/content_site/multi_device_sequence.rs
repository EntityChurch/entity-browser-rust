//! **C-2 — two devices, one key, one signed-root sequence. A MEASUREMENT.**
//!
//! Arch's open core item is *multi-device publishing*: two devices sharing one
//! identity, both advancing one signed root sequence, with nothing arbitrating
//! them. It was routed 2026-09-05 as **not ours to fix** (`core/peer`, root
//! sequence advance) and **ours to exhibit**, because *"two devices sharing one
//! key"* is a configuration this repo already supports. This file is the
//! exhibition. It proposes nothing.
//!
//! ## The finding that makes it cheap, and it is in OUR code
//!
//! Sequence continuity is anchored in the **output directory**, not in the
//! identity. [`RootProjector::adopt_prior_head`] recovers the previous head from
//! the directory being published into, because every CLI run builds a fresh
//! in-memory peer ([`resolve_publish_source`](super::publish)) and the only
//! durable half is the **keypair**. So two out-dirs under one key are two
//! independent sequences — the C-2 configuration, reachable with no second
//! machine.
//!
//! That anchoring is not a mistake: before it, *every* emit published `seq 0`
//! regardless of history, which made the consumer's rollback floor inert against
//! our own publisher. It was the right fix for one publisher and it is precisely
//! what leaves this case open.
//!
//! ## The bound, stated rather than discovered
//!
//! **Tori cannot publish**, so the literal *"desktop app plus a browser
//! profile"* framing is not reachable today — there is no verb that reads a
//! long-lived native store (F3's other side). **The rig here is two out-dirs
//! under one keypair, and that is the same mechanism**: same identity,
//! independent head recovery, nothing shared. Say which one you ran.
//!
//! ## What each test licenses you to say
//!
//! Every cell records **the emitted `seq`** and, separately, **what a consumer
//! does**, because they are different questions and only the second is a defect.
//! Each carries its own control: a measurement whose expected value equals what
//! the rig would produce anyway measures the rig.
//!
//! | cell | test | claim |
//! |---|---|---|
//! | 1 | `a_second_device_publishing_under_one_identity_restarts_the_sequence_at_zero` | the emitter side |
//! | 2 | `a_cold_consumer_takes_whichever_devices_tree_it_reaches_and_says_nothing` | no floor on a first visit |
//! | 3 | `a_session_that_has_seen_one_device_refuses_the_other_as_a_rollback` | **the headline** |
//! | 3b | `which_device_is_refused_depends_only_on_the_order_they_were_read_in` | the asymmetry |
//! | 4 | `two_devices_at_the_same_seq_serve_different_trees_and_no_floor_can_see_it` | **the cell with no defence** |
//! | 5 | `a_forked_continuation_leaves_two_roots_naming_one_predecessor` | what "just copy the head" produces |
//! | — | `one_publisher_held_across_both_directories_advances_normally` | **the falsifier**, and it isolates the cause |
//!
//! **Read the falsifier first.** The same two directories, published by one
//! process, advance normally and are accepted in either order — so two
//! *directories* are not the cause. The cause is that the durable state boundary
//! is the **keypair and nothing else**: a second *process* has no head, wherever
//! it writes.
//!
//! **Cell 3 is the headline:** the consumer's only defence against a rollback is
//! also what breaks legitimate multi-device publishing, so a fix cannot be
//! *"tighten the floor"*. **Cell 4 must not be skipped:** a floor compares `seq`,
//! and two trees at the *same* `seq` never trip one — the same shape as the
//! original *"two trees both at zero never go backwards"* defect.
//!
//! **Do not fix this by persisting the head beside the keypair.** It makes cell 1
//! green and does nothing for cells 3–5: two devices that cannot see each other's
//! `ENTITY_DATA_DIR` are the whole problem.
//!
//! Map: `docs/plans/PLAN-2026-09-09-THE-C-2-MEASUREMENT-TWO-DEVICES-ONE-KEY.md`.

use std::cell::RefCell;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use entity_hash::Hash;

use super::format::{NavItem, SiteManifest, SitePage};
use super::http_poll::{BinSource, Freshness, PollError};
use super::publish_fixture::emit_owned_sites;
use super::read::OwnedSite;
use super::signed_fetch::{resolve_signed, PinnedPublisher, SignedFetchError, SignedSession};
use super::signed_root::{RootProjector, SignedRootReport, PUBLISHED_ROOT_REL};

/// **One identity, shared by every device below.** The durable half of a real
/// deployment is exactly this: `persistence::publisher_keypair`, one keypair,
/// however many machines hold a copy of it.
const SEED: [u8; 32] = [0xc2; 32];

const KEY: &str = "sites/home/pages/index";

fn identity() -> entity_crypto::Keypair {
    entity_crypto::Keypair::from_seed(SEED)
}

fn peer_id() -> String {
    identity().peer_id().as_str().to_string()
}

/// A site whose index page carries `body`, so the tree that answered is
/// identifiable from the bytes rather than from which directory we pointed at.
fn site(peer_id: &str, body: &str) -> OwnedSite {
    OwnedSite {
        peer_id: peer_id.to_string(),
        site_id: "home".into(),
        manifest: SiteManifest::new("home", "Home", "index", vec![NavItem::new("Home", "/index")]),
        pages: vec![
            ("index".into(), SitePage::markdown("Home", body)),
            ("other".into(), SitePage::markdown("Other", "# a second authored page")),
        ],
        assets: vec![],
        content: Vec::new(),
    }
}

/// **One publish, as one device performs it.**
///
/// A **fresh [`RootProjector`] per call** is the load-bearing detail: that is the
/// CLI's shape, and reusing one projector across two directories is a *live
/// peer's* shape, which is the case that has always worked. The existing
/// rollback gate in `signed_fetch` reuses one projector deliberately; this file
/// must not, or it measures a single device publishing twice.
fn device_publish(out: &Path, body: &str) -> SignedRootReport {
    let mut root = RootProjector::new(identity()).expect("projector builds");
    let pid = root.peer_id().to_string();
    root.adopt_prior_head(out).expect("a prior head in THIS out-dir is adopted");
    let s = site(&pid, body);
    emit_owned_sites(out, std::slice::from_ref(&s), "", Some(&mut root)).expect("emits");
    root.finish(out).expect("signs a root")
}

/// The published-root manifest a device left in `out`, decoded — `seq` and
/// `predecessor` read from the artifact a consumer would fetch, not from our own
/// report of what we thought we wrote.
fn head_on_disk(out: &Path) -> entity_types::PublishedRootData {
    let path = out.join(peer_id()).join(PUBLISHED_ROOT_REL);
    let bytes = fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let entity = entity_wire::decode_entity(&bytes).expect("the head decodes");
    entity_types::PublishedRootData::from_entity(&entity).expect("it is a published-root")
}

fn pin() -> PinnedPublisher {
    PinnedPublisher::from_peer_id("", &peer_id()).expect("a canonical peer-id carries its key")
}

/// The bytes of the index page a resolve returned, for identifying *which*
/// device's tree answered.
fn body_of(entity: &entity_entity::Entity) -> String {
    String::from_utf8_lossy(&entity.data).to_string()
}

/// Copy a whole published directory — one device's out-dir handed to another,
/// which is cell 5's setup and the shape a naive *"just copy the head across"*
/// fix produces.
fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).expect("readable dir").flatten() {
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if src.is_dir() {
            fs::create_dir_all(&dst).unwrap();
            copy_tree(&src, &dst);
        } else {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::copy(&src, &dst).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// A consumer over a published directory
// ---------------------------------------------------------------------------

/// A [`BinSource`] reading one device's out-dir — the real consumer path with
/// the network removed.
///
/// A near-twin of `signed_fetch`'s test-only `DirSource`, minus the substitution
/// and unreachability affordances this file has no use for. Kept local rather
/// than promoted: a shared test double would have to grow both files' needs, and
/// nothing here is a *rule* two call sites could disagree about (the C15 case),
/// only a fixture.
struct Device {
    root: PathBuf,
}

impl Device {
    fn at(root: &Path) -> Self {
        Self { root: root.to_path_buf() }
    }
}

impl BinSource for Device {
    fn get(
        &self,
        url: String,
        _freshness: Freshness,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>> {
        let rel = url.trim_start_matches('/').to_string();
        let r = fs::read(self.root.join(&rel))
            .map_err(|e| super::http_poll::poll_error_for_io(&rel, &e));
        Box::pin(std::future::ready(r))
    }
}

/// Minimal executor — these futures are `!Send` by design (a wasm `JsFuture`
/// is), so no runtime is involved.
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

// ---------------------------------------------------------------------------
// Cell 1 — the emitter side
// ---------------------------------------------------------------------------

/// **CELL 1.** A device publishing for the first time under an identity that is
/// already at `seq 1` somewhere else emits **`seq 0`** — a silent sequence
/// restart under a live identity, with a valid signature over it.
///
/// **The control is the second publish into device A's own directory.** Without
/// it this test would pass on a rig where *nothing ever advances*, which is the
/// defect this whole area was already fixed for once. `advances` is the
/// falsifier for `restarts`.
///
/// What this licenses you to say: the identity does not carry its own sequence;
/// **the directory does**. Nothing here is a consumer-visible defect yet — cells
/// 2–5 are where it becomes one.
#[test]
fn a_second_device_publishing_under_one_identity_restarts_the_sequence_at_zero() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();

    let a1 = device_publish(a.path(), "# device A, publish 1");
    assert_eq!(a1.seq, 0, "a first publish anywhere is seq 0");

    // CONTROL: the same device, the same out-dir, again. This is the property
    // `a_second_publish_into_the_same_directory_advances_the_sequence` pins, and
    // it is repeated here so the measurement below cannot be explained by a rig
    // in which the sequence never moves at all.
    let a2 = device_publish(a.path(), "# device A, publish 2");
    assert_eq!(a2.seq, 1, "CONTROL: one device's own out-dir continues its sequence");

    // MEASUREMENT: a second device, same key, its own out-dir, never seen A.
    let b1 = device_publish(b.path(), "# device B, publish 1");
    assert_eq!(
        b1.seq, 0,
        "a second device restarts the sequence at zero under a live identity"
    );

    // Both are the same publisher, and both heads verify — this is not two
    // identities, and nothing in either artifact is malformed.
    let head_a = head_on_disk(a.path());
    let head_b = head_on_disk(b.path());
    assert_eq!(head_a.peer_id, head_b.peer_id, "one identity, two sequences");
    assert_eq!((head_a.seq, head_b.seq), (1, 0));
    assert!(head_a.predecessor.is_some(), "A's second publish chains off its first");
    assert!(
        head_b.predecessor.is_none(),
        "B believes it is a first publish — which is what makes it indistinguishable from one"
    );

    eprintln!(
        "C-2 cell 1: one identity {} — device A at seq {}, device B at seq {}",
        head_a.peer_id, head_a.seq, head_b.seq
    );
}

/// **THE FALSIFIER FOR THE WHOLE FILE — and it isolates the cause to the
/// process, not to the directory.**
///
/// Publish into the *same two out-dirs* with **one** [`RootProjector`] held
/// across both calls, and the sequence advances `0 → 1` and a session accepts
/// both trees in either order. Everything cells 1–5 report disappears.
///
/// So two directories are **not** the cause. A live peer that outlives both
/// publishes carries the head in its own location index and is perfectly happy
/// to publish one sequence into two places — which is exactly what
/// `a_session_refuses_a_seq_rollback_that_a_per_call_client_accepts` relies on to
/// manufacture a real `seq` ordering.
///
/// **The cause is that the durable state boundary is the keypair and nothing
/// else.** A second *process* has no head, wherever it writes; a second
/// *directory* is incidental. That is the sentence to hand to whoever designs
/// the fix, and it is the reason "give device B a starting sequence" only moves
/// the problem into cell 4.
#[test]
fn one_publisher_held_across_both_directories_advances_normally() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();

    // ONE projector, two out-dirs — a live peer's shape, not the CLI's.
    let mut root = RootProjector::new(identity()).expect("projector builds");
    let pid = root.peer_id().to_string();

    let s1 = site(&pid, "# one publisher, first location");
    emit_owned_sites(a.path(), std::slice::from_ref(&s1), "", Some(&mut root)).unwrap();
    let r1 = root.finish(a.path()).unwrap();

    let s2 = site(&pid, "# one publisher, second location");
    emit_owned_sites(b.path(), std::slice::from_ref(&s2), "", Some(&mut root)).unwrap();
    let r2 = root.finish(b.path()).unwrap();

    assert_eq!(
        (r1.seq, r2.seq),
        (0, 1),
        "held in one process, the sequence advances across directories"
    );

    // And no consumer complains, in either direction that matters: the second
    // location is strictly newer, which is what the floor is built to accept.
    let session = SignedSession::new(pin());
    let first = block_on(session.resolve(&Device::at(a.path()), KEY)).expect("first location");
    assert!(body_of(&first).contains("first location"), "{}", body_of(&first));
    let second = block_on(session.resolve(&Device::at(b.path()), KEY))
        .expect("the newer location is accepted");
    assert!(body_of(&second).contains("second location"), "{}", body_of(&second));

    eprintln!(
        "C-2 falsifier: one process, two directories, seq {} then {} — no restart, no refusal",
        r1.seq, r2.seq
    );
}

// ---------------------------------------------------------------------------
// Cell 2 — the cold consumer
// ---------------------------------------------------------------------------

/// **CELL 2.** A visitor with no session has **no floor**, so it takes whichever
/// device's tree it reaches, in either order, with no signal that another exists.
///
/// This is the ordinary case, not an edge: `session_cache` records that
/// forgetting a session lowers the floor to zero, and
/// `a_session_refuses_a_seq_rollback_that_a_per_call_client_accepts` already
/// names the one-shot path as a hole. C-2 is what makes that hole *reachable
/// without an attacker* — the two trees are both genuinely ours.
#[test]
fn a_cold_consumer_takes_whichever_devices_tree_it_reaches_and_says_nothing() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    device_publish(a.path(), "# device A, publish 1");
    device_publish(a.path(), "# device A, publish 2");
    device_publish(b.path(), "# device B, publish 1");

    let pin = pin();

    // A fresh visitor reaching B first.
    let from_b = block_on(resolve_signed(&Device::at(b.path()), &pin, KEY))
        .expect("a cold consumer accepts device B");
    assert!(body_of(&from_b).contains("device B"), "{}", body_of(&from_b));

    // Another fresh visitor — a new tab, a new boot — reaching A.
    let from_a = block_on(resolve_signed(&Device::at(a.path()), &pin, KEY))
        .expect("a cold consumer accepts device A");
    assert!(body_of(&from_a).contains("device A, publish 2"), "{}", body_of(&from_a));

    assert_ne!(
        from_a.content_hash, from_b.content_hash,
        "the two devices really are serving different bytes under one key"
    );
    eprintln!("C-2 cell 2: both devices accepted by a cold consumer, in either order");
}

// ---------------------------------------------------------------------------
// Cell 3 — the headline
// ---------------------------------------------------------------------------

/// **CELL 3 — THE HEADLINE.** A session that has read device A (`seq 1`) refuses
/// device B (`seq 0`) as a **rollback**: same key, same peer-id, valid
/// signature, every hash correct, and the tree is one the publisher genuinely
/// published from their other machine.
///
/// **So the legitimate second device is indistinguishable from an attack**, and
/// that is the finding: the consumer's only defence against a rollback is the
/// same mechanism that breaks multi-device publishing. A fix cannot be *"tighten
/// the floor"* — the floor is behaving exactly as designed.
///
/// The control is the first resolve: A must be served before the refusal means
/// anything, or this passes on a rig where nothing resolves at all.
#[test]
fn a_session_that_has_seen_one_device_refuses_the_other_as_a_rollback() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    device_publish(a.path(), "# device A, publish 1");
    device_publish(a.path(), "# device A, publish 2");
    device_publish(b.path(), "# device B, publish 1");

    let session = SignedSession::new(pin());

    // CONTROL: the session reads A and is satisfied.
    let seen = block_on(session.resolve(&Device::at(a.path()), KEY)).expect("device A resolves");
    assert!(body_of(&seen).contains("device A, publish 2"), "{}", body_of(&seen));

    // MEASUREMENT: the same publisher's other machine.
    let refused = block_on(session.resolve(&Device::at(b.path()), KEY));
    assert!(
        matches!(&refused, Err(SignedFetchError::Verify(e)) if e.contains("rollback")),
        "device B must be refused as a rollback — that is the finding, not a bug in the rig: {refused:?}"
    );
    eprintln!("C-2 cell 3: the publisher's own second device is refused as a rollback: {refused:?}");
}

/// **CELL 3b — the asymmetry, and it is the part that makes cell 3 unfixable by
/// tuning.** The *same two devices* either work or are refused depending purely
/// on which one the visitor happened to read first.
///
/// Read B then A and the sequence goes **up**, so nothing trips: the reader is
/// silently moved onto the other machine's tree. Read A then B and it is a
/// rollback. Two visitors of one deployment, same instant, opposite outcomes —
/// and neither is told anything.
#[test]
fn which_device_is_refused_depends_only_on_the_order_they_were_read_in() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    device_publish(a.path(), "# device A, publish 1");
    device_publish(a.path(), "# device A, publish 2");
    device_publish(b.path(), "# device B, publish 1");

    // B first: seq 0, then A's seq 1. Upwards, so the floor is never troubled —
    // and the content the reader is on changes machine underneath them.
    let session = SignedSession::new(pin());
    let first = block_on(session.resolve(&Device::at(b.path()), KEY)).expect("B resolves first");
    assert!(body_of(&first).contains("device B"), "{}", body_of(&first));
    let second = block_on(session.resolve(&Device::at(a.path()), KEY))
        .expect("A is accepted after B — the sequence went up");
    assert!(body_of(&second).contains("device A, publish 2"), "{}", body_of(&second));

    // And the reverse order, in a second session, is cell 3's refusal.
    let reverse = SignedSession::new(pin());
    block_on(reverse.resolve(&Device::at(a.path()), KEY)).expect("A resolves first");
    let refused = block_on(reverse.resolve(&Device::at(b.path()), KEY));
    assert!(
        matches!(&refused, Err(SignedFetchError::Verify(_))),
        "the same pair of devices, read the other way round, is a refusal: {refused:?}"
    );
    eprintln!("C-2 cell 3b: B→A silently swaps trees; A→B is refused. Same two devices.");
}

// ---------------------------------------------------------------------------
// Cell 4 — the cell with no defence
// ---------------------------------------------------------------------------

/// **CELL 4 — THE ONE THAT MUST NOT BE SKIPPED.** Two devices at the **same**
/// `seq`, serving **different** content, both validly signed under one key.
///
/// A rollback floor compares `seq`. Two trees at one `seq` never compare
/// unfavourably, so **nothing in the chain has anything to say** — not the
/// signature, not the hash recompute, not the floor. The session accepts both
/// and the second silently replaces the first.
///
/// This is the same shape as the defect this whole area was earned on: *two
/// trees both at zero never go backwards*. Fixing cell 1 by giving device B a
/// starting sequence puts every deployment in **this** cell instead, which is
/// why cell 1 is not the interesting one.
#[test]
fn two_devices_at_the_same_seq_serve_different_trees_and_no_floor_can_see_it() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    device_publish(a.path(), "# device A, publish 1");
    device_publish(a.path(), "# device A, publish 2");
    device_publish(b.path(), "# device B, publish 1");
    device_publish(b.path(), "# device B, publish 2");

    let head_a = head_on_disk(a.path());
    let head_b = head_on_disk(b.path());
    assert_eq!(head_a.seq, head_b.seq, "the premise: both devices are at one seq");
    assert_ne!(
        head_a.root_hash, head_b.root_hash,
        "the premise: they commit to different trees"
    );

    let session = SignedSession::new(pin());
    let first = block_on(session.resolve(&Device::at(a.path()), KEY)).expect("A resolves");
    assert!(body_of(&first).contains("device A, publish 2"), "{}", body_of(&first));

    let second = block_on(session.resolve(&Device::at(b.path()), KEY))
        .expect("B at the same seq is accepted — nothing compares unfavourably");
    assert!(
        body_of(&second).contains("device B, publish 2"),
        "the tree really did change underneath the session: {}",
        body_of(&second)
    );
    eprintln!(
        "C-2 cell 4: two signed roots at seq {} under one key, both accepted, no signal",
        head_a.seq
    );
}

// ---------------------------------------------------------------------------
// Cell 5 — the forked continuation
// ---------------------------------------------------------------------------

/// **CELL 5.** Hand device B a copy of device A's out-dir — the *"just carry the
/// head across"* fix — and both then advance from it. The result is two roots at
/// one `seq` **naming the same predecessor**.
///
/// Two things worth carrying out of this cell:
///
/// 1. It lands in **cell 4**, which has no defence. Seeding B closes cell 1 and
///    buys nothing, which is the plan's stated reason not to reach for it.
/// 2. **The evidence a fork happened is IN the artifact and nothing consults
///    it.** `predecessor` is on every published root; two roots at one `seq`
///    naming one predecessor is a fork, statable without any coordination
///    between the devices. That is a fact about what a detector *could* use — it
///    is not a design, and designing the arbitration is `core/peer`'s.
#[test]
fn a_forked_continuation_leaves_two_roots_naming_one_predecessor() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    device_publish(a.path(), "# device A, publish 1");
    device_publish(a.path(), "# device A, publish 2");

    // The seeding: B starts from A's published state.
    copy_tree(a.path(), b.path());
    let seeded = head_on_disk(b.path());
    assert_eq!(seeded.seq, 1, "B now believes it is where A was");

    // Both advance, independently, from that shared point.
    let a3 = device_publish(a.path(), "# device A, publish 3");
    let b3 = device_publish(b.path(), "# device B, publish 3");
    assert_eq!((a3.seq, b3.seq), (2, 2), "both continue the same sequence number");

    let head_a = head_on_disk(a.path());
    let head_b = head_on_disk(b.path());
    assert_ne!(head_a.root_hash, head_b.root_hash, "different trees");
    assert_eq!(
        head_a.predecessor, head_b.predecessor,
        "both name the seeded head as their predecessor — the fork is stated in the artifact"
    );
    let shared: Option<Hash> = head_a.predecessor;
    assert!(shared.is_some(), "there is a predecessor to share");

    // And a session accepts both, exactly as cell 4 does: seeding did not help.
    let session = SignedSession::new(pin());
    block_on(session.resolve(&Device::at(a.path()), KEY)).expect("A resolves");
    let swapped = block_on(session.resolve(&Device::at(b.path()), KEY))
        .expect("the forked continuation is accepted too");
    assert!(body_of(&swapped).contains("device B, publish 3"), "{}", body_of(&swapped));

    eprintln!(
        "C-2 cell 5: two roots at seq {} sharing predecessor {} — a fork, detectable in principle, \
         detected by nothing",
        head_a.seq,
        shared.map(|h| h.to_hex()).unwrap_or_default()
    );
}
