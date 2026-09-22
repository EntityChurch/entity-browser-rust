//! `APP-CONVENTION-FEED` §6 — **the gatherer**: publish what you gathered, so
//! the next reader does not have to gather it again.
//!
//! [`crate::feed`] carries the codec, [`crate::feed_publish`] emits a peer's own
//! feed and [`crate::feed_read`] consumes somebody else's. This is the act that
//! closes the loop between them: **consume from N sources and publish the
//! result**, which is the one operation that makes aggregation aggregatable.
//!
//! ## Why this is the interesting verb and not just a third emitter
//!
//! The replication proposal states it as the load-bearing property: *what a peer
//! obtains, it may publish; a published result is the same kind of object as the
//! sources it was built from.* The argument for it is a typing argument, and it
//! is the best sentence in that document — **the systems that centralized did not
//! choose to. Their aggregator's output was a different type from its input** (an
//! API, a query response, a database), so nobody could consume it and there ended
//! up being exactly one of them. A mechanism closed under its own output has no
//! privileged tier to centralize into.
//!
//! ## ⛔ The two preconditions, and they are the whole of this module
//!
//! Ruled as `D20` on 2026-09-11, and both were assumed handled for three
//! revisions before a build round found them:
//!
//! 1. **Byte preservation.** A republished entity is bound byte-identically to
//!    the form it was obtained in — *including not decoding it through a type
//!    that does not fully declare it.* That is [`crate::feed_read::Obtained`]'s
//!    job and this module never re-encodes; [`plan_mirror`] re-checks rather than
//!    inherits, so the invariant is local to the act that depends on it.
//! 2. **Author-anchored evidence surviving detachment.** An `Entity` carries no
//!    signer — authenticity in this substrate is normally *root-anchored*, and a
//!    gatherer's root **cannot** commit to another peer's keys. So every carried
//!    entry travels with the author's own detached `system/signature`, which
//!    verifies from the bytes alone.
//!
//! ⭐ **What that buys, stated so the shape of the output directory makes
//! sense:** the carried bytes are deliberately **not** in the gatherer's signed
//! root. The gatherer signs *the mirror record* — its own statement of what it
//! gathered — and the entries are evidence that stands on its own. A root that
//! claimed them would be asserting something it has no standing to assert, which
//! is why `RootProjector::record` skips a foreign peer rather than being taught
//! to include one.
//!
//! ## Two findings from building it — both ruled, and both changed the spec
//!
//! **1. §6's mirror was a THREAD mirror and the closure trace is a TIMELINE.**
//! v0.1 defined `reference` as pinned-only, so `subject: reference` pinned one
//! entity, while the trace the whole design is argued from is a gatherer
//! following three *authors* and republishing their timelines — a growing prefix
//! that cannot be pinned. **Ruled `A-57`: the subject widens to
//! `any-reference`**, and [`MirrorSubject`] is the two kinds. The one thing to
//! read before touching the key is `FEED-R27`: the derivation is over
//! *identifying fields only*, because `at` and `via` are optional hints and a
//! derivation that includes an optional field is not a derivation — two
//! gatherers of one author would publish correct, verifiable views at two
//! addresses, each unable to compute the other's, with no error anywhere.
//!
//! **2. A mirror is NOT consumed by the identical code path a feed is**, which is
//! weaker than the closure sentence reads. At the **entry** it is identical —
//! [`read_mirror`] and `read_feed` share `finish_entry`, byte for byte, and that
//! is the part the property actually needs. At the **set** it is not: a feed is
//! an index head plus key-addressed pages, a mirror is one record naming pins. So
//! *"Erin follows Bob the same way she follows a person"* is true of how she
//! renders and verifies and false of how she walks. The fixed point still closes
//! — a gatherer can gather a mirror, since [`plan_mirror`] takes the same
//! [`ReadEntry`] rows either reader produces — it closes `mirror → mirror` rather
//! than `feed → feed`.
//!
//! ⛔ **That measurement made the `MUST` stricter, and the new clause binds this
//! module: `DX-R4` forbids publishing, under our own namespace, an author's own
//! set-layer object over content that author did not place there.** A reader
//! taking the flat sentence literally goes looking for an `app/feed/index` under
//! the *gatherer* — which is the gatherer asserting a claim only the author can
//! make, unauthenticated besides, since the authorship instrument signs entries
//! and not sets. `a_gatherer_publishes_no_set_layer_object_of_the_authors` is
//! `DX-C6` and it inspects what a plan binds.
//! *A sentence true at the layer everyone is thinking about and false at the
//! layer nobody is reviews clean forever.*

#![allow(dead_code)] // no verb publishes a mirror yet; the gates are native

use std::future::Future;
use std::pin::Pin;

use entity_entity::Entity;
use entity_hash::Hash;

use crate::entity_ref::EntityRef;
use crate::feed::{entry_key, FeedError, FeedMirror, MirrorSubject};
use crate::feed_read::{finish_entry, recomputed_hash, FeedSource, ReadEntry};

// ---------------------------------------------------------------------------
// Gathering — the plan
// ---------------------------------------------------------------------------

/// One foreign entity a mirror publish carries, **at the address its own author
/// would have bound it at**.
///
/// `peer` is the author's, never the gatherer's. That is what lets a consumer
/// reach it with the reader it already has: `finish_entry` asks for
/// `app/feed/entries/{hex}` and `system/signature/{hex}` under the author, and a
/// mirror's origin answers both because the gatherer wrote them there.
#[derive(Debug, Clone, PartialEq)]
pub struct Carried {
    pub peer: String,
    pub key: String,
    pub entity: Entity,
}

/// Everything one mirror publish emits: the gatherer's signed record, the
/// foreign bytes it carries, and the hash-addressed closure beneath them.
#[derive(Debug, Clone, PartialEq)]
pub struct MirrorPlan {
    pub record: FeedMirror,
    pub carried: Vec<Carried>,
    /// **The `system/content` closure behind every pointer body**, hash-
    /// addressed and therefore bound at no tree key at all.
    ///
    /// ⛔ **This field exists because the first cut did not have it and
    /// published a dangling reference** — the identical defect `publish_feed`
    /// shipped with one convention over, and for the identical reason: a body
    /// over EMBED §3's 16 KiB ceiling is *refused inline*, so the pointer arm is
    /// the only conformant way to carry one, which makes the closure a
    /// requirement of that arm rather than a nicety.
    ///
    /// It was invisible to every gate in `feed_mirror` and every gate in
    /// `feed_fetch`, because they all run against a map-backed double that
    /// serves whatever it was handed. **What found it was `--verify`'s new
    /// mirror arm, on its first run against a fixture carrying one long post**
    /// — *"the post appears in the index and its body is empty"* — which is the
    /// case for `AUDIT F8`'s rule made as plainly as it can be made: every
    /// declaring type owes an arm, in the commit that introduces it, and the
    /// arm finds things the type's own round-trip cannot.
    pub content: Vec<Entity>,
}

impl MirrorPlan {
    /// How many entries this mirror holds.
    pub fn entry_count(&self) -> usize {
        self.record.entries.len()
    }

    /// The peers whose namespaces this plan writes into — **never the
    /// gatherer's**, which is the whole of why a mirror needs `PublishAxis::
    /// carried_peers`. Sorted and deduped, because a caller uses it to decide
    /// what a clean may touch and a list with an order nobody chose is one
    /// somebody eventually depends on.
    pub fn carried_peers(&self) -> Vec<String> {
        let mut peers: Vec<String> = self.carried.iter().map(|c| c.peer.clone()).collect();
        peers.sort();
        peers.dedup();
        peers
    }

    /// How many of them travel with an author's signature.
    ///
    /// **Reported rather than required.** §6.1 rule 3 obliges a reader to present
    /// an unsigned entry as unattributed; it does not oblige a gatherer to drop
    /// it, and dropping would be the wrong repair — a mirror's only lie is
    /// omission.
    pub fn attributable(&self) -> usize {
        self.carried.iter().filter(|c| c.key.starts_with("system/signature/")).count()
    }
}

/// Why a gather refused to plan.
#[derive(Debug, Clone, PartialEq)]
pub enum GatherError {
    /// A row's bytes do not hash to the address the row claims. **Refused at the
    /// gatherer**, not passed on: republishing it would bind bytes under a name
    /// that does not address them, and the signature fetched by that name would
    /// travel with bytes it does not cover.
    RowDoesNotAddress { claimed: Hash, actual: Hash },
    /// A row we could not re-address at all.
    RowUnencodable { claimed: Hash },
    /// ⛔ **An entry declares a body blob this gather does not hold.**
    ///
    /// Refused rather than carried, because the alternative is a mirror that
    /// *names a post and serves an empty one* — the post appears, the body is
    /// blank, and nothing anywhere says why. A short mirror is legal (§1.3) and
    /// a hollow entry is not: omission is a view somebody can reason about,
    /// and an entry whose declared closure is absent is a claim that does not
    /// resolve.
    ///
    /// **The refusal is on `plan_mirror` rather than on its callers on
    /// purpose** (AP44): a `content` argument a caller could pass empty is a
    /// step the next caller forgets, and the consequence is not an error but a
    /// silently broken publication.
    BodyClosureMissing { entry: Hash, blob: Hash },
    /// A carried blob's bytes do not hash to the address the entry named.
    ClosureDoesNotAddress { claimed: Hash, actual: Hash },
}

impl std::fmt::Display for GatherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatherError::RowDoesNotAddress { claimed, actual } => write!(
                f,
                "a gathered row claims {} and its bytes hash to {} — refusing to republish it",
                claimed.to_hex(),
                actual.to_hex()
            ),
            GatherError::RowUnencodable { claimed } => {
                write!(f, "a gathered row claiming {} does not re-address", claimed.to_hex())
            }
            GatherError::BodyClosureMissing { entry, blob } => write!(
                f,
                "entry {} declares body blob {} and this gather does not hold it — \
                 publishing it would name a post and serve an empty one",
                entry.to_hex(),
                blob.to_hex()
            ),
            GatherError::ClosureDoesNotAddress { claimed, actual } => write!(
                f,
                "a gathered body blob claims {} and its bytes hash to {} — refusing to \
                 republish it",
                claimed.to_hex(),
                actual.to_hex()
            ),
        }
    }
}

/// **The gather decision, as one pure function.**
///
/// Takes what a reader obtained and produces what a publisher will emit. Pure
/// and native for the reason every decision in this repo that matters is: the
/// alternative is reaching it through a projection, a temp directory and an
/// async pump, and then every combination of *(signed × unsigned × substituted)*
/// costs a publish.
///
/// ## What it refuses, and why it re-checks something the reader already checked
///
/// `read_feed` compares every entry's computed address against the pin the index
/// named, so rows arriving from it cannot fail here. Rows do not have to arrive
/// from it — a gatherer is exactly the caller that assembles from several
/// readers — and this is the act whose correctness depends on the invariant.
/// ***A guard on the act that needs it is one nobody has to remember to call***
/// (AP44), and re-hashing a body we already hold costs nothing.
///
/// ## What it deliberately does NOT refuse
///
/// An entry whose signature is **absent** is carried without one, and an entry
/// whose signature is **present and invalid** is carried *with it, unchanged*.
/// Dropping a bad signature would convert *"this is forged"* into *"nobody signed
/// this"* at the consumer — two facts that send a person to different places —
/// and a mirror is not the layer that adjudicates. The consumer re-verifies
/// every one from the bytes; §6.1 rule 3 already says what it must then present.
pub fn plan_mirror(
    gathered_by: &str,
    subject: &MirrorSubject,
    rows: &[ReadEntry],
    closure: &[Entity],
    gathered_at: u64,
) -> Result<MirrorPlan, GatherError> {
    let mut entries = Vec::with_capacity(rows.len());
    let mut carried = Vec::with_capacity(rows.len() * 2);
    // Indexed by the address each blob claims, so the lookup below is by the
    // same name the entry declares — never by position, which would let a
    // caller hand over the right number of wrong bytes.
    let mut held: std::collections::BTreeMap<String, &Entity> = Default::default();
    for blob in closure {
        let actual = recomputed_hash(blob)
            .map_err(|_| GatherError::RowUnencodable { claimed: blob.content_hash })?;
        if actual != blob.content_hash {
            return Err(GatherError::ClosureDoesNotAddress {
                claimed: blob.content_hash,
                actual,
            });
        }
        held.insert(actual.to_hex(), blob);
    }
    let mut needed: Vec<Hash> = Vec::new();

    for row in rows {
        let actual = recomputed_hash(&row.obtained.entity)
            .map_err(|_| GatherError::RowUnencodable { claimed: row.hash })?;
        if actual != row.hash {
            return Err(GatherError::RowDoesNotAddress { claimed: row.hash, actual });
        }

        // **The declared closure, walked transitively.** A pointer body names a
        // `system/content/blob`, and *that* declares its chunks — same two
        // levels `--verify` and `publish_feed` both walk, and stopping at the
        // first would carry a blob whose reassembly ends nowhere.
        let mut queue = crate::feed_tree::body_blob_hashes(&row.entry.body);
        let mut seen: std::collections::BTreeSet<String> = Default::default();
        while let Some(h) = queue.pop() {
            if !seen.insert(h.to_hex()) {
                continue;
            }
            let Some(blob) = held.get(&h.to_hex()) else {
                return Err(GatherError::BodyClosureMissing { entry: row.hash, blob: h });
            };
            needed.push(h);
            if blob.entity_type == entity_types::TYPE_CONTENT_BLOB {
                if let Ok(chunks) = crate::content_site::asset_store::chunk_hashes_of(blob) {
                    queue.extend(chunks);
                }
            }
        }

        let author = row.entry.author.clone();
        entries.push(EntityRef::pin(author.clone(), row.hash));
        carried.push(Carried {
            peer: author.clone(),
            key: entry_key(&row.hash),
            // **The obtained entity, cloned. Never `row.entry.to_entity()`.**
            entity: row.obtained.entity.clone(),
        });
        if let Some(sig) = &row.obtained.signature {
            carried.push(Carried {
                peer: author.clone(),
                key: crate::feed::signature_key(&author, &row.hash),
                entity: sig.clone(),
            });
        }
    }

    // Only what the entries actually declare — a caller handing over a wider
    // closure publishes bytes nothing in this mirror names, which is the orphan
    // shape `--verify` reports and the next publisher's clean cannot remove.
    let mut carried_content: Vec<Entity> = Vec::new();
    let mut emitted: std::collections::BTreeSet<String> = Default::default();
    for h in needed {
        if emitted.insert(h.to_hex()) {
            carried_content.push((*held.get(&h.to_hex()).expect("checked above")).clone());
        }
    }

    Ok(MirrorPlan {
        record: FeedMirror::new(subject.reference(), entries, gathered_at, gathered_by),
        carried,
        content: carried_content,
    })
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// Project a mirror into `dir` through `root`: the gatherer's signed record, and
/// every carried body at its own author's address.
///
/// **This records; it does not `finish`.** Same rule as `publish_feed` and for
/// the same reason — one publish or none.
///
/// ⚠ **The carried bodies are written and NOT bound.**
/// `RootProjector::record` early-returns on a foreign peer, so the files land in
/// the output directory and the gatherer's signed root names none of them. That
/// is not a limitation being worked around; it is the shape the tree tier forces
/// and the reason §6 needs detached signatures at all.
#[cfg(not(target_arch = "wasm32"))]
pub fn publish_mirror(
    dir: &std::path::Path,
    root: &mut crate::content_site::signed_root::RootProjector,
    plan: &MirrorPlan,
) -> Result<(), String> {
    let gatherer = root.peer_id().to_string();
    if plan.record.gathered_by != gatherer {
        return Err(format!(
            "the mirror record says it was gathered by {} and this publisher is {gatherer}",
            plan.record.gathered_by
        ));
    }

    for item in &plan.carried {
        write(dir, &item.peer, &item.key, &item.entity, root)?;
    }
    // **Hash-addressed, so `put_only` and not `write`.** A pointer body names
    // its blob by hash; binding it under a tree key as well would publish the
    // same bytes twice and make the root commit to something the consumer
    // already reaches. Same door and same reason as `publish_feed`'s closure
    // loop and an oversized site figure's blob.
    for blob in &plan.content {
        root.put_only(blob);
    }

    // §6.0.1 — the key is derived from the subject, for **both** kinds. This used
    // to refuse a live subject, because v0.1's `subject` was pinned-only and a
    // key we cannot derive is a mirror nobody can find; `A-57` widened the field
    // and gave the live kind its own derivation, so the refusal is gone rather
    // than relaxed.
    let key = MirrorSubject::from_reference(&plan.record.subject).key();
    let entity = plan.record.to_entity()?;
    write(dir, &gatherer, &key, &entity, root)
}

#[cfg(not(target_arch = "wasm32"))]
fn write(
    dir: &std::path::Path,
    peer_id: &str,
    key: &str,
    ent: &Entity,
    root: &mut crate::content_site::signed_root::RootProjector,
) -> Result<(), String> {
    crate::content_site::publish_fixture::write_entity(dir, peer_id, key, ent, Some(root))
        .map_err(|e| format!("write {key}: {e}"))
}

// ---------------------------------------------------------------------------
// Reading a mirror
// ---------------------------------------------------------------------------

/// A source that can be asked about **any** peer at one place.
///
/// [`FeedSource`] is this trait scoped to one peer, and that scoping is exactly
/// right for a feed — a feed is one author's. A mirror is not: §6.2's whole point
/// is that *a conversation spans publishers*, so one record names entries by
/// several authors and every one of them is served by the **gatherer's** origin
/// under that author's segment.
///
/// Shaped like [`FeedSource`] — a boxed `!Send` future, owned arguments — for the
/// same reason it is: the production implementation wraps an HTTP fetch or an L1
/// dispatch and cannot borrow `self` into the future.
pub trait MirrorSource {
    fn get(
        &self,
        peer: String,
        relative_key: String,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>>;
}

/// One peer's view of a [`MirrorSource`] — the adapter that makes the scoping
/// relationship above **one expression** rather than two similar traits.
struct Scoped<'a, M: ?Sized> {
    src: &'a M,
    peer: String,
}

impl<M: MirrorSource + ?Sized> FeedSource for Scoped<'_, M> {
    fn get(
        &self,
        relative_key: String,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
        self.src.get(self.peer.clone(), relative_key)
    }
}

/// Why reading a mirror stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum MirrorReadError {
    /// No mirror of this subject at the gatherer's derivable key. **An ordinary
    /// fact**, and distinct from the next one: *this gatherer has not gathered
    /// this* is not *we could not reach them*.
    NoMirror { key: String },
    /// We could not look.
    Unreachable { key: String, detail: String },
    /// The record does not decode, or is not the gatherer's own.
    Malformed { key: String, source: FeedError },
    /// The mirror names an entry and its bytes hash to something else. §6.1 rule
    /// 2 — **a mirror can omit but never substitute** — so this is fatal where a
    /// missing entry is merely a shorter view.
    Substituted { named: Hash, served: Hash },
}

impl std::fmt::Display for MirrorReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MirrorReadError::NoMirror { key } => {
                write!(f, "this peer has published no mirror at {key}")
            }
            MirrorReadError::Unreachable { key, detail } => {
                write!(f, "could not read {key}: {detail}")
            }
            MirrorReadError::Malformed { key, source } => write!(f, "{key}: {source}"),
            MirrorReadError::Substituted { named, served } => write!(
                f,
                "the mirror names {} and the bytes served hash to {} — \
                 a mirror may omit an entry, never substitute one",
                named.to_hex(),
                served.to_hex()
            ),
        }
    }
}

/// Read `gatherer`'s mirror of `subject`, from `src`.
///
/// ⭐ **Every entry goes through the same `finish_entry` a direct feed read
/// uses** — decode, fetch the author's detached signature, attribute — so an
/// entry that arrives via a stranger is verified exactly as one that arrives from
/// its author, with no branch and no weaker path. *That is the closure property
/// where it is checkable.*
///
/// **An entry the mirror names and the origin does not serve is skipped**, not
/// fatal: §1.3 makes a partial view short rather than wrong, and a gatherer whose
/// origin lost one body must not become unreadable. A **substituted** body is
/// fatal, for §6.1 rule 2's reason.
pub async fn read_mirror<M: MirrorSource + ?Sized>(
    src: &M,
    gatherer: &str,
    subject: &MirrorSubject,
    limit: usize,
) -> Result<Vec<ReadEntry>, MirrorReadError> {
    // **The reader COMPUTES the address; it never discovers it** (§6.0.1). That
    // is the whole of what makes a mirror usable as a source: a reader holding
    // the subject needs no index, no query and no prior hop to know where to ask.
    let key = subject.key();
    let entity = match src.get(gatherer.to_string(), key.clone()).await {
        Err(detail) => return Err(MirrorReadError::Unreachable { key, detail }),
        Ok(None) => return Err(MirrorReadError::NoMirror { key }),
        Ok(Some(e)) => e,
    };
    let record = FeedMirror::from_entity(&entity, gatherer)
        .map_err(|source| MirrorReadError::Malformed { key: key.clone(), source })?;

    let mut out = Vec::new();
    for reference in &record.entries {
        if out.len() >= limit {
            break;
        }
        // §2.2 makes `entries` pinned-only and `from_entity` already refused
        // anything else, so this is a pin and its hash is the entry's identity.
        let EntityRef::Pinned { peer: author, hash, .. } = reference else { continue };
        let scoped = Scoped { src, peer: author.clone() };
        let entry_at = entry_key(hash);
        let Ok(Some(body)) = scoped.get(entry_at.clone()).await else { continue };

        let served = recomputed_hash(&body)
            .map_err(|_| MirrorReadError::Substituted { named: *hash, served: *hash })?;
        if served != *hash {
            return Err(MirrorReadError::Substituted { named: *hash, served });
        }
        // The identical consumer. Not a mirror-flavoured copy of it.
        match finish_entry(&scoped, author, *hash, &entry_at, body).await {
            Ok(row) => out.push(row),
            // A row that does not decode is skipped for `read_by_enumeration`'s
            // reason: a stranger's tree may hold anything, and one bad body must
            // not hide the rest of a conversation.
            Err(_) => continue,
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed_read::{block_on, read_feed, Attribution, Tree};
    use std::collections::BTreeMap;

    /// A whole origin: `(peer, key) -> entity`. The double a mirror needs, since
    /// one origin serves several peers' segments.
    #[derive(Default, Clone)]
    struct Origin(BTreeMap<(String, String), Entity>);

    impl MirrorSource for Origin {
        fn get(
            &self,
            peer: String,
            relative_key: String,
        ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
            let found = self.0.get(&(peer, relative_key)).cloned();
            Box::pin(std::future::ready(Ok(found)))
        }
    }

    impl Origin {
        fn absorb(&mut self, peer: &str, tree: &Tree) {
            for (k, v) in &tree.0 {
                self.0.insert((peer.to_string(), k.clone()), v.clone());
            }
        }
        fn take(&mut self, plan: &MirrorPlan, gatherer: &str) {
            for c in &plan.carried {
                self.0.insert((c.peer.clone(), c.key.clone()), c.entity.clone());
            }
            // The same derivation the publisher uses (§6.0.1) — **never a key
            // spelled here**, or the gates would agree with each other about an
            // address production disagrees with.
            let key = MirrorSubject::from_reference(&plan.record.subject).key();
            self.0.insert((gatherer.to_string(), key), plan.record.to_entity().unwrap());
        }
    }

    /// The thread subject these gates use: A's entry, published by A.
    fn thread(author: &str, root: Hash) -> MirrorSubject {
        MirrorSubject::thread(author, root)
    }

    fn gathered_rows(n: usize) -> (Vec<ReadEntry>, String, Tree) {
        let (tree, author, _) = crate::feed_publish::tests::published_tree(n);
        let rows = block_on(read_feed(&tree, &author, 100)).expect("the feed reads");
        (rows, author, tree)
    }

    /// ⭐⭐ **THE CLOSURE GATE: A → B → C.**
    ///
    /// A authors and publishes. B consumes A and republishes byte-preserving into
    /// its own publication. C consumes B **with the same entry consumer**, does
    /// not know B authored none of it, and every entry comes back attributed to
    /// **A** with every hash byte-identical at both hops.
    ///
    /// ⚠ **Stated because it is the trap: this gate CANNOT see a re-encoding
    /// gatherer.** Every fixture entry here was written by our own encoder, so
    /// decode-and-re-encode is lossless over them and the neuter that replaces
    /// `plan_mirror`'s carried bytes with `row.entry.to_entity()` leaves this
    /// green. §6.1 says so in as many words — *a round trip through bytes your own
    /// encoder produced proves nothing.* The gate that reds under that neuter is
    /// [`a_gatherer_that_re_encodes_publishes_a_feed_nobody_wrote`], and it is
    /// where the byte-preservation `MUST` is actually measured. Falsified in that
    /// direction: the neuter reds exactly one of these two.
    #[test]
    fn what_a_peer_obtains_it_may_publish_and_the_next_reader_cannot_tell() {
        let (rows, author, _) = gathered_rows(4);
        assert!(rows.iter().all(|r| r.attribution == Attribution::Signed));

        let gatherer = "2GathererPeerIdForTheseGates";
        let subject = thread(&author, rows[0].hash);
        let plan = plan_mirror(gatherer, &subject, &rows, &[], 1_757_000_999).expect("the gather plans");

        let mut origin = Origin::default();
        origin.take(&plan, gatherer);

        let read = block_on(read_mirror(&origin, gatherer, &subject, 100))
            .expect("the mirror reads");

        assert_eq!(read.len(), 4, "every carried entry came back");
        for (before, after) in rows.iter().zip(read.iter()) {
            assert_eq!(after.hash, before.hash, "a hash moved across republication");
            assert_eq!(
                after.obtained.entity.data, before.obtained.entity.data,
                "the bytes are not the bytes"
            );
            assert_eq!(
                after.attribution,
                Attribution::Signed,
                "an entry that arrived via a stranger lost its authorship"
            );
            assert_eq!(after.entry.author, author, "attribution followed the carrier");
        }
    }

    /// ⛔ **THE CONTROL ARM — republishing the way a gatherer naturally would.**
    ///
    /// Decode the body through a struct, put it back. Against a fixture *our own
    /// encoder wrote* that round trip is lossless and the hashes survive — which
    /// is why the first cut of this gate measured nothing and said so by failing:
    /// ***a round trip through bytes your own encoder produced proves nothing***,
    /// the hazard §6.1 spells out for whoever implements rule 1.
    ///
    /// So the arm is built the way the loss actually reaches a gatherer: **a
    /// publisher carries a field this build has never heard of** — the realistic
    /// case, since a gatherer aggregates types it did not write — and V7 §2.6
    /// obliges us to ignore exactly those. The author signs *their* bytes. Then:
    /// the re-encode drops the field, **the hash moves, nothing errors anywhere**,
    /// and the author's signature stops naming what was bound, so `FEED-R4`
    /// obliges the entry to render **unattributed**.
    ///
    /// *A complete, verifiable, correctly-walked publication in which nobody
    /// wrote anything.*
    #[test]
    fn a_gatherer_that_re_encodes_publishes_a_feed_nobody_wrote() {
        let (rows, author, _) = gathered_rows(1);

        // What the author published: their entry, plus one field from next year.
        let base = rows[0].obtained.entity.clone();
        let mut body: entity_ecf::Value =
            ciborium::from_reader(base.data.as_slice()).expect("the body decodes");
        let entity_ecf::Value::Map(fields) = &mut body else { panic!("an entry body is a map") };
        fields.push((entity_ecf::text("a_field_from_next_year"), entity_ecf::uinteger(7)));
        let published =
            Entity::new(&base.entity_type, entity_ecf::to_ecf(&body)).expect("it encodes");
        let published_hash = published.content_hash;

        // …signed by the author, over the bytes they published.
        let signer = crate::content_site::signed_root::RootProjector::new(
            entity_crypto::Keypair::from_seed([7u8; 32]),
        )
        .expect("the fixture author");
        assert_eq!(signer.peer_id(), author, "the fixture's identity moved");
        let sig = crate::feed_publish::signature_entity(&signer, &published_hash)
            .expect("the author signs their own entry");

        assert_eq!(
            crate::feed_read::attribute(&author, &published_hash, Some(&sig)),
            Attribution::Signed,
            "the starting state is an entry that IS attributable — otherwise this \
             arm measures a broken fixture"
        );

        // The naive gatherer: decode through a struct, put it back.
        let decoded = crate::feed::FeedEntry::from_entity(&published, &author)
            .expect("it decodes — §2.6 says ignore what you do not know");
        let re_encoded = decoded.to_entity().expect("and it re-encodes, with no error");

        assert_ne!(
            re_encoded.content_hash, published_hash,
            "the unknown field survived the round trip — then this arm is measuring \
             our own encoder against itself, which is the hazard §6.1 names"
        );
        assert!(
            !crate::feed_read::attribute(&author, &re_encoded.content_hash, Some(&sig))
                .may_name_the_author(),
            "a re-encoded entry was still attributable — the loss this rule exists \
             for would be invisible"
        );

        // And what `plan_mirror` does with the same input, which is the point.
        let row = ReadEntry {
            hash: published_hash,
            entry: decoded,
            attribution: Attribution::Signed,
            obtained: crate::feed_read::Obtained {
                entity: published.clone(),
                signature: Some(sig.clone()),
            },
        };
        let gatherer = "2GathererPeerIdForTheseGates";
        let plan =
            plan_mirror(gatherer, &thread(&author, published_hash), &[row], &[], 9)
                .expect("the gather plans");
        let carried = &plan.carried[0].entity;
        assert_eq!(
            carried.data, published.data,
            "the gatherer re-encoded the entry it was carrying"
        );
        assert_eq!(carried.content_hash, published_hash);

        let mut origin = Origin::default();
        origin.take(&plan, gatherer);
        let read = block_on(read_mirror(&origin, gatherer, &thread(&author, published_hash), 10)).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(
            read[0].attribution,
            Attribution::Signed,
            "an entry carrying a field we do not understand lost its author"
        );
    }

    /// ⛔⭐ **A POST WHOSE BODY IS A POINTER CANNOT BE MIRRORED WITHOUT ITS
    /// BYTES — and the refusal is on the plan, not on its callers.**
    ///
    /// Found by `--verify`'s mirror arm on its first run, not by reasoning:
    /// every fixture in this module is all-inline, so nothing here could
    /// exhibit it, and the end-to-end publish reported *"the post appears in
    /// the index and its body is empty"* about a gather that had just
    /// succeeded. The identical defect `publish_feed` shipped with, one
    /// convention over — *a test population you generated cannot contain the
    /// shape you are missing.*
    ///
    /// Refused rather than carried, because a short mirror is legal (§1.3) and
    /// a **hollow** entry is not: omission is a view somebody can reason about,
    /// and a post that renders blank with nothing saying why is not.
    ///
    /// Both directions, because either one alone is satisfied by an
    /// implementation that always refuses or always accepts.
    #[test]
    fn an_entry_whose_body_is_a_pointer_is_refused_without_the_bytes_it_names() {
        use crate::embed::{EmbedData, EmbedNode, EmbedPayload};

        let (rows, author, _) = gathered_rows(1);
        let blob = Entity::new("system/content", entity_ecf::to_ecf(&entity_ecf::text("x")))
            .unwrap();
        let blob_hash = blob.content_hash;

        // An entry whose body names those bytes rather than carrying them.
        let entry = crate::feed::FeedEntry::new(
            &author,
            1_000,
            EmbedNode::new(
                "text/plain",
                EmbedData::new(EmbedPayload::Pointer(blob_hash), "a long post"),
            ),
        );
        let entity = entry.to_entity().unwrap();
        let row = ReadEntry {
            hash: entity.content_hash,
            entry,
            attribution: crate::feed_read::Attribution::Signed,
            obtained: crate::feed_read::Obtained { entity, signature: None },
        };
        let subject = thread(&author, rows[0].hash);

        match plan_mirror("2Gatherer", &subject, &[row.clone()], &[], 9) {
            Err(GatherError::BodyClosureMissing { entry, blob }) => {
                assert_eq!(entry, row.hash);
                assert_eq!(blob, blob_hash);
            }
            other => panic!("a hollow post was planned for republication: {other:?}"),
        }

        // …and with the bytes it is carried, hash-addressed and bound at no
        // tree key — which is what makes the refusal above about the closure
        // rather than about pointer bodies.
        let plan = plan_mirror("2Gatherer", &subject, &[row], std::slice::from_ref(&blob), 9)
            .expect("a pointer body with its bytes plans");
        assert_eq!(plan.content, vec![blob]);
        assert!(
            plan.carried.iter().all(|c| c.key.starts_with(crate::feed::entry_prefix())),
            "a content blob was bound at a tree key — it is reached by hash"
        );
    }

    /// A caller handing over more than the entries declare publishes bytes
    /// nothing names. The plan carries the **declared** closure, not the offer.
    #[test]
    fn a_closure_wider_than_what_the_entries_declare_is_not_carried() {
        let (rows, author, _) = gathered_rows(1);
        let stranger =
            Entity::new("system/content", entity_ecf::to_ecf(&entity_ecf::text("unnamed")))
                .unwrap();
        let plan = plan_mirror(
            "2Gatherer",
            &thread(&author, rows[0].hash),
            &rows,
            std::slice::from_ref(&stranger),
            9,
        )
        .expect("an all-inline gather plans");
        assert!(
            plan.content.is_empty(),
            "a blob no entry declares was published as part of this mirror"
        );
    }

    /// A gather refuses bytes that do not address to the name it is about to bind
    /// them under — locally, rather than trusting the reader that produced them.
    #[test]
    fn a_row_whose_bytes_do_not_address_is_refused_at_the_gatherer() {
        let (mut rows, author, _) = gathered_rows(2);
        let real = rows[0].hash;
        rows[0].obtained.entity = rows[1].obtained.entity.clone();

        match plan_mirror("2Gatherer", &thread(&author, real), &rows, &[], 1) {
            Err(GatherError::RowDoesNotAddress { claimed, actual }) => {
                assert_eq!(claimed, real);
                assert_ne!(actual, real);
            }
            other => panic!("a mis-addressed row was planned for republication: {other:?}"),
        }
    }

    /// §6.1 rule 3's other half: an entry with **no** signature is still carried,
    /// and the consumer presents it unattributed. Dropping it would be a mirror
    /// lying by omission about something it holds; minting one is the thing a
    /// mirror structurally cannot do.
    #[test]
    fn an_unsigned_entry_is_carried_and_comes_back_unattributed() {
        let (mut rows, author, _) = gathered_rows(2);
        rows[0].obtained.signature = None;

        let subject = thread(&author, rows[0].hash);
        let gatherer = "2GathererPeerIdForTheseGates";
        let plan = plan_mirror(gatherer, &subject, &rows, &[], 9).expect("the gather plans");
        assert_eq!(plan.entry_count(), 2, "the unsigned entry was dropped");
        assert_eq!(plan.attributable(), 1);

        let mut origin = Origin::default();
        origin.take(&plan, gatherer);
        let read = block_on(read_mirror(&origin, gatherer, &subject, 100)).unwrap();

        assert_eq!(read.len(), 2);
        assert_eq!(
            read[0].attribution,
            Attribution::Unattributed(crate::feed_read::Unattributed::NoSignature)
        );
        assert_eq!(read[1].attribution, Attribution::Signed);
    }

    /// ⛔ **A mirror may omit; it may never substitute** — and the two are
    /// different outcomes, because only one of them is a defect.
    #[test]
    fn a_mirror_that_substitutes_a_body_is_refused_and_one_that_omits_is_short() {
        let (rows, author, _) = gathered_rows(3);
        let gatherer = "2GathererPeerIdForTheseGates";
        let subject = thread(&author, rows[0].hash);
        let plan = plan_mirror(gatherer, &subject, &rows, &[], 9).unwrap();

        // Omission: the record still names three, the origin serves two.
        let mut short = Origin::default();
        short.take(&plan, gatherer);
        short.0.remove(&(rows[1].entry.author.clone(), entry_key(&rows[1].hash)));
        let read = block_on(read_mirror(&short, gatherer, &subject, 100))
            .expect("a short mirror is readable");
        assert_eq!(read.len(), 2, "an omitted entry made the whole mirror unreadable");

        // Substitution: entry 1's address, entry 2's bytes.
        let mut swapped = Origin::default();
        swapped.take(&plan, gatherer);
        let other = swapped
            .0
            .get(&(rows[1].entry.author.clone(), entry_key(&rows[1].hash)))
            .unwrap()
            .clone();
        swapped.0.insert((rows[0].entry.author.clone(), entry_key(&rows[0].hash)), other);
        assert!(
            matches!(
                block_on(read_mirror(&swapped, gatherer, &subject, 100)),
                Err(MirrorReadError::Substituted { .. })
            ),
            "a substituted body was accepted"
        );
    }

    /// A record claiming somebody else assembled it is refused — the structure
    /// already says who published it, and a body disagreeing with the structure
    /// is a second source of one fact.
    #[test]
    fn a_mirror_record_cannot_name_a_different_gatherer() {
        let (rows, author, _) = gathered_rows(1);
        let plan = plan_mirror("2SomeoneElse", &thread(&author, rows[0].hash), &rows, &[], 9)
            .unwrap();
        let mut origin = Origin::default();
        origin.take(&plan, "2TheActualGatherer");

        assert!(matches!(
            block_on(read_mirror(&origin, "2TheActualGatherer", &thread(&author, rows[0].hash), 100)),
            Err(MirrorReadError::Malformed {
                source: FeedError::AuthorIsNotTheNamespace { .. },
                ..
            })
        ));
    }

    /// *This gatherer has not gathered that* and *we could not reach them* are
    /// different reports, and only one of them is about the gatherer's choices.
    #[test]
    fn an_absent_mirror_is_not_the_same_report_as_an_unreachable_one() {
        let (rows, author, _) = gathered_rows(1);
        let empty = Origin::default();
        assert!(matches!(
            block_on(read_mirror(&empty, "2Gatherer", &thread(&author, rows[0].hash), 10)),
            Err(MirrorReadError::NoMirror { .. })
        ));

        struct Dead;
        impl MirrorSource for Dead {
            fn get(
                &self,
                _peer: String,
                _key: String,
            ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
                Box::pin(std::future::ready(Err("504".to_string())))
            }
        }
        assert!(matches!(
            block_on(read_mirror(&Dead, "2Gatherer", &thread(&author, rows[0].hash), 10)),
            Err(MirrorReadError::Unreachable { .. })
        ));
        let _ = author;
    }

    /// ⭐ **The fixed point: a gatherer can gather a gatherer.**
    ///
    /// C reads B's mirror and republishes it as its own. D reads C and still gets
    /// A's entries, attributed to A. This is what *"an aggregator can aggregate
    /// aggregators"* means when it is run rather than asserted — and note which
    /// type recurses: `mirror → mirror`, not `feed → feed`.
    #[test]
    fn a_gatherer_can_gather_a_gatherer_and_authorship_survives_both_hops() {
        let (rows, author, _) = gathered_rows(3);
        let b = "2TheFirstGatherer";
        let c = "2TheSecondGatherer";

        let plan_b =
            plan_mirror(b, &thread(&author, rows[0].hash), &rows, &[], 1).unwrap();
        let mut origin = Origin::default();
        origin.take(&plan_b, b);

        let via_b = block_on(read_mirror(&origin, b, &thread(&author, rows[0].hash), 100)).unwrap();
        let plan_c =
            plan_mirror(c, &thread(&author, rows[0].hash), &via_b, &[], 2).unwrap();
        origin.take(&plan_c, c);

        let via_c = block_on(read_mirror(&origin, c, &thread(&author, rows[0].hash), 100)).unwrap();
        assert_eq!(via_c.len(), 3);
        for (first, third) in rows.iter().zip(via_c.iter()) {
            assert_eq!(third.hash, first.hash, "a hash moved on the second hop");
            assert_eq!(third.attribution, Attribution::Signed);
            assert_eq!(third.entry.author, author);
        }
    }

    /// ⛔⭐ **`FEED-R26` where it bites: a timeline mirror is still FOUND after
    /// the author posts again.**
    ///
    /// The tempting subject for a timeline is the author's index head, and §6.0
    /// spends a paragraph forbidding it. This is the reason, run rather than
    /// argued: the head's hash changes on every publish, so a head-pinned mirror
    /// moves to a new address each time the author writes — the reader who
    /// computed the address before the post finds **nothing**, and the gatherer
    /// republishing gets a second slot instead of updating its own. *A witness
    /// masquerading as an identity.*
    ///
    /// Both halves are here on purpose. The positive says the live-subject address
    /// survives; the counter-example performs the forbidden derivation over the
    /// **same two real heads** and shows the read failing. Without the second
    /// half the first is satisfied by any address that does not depend on the
    /// author's state at all, including a constant.
    #[test]
    fn a_timeline_mirror_is_still_found_after_the_author_posts_again() {
        let (rows, author, _) = gathered_rows(3);
        // ⚠ **3 → 15, and the jump is the fixture's constraint rather than a
        // taste.** `published_tree` publishes at a FIXED clock, so the head's
        // `updated_at` does not move and `current` is the only field left that
        // can — which needs a page boundary crossed (page size 10). Written with
        // 3 → 4 first, and it failed on its own anti-vacuity assertion: the two
        // heads were byte-identical. *A rig that holds a variable still cannot
        // measure a property that is about it.*
        let head_before = published_head(3);
        let gatherer = "2GathererPeerIdForTheseGates";

        let subject = MirrorSubject::timeline(&author);
        let plan = plan_mirror(gatherer, &subject, &rows, &[], 1).expect("the gather plans");
        let mut origin = Origin::default();
        origin.take(&plan, gatherer);

        // The author posts. Their head is a different entity now.
        let head_after = published_head(15);
        assert_ne!(
            head_before, head_after,
            "the fixture's head did not move, so this gate measures nothing"
        );

        // A reader deriving the address from the author's FEED still finds it —
        // and note it is recomputed from scratch, not the value from before.
        let recomputed = MirrorSubject::timeline(&author);
        let read = block_on(read_mirror(&origin, gatherer, &recomputed, 100))
            .expect("the mirror moved when the author posted");
        assert_eq!(read.len(), 3);

        // …and the derivation §6.0 forbids, over those same two heads.
        let pinned_before = MirrorSubject::thread(&author, head_before);
        let pinned_after = MirrorSubject::thread(&author, head_after);
        assert_ne!(pinned_before.key(), pinned_after.key());
        let mut head_pinned = Origin::default();
        let head_plan = plan_mirror(gatherer, &pinned_before, &rows, &[], 1).unwrap();
        head_pinned.take(&head_plan, gatherer);
        assert!(
            matches!(
                block_on(read_mirror(&head_pinned, gatherer, &pinned_after, 100)),
                Err(MirrorReadError::NoMirror { .. })
            ),
            "pinning the head gave a stable address, which would make R26 about nothing"
        );
    }

    /// The author's own index head, at a given number of posts — the value §6.0
    /// forbids as a subject, which the gate above needs two of.
    fn published_head(posts: usize) -> Hash {
        let (tree, _, _) = crate::feed_publish::tests::published_tree(posts);
        tree.0
            .get(crate::feed::index_head_key())
            .expect("a published feed has a head")
            .content_hash
    }

    /// ⛔ **`DX-C6` — the gatherer publishes no SET-LAYER object of the
    /// author's.**
    ///
    /// This exists because the closure `MUST` used to be one flat sentence —
    /// *republished entries are consumable by the identical code path* — which is
    /// true at the entry layer and **false at the set layer**, and a reader taking
    /// it literally publishes an `app/feed/index` under the **gatherer's** name
    /// over entries the author never put there. That is a claim only the author
    /// can make, and it is unauthenticated besides: the authorship instrument
    /// signs entries, not sets. *Every byte of the forgery verifies.*
    ///
    /// So the check is over **what a plan binds**, not over what it renders.
    /// Three properties, and the third is the one a tidy implementation loses:
    ///
    /// 1. under a **foreign** peer, only content-addressed entries and their
    ///    signatures — nothing whose key is an author's own index;
    /// 2. under the **gatherer**, exactly one binding, the mirror record;
    /// 3. that record is at §6.0.1's derived key, so it presents as *a mirror*
    ///    and cannot be mistaken for the author's own entry point.
    #[test]
    fn a_gatherer_publishes_no_set_layer_object_of_the_authors() {
        let (rows, author, _) = gathered_rows(3);
        let gatherer = "2GathererPeerIdForTheseGates";
        let subject = thread(&author, rows[0].hash);
        let plan = plan_mirror(gatherer, &subject, &rows, &[], 9).unwrap();

        // 1 — the foreign half.
        assert!(!plan.carried.is_empty(), "a plan carrying nothing proves nothing here");
        for item in &plan.carried {
            assert_eq!(item.peer, author, "a carried body left the author's namespace");
            let is_entry = item.key.starts_with(crate::feed::entry_prefix());
            let is_signature = item.key.starts_with("system/signature/");
            assert!(
                is_entry || is_signature,
                "a mirror bound {} under {author} — DX-R4 admits entries and their \
                 signatures and nothing else",
                item.key
            );
            // Named explicitly, because these are the two the flat sentence
            // invites and neither is caught by the shape test above.
            assert_ne!(item.key, crate::feed::index_head_key(), "the author's index head, forged");
            assert!(
                !item.key.starts_with(&format!("{}/", crate::feed::index_head_key())),
                "an index PAGE of the author's, forged: {}",
                item.key
            );
        }

        // 2 and 3 — the gatherer's own half is one record, and it is a mirror.
        let mut origin = Origin::default();
        origin.take(&plan, gatherer);
        let ours: Vec<&String> =
            origin.0.keys().filter(|(p, _)| p == gatherer).map(|(_, k)| k).collect();
        assert_eq!(
            ours.len(),
            1,
            "the gatherer bound more than its own record under its own name: {ours:?}"
        );
        assert_eq!(ours[0], &subject.key());
        assert!(ours[0].starts_with(crate::feed::mirror_prefix()));
    }
}
