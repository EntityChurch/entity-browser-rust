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
//! ## ⚠ Two findings from building it, stated here and routed
//!
//! **1. §6's mirror is a THREAD mirror, and the closure trace is a TIMELINE.**
//! §2.2 defines `reference` as pinned-only, so `subject: reference` pins one
//! entity — *"the root entry this view is of"*, which §6.2's rationale is written
//! about. The replication proposal's own §15.1 trace is a gatherer following
//! three *authors* and republishing their timelines, and a timeline is a growing
//! prefix that cannot be pinned. Both readings are built here (a subject is just
//! a reference) and only the thread one has a derivable address; see
//! [`crate::feed::FeedMirror`].
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

#![allow(dead_code)] // no verb publishes a mirror yet; the gates are native

use std::future::Future;
use std::pin::Pin;

use entity_entity::Entity;
use entity_hash::Hash;

use crate::entity_ref::EntityRef;
use crate::feed::{entry_key, mirror_key, FeedError, FeedMirror};
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

/// Everything one mirror publish emits: the gatherer's signed record, and the
/// foreign bytes it carries.
#[derive(Debug, Clone, PartialEq)]
pub struct MirrorPlan {
    pub record: FeedMirror,
    pub carried: Vec<Carried>,
}

impl MirrorPlan {
    /// How many entries this mirror holds.
    pub fn entry_count(&self) -> usize {
        self.record.entries.len()
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
    subject: EntityRef,
    rows: &[ReadEntry],
    gathered_at: u64,
) -> Result<MirrorPlan, GatherError> {
    let mut entries = Vec::with_capacity(rows.len());
    let mut carried = Vec::with_capacity(rows.len() * 2);

    for row in rows {
        let actual = recomputed_hash(&row.obtained.entity)
            .map_err(|_| GatherError::RowUnencodable { claimed: row.hash })?;
        if actual != row.hash {
            return Err(GatherError::RowDoesNotAddress { claimed: row.hash, actual });
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

    Ok(MirrorPlan {
        record: FeedMirror::new(subject, entries, gathered_at, gathered_by),
        carried,
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

    let subject_hash = match &plan.record.subject {
        EntityRef::Pinned { hash, .. } => *hash,
        // Unreachable through `FeedMirror::from_entity`, which refuses a live
        // subject — but `plan_mirror` takes a reference by argument, and a key
        // we cannot derive is a mirror nobody can find.
        EntityRef::Live { .. } => {
            return Err("a mirror subject must be a pin — §2.2's `reference` is pinned-only, \
                        and the key is derived from the subject's hash"
                .to_string())
        }
    };
    let entity = plan.record.to_entity()?;
    write(dir, &gatherer, &mirror_key(&subject_hash), &entity, root)
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
    subject: &Hash,
    limit: usize,
) -> Result<Vec<ReadEntry>, MirrorReadError> {
    let key = mirror_key(subject);
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
            let subject = match &plan.record.subject {
                EntityRef::Pinned { hash, .. } => *hash,
                _ => panic!("a pinned subject"),
            };
            self.0.insert(
                (gatherer.to_string(), mirror_key(&subject)),
                plan.record.to_entity().unwrap(),
            );
        }
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
        let subject = EntityRef::pin(author.clone(), rows[0].hash);
        let plan = plan_mirror(gatherer, subject, &rows, 1_757_000_999).expect("the gather plans");

        let mut origin = Origin::default();
        origin.take(&plan, gatherer);

        let read = block_on(read_mirror(&origin, gatherer, &rows[0].hash, 100))
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
            plan_mirror(gatherer, EntityRef::pin(author.clone(), published_hash), &[row], 9)
                .expect("the gather plans");
        let carried = &plan.carried[0].entity;
        assert_eq!(
            carried.data, published.data,
            "the gatherer re-encoded the entry it was carrying"
        );
        assert_eq!(carried.content_hash, published_hash);

        let mut origin = Origin::default();
        origin.take(&plan, gatherer);
        let read = block_on(read_mirror(&origin, gatherer, &published_hash, 10)).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(
            read[0].attribution,
            Attribution::Signed,
            "an entry carrying a field we do not understand lost its author"
        );
    }

    /// A gather refuses bytes that do not address to the name it is about to bind
    /// them under — locally, rather than trusting the reader that produced them.
    #[test]
    fn a_row_whose_bytes_do_not_address_is_refused_at_the_gatherer() {
        let (mut rows, author, _) = gathered_rows(2);
        let real = rows[0].hash;
        rows[0].obtained.entity = rows[1].obtained.entity.clone();

        match plan_mirror("2Gatherer", EntityRef::pin(author, real), &rows, 1) {
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

        let subject = EntityRef::pin(author.clone(), rows[0].hash);
        let gatherer = "2GathererPeerIdForTheseGates";
        let plan = plan_mirror(gatherer, subject, &rows, 9).expect("the gather plans");
        assert_eq!(plan.entry_count(), 2, "the unsigned entry was dropped");
        assert_eq!(plan.attributable(), 1);

        let mut origin = Origin::default();
        origin.take(&plan, gatherer);
        let read = block_on(read_mirror(&origin, gatherer, &rows[0].hash, 100)).unwrap();

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
        let subject = EntityRef::pin(author, rows[0].hash);
        let plan = plan_mirror(gatherer, subject, &rows, 9).unwrap();

        // Omission: the record still names three, the origin serves two.
        let mut short = Origin::default();
        short.take(&plan, gatherer);
        short.0.remove(&(rows[1].entry.author.clone(), entry_key(&rows[1].hash)));
        let read = block_on(read_mirror(&short, gatherer, &rows[0].hash, 100))
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
                block_on(read_mirror(&swapped, gatherer, &rows[0].hash, 100)),
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
        let plan = plan_mirror("2SomeoneElse", EntityRef::pin(author, rows[0].hash), &rows, 9)
            .unwrap();
        let mut origin = Origin::default();
        origin.take(&plan, "2TheActualGatherer");

        assert!(matches!(
            block_on(read_mirror(&origin, "2TheActualGatherer", &rows[0].hash, 100)),
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
            block_on(read_mirror(&empty, "2Gatherer", &rows[0].hash, 10)),
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
            block_on(read_mirror(&Dead, "2Gatherer", &rows[0].hash, 10)),
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
            plan_mirror(b, EntityRef::pin(author.clone(), rows[0].hash), &rows, 1).unwrap();
        let mut origin = Origin::default();
        origin.take(&plan_b, b);

        let via_b = block_on(read_mirror(&origin, b, &rows[0].hash, 100)).unwrap();
        let plan_c =
            plan_mirror(c, EntityRef::pin(author.clone(), rows[0].hash), &via_b, 2).unwrap();
        origin.take(&plan_c, c);

        let via_c = block_on(read_mirror(&origin, c, &rows[0].hash, 100)).unwrap();
        assert_eq!(via_c.len(), 3);
        for (first, third) in rows.iter().zip(via_c.iter()) {
            assert_eq!(third.hash, first.hash, "a hash moved on the second hop");
            assert_eq!(third.attribution, Attribution::Signed);
            assert_eq!(third.entry.author, author);
        }
    }
}
