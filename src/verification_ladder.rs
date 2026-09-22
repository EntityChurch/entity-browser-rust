//! **Arch's verification ladder, EXECUTED** — what survives detachment from
//! the author's tree, measured against real published bytes.
//!
//! `EXPLORATION-THE-VERIFICATION-LADDER-WHAT-A-PUBLISHER-SIGNS-AND-WHAT-A-READER-CAN-CHECK`
//! (arch, 2026-09-06) states five rungs and a **composer contract** — *a reader
//! can only climb a rung the publisher paid for*. It is prose, and its rung-1
//! row reads **"RULED, unlanded"** against a measurement taken at our `d9cc645`.
//! Our feed publisher landed rung 1 at `faf73cb`, **four days later**. This
//! module runs the ladder rather than restating it.
//!
//! ## The one property the whole thing turns on
//!
//! [`crate::feed_read::attribute`] takes **no store, no index and no source**.
//! It derives the author's public key from the author's *peer id* and rebuilds
//! the expected signer identity hash locally, so rung 1 costs **two bodies and
//! a string** and reaches back to no origin at all. That is not an
//! optimisation — it is what makes a detached entity speak for itself, and it
//! is why [`a_rung_1_entry_still_names_its_author_with_the_origin_deleted`] can
//! delete the publisher's entire tree before it verifies anything.
//!
//! Root-anchored evidence cannot do this, and the reason is structural rather
//! than a matter of cost: [`crate::content_site::signed_fetch::SignedSession`]
//! resolves a **key against a source**, re-fetching the manifest every time so
//! the `seq` floor stays meaningful. There is no verb anywhere in the cohort
//! that takes *(entity, root, path)* and answers *"is this in that root"* —
//! measured 2026-09-14 across `entity-core-{rust,go,py}`, zero occurrences of
//! any inclusion-proof spelling. So rung 2 is **origin-shaped by construction**:
//! carrying it means reconstructing something that looks like the author's
//! origin, and [`a_carried_snapshot_is_refused_once_the_author_publishes_again`]
//! is what that costs.
//!
//! ## What this module does NOT establish
//!
//! Rung 1 says *who wrote these bytes*. It says **nothing about where to get
//! them**, and that distinction is the whole of the open locator question: the
//! substitute chain's admission gate
//! (`extensions/storage-substitute-sources`, `verify_entry_signature_against`)
//! demands a signature whose `target` is the **chain entry's** hash — an entity
//! in *my* tree asserting where A's content lives. An entry's own invariant
//! signature has `target = the content hash`, so it does not and cannot satisfy
//! that gate. **Signing content does not sign a claim about content.** These
//! are two asks, and conflating them is how "just sign the entities" reads as
//! an answer to a question it does not touch.

#![cfg(all(test, not(target_arch = "wasm32")))]

use std::fs;
use std::path::Path;

use entity_entity::Entity;
use entity_hash::Hash;

use crate::content_site::http_poll::{crack_pointer, verify_and_decode};
use crate::content_site::signed_root::RootProjector;
use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
use crate::feed::{entry_key, signature_key, FeedEntry};
use crate::feed_publish::publish_feed;
use crate::feed_read::{attribute, Attribution, Unattributed};

const NOW: u64 = 1_757_000_000_000;

fn identity() -> entity_crypto::Keypair {
    entity_crypto::Keypair::from_seed([0x5c; 32])
}

fn body(s: &str) -> EmbedNode {
    EmbedNode::new("text/plain", EmbedData::new(EmbedPayload::Inline(s.as_bytes().to_vec()), s))
}

fn posts(author: &str, n: usize) -> Vec<FeedEntry> {
    (0..n).map(|i| FeedEntry::new(author, NOW + i as u64, body(&format!("post {i}")))).collect()
}

/// Resolve one published key the way a consumer does: crack the pointer, fetch
/// the body from the content store, **verify it hashes to its own address**.
///
/// Deliberately not a helper that hands back the entity from memory — the
/// subject here is *bytes on disk that somebody else wrote*, and a rig that
/// shortcuts the two-hop is measuring its own variables.
fn resolve(dir: &Path, peer: &str, key: &str) -> (Hash, Entity, usize) {
    let ptr = fs::read(dir.join(peer).join(format!("{key}.bin"))).expect("the pointer is published");
    let hash = crack_pointer(&ptr).expect("the pointer cracks");
    let hex = hash.to_hex();
    let blob = dir.join("content").join(&hex[0..2]).join(&hex[2..4]).join(&hex);
    let bytes = fs::read(&blob).expect("the body is published");
    let len = bytes.len();
    let ent = verify_and_decode(&bytes, &hash).expect("the body hashes to its address");
    (hash, ent, len)
}

/// Every `system/signature` a published tree carries, as `(target, signer)`.
fn published_signatures(dir: &Path, peer: &str) -> Vec<Hash> {
    let sig_dir = dir.join(peer).join("system/signature");
    let Ok(entries) = fs::read_dir(&sig_dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|s| s.to_str()) != Some("bin") {
            continue;
        }
        let Ok(ptr) = fs::read(&p) else { continue };
        let Ok(hash) = crack_pointer(&ptr) else { continue };
        let hex = hash.to_hex();
        let blob = dir.join("content").join(&hex[0..2]).join(&hex[2..4]).join(&hex);
        let Ok(bytes) = fs::read(&blob) else { continue };
        let Ok(ent) = verify_and_decode(&bytes, &hash) else { continue };
        if let Ok(sig) = entity_types::SignatureData::from_entity(&ent) {
            out.push(sig.target);
        }
    }
    out.sort_by_key(|h| h.to_hex());
    out
}

// ---------------------------------------------------------------------------
// Rung 1 — authorship, detached
// ---------------------------------------------------------------------------

/// **The claim, run: a rung-1 entry names its author with the publisher's whole
/// origin deleted.**
///
/// Two bodies are lifted out of a real projection — the entry and its
/// `system/signature` — and then `remove_dir_all` takes the tree, the root, the
/// root's signature, the identity entity, the index and every other post with
/// it. What is left is what a mirror can hand a stranger, and it is enough.
///
/// **The anti-vacuity half is the deletion, and it is asserted.** A test that
/// merely called `attribute` on bytes it still had a tree behind would pass
/// identically with the origin intact, and would be measuring the function
/// signature rather than the property.
#[test]
fn a_rung_1_entry_still_names_its_author_with_the_origin_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let mut root = RootProjector::new(identity()).unwrap();
    let author = root.peer_id().to_string();
    let report = publish_feed(dir.path(), &mut root, &posts(&author, 4), &[], 3, NOW).unwrap();
    root.finish(dir.path()).expect("the root signs over the whole feed");

    let target = report.entry_hashes[1];
    let (entry_hash, _entry, entry_len) =
        resolve(dir.path(), &author, &entry_key(&target));
    let (_sig_hash, sig, sig_len) =
        resolve(dir.path(), &author, &signature_key(&author, &target));
    assert_eq!(entry_hash, target, "the rig resolved the entry it meant to");

    // The author is gone. Not unreachable — gone.
    let path = dir.path().to_path_buf();
    fs::remove_dir_all(&path).expect("the origin is removed");
    assert!(!path.exists(), "the anti-vacuity guard: the tree must really be gone");

    assert_eq!(
        attribute(&author, &entry_hash, Some(&sig)),
        Attribution::Signed,
        "rung 1 must hold on the two carried bodies alone"
    );

    // The cost, stated rather than implied: two bodies and a peer-id string.
    assert!(
        entry_len + sig_len < 1024,
        "the carried evidence is small and constant: {entry_len} + {sig_len} bytes"
    );
}

/// **The composer contract, from the other end: a reader cannot climb a rung
/// the publisher did not pay for.**
///
/// The same detached entry, with the signature the publisher minted simply not
/// carried, is `NoSignature` — presented as by nobody. This is the state
/// `a_site_page_was_never_given_a_signature_to_carry` shows is the *default*
/// for every other axis we publish.
#[test]
fn the_same_entry_without_its_carried_signature_is_attributable_to_nobody() {
    let dir = tempfile::tempdir().unwrap();
    let mut root = RootProjector::new(identity()).unwrap();
    let author = root.peer_id().to_string();
    let report = publish_feed(dir.path(), &mut root, &posts(&author, 2), &[], 3, NOW).unwrap();
    root.finish(dir.path()).unwrap();

    let target = report.entry_hashes[0];
    let (entry_hash, _e, _) = resolve(dir.path(), &author, &entry_key(&target));

    assert_eq!(
        attribute(&author, &entry_hash, None),
        Attribution::Unattributed(Unattributed::NoSignature),
        "the bytes are intact and unattributable — rung 0 without rung 1"
    );
}

// ---------------------------------------------------------------------------
// The scope gap — which axes paid for rung 1
// ---------------------------------------------------------------------------

/// **A feed publish mints one signature per entry PLUS one over the root, and
/// the ratio is the finding.**
///
/// Arch's §2 measured *"nothing signs individual entities"* at `d9cc645`. This
/// is the same measurement taken from a **published projection** rather than
/// from source, four days later, on the axis that changed: `n` entries produce
/// `n + 1` signatures.
///
/// Asserted as a **ratio against the entry count**, never as a literal, because
/// a literal tracks the fixture and this property is about the emitter.
#[test]
fn a_feed_publish_mints_one_signature_per_entry_and_one_over_the_root() {
    for n in [1usize, 4, 9] {
        let dir = tempfile::tempdir().unwrap();
        let mut root = RootProjector::new(identity()).unwrap();
        let author = root.peer_id().to_string();
        let report = publish_feed(dir.path(), &mut root, &posts(&author, n), &[], 3, NOW).unwrap();
        let signed_root = root.finish(dir.path()).unwrap();

        let targets = published_signatures(dir.path(), &author);
        assert_eq!(
            targets.len(),
            n + 1,
            "{n} entries must yield {} signatures (one each, one over the root), got {}",
            n + 1,
            targets.len()
        );
        for h in &report.entry_hashes {
            assert!(
                targets.contains(h),
                "every entry must carry its own detached signature; {} does not",
                h.to_hex()
            );
        }
        // And the extra one is the root's — so the count is explained, not
        // merely satisfied.
        assert!(
            targets.iter().any(|t| t.to_hex() == signed_root.head_hex),
            "the root's own signature must be among them"
        );
    }
}

/// **The gap, measured on the axis that did not move: a site page is published
/// with no signature of its own, so there is nothing for a mirror to carry.**
///
/// This is the state of every non-feed axis we publish — sites, and the app
/// sets beside them. A publish signs exactly one thing, the root, and a page
/// lifted out of that tree has integrity and no authorship.
///
/// **The assertion is on the emitted set, not on `attribute`.** Asking
/// `attribute` for a verdict here would only re-measure `None → NoSignature`,
/// which the test above already pins; the subject is the *publisher's*
/// omission, and the only place that is visible is what landed on disk.
#[test]
fn a_site_page_was_never_given_a_signature_to_carry() {
    use crate::content_site::format::{NavItem, SiteManifest, SitePage};
    use crate::content_site::publish_fixture::emit_owned_sites;
    use crate::content_site::read::OwnedSite;

    let dir = tempfile::tempdir().unwrap();
    let mut root = RootProjector::new(identity()).unwrap();
    let author = root.peer_id().to_string();

    let site = OwnedSite {
        peer_id: author.clone(),
        site_id: "home".into(),
        manifest: SiteManifest::new("home", "Home", "index", vec![NavItem::new("Home", "/index")]),
        pages: vec![
            ("index".into(), SitePage::markdown("Home", "# authored")),
            ("other".into(), SitePage::markdown("Other", "# also authored")),
            ("third".into(), SitePage::markdown("Third", "# and a third")),
        ],
        assets: vec![],
        content: Vec::new(),
    };
    emit_owned_sites(dir.path(), std::slice::from_ref(&site), "", Some(&mut root)).unwrap();
    let signed_root = root.finish(dir.path()).unwrap();

    let targets = published_signatures(dir.path(), &author);
    assert_eq!(
        targets.len(),
        1,
        "a site publish signs exactly one thing; got {} signatures",
        targets.len()
    );
    assert_eq!(
        targets[0].to_hex(),
        signed_root.head_hex,
        "and the one thing it signs is the root, not any page"
    );

    // The control that makes the count mean something: there really were
    // several distinct page entities in that projection to have signed.
    assert!(
        signed_root.keys >= 4,
        "the manifest plus three pages must be in the trie, else the zero is vacuous: {} keys",
        signed_root.keys
    );
}

// ---------------------------------------------------------------------------
// Rung 2 — publication, and why carrying it is different in kind
// ---------------------------------------------------------------------------

/// **A carried snapshot is refused by a reader who has seen the author publish
/// again — and it is refused for being OLD, not for being wrong.**
///
/// This is the property that separates rung 1 from rung 2 in practice rather
/// than in cost. Root-anchored evidence is a claim about *the author's tree at
/// an instant*, so a third party carrying it is carrying a snapshot; the
/// consumer's anti-rollback floor — the thing that makes a signed root worth
/// having — is exactly what refuses a snapshot once a newer one has been seen.
/// Everything about the carried tree verifies. It is declined anyway.
///
/// **`Declined`, not `Verify`**, and the variant is the point: the carrier has
/// no defect, and neither does the author. The refusal is this reader's policy.
/// A detached signature has no equivalent — it does not age, because it makes
/// no claim about *now*.
#[test]
fn a_carried_snapshot_is_refused_once_the_author_publishes_again() {
    use crate::content_site::publish_layout::PublishLayout;
    use crate::content_site::signed_fetch::{PinnedPublisher, SignedFetchError, SignedSession};
    use crate::feed::index_head_key;
    use crate::feed_gather::DirOrigin;
    use crate::feed_read::block_on;

    let carried = tempfile::tempdir().unwrap();
    let current = tempfile::tempdir().unwrap();

    // ONE publisher, two publishes — the only way to get a real `seq` ordering.
    let kp = identity();
    let pubkey = kp.public_key_bytes().to_vec();
    let key_type = kp.key_type();
    let mut root = RootProjector::new(kp).unwrap();
    let author = root.peer_id().to_string();

    publish_feed(carried.path(), &mut root, &posts(&author, 2), &[], 3, NOW).unwrap();
    let v1 = root.finish(carried.path()).unwrap();

    publish_feed(current.path(), &mut root, &posts(&author, 3), &[], 3, NOW + 1).unwrap();
    let v2 = root.finish(current.path()).unwrap();
    assert!(v2.seq > v1.seq, "the republish must advance seq: {} then {}", v1.seq, v2.seq);

    let pin = PinnedPublisher {
        origin: String::new(),
        layout: PublishLayout::conventional("", &author),
        peer_id: author.clone(),
        pubkey,
        key_type,
    };

    let session = SignedSession::new(pin);
    // The reader sees the author's current tree first — as anyone following the
    // author directly would.
    block_on(session.resolve(&DirOrigin(current.path().to_path_buf()), index_head_key()))
        .expect("the author's current feed resolves");

    // Now the same reader is offered the snapshot a third party carried.
    let offered =
        block_on(session.resolve(&DirOrigin(carried.path().to_path_buf()), index_head_key()));
    assert!(
        matches!(&offered, Err(SignedFetchError::Declined(e)) if e.contains("rollback")),
        "a carried snapshot must be DECLINED as stale, and must not be reported as \
         a verification failure by anybody: {offered:?}"
    );

    // **The other arm, and it is what makes "perishable" the right word.** The
    // identical carried bytes, offered to a reader who has *not* seen the
    // author's current tree, are accepted without complaint. So the snapshot is
    // neither valid nor invalid on its own terms — its fate is decided by the
    // recipient's history, which is a property no detached signature has. A
    // mirror handing the same bundle to two readers gets two answers.
    let naive = SignedSession::new(PinnedPublisher {
        origin: String::new(),
        layout: PublishLayout::conventional("", &author),
        peer_id: author.clone(),
        pubkey: identity().public_key_bytes().to_vec(),
        key_type: identity().key_type(),
    });
    block_on(naive.resolve(&DirOrigin(carried.path().to_path_buf()), index_head_key())).expect(
        "the demonstration only holds if the carried snapshot is otherwise perfectly good",
    );
}

// ---------------------------------------------------------------------------
// The cost probe
// ---------------------------------------------------------------------------

/// **What rung 1 costs a publisher, measured — `#[ignore]`d and asserting
/// nothing.**
///
/// Arch's §2 prices rung 1 as *"one signature per entry"*. That is the count;
/// this is the **bytes**, which is what decides whether the answer to *"what
/// else should be signed"* is "everything" or "some of it". A threshold nobody
/// has earned would be a flake, and the point is the shape of the ratio.
///
/// ## Measured 2026-09-14, and the shape is the answer
///
/// ```text
///    posts   entry bytes     sig bytes   sig/entry      files
///       10          1890          2070       1.10         51
///      100         19080         20700       1.08        443
///      500         96280        103500       1.07       2087
/// ```
///
/// **A signature is 207 bytes, flat** — 64 signature + two 33-byte hashes + the
/// type string and CBOR frame — and **two files** (pointer + blob), whatever it
/// signs. So the cost is *constant per entity*, not proportional, and the only
/// case where it is expensive in relative terms is the case where it is
/// trivial in absolute ones: rung 1 more than **doubles** a feed of 189-byte
/// posts, and costs **+0.7%** on a 30 KB site page, **+0.04%** on the 476 KB
/// figure this repo's largest published asset actually is.
///
/// ⇒ **the decision axis is not which entities are important, it is which
/// entities TRAVEL.** A per-entity signature cannot be priced out for anything
/// substantial, and for the small entities where the ratio looks alarming the
/// absolute number is 207 bytes. Whatever else is contentious about rung 1,
/// bytes are not the argument against it.
///
/// `make test-one T="the_cost_of_rung_1 --ignored"`
#[test]
#[ignore]
fn the_cost_of_rung_1() {
    println!("{:>8}  {:>12}  {:>12}  {:>10}  {:>9}", "posts", "entry bytes", "sig bytes", "sig/entry", "files");
    for n in [10usize, 100, 500] {
        let dir = tempfile::tempdir().unwrap();
        let mut root = RootProjector::new(identity()).unwrap();
        let author = root.peer_id().to_string();
        let report = publish_feed(dir.path(), &mut root, &posts(&author, n), &[], 32, NOW).unwrap();
        root.finish(dir.path()).unwrap();

        let mut entry_bytes = 0usize;
        let mut sig_bytes = 0usize;
        for h in &report.entry_hashes {
            let (_, _, e) = resolve(dir.path(), &author, &entry_key(h));
            let (_, _, s) = resolve(dir.path(), &author, &signature_key(&author, h));
            entry_bytes += e;
            sig_bytes += s;
        }
        let files = walkdir_count(dir.path());
        println!(
            "{n:>8}  {entry_bytes:>12}  {sig_bytes:>12}  {:>9.2}  {files:>9}",
            sig_bytes as f64 / entry_bytes as f64
        );
    }
}

fn walkdir_count(dir: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = fs::read_dir(&p) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                n += 1;
            }
        }
    }
    n
}
