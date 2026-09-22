//! The **subgraph reader** — read a peer's own feed off the live tree, so a
//! publish projects real state rather than a hand-built fixture.
//!
//! This is `APP-CONVENTION-FEED`'s answer to
//! [`content_site::read`](crate::content_site::read), and it is deliberately
//! the same shape: one read, N projections. It exists because *what enters a
//! publish's projection* was a hardcoded enumeration of two L5 conventions
//! (`emit_owned_sites` + `for set in app_sets`) rather than a policy over the
//! tree, and a third axis is a reader beside the other two — **not a new
//! verb.** `feed_publish`'s
//! `a_second_axis_signed_by_its_own_projector_un_names_the_first` is the
//! measurement that makes that binding: two projectors over one directory do
//! not compose.
//!
//! ## Why this is not in [`crate::feed_read`]
//!
//! That module is the **foreign** consumer: an `async` walk of somebody else's
//! signed origin, over a `FeedSource` whose only real implementations are HTTP.
//! This one reads the **bound peer's own** tree, synchronously, through the
//! arm-aware [`Peers`] accessors — the same L0 read `content_site::read` makes,
//! with the same Worker-arm caveat (the cache mirror is fed only for subscribed
//! prefixes, so a publish of a not-yet-observed subtree must subscribe first).
//! Two directions, two arm rules, two modules.
//!
//! ## A peer has ONE feed, so this is `read_owned_feed` and not `read_all_feeds`
//!
//! The plan called for a `read_all_feeds` beside `read_all_sites`, by analogy.
//! The analogy does not hold: a peer may own **any number of sites**, each with
//! its own id under `sites/{site_id}/`, but §4.2 pins the index at exactly one
//! path — `/{peer}/app/feed/index` — so a peer has one feed the way it has one
//! roster. The sweep is over entries, not over feeds.
//!
//! ## The ENTRIES are the authored fact; the index is derived at publish time
//!
//! §4's index is an index: `current`, `oldest`, and pages of pins. Every byte of
//! it is recomputable from the entry set plus a page size, which is exactly what
//! [`crate::feed_publish::plan_index`] does — so it is a **projection
//! artifact**, like `sites/index.html`, and the tree carries entries only. Two
//! sources for one fact is the drift C15 exists to stop, and the one that can go
//! stale is the derived one.
//!
//! **What that costs, stated rather than discovered later: a BACKDATED post
//! rewrites the archive from its insertion point.** Pages are assigned by
//! position in the authored order, so an entry inserted in the middle shifts
//! every entry after it — §4.3 rule 3's renumbering hazard, arriving through the
//! ordinary act of posting an old thing. Appending (the overwhelmingly common
//! case) touches only the last page, which is
//! [`crate::feed_publish`]'s §2 property and is gated there. The alternative is
//! to persist each entry's page assignment, which is a durability model this
//! seat does not have and which `C-2` measured a second device cannot recover.
//!
//! ## An author who has posted nothing does not get an empty feed published
//!
//! [`read_owned_feed`] answers `None` when the entries prefix is empty, and the
//! publish axis then projects nothing at all. That is a decision, not an
//! omission: a reader distinguishes *"this author has no feed"*
//! ([`FeedReadError::NoIndex`](crate::feed_read::FeedReadError::NoIndex)) from
//! *"this author has posted nothing"* (a head with an empty page 0), and
//! emitting the second for **every** site publisher who has never used the
//! convention would be making a claim on their behalf. `publish_feed` still
//! emits the empty head when it is handed an empty entry set — the trigger is
//! what changed, not the capability.

#![allow(dead_code)] // the WASM arm has no publisher; the native axis is the consumer

use std::collections::BTreeSet;

use entity_entity::Entity;
use entity_hash::Hash;

use crate::embed::{EmbedNode, EmbedPayload};
use crate::feed::{entry_prefix, FeedEntry, FEED_ENTRY_TYPE};
use crate::peers::Peers;

/// A whole feed read off the tree — the third publish axis's payload.
///
/// Owned rather than borrowed for [`crate::content_site::read::OwnedSite`]'s
/// reason: the data comes from decoding entities, not from `'static` literals.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedFeed {
    pub peer_id: String,
    /// **Oldest-first** — the order [`crate::feed_publish::publish_feed`] takes,
    /// and the order §4.3 rule 1's key-addressing property depends on. See
    /// [`sort_key`] for what makes it total.
    pub entries: Vec<FeedEntry>,
    /// The `system/content` blob + chunk entities every **pointer** body in
    /// [`Self::entries`] resolves through, deduped by hash.
    ///
    /// **This is what makes an `OwnedFeed` self-contained**, and it is the same
    /// obligation `OwnedSite::content` carries for an oversized figure: EMBED
    /// §3's pointer arm means an entry's body may hold no bytes of its own, and
    /// a projection that wrote the entry without these publishes a post nobody
    /// can read. Carried on the value so no caller has to remember (AP44).
    pub content: Vec<Entity>,
}

/// Read `peer_id`'s feed off their own tree. `None` when they have authored no
/// entries — see the module doc for why that is not an empty feed.
///
/// Entries are decoded **type-guarded** (AP42: a prefix scan can surface
/// anything, and a decoder that goes by field name adopts it) and
/// **namespace-checked** — `FeedEntry::from_entity` takes the peer id and
/// refuses an entry claiming a different author, which is `FEED-R1` applied at
/// the one boundary where the namespace is a fact rather than a claim.
pub fn read_owned_feed(peers: &Peers, peer_id: &str) -> Option<OwnedFeed> {
    let prefix = format!("/{peer_id}/{}", entry_prefix());
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for entry in peers.tree_listing(peer_id, &prefix) {
        if entry.path.strip_prefix(&prefix).is_some_and(|rest| !rest.is_empty()) {
            keys.insert(entry.path.clone());
        }
    }

    let mut decoded: Vec<(Hash, FeedEntry)> = Vec::with_capacity(keys.len());
    for path in &keys {
        let Some(e) = peers.get_entity(peer_id, path) else { continue };
        if e.entity_type != FEED_ENTRY_TYPE {
            continue;
        }
        match FeedEntry::from_entity(&e, peer_id) {
            Ok(entry) => decoded.push((e.content_hash, entry)),
            // One line per bad entry rather than a refusal: an entry we cannot
            // decode is one post, and failing the whole publish over it would
            // make one malformed row cost an author their entire feed.
            Err(why) => tracing::warn!(
                path = %path,
                ?why,
                "a feed entry in this peer's own tree did not decode — it will not be published"
            ),
        }
    }
    if decoded.is_empty() {
        return None;
    }

    decoded.sort_by_key(sort_key);

    // The pointer bodies' blob closures. A pointer we cannot resolve is
    // **skipped, not fatal**, for `OwnedSite`'s reason and D24's: refusing the
    // whole feed because one attachment's bytes are not held locally turns a
    // gap into a missing feed, and the entry is still worth publishing — a
    // consumer on another origin may hold the blob.
    let mut content: Vec<Entity> = Vec::new();
    let mut seen: BTreeSet<Hash> = BTreeSet::new();
    for (entry_hash, entry) in &decoded {
        for blob in body_blob_hashes(&entry.body) {
            match crate::content_site::asset_store::blob_closure_via(&blob, |h| {
                peers.content_by_hash(peer_id, h)
            }) {
                Ok(entities) => {
                    for e in entities {
                        if seen.insert(e.content_hash) {
                            content.push(e);
                        }
                    }
                }
                Err(why) => tracing::warn!(
                    entry = %entry_hash.to_hex(),
                    blob = %blob.to_hex(),
                    %why,
                    "a feed entry's body pointer could not be resolved from the local \
                     content store — the entry publishes without its bytes"
                ),
            }
        }
    }

    Some(OwnedFeed {
        peer_id: peer_id.to_string(),
        entries: decoded.into_iter().map(|(_, e)| e).collect(),
        content,
    })
}

/// The total order entries are published in.
///
/// `created_at` first, because that is the authored order §4.3's page
/// assignment is about — and the **content hash as tie-break, because the clock
/// alone is not a total order**. §2.3.2 makes `created_at` *"a display
/// heuristic, never an ordering authority"*, and two posts sharing a millisecond
/// is not exotic (a bulk ingest of a dated archive gives every post the same
/// midnight). Left to the map's iteration order those two would swap between
/// publishes on nothing, moving a page boundary and rewriting the archive —
/// §4.3 rule 1's property defeated by a coin flip. A hash tie-break is stable,
/// arbitrary, and reproducible on a second device, which is the one that
/// matters (`C-2`).
///
/// ⚠ **Today it changes no answer, and saying so is the point.** The prefix
/// scan above collects into a `BTreeSet` of hex-keyed paths, so the sort's input
/// is already in hash order and `sort_by` is stable — the tie-break is redundant
/// *given those two choices*, and a test through [`read_owned_feed`] cannot
/// falsify it. What it buys is that the total order is a property of **this
/// comparator** rather than an emergent consequence of a collection type and a
/// sort's stability guarantee, either of which a later edit can change without
/// anything going red. Gated at the level where that is real:
/// [`tests::two_entries_sharing_a_millisecond_still_have_one_stable_order`].
pub(crate) fn sort_key(row: &(Hash, FeedEntry)) -> (u64, Hash) {
    (row.1.created_at, row.0)
}

/// Every content-store blob an entry body depends on: the payload's, when it
/// took EMBED §3's pointer arm, plus each rendition's.
///
/// **One expression covering both**, rather than a payload arm and a rendition
/// arm — they are the same fact (a hash naming bytes that must travel with the
/// entry) and a second arm is a second place to forget the closure. A `child`
/// payload names an entity rather than a blob and is not one of these; nothing
/// in this crate mints one, and the day something does it owes its own row here.
pub(crate) fn body_blob_hashes(body: &EmbedNode) -> Vec<Hash> {
    let mut out = Vec::new();
    if let EmbedPayload::Pointer(h) = &body.data.payload {
        out.push(*h);
    }
    for r in &body.data.renditions {
        out.push(r.pointer);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{EmbedData, EmbedNode};
    use crate::feed::entry_key;
    use std::sync::Arc;

    const NOW: u64 = 1_757_000_000_000;

    fn body(s: &str) -> EmbedNode {
        EmbedNode::new("text/plain", EmbedData::new(EmbedPayload::Inline(s.as_bytes().to_vec()), s))
    }

    /// Write one entry into `peer_id`'s tree at the key the publisher binds it
    /// at, and hand back its hash.
    fn seed_entry(peers: &Peers, peer_id: &str, entry: &FeedEntry) -> Hash {
        let e = entry.to_entity().unwrap();
        let hash = e.content_hash;
        peers.seed_write(peer_id, format!("/{peer_id}/{}", entry_key(&hash)), e);
        hash
    }

    #[test]
    fn an_author_who_has_posted_nothing_has_no_feed_rather_than_an_empty_one() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        assert!(
            read_owned_feed(&peers, &pid).is_none(),
            "a peer with no entries contributes no feed axis — publishing an empty head \
             on their behalf would claim they have a feed"
        );
    }

    #[test]
    fn entries_come_back_oldest_first_whatever_order_they_were_written_in() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();

        // Written newest-first on purpose: the key is the content hash, so the
        // prefix scan's order has nothing to do with the authored order.
        for i in (0..5u64).rev() {
            seed_entry(&peers, &pid, &FeedEntry::new(&pid, NOW + i * 1000, body(&format!("post {i}"))));
        }

        let feed = read_owned_feed(&peers, &pid).expect("five entries make a feed");
        let stamps: Vec<u64> = feed.entries.iter().map(|e| e.created_at).collect();
        assert_eq!(
            stamps,
            vec![NOW, NOW + 1000, NOW + 2000, NOW + 3000, NOW + 4000],
            "oldest-first is what §4.3's page assignment is about"
        );
        let bodies: Vec<&str> = feed.entries.iter().map(|e| e.body.data.fallback.as_str()).collect();
        assert_eq!(bodies, vec!["post 0", "post 1", "post 2", "post 3", "post 4"]);
    }

    /// **The clock is not a total order, and a publish that is not a total order
    /// rewrites the archive on a coin flip.** Two posts in one millisecond is
    /// what a bulk ingest of a dated archive produces.
    ///
    /// ⚠ **Asserted against the COMPARATOR, not against
    /// [`read_owned_feed`] — because through the reader this property is
    /// currently unfalsifiable.** The prefix scan collects into a
    /// `BTreeSet<String>` of hex-keyed paths, so the input to the sort is
    /// already in hash order, and `sort_by` is stable — delete the tie-break
    /// and the reader returns the identical answer. A test at that level would
    /// pass with the thing it names removed, which is worse than no test.
    ///
    /// So the tie-break is defence against a change to the *collection* (a
    /// `HashSet`, an unstable sort, a listing that stops being sorted), and the
    /// level where that is real is the comparator: two different input orders
    /// must produce one output. Falsified — drop `.then(hash)` and this reds
    /// while every other test in the module stays green.
    #[test]
    fn two_entries_sharing_a_millisecond_still_have_one_stable_order() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let rows: Vec<(Hash, FeedEntry)> = ["alpha", "beta", "gamma"]
            .iter()
            .map(|tag| {
                let e = FeedEntry::new(&pid, NOW, body(tag));
                (e.to_entity().unwrap().content_hash, e)
            })
            .collect();

        let sorted = |mut v: Vec<(Hash, FeedEntry)>| {
            v.sort_by_key(sort_key);
            v.into_iter().map(|(_, e)| e.body.data.fallback).collect::<Vec<_>>()
        };
        let forwards = sorted(rows.clone());
        let backwards = sorted(rows.into_iter().rev().collect());
        assert_eq!(
            forwards, backwards,
            "one authored instant, two input orders, one published order"
        );
        assert_eq!(forwards.len(), 3, "the control: nothing was dropped");
    }

    /// AP42's guard, in the one place a feed can meet it: the entries prefix is
    /// ours, and a prefix scan surfaces whatever is under it. An entity of
    /// another type parked there must not be decoded by field name.
    #[test]
    fn an_entity_of_another_type_under_the_entries_prefix_is_not_adopted() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let good = FeedEntry::new(&pid, NOW, body("mine"));
        seed_entry(&peers, &pid, &good);

        // Same shape, different type tag — the discriminator that already
        // exists and that a name-based decoder throws away.
        let mut impostor = FeedEntry::new(&pid, NOW + 1, body("not an entry")).to_entity().unwrap();
        impostor.entity_type = "app/share/record".into();
        peers.seed_write(&pid, format!("/{pid}/{}entities-are-not-entries", entry_prefix()), impostor);

        let feed = read_owned_feed(&peers, &pid).expect("the real entry still reads");
        assert_eq!(feed.entries.len(), 1, "only the app/feed/entry was taken");
        assert_eq!(feed.entries[0].body.data.fallback, "mine");
    }

    /// `FEED-R1` at the one boundary where the namespace is a fact: an entry
    /// claiming somebody else's authorship, sitting in our tree, is not ours to
    /// republish under our own signed root.
    #[test]
    fn an_entry_claiming_another_author_is_not_published_from_our_tree() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        seed_entry(&peers, &pid, &FeedEntry::new(&pid, NOW, body("mine")));

        let theirs = FeedEntry::new("QmSomebodyElse", NOW + 1, body("theirs")).to_entity().unwrap();
        let hash = theirs.content_hash;
        peers.seed_write(&pid, format!("/{pid}/{}", entry_key(&hash)), theirs);

        let feed = read_owned_feed(&peers, &pid).expect("our own entry still reads");
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(feed.entries[0].author, pid);
    }

    /// **The closure travels with the feed.** An entry whose body took EMBED
    /// §3's pointer arm holds no bytes of its own, so the projection needs the
    /// blob and its chunks — the same obligation an oversized figure puts on
    /// `OwnedSite::content`, and the same failure if it is missed: a post that
    /// publishes and renders empty.
    #[test]
    fn a_pointer_bodys_bytes_come_back_with_the_feed() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();

        // Over EMBED §3's inline range, so `stage` takes the pointer arm.
        let big = vec![b'x'; crate::embed::INLINE_PAYLOAD_MAX + 1];
        let scratch: Arc<dyn entity_store::ContentStore> =
            Arc::new(entity_store::MemoryContentStore::new());
        let staged = crate::content_site::asset_store::stage("text/plain", big, &scratch).unwrap();
        let pointer = staged.asset.pointer().expect("an oversized body is a pointer");
        for e in &staged.content {
            peers.seed_content(&pid, e.clone());
        }

        let node = EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Pointer(pointer), "a long post"),
        );
        seed_entry(&peers, &pid, &FeedEntry::new(&pid, NOW, node));

        let feed = read_owned_feed(&peers, &pid).expect("the feed reads");
        assert_eq!(feed.entries.len(), 1);
        assert!(
            feed.content.iter().any(|e| e.content_hash == pointer),
            "the blob the body points at came back with the feed: {:?}",
            feed.content.iter().map(|e| e.content_hash.to_hex()).collect::<Vec<_>>()
        );
        assert!(feed.content.len() >= 2, "the blob and at least one chunk");
    }

    /// A pointer whose bytes are not held is a gap in one post, never a reason
    /// to publish no feed — D24's direction, applied to an author's own tree.
    #[test]
    fn a_body_pointer_we_do_not_hold_costs_one_posts_bytes_and_not_the_feed() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let dangling = Hash::compute("system/content/blob", b"nobody has these bytes");
        let node = EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Pointer(dangling), "a post we cannot fully carry"),
        );
        seed_entry(&peers, &pid, &FeedEntry::new(&pid, NOW, node));
        seed_entry(&peers, &pid, &FeedEntry::new(&pid, NOW + 1, body("an ordinary post")));

        let feed = read_owned_feed(&peers, &pid).expect("the feed still reads");
        assert_eq!(feed.entries.len(), 2, "both posts publish");
        assert!(feed.content.is_empty(), "and the bytes we do not hold are simply absent");
    }
}
