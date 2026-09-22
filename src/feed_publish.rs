//! `APP-CONVENTION-FEED` §1.1 and §4 — **publishing a feed.** Phase 2b.
//!
//! [`crate::feed`] is the codec and [`crate::feed_read`] is the consumer. This
//! is the emitter: entries and their detached signatures and the index,
//! projected through the same [`RootProjector`] a site publish uses.
//!
//! **The reader half used to live here** and moved out once it had somewhere to
//! go. Keeping it beside the publisher was right while it had no other consumer
//! — a reader with no publisher to test it against is the shape this module
//! exists to argue against — and it stopped being right the moment a window
//! needed it, because this module is `cfg(not(wasm32))` and that gate was
//! **hiding a synchronous [`FeedSource`](crate::feed_read::FeedSource) in a
//! codebase where every real source of a foreign entity is `async`.* See
//! [`crate::feed_read`]'s module doc: the seam built to make the reader testable
//! had made it unshippable, and only a module nothing could import kept it out
//! of sight.
//!
//! ## Why this is the slice rather than `FEED-R2` alone
//!
//! The handoff scoped 2b as *"mint the signature entity"*, which is two dozen
//! lines and would have left **three consecutive phases of codec with no
//! consumer**. That is the population problem this repo has been bitten by
//! twice — *green because the failing configuration was not in the test
//! population, not green by inheritance* — and a format module's round-trip test
//! cannot see anything that lives between the modules. Everything in §6 below
//! was found by closing the loop and none of it by reading.
//!
//! **Nothing here is user-facing and nothing is on the wire.** No CLI verb
//! publishes a feed; the gate is a native test that publishes into a temp dir
//! and reads it back. That is deliberate — the point is to exercise the design,
//! not to ship a surface.
//!
//! ## 1. Where an entry lives, and why the convention does not say
//!
//! §2: ***"The cross-impl contract is the type tag, not the path."*** Aggregation
//! is a `type_filter` query over the universal tree, so the tag is the index key
//! and the tree path is a local choice. We bind an entry at
//! [`entry_key`] — `app/feed/entries/{hex(entry_hash)}` — **keyed by its own
//! content hash**, for a reason worth keeping: §2.2.1 makes every reference to an
//! entry a **pin**, so the entry's identity *is* its hash, and a mutable key
//! would let a publisher move different bytes under the address a `via: path`
//! hint names. A key that is the identity cannot drift from it.
//!
//! **A consumer does not need to know this key.** A pin resolves by hash out of
//! the projected closure, and [`read_feed`] reaches every entry through the
//! index. The key exists so the publisher can bind, and so that §4.3 rule 6's
//! fallback has something to enumerate.
//!
//! ## 2. Pages fill oldest-first and are read newest-first, and getting that
//! backwards rewrites the archive on every post
//!
//! §4.3 rule 1 makes pages **key-addressed, never hash-chained**, so that
//! *"rewriting page 12 changes page 12's binding and nothing else"*. That
//! property only holds if a **new entry lands on the LAST page** — page 0 is the
//! oldest. Fill newest-first instead and every publish shifts every entry one
//! slot, so every page is rewritten, every reader's cursor is invalidated, and
//! §4.3 rule 3's *never renumber* is violated by the ordinary act of posting.
//!
//! **Within** a page the order is newest-first (§4.5), and it is **authored**,
//! not derived. So the two orders run opposite to each other by design:
//! [`plan_index`] takes entries oldest-first, chunks them in that order, and
//! reverses inside each chunk.
//!
//! **And the ordering is only half of rule 1's property — the other half is the
//! CLOCK, which the first cut of this module got wrong and its own gate could
//! not see.** Every page was stamped with the instant of the publish, so a
//! publisher posting tomorrow rewrote every page's bytes, re-projected the whole
//! archive and invalidated every page a reader had cached — rule 1's cost
//! argument defeated by the ordinary act of posting, which is the same sentence
//! §2 opens with about the ordering. It passed because
//! `posting_again_rewrites_only_the_last_page` publishes twice at one instant,
//! which is not a republish but one publish run twice. ***Ask what your gate's
//! expected value depends on***: that one depended on time not passing. A page's
//! `updated_at` is a **witness of its own entries** now — see [`plan_index`].
//!
//! ## 3. `signer = author` is FEED's shorthand for a field in somebody else's
//! type, and the naive reading produces something no verifier can use
//!
//! §1.1 says the signature entity carries *"`target = entry_hash` and
//! `signer = author`"*. `system/signature` is the **core protocol's** type, and
//! its `signer` field is *"content hash of the signer's identity entity (**NOT**
//! peer_id string)"* — the kernel shouts that parenthesis. So *"signer = author"*
//! means *the signer is the author*, expressed in the field's own type, and an
//! implementer reading FEED alone puts a peer id in a `bstr` slot that wants a
//! hash. Routed; we emit the identity hash [`RootProjector::sign_detached`]
//! hands back, which is the same field `registry_publish` fills.
//!
//! ## 4. Attribution, `FEED-R4` and the six ways it can fail are the READER's
//! — [`crate::feed_read`].
//!
//! ## 6. What closing the loop found, and none of it was visible from the codec
//!
//! - **§4.3 rule 6's fallback names a prefix the convention never defines** —
//!   `A-38`. The finding moved with the reader; see
//!   [`crate::feed_read::read_feed`].
//! - **An empty feed still needs a head**, or a reader cannot tell *"this author
//!   has posted nothing"* from *"this author has no feed"*. [`publish_feed`]
//!   emits `current: 0` with an empty page 0.
//! - **The index and the entries are one publish or the index is a liar.** A
//!   `finish` between them would sign a root naming pages whose entries are not
//!   in the closure yet.

// **Native only, and this was found by `make wasm` rather than by reading.**
// `RootProjector` and `publish_fixture` are both `cfg(not(wasm32))` — a
// publisher writes a directory, and the browser has none — so a module that
// imports them unconditionally breaks the WASM build while `make test` and
// `make lint` stay green, because neither compiles that target. That is the
// charter's *"always run `make wasm` after changes"* earning its keep on the
// first module it could apply to.
//
// The READER half (`attribute`, `read_feed`, `FeedSource`) has no such
// dependency and would compile anywhere; it lives here rather than in `feed`
// because a reader with no publisher to test it against is the shape this
// module exists to argue against. When a window follows a feed, that half moves
// and this gate narrows to the emitter.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)] // no CLI verb publishes a feed yet; the gate is native

use std::path::Path;

use entity_entity::Entity;
use entity_hash::Hash;

use crate::content_site::signed_root::RootProjector;
use crate::embed::EmbedPayload;
use crate::entity_ref::EntityRef;
use crate::feed::{
    entry_key, index_head_key, index_page_key, signature_key, FeedEntry, IndexHead, IndexPage,
};

/// How many entries we put on a page.
///
/// §4.3 rule 5 declines to name a number **on purpose** (*"what is normative is
/// the shape, not the arithmetic"*), and `FEED-R12` forbids a reader assuming
/// one — so this is a publisher-side choice with no wire meaning, and no reader
/// in this crate reads it. Small enough that a reader fetching one page to see
/// three new posts is not paying for a hundred.
pub const DEFAULT_PAGE_SIZE: usize = 32;

// ---------------------------------------------------------------------------
// FEED-R2 — the detached signature
// ---------------------------------------------------------------------------

/// The `system/signature` entity `FEED-R2` requires, for one entry.
///
/// The **only** thing this module adds to `registry_publish`'s emitter is the
/// target: same projector, same algorithm, same identity hash, same invariant
/// pointer. That sameness is the point — a second expression of *"how this
/// system signs one entity"* is what C15 exists to stop.
pub fn signature_entity(root: &RootProjector, entry_hash: &Hash) -> Result<Entity, String> {
    let (signature, signer) = root.sign_detached(entry_hash);
    entity_types::SignatureData {
        target: *entry_hash,
        signer,
        algorithm: root.algorithm().into(),
        signature,
    }
    .to_entity()
    .map_err(|e| format!("feed signature encodes: {e:?}"))
}

// ---------------------------------------------------------------------------
// §4 — the index
// ---------------------------------------------------------------------------

/// A feed's index, planned but not yet projected.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexPlan {
    /// Page *n* is `pages[n]`. Page 0 is the **oldest**; see the module doc's §2.
    pub pages: Vec<IndexPage>,
    pub head: IndexHead,
}

/// One entry, as the index needs it: where it lives and when it was authored.
///
/// The second half is here because **a page's `updated_at` is derived from its
/// own contents** — see [`plan_index`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagedEntry {
    pub hash: Hash,
    /// The entry's own `created_at`.
    pub created_at: u64,
}

/// Chunk entries into §4's pages.
///
/// `entries` is **oldest-first** — the order they were authored. Pages fill
/// in that order so that posting touches only the last page; within a page the
/// order is reversed, because §4.5's one ordering contract is newest-first
/// *within* a page. See the module doc's §2 for what filling the other way
/// costs.
///
/// ## A page's `updated_at` is a WITNESS of its contents, not the publish clock
///
/// §4.3 rule 1's [MUST] buys one property: *"rewriting page 12 changes page 12's
/// binding and nothing else — `O(tree depth)`, the same cost as posting."*
/// Stamping every page with the instant of the publish **defeats that property
/// through the ordinary act of posting**: page 0 holds the same three entries it
/// held yesterday, and its bytes move anyway, so every publish re-projects the
/// whole archive and invalidates every cached page a reader holds. Measured, not
/// reasoned — [`tests::a_page_nobody_touched_keeps_its_bytes_when_the_publisher_posts_later`]
/// was seen red, with page 0's content hash moving on nothing but the clock.
///
/// **§4.2 gives `updated_at` no semantics at all** — three CDDL lines in the
/// whole convention and no prose — so the reading is ours to make and this is
/// the one that keeps the [MUST]'s property. A page's stamp is the newest
/// `created_at` it carries: *this page last changed when its newest entry was
/// written*, which is true, and which an unchanged page reproduces for free
/// forever.
///
/// **Deliberately a witness rather than a prior-state read (AP44).** The
/// alternative — carry the previous publish's stamp forward for pages that did
/// not change — keeps *publish time* as the meaning, and it holds only as long
/// as the publisher still has the out-dir that publish wrote. `C-2` measured
/// exactly that: continuity here is recovered from the output directory, so a
/// second device publishing the same feed has no prior stamps to carry and would
/// rewrite the whole archive. A derived stamp needs nothing carried and is
/// therefore the same on every device. **Routed to arch**, because two readings
/// of an undefined field differ observably on the wire.
///
/// The **head** keeps the publish instant: it is one small mutable pointer that
/// genuinely changes on every publish, and it is what a reader polls.
pub fn plan_index(
    author: &str,
    entries: &[PagedEntry],
    page_size: usize,
    updated_at: u64,
) -> Result<IndexPlan, String> {
    if page_size == 0 {
        return Err("a page size of zero would publish an unbounded head".into());
    }
    let mut pages: Vec<IndexPage> = Vec::new();
    for (n, chunk) in entries.chunks(page_size).enumerate() {
        let mut refs: Vec<EntityRef> =
            chunk.iter().map(|e| EntityRef::pin(author, e.hash)).collect();
        // §4.5 — newest first WITHIN the page. The chunk arrived oldest-first.
        refs.reverse();
        // The witness. `max`, not `last`: §4.5 makes the order authored, so the
        // newest entry is not necessarily the one at the end.
        let stamp = chunk.iter().map(|e| e.created_at).max().unwrap_or(updated_at);
        pages.push(IndexPage::new(n as u64, refs, stamp));
    }
    if pages.is_empty() {
        // An author who has posted nothing still has a feed, and a reader must
        // be able to tell that from an author who has no feed at all. There is
        // no content to witness, so this one carries the publish instant.
        pages.push(IndexPage::new(0, Vec::new(), updated_at));
    }
    let current = pages.len() as u64 - 1;
    Ok(IndexPlan { pages, head: IndexHead::new(current, updated_at) })
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// What one feed publish emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedFeed {
    pub entry_count: usize,
    pub page_count: usize,
    /// Every entry's hash, oldest-first — the order they were handed in.
    pub entry_hashes: Vec<Hash>,
}

/// Project a whole feed into `dir` through `root`: every entry, every entry's
/// detached signature, every index page, the head, and the `system/content`
/// closure any pointer body depends on.
///
/// **This records; it does not [`RootProjector::finish`].** The caller signs the
/// root once, after — a `finish` between the entries and the index would sign a
/// root whose pages name entries that are not in its own closure, which is an
/// index that lies about its author's tree. One publish or none.
///
/// `entries` is **oldest-first**. `content` is the blob + chunk closure behind
/// every entry body that took EMBED §3's **pointer** arm
/// ([`crate::feed_tree::OwnedFeed::content`]); an empty slice is the ordinary
/// all-inline case.
pub fn publish_feed(
    dir: &Path,
    root: &mut RootProjector,
    entries: &[FeedEntry],
    content: &[Entity],
    page_size: usize,
    updated_at: u64,
) -> Result<PublishedFeed, String> {
    let author = root.peer_id().to_string();

    let mut hashes: Vec<Hash> = Vec::with_capacity(entries.len());
    let mut paged: Vec<PagedEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        // `FEED-R1` from the emitting side. The decoder refuses a mismatch; a
        // publisher that could still WRITE one would be publishing entities its
        // own reader is obliged to reject.
        if entry.author != author {
            return Err(format!(
                "entry claims author {} but this publisher is {author}",
                entry.author
            ));
        }
        check_body_is_publishable(entry)?;

        let entity = entry.to_entity()?;
        let hash = entity.content_hash;
        write_entity(dir, &author, &entry_key(&hash), &entity, root)?;

        // `FEED-R2`. Bound at the kernel's invariant pointer, so a consumer that
        // already knows how to fetch a root's signature fetches this one the
        // same way.
        let sig = signature_entity(root, &hash)?;
        write_entity(dir, &author, &signature_key(&author, &hash), &sig, root)?;

        hashes.push(hash);
        paged.push(PagedEntry { hash, created_at: entry.created_at });
    }

    // The `system/content` closure behind every pointer body. **Hash-addressed,
    // so `put_only` and not `write_entity`** — a pointer names its blob by hash,
    // and binding it under a second tree address would make the root commit to
    // something the consumer already reaches, and publish the same bytes twice.
    // Same door, same reason, as an oversized site figure's blob
    // (`publish_fixture::emit_owned_sites`).
    //
    // **Without this an entry over EMBED §3's 16 KiB ceiling publishes a
    // dangling reference**: the post renders as an empty body and nothing says
    // why. `check_body_is_publishable` refuses an oversized *inline* payload, so
    // the pointer arm is the only conformant way to carry one — which makes the
    // closure a requirement of that arm, not a nicety.
    for entity in content {
        root.put_only(entity);
    }

    let plan = plan_index(&author, &paged, page_size, updated_at)?;
    for page in &plan.pages {
        let entity = page.to_entity()?;
        write_entity(dir, &author, &index_page_key(page.page), &entity, root)?;
    }
    let head = plan.head.to_entity()?;
    write_entity(dir, &author, index_head_key(), &head, root)?;

    Ok(PublishedFeed {
        entry_count: entries.len(),
        page_count: plan.pages.len(),
        entry_hashes: hashes,
    })
}

/// The one conformance check a feed publisher owes that the codec cannot make.
///
/// EMBED §3's `inline-payload` is `bstr .size (1..16384)`, and the codec cannot
/// enforce it because **authoring and transcribing are different acts** — a
/// decoder that refused an oversized inline payload would make the site cache's
/// legacy write-through unreadable (see
/// [`EmbedPayload::inline_is_conformant`]). Authoring is where the range is
/// ours to keep, and this is the feed's authoring boundary.
fn check_body_is_publishable(entry: &FeedEntry) -> Result<(), String> {
    if let EmbedPayload::Inline(bytes) = &entry.body.data.payload {
        if !EmbedPayload::inline_is_conformant(bytes.len()) {
            return Err(format!(
                "entry body carries {} inline bytes, outside EMBED §3's 1..=16384 — \
                 it owes a pointer payload",
                bytes.len()
            ));
        }
    }
    Ok(())
}

fn write_entity(
    dir: &Path,
    peer_id: &str,
    key: &str,
    ent: &Entity,
    root: &mut RootProjector,
) -> Result<(), String> {
    crate::content_site::publish_fixture::write_entity(dir, peer_id, key, ent, Some(root))
        .map_err(|e| format!("write {key}: {e}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::embed::{EmbedData, EmbedNode};
    // The consumer half, exercised here because this is where a real projection
    // exists to read back. The `Tree` double and the executor are its.
    use crate::feed_read::{
        block_on, read_feed, attribute, Attribution, FeedReadError, FeedSource, Tree, Unattributed,
    };

    const NOW: u64 = 1_757_000_000_000;

    fn identity() -> entity_crypto::Keypair {
        entity_crypto::Keypair::from_seed([7u8; 32])
    }

    fn other_identity() -> entity_crypto::Keypair {
        entity_crypto::Keypair::from_seed([9u8; 32])
    }

    fn body(s: &str) -> EmbedNode {
        EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Inline(s.as_bytes().to_vec()), s),
        )
    }

    /// A whole tree in a map — the seam the module doc's §6 credits, and what
    /// every *decision* test below runs against.

    /// A published directory, served the way an origin serves one.
    ///
    /// **The production type**, not a copy of it: this was a `BinSource` spelled
    /// out here until [`crate::feed_gather`] needed the same thing for a real
    /// verb. A second spelling beside it would be C15's defect on a path this
    /// module's own gates depend on.
    ///
    /// `pub(crate)` so [`crate::feed_fetch`]'s gates can point the real
    /// `OriginFeedSource` at a real published tree — the only place in this
    /// crate that a projector exists to make one.
    pub(crate) use crate::feed_gather::DirOrigin as Origin;

    /// **A feed read through the REAL consumer** — the signed-root client, the
    /// manifest, the trie walk, the two-hop pointer/blob fetch and the signature
    /// verification the product uses for a site.
    ///
    /// Written this way after the first cut hand-decoded the projection's `.bin`
    /// pointers. That version measured *a decoder I wrote in the same file as
    /// the encoder*, which is the mistake `make crossimpl-site` names: the day
    /// the test mirrors the thing under test, the gate measures nothing.
    /// ⚠ **The `Rc` and the owned `PathBuf` are the async trait doing its job.**
    /// A boxed future cannot borrow `self`, so an implementor must own what it
    /// needs — which is exactly what the production implementation has to do
    /// anyway, since a `JsFuture` outlives the call that created it. The old
    /// synchronous trait let this struct hold plain borrows and `block_on`
    /// inside the method, which compiled, passed, and could never have been
    /// wired to a browser.
    struct SignedOrigin {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        session: std::rc::Rc<crate::content_site::signed_fetch::SignedSession>,
    }

    impl FeedSource for SignedOrigin {
        fn get(
            &self,
            relative_key: String,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Option<Entity>, String>>>,
        > {
            use crate::content_site::signed_fetch::SignedFetchError;
            let session = std::rc::Rc::clone(&self.session);
            let path = self.path.clone();
            Box::pin(async move {
                match session.resolve(&Origin(path), &relative_key).await {
                    Ok(e) => Ok(Some(e)),
                    Err(SignedFetchError::Absent) => Ok(None),
                    Err(e) => Err(format!("{e:?}")),
                }
            })
        }
    }

    /// Publish into a temp dir with a real projector and sign the root.
    fn publish_signed(entries: &[FeedEntry], page_size: usize) -> (SignedOrigin, PublishedFeed) {
        publish_signed_at(entries, page_size, NOW)
    }

    /// …at a stated instant. **Separate because the clock is a variable in this
    /// module, not a constant** — see
    /// [`a_page_nobody_touched_keeps_its_bytes_when_the_publisher_posts_later`].
    fn publish_signed_at(
        entries: &[FeedEntry],
        page_size: usize,
        updated_at: u64,
    ) -> (SignedOrigin, PublishedFeed) {
        publish_signed_with_content(entries, &[], page_size, updated_at)
    }

    /// …carrying a pointer body's blob closure. Separate because every other
    /// gate here is all-inline, and an empty closure is the case that cannot
    /// exhibit the defect it guards.
    fn publish_signed_with_content(
        entries: &[FeedEntry],
        content: &[Entity],
        page_size: usize,
        updated_at: u64,
    ) -> (SignedOrigin, PublishedFeed) {
        let dir = tempfile::tempdir().unwrap();
        let mut root = RootProjector::new(identity()).unwrap();
        let author = root.peer_id().to_string();
        let report =
            publish_feed(dir.path(), &mut root, entries, content, page_size, updated_at).unwrap();
        root.finish(dir.path()).expect("the root signs over the whole feed");
        let pin = crate::content_site::signed_fetch::PinnedPublisher::from_peer_id("", &author)
            .expect("a canonical peer-id carries its key");
        let session =
            std::rc::Rc::new(crate::content_site::signed_fetch::SignedSession::new(pin));
        let path = dir.path().to_path_buf();
        (SignedOrigin { _dir: dir, path, session }, report)
    }

    /// Harvest a published feed into a plain map **through the signed
    /// consumer**, so the decision tests below start from bytes that really
    /// travelled and can then delete one thing.
    fn publish(entries: &[FeedEntry], page_size: usize) -> (Tree, PublishedFeed, String) {
        publish_at(entries, page_size, NOW)
    }

    fn publish_at(
        entries: &[FeedEntry],
        page_size: usize,
        updated_at: u64,
    ) -> (Tree, PublishedFeed, String) {
        let (origin, report) = publish_signed_at(entries, page_size, updated_at);
        let author = author_id();
        let mut tree = Tree::default();
        let mut take = |tree: &mut Tree, key: String| {
            if let Ok(Some(e)) = block_on(origin.get(key.clone())) {
                tree.0.insert(key, e);
            }
        };
        take(&mut tree, index_head_key().to_string());
        // Pages: the head is authoritative about which numbers are in use.
        if let Ok(Some(h)) = block_on(origin.get(index_head_key().to_string())) {
            let head = IndexHead::from_entity(&h).unwrap();
            for n in head.oldest..=head.current {
                take(&mut tree, index_page_key(n));
            }
        }
        for hash in &report.entry_hashes {
            take(&mut tree, entry_key(hash));
            take(&mut tree, signature_key(&author, hash));
        }
        (tree, report, author)
    }

    /// A published, signed feed on disk, for gates in **other** modules that
    /// need a real origin rather than a map.
    pub(crate) fn published_origin(n: usize) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let mut root = RootProjector::new(identity()).unwrap();
        let author = root.peer_id().to_string();
        publish_feed(dir.path(), &mut root, &entries(&author, n), &[], 3, NOW).unwrap();
        root.finish(dir.path()).expect("the root signs over the whole feed");
        (dir, author)
    }

    /// A second publisher's identity, for gates that need a peer id which is
    /// canonical but is **not** the author of the tree in hand.
    pub(crate) fn other_author_id() -> String {
        RootProjector::new(other_identity()).unwrap().peer_id().to_string()
    }

    /// A published feed harvested into a map, for gates in **other** modules
    /// that need real published bytes and have no projector.
    pub(crate) fn published_tree(n: usize) -> (crate::feed_read::Tree, String, PublishedFeed)
    {
        let author = author_id();
        let (tree, report, author) = publish(&entries(&author, n), 10);
        (tree, author, report)
    }

    /// A published feed **left on disk**, for a consumer that needs a directory
    /// rather than a harvested map.
    ///
    /// `published_tree` harvests into a `Tree` and drops the temp dir, which is
    /// right for every decision gate here and useless to [`crate::feed_gather`],
    /// whose whole subject is reading a real projection. The `TempDir` is
    /// returned rather than leaked so the caller's scope owns the lifetime.
    pub(crate) fn published_dir(n: usize) -> (tempfile::TempDir, String) {
        let author = author_id();
        let dir = tempfile::tempdir().unwrap();
        let mut root = RootProjector::new(identity()).unwrap();
        publish_feed(dir.path(), &mut root, &entries(&author, n), &[], 10, NOW)
            .expect("the fixture publishes");
        root.finish(dir.path()).expect("the root signs over the whole feed");
        (dir, author)
    }

    /// The newest `created_at` [`published_dir`] would carry at `n` posts — so a
    /// gate about the stamp can name the value without recomputing the fixture's
    /// arithmetic beside it.
    pub(crate) fn newest_created_at(n: usize) -> u64 {
        entries(&author_id(), n).iter().map(|e| e.created_at).max().unwrap_or(0)
    }

    fn entries(author: &str, n: usize) -> Vec<FeedEntry> {
        (0..n)
            .map(|i| FeedEntry::new(author, NOW + i as u64, body(&format!("post {i}"))))
            .collect()
    }

    fn author_id() -> String {
        RootProjector::new(identity()).unwrap().peer_id().to_string()
    }

    // -- the end-to-end loop ----------------------------------------------

    /// **THE GATE THIS WHOLE MODULE EXISTS FOR: publish a feed and read it
    /// back.**
    ///
    /// Three phases of codec had no consumer, and a round-trip test inside one
    /// module cannot see anything that lives between them. This one crosses
    /// `feed` → `entity_ref` → `embed` → `RootProjector` → the on-disk
    /// projection → back, and every assertion is about a fact a person would
    /// notice: the posts are all there, they are in the order they were
    /// published, and each one is attributable to the person who wrote it.
    #[test]
    fn a_published_feed_reads_back_in_order_and_every_entry_is_attributable() {
        let author = author_id();
        let posted = entries(&author, 7);
        // The REAL consumer: manifest, root signature, trie walk, two-hop fetch.
        let (origin, report) = publish_signed(&posted, 3);

        assert_eq!(report.entry_count, 7);
        assert_eq!(report.page_count, 3, "7 entries at 3 a page is pages 0,1,2");

        let read = block_on(read_feed(&origin, &author, 100)).expect("the feed reads");
        assert_eq!(read.len(), 7, "every entry the index names came back");

        // §4.5: newest first, pages walked down from `current`. So the read
        // order is the reverse of the authoring order.
        let fallbacks: Vec<&str> = read.iter().map(|r| r.entry.body.data.fallback.as_str()).collect();
        assert_eq!(
            fallbacks,
            vec!["post 6", "post 5", "post 4", "post 3", "post 2", "post 1", "post 0"],
            "newest first, within pages and across them"
        );

        for r in &read {
            assert_eq!(r.attribution, Attribution::Signed, "FEED-R2 covers every entry");
            assert!(r.attribution.may_name_the_author());
            assert_eq!(r.entry.author, author, "FEED-R1 held through the round trip");
        }
    }

    /// **THE ANTI-VACUITY GUARD FOR THE GATE ABOVE.** *"The feed read back"* is
    /// true of any rig that reads files out of a directory, and the claim being
    /// made is much stronger: that it came through a **verified** signed root.
    ///
    /// So: move one byte of the published root manifest and require the read to
    /// fail. Without this, a `SignedOrigin` that had quietly degraded to
    /// plain file reads would carry every other test in this module.
    #[test]
    fn the_round_trip_really_goes_through_the_signed_root_and_not_past_it() {
        let author = author_id();
        let (origin, _) = publish_signed(&entries(&author, 3), 10);
        assert_eq!(
            block_on(read_feed(&origin, &author, 10)).map(|r| r.len()),
            Ok(3),
            "the control: this origin serves a feed"
        );

        // The manifest is the signed head. Corrupt it in place.
        let manifest = origin
            .path
            .join(&author)
            .join(crate::content_site::paths::PUBLISHED_ROOT_REL);
        let mut bytes = std::fs::read(&manifest).expect("the projection wrote a published root");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&manifest, &bytes).unwrap();

        let after = block_on(read_feed(&origin, &author, 10));
        assert!(
            after.is_err(),
            "a tampered root must not still serve a feed — got {after:?}"
        );
    }

    /// **The falsifier for the ordering claim, and it is not a restatement of
    /// it.** Publishing again with one more entry must touch only the LAST page
    /// — §4.3 rule 1's whole property. Fill pages newest-first instead and every
    /// page's bytes move, which is `FEED-R11`'s renumbering hazard arriving
    /// through the publisher.
    #[test]
    fn posting_again_rewrites_only_the_last_page() {
        let author = author_id();
        let first = entries(&author, 6);
        let (before, _, _) = publish(&first, 3);

        let mut second = first.clone();
        second.push(FeedEntry::new(&author, NOW + 99, body("post 6")));
        let (after, _, _) = publish(&second, 3);

        let page_hash = |t: &Tree, n: u64| {
            t.0.get(&index_page_key(n)).map(|e| e.content_hash)
        };
        assert_eq!(
            page_hash(&before, 0),
            page_hash(&after, 0),
            "page 0 is byte-identical across the two publishes"
        );
        assert_eq!(page_hash(&before, 1), page_hash(&after, 1), "and so is page 1");
        assert!(
            page_hash(&after, 2).is_some() && page_hash(&before, 2).is_none(),
            "the new entry opened page 2 and left the archive alone"
        );
    }

    /// **The same claim as above, with the one variable the rig above holds
    /// still: the CLOCK.**
    ///
    /// `posting_again_rewrites_only_the_last_page` publishes twice at `NOW`,
    /// which is not a republish — it is one publish run twice. A publisher
    /// posting tomorrow passes tomorrow's clock, and if that clock reaches every
    /// page then every page's bytes move, the whole archive is re-projected, and
    /// §4.3 rule 1's *"rewriting page 12 changes page 12's binding and nothing
    /// else — `O(tree depth)`, the same cost as posting"* is defeated by the
    /// ordinary act of posting.
    ///
    /// ***Ask what your gate's expected value depends on.*** That one depended
    /// on time not passing.
    #[test]
    fn a_page_nobody_touched_keeps_its_bytes_when_the_publisher_posts_later() {
        let author = author_id();
        let first = entries(&author, 6);
        let (before, _, _) = publish_at(&first, 3, NOW);

        // A day later, one new post.
        let tomorrow = NOW + 86_400_000;
        let mut second = first.clone();
        second.push(FeedEntry::new(&author, tomorrow, body("post 6")));
        let (after, _, _) = publish_at(&second, 3, tomorrow);

        let page_hash = |t: &Tree, n: u64| t.0.get(&index_page_key(n)).map(|e| e.content_hash);
        assert_eq!(
            page_hash(&before, 0),
            page_hash(&after, 0),
            "page 0 holds the same three entries it held yesterday, so its bytes \
             must not move — a page that did not change was not updated"
        );
        assert_eq!(page_hash(&before, 1), page_hash(&after, 1), "and so is page 1");
        assert!(
            page_hash(&after, 2).is_some(),
            "the control: the new post really did open a third page"
        );
    }

    /// **§4.2's `page MUST equal its key`, measured through the PROJECTION.**
    /// The codec's version of this test builds an entity and hands it a
    /// disagreeing key; this one asserts the publisher never creates the
    /// disagreement in the first place.
    #[test]
    fn every_projected_page_answers_to_the_key_it_was_written_at() {
        let author = author_id();
        let (tree, report, _) = publish(&entries(&author, 10), 4);
        for n in 0..report.page_count as u64 {
            let entity = tree.0.get(&index_page_key(n)).expect("the page is projected");
            IndexPage::from_entity(entity, n).expect("its body agrees with its key");
        }
    }

    /// An author who has posted nothing still has a feed, and a reader must be
    /// able to tell that from an author who has none. Absent that, *"no posts
    /// yet"* and *"this is not a feed publisher"* are one screen.
    #[test]
    fn an_author_with_no_posts_publishes_a_head_and_an_empty_page() {
        let (tree, report, author) = publish(&[], DEFAULT_PAGE_SIZE);
        assert_eq!(report.page_count, 1);
        assert!(tree.0.contains_key(index_head_key()), "there is a head");
        assert_eq!(block_on(read_feed(&tree, &author, 10)).unwrap(), vec![], "and it is empty");

        let nothing = Tree::default();
        assert!(
            matches!(block_on(read_feed(&nothing, &author, 10)), Err(FeedReadError::NoIndex { .. })),
            "where an author with NO feed is a different answer"
        );
    }

    // -- FEED-R2 / FEED-R4 -------------------------------------------------

    /// `FEED-R2`'s signature lands at the kernel's invariant pointer, so a
    /// consumer that can already fetch a root's signature fetches this one the
    /// same way.
    #[test]
    fn every_entry_carries_a_signature_at_the_invariant_pointer() {
        let author = author_id();
        let (tree, report, author) = publish(&entries(&author, 3), 10);
        for hash in &report.entry_hashes {
            let key = signature_key(&author, hash);
            let sig = tree.0.get(&key).unwrap_or_else(|| panic!("no signature at {key}"));
            let decoded = entity_types::SignatureData::from_entity(sig).unwrap();
            assert_eq!(decoded.target, *hash, "it covers the entry it is filed under");
            assert_eq!(
                attribute(&author, hash, Some(sig)),
                Attribution::Signed
            );
        }
    }

    /// **`FEED-R4`, and the reason it is a type rather than a flag.** An entry
    /// whose signature is absent is presented as unattributed — never as by the
    /// author, whose name is right there in the entry body.
    #[test]
    fn an_entry_with_no_signature_is_unattributed_and_never_attributed_to_its_author() {
        let author = author_id();
        let (mut tree, report, author) = publish(&entries(&author, 2), 10);
        let orphan = report.entry_hashes[0];
        tree.0.remove(&signature_key(&author, &orphan));

        let read = block_on(read_feed(&tree, &author, 10)).unwrap();
        let target = read.iter().find(|r| r.hash == orphan).expect("the entry is still there");
        assert_eq!(
            target.attribution,
            Attribution::Unattributed(Unattributed::NoSignature),
            "the entry survives — FEED-R4 is about what we SAY, not about refusing it"
        );
        assert!(
            !target.attribution.may_name_the_author(),
            "and its author field must not be enough to name anyone"
        );
        // The control: the other entry is unaffected, so this is not a rig that
        // fails to attribute anything.
        assert!(read.iter().any(|r| r.attribution == Attribution::Signed));
    }

    /// **Every way an attribution can fail, each with its own word.** A copied
    /// signature, a forged one, one by a different author and one we simply
    /// cannot check are four different reports going to four different people
    /// — collapsing any pair loses the only distinction that decides what to do.
    #[test]
    fn each_way_an_entry_fails_to_be_attributable_has_its_own_outcome() {
        let mut root = RootProjector::new(identity()).unwrap();
        let author = root.peer_id().to_string();
        let entry = FeedEntry::new(&author, NOW, body("mine")).to_entity().unwrap();
        let other_entry = FeedEntry::new(&author, NOW + 1, body("also mine")).to_entity().unwrap();
        let good = signature_entity(&root, &entry.content_hash).unwrap();

        assert_eq!(attribute(&author, &entry.content_hash, Some(&good)), Attribution::Signed);
        assert_eq!(
            attribute(&author, &entry.content_hash, None),
            Attribution::Unattributed(Unattributed::NoSignature)
        );

        // A signature lifted from the author's OTHER entry. It verifies
        // perfectly — against the wrong target.
        let lifted = signature_entity(&root, &other_entry.content_hash).unwrap();
        assert_eq!(
            attribute(&author, &entry.content_hash, Some(&lifted)),
            Attribution::Unattributed(Unattributed::WrongTarget {
                target: other_entry.content_hash,
                entry: entry.content_hash,
            })
        );

        // A different peer signing this author's entry, correctly, with their
        // own key. Nothing is malformed and nothing is forged; it is simply not
        // the author's signature.
        let stranger = RootProjector::new(other_identity()).unwrap();
        let theirs = signature_entity(&stranger, &entry.content_hash).unwrap();
        assert_eq!(
            attribute(&author, &entry.content_hash, Some(&theirs)),
            Attribution::Unattributed(Unattributed::SignerIsNotTheAuthor)
        );

        // The author's signer field with somebody else's bytes — the forgery
        // case, and the only one that reaches `verify`.
        let mut forged = entity_types::SignatureData::from_entity(&good).unwrap();
        forged.signature[0] ^= 0xff;
        assert_eq!(
            attribute(&author, &entry.content_hash, Some(&forged.to_entity().unwrap())),
            Attribution::Unattributed(Unattributed::BadSignature)
        );

        // Not a signature at all.
        assert!(matches!(
            attribute(&author, &entry.content_hash, Some(&entry)),
            Attribution::Unattributed(Unattributed::SignatureUnreadable { .. })
        ));

        // A peer id that does not carry its key is OUR limit, not a verdict on
        // the entry — "we could not check" must never render as "not theirs".
        assert!(matches!(
            attribute("not-a-peer-id", &entry.content_hash, Some(&good)),
            Attribution::Unattributed(Unattributed::KeyNotInPeerId { .. })
        ));

        // Ensure the roster above is the whole roster: a seventh reason must
        // fail this, not quietly render as attributed.
        let _exhaustive = |u: Unattributed| match u {
            Unattributed::NoSignature
            | Unattributed::KeyNotInPeerId { .. }
            | Unattributed::SignatureUnreadable { .. }
            | Unattributed::WrongTarget { .. }
            | Unattributed::SignerIsNotTheAuthor
            | Unattributed::BadSignature => (),
        };
    }

    /// `FEED-R1` from the emitting side. A publisher that could write an entry
    /// its own reader is obliged to reject is a publisher shipping garbage into
    /// a signed root.
    #[test]
    fn a_publisher_refuses_to_sign_an_entry_by_somebody_else() {
        let dir = tempfile::tempdir().unwrap();
        let mut root = RootProjector::new(identity()).unwrap();
        let forged = FeedEntry::new("QmSomebodyElse", NOW, body("not mine"));
        let err = publish_feed(dir.path(), &mut root, &[forged], &[], 4, NOW).unwrap_err();
        assert!(err.contains("QmSomebodyElse"), "and it says whose name it refused: {err}");
    }

    /// **A POST TOO BIG TO INLINE PUBLISHES ITS BYTES, OR IT PUBLISHES A
    /// DANGLING REFERENCE.**
    ///
    /// `check_body_is_publishable` refuses an oversized *inline* payload, so
    /// EMBED §3's **pointer** arm is the only conformant way to carry a long
    /// post — and until this landed, `publish_feed` wrote the entry entity and
    /// nothing else. The entry resolves, the index names it, the signature
    /// verifies, and the body is a hash naming bytes that are not in the
    /// projection: **a post that renders empty with nothing anywhere saying
    /// why.** The same obligation an oversized figure puts on a site publish,
    /// missed here because every fixture in this module was all-inline — *a
    /// test population you generated cannot contain the shape you are missing.*
    ///
    /// Asserted as the **consequence** — the bytes come back — rather than as
    /// "a blob file exists": reassembled through `entity_content::reassemble`,
    /// the same kernel call the production resolver makes, out of what the
    /// published origin actually serves.
    #[test]
    fn a_post_whose_body_is_a_pointer_publishes_the_bytes_it_points_at() {
        use crate::content_site::http_poll::fetch_content;
        use entity_store::{ContentStore, MemoryContentStore};
        use std::sync::Arc;

        let author = author_id();
        // Over the inline range, so `stage` takes the pointer arm and the body
        // holds no bytes of its own.
        let long_post = vec![b'p'; crate::embed::INLINE_PAYLOAD_MAX + 4096];
        let scratch: Arc<dyn ContentStore> = Arc::new(MemoryContentStore::new());
        let staged =
            crate::content_site::asset_store::stage("text/plain", long_post.clone(), &scratch)
                .unwrap();
        let pointer = staged.asset.pointer().expect("oversized bytes take the pointer arm");

        let node = EmbedNode::new(
            "text/plain",
            EmbedData::new(EmbedPayload::Pointer(pointer), "a very long post"),
        );
        let (origin, _) = publish_signed_with_content(
            &[FeedEntry::new(&author, NOW, node)],
            &staged.content,
            4,
            NOW,
        );

        // The post is there and still says it is a pointer.
        let read = block_on(read_feed(&origin, &author, 10)).expect("the feed reads");
        assert_eq!(read.len(), 1);
        assert_eq!(
            read[0].entry.body.data.payload,
            EmbedPayload::Pointer(pointer),
            "the body travelled as a pointer, which is what makes the closure load-bearing"
        );

        // …and the bytes it points at are served by the origin. Fetch the blob
        // and every chunk it names, then reassemble.
        //
        // **D24 / `foreign-cache-lint`: these two `fetch_content` calls are
        // counted and baselined, and they produce NO durable copy.** They read a
        // directory this test published three lines up, into a scratch
        // `MemoryContentStore` that is dropped at the end of the function —
        // there is no store to go stale and no currency question to answer.
        // Going through `fetch_content` rather than `std::fs::read` is what
        // makes the assertion strong: it is the consumer's own two-hop, so it
        // verifies the served bytes hash to the pointer the body carries. Same
        // shape and same justification as the four rows already baselined for
        // `publish_fixture.rs`, which are also test-only.
        let src = Origin(origin.path.clone());
        let blob = block_on(fetch_content(&src, "", &pointer))
            .expect("the blob the body points at is in the projection");
        let chunks = crate::content_site::asset_store::chunk_hashes_of(&blob).unwrap();
        assert!(!chunks.is_empty(), "the control: an oversized body really did chunk");
        let held = MemoryContentStore::new();
        held.put(blob).unwrap();
        for ch in &chunks {
            held.put(
                block_on(fetch_content(&src, "", ch))
                    .unwrap_or_else(|e| panic!("chunk {} is missing from the projection: {e:?}", ch.to_hex())),
            )
            .unwrap();
        }
        let held: Arc<dyn ContentStore> = Arc::new(held);
        assert_eq!(
            entity_content::reassemble(&held, &pointer).expect("the closure reassembles"),
            long_post,
            "the post's own bytes came back out of the published origin"
        );
    }

    /// The authoring bound EMBED §3 sets and the codec deliberately does not
    /// enforce — see [`check_body_is_publishable`]. Asserted from **both sides**
    /// of the range, because each end has a different wrong implementation.
    #[test]
    fn a_body_outside_the_inline_range_is_refused_at_the_authoring_boundary() {
        let author = author_id();
        let at_the_ceiling = vec![b'x'; crate::embed::INLINE_PAYLOAD_MAX];
        let over = vec![b'x'; crate::embed::INLINE_PAYLOAD_MAX + 1];

        let node = |bytes: Vec<u8>| {
            EmbedNode::new("text/plain", EmbedData::new(EmbedPayload::Inline(bytes), "big"))
        };
        let publish_one = |n: EmbedNode| {
            let dir = tempfile::tempdir().unwrap();
            let mut root = RootProjector::new(identity()).unwrap();
            let e = FeedEntry::new(&author, NOW, n);
            publish_feed(dir.path(), &mut root, &[e], &[], 4, NOW).map(|_| ())
        };
        assert!(publish_one(node(at_the_ceiling)).is_ok(), "16384 is the last legal inline body");
        let err = publish_one(node(over)).unwrap_err();
        assert!(err.contains("16384"), "and 16385 is refused, saying why: {err}");
    }

    // -- the index -----------------------------------------------------------

    /// `plan_index`'s two orders run opposite to each other and both are the
    /// convention's. Pinned here rather than only through the round trip,
    /// because the round trip would pass if BOTH were reversed.
    #[test]
    fn pages_fill_oldest_first_and_read_newest_first_within_a_page() {
        let author = "QmAuthor";
        let hashes: Vec<Hash> = (0..5)
            .map(|i| Hash::compute("test/note", format!("e{i}").as_bytes()))
            .collect();
        let plan = plan_index(author, &paged(&hashes), 2, NOW).unwrap();

        assert_eq!(plan.pages.len(), 3);
        assert_eq!(plan.head.current, 2, "the newest page is the highest number");
        assert_eq!(plan.head.oldest, 0);

        // Page 0 holds the two OLDEST, newest-of-those first.
        let page0: Vec<Hash> = plan.pages[0]
            .entries
            .iter()
            .map(|r| match r {
                EntityRef::Pinned { hash, .. } => *hash,
                _ => panic!("§2.2.1 types index entries as pinned"),
            })
            .collect();
        assert_eq!(page0, vec![hashes[1], hashes[0]]);
        assert_eq!(plan.pages[2].entries.len(), 1, "the last page is the short one");
    }

    #[test]
    fn a_page_size_of_zero_is_refused_rather_than_publishing_one_unbounded_page() {
        assert!(plan_index("QmAuthor", &[], 0, NOW).is_err());
    }

    /// Synthetic entries for the pure planner: stamps ascending with the index,
    /// so `max` and `last` agree — which is why the test below deliberately
    /// hands it a page where they do **not**.
    fn paged(hashes: &[Hash]) -> Vec<PagedEntry> {
        hashes
            .iter()
            .enumerate()
            .map(|(i, h)| PagedEntry { hash: *h, created_at: NOW + i as u64 })
            .collect()
    }

    /// **The page stamp is a witness of the page, and the publish clock does not
    /// reach it.** The unit-level statement of
    /// [`a_page_nobody_touched_keeps_its_bytes_when_the_publisher_posts_later`],
    /// which measures the same rule through a whole projection.
    ///
    /// The last page is the one entitled to move, and the head always is.
    #[test]
    fn a_pages_stamp_comes_from_its_own_entries_and_the_head_carries_the_publish_clock() {
        let author = "QmAuthor";
        let hashes: Vec<Hash> = (0..3)
            .map(|i| Hash::compute("test/note", format!("e{i}").as_bytes()))
            .collect();
        // §4.5 makes the order AUTHORED, so the newest entry need not be last.
        // `max`, not `last` — a backdated post at the end must not drag the
        // page's stamp backwards.
        let out_of_order = vec![
            PagedEntry { hash: hashes[0], created_at: NOW + 500 },
            PagedEntry { hash: hashes[1], created_at: NOW + 9_000 },
            PagedEntry { hash: hashes[2], created_at: NOW + 20 },
        ];
        let plan = plan_index(author, &out_of_order, 3, NOW + 1_000_000).unwrap();
        assert_eq!(
            plan.pages[0].updated_at,
            NOW + 9_000,
            "the newest entry on the page, not the last one and not the clock"
        );
        assert_eq!(
            plan.head.updated_at,
            NOW + 1_000_000,
            "the head is the mutable pointer and it does carry the publish instant"
        );

        // An empty feed has no content to witness, so page 0 falls back to the
        // publish instant — the one page the clock legitimately reaches.
        let empty = plan_index(author, &[], 4, NOW + 77).unwrap();
        assert_eq!(empty.pages[0].updated_at, NOW + 77);
    }

    /// **§4.3 rule 6's other direction: the index is not the authority.** An
    /// entry the index names that does not resolve makes the view *short*, never
    /// corrupt — a publisher who unpublished an entry has not broken their feed.
    #[test]
    fn an_index_naming_an_entry_that_does_not_resolve_yields_a_short_view() {
        let author = author_id();
        let (mut tree, report, author) = publish(&entries(&author, 3), 10);
        tree.0.remove(&entry_key(&report.entry_hashes[1]));

        let read = block_on(read_feed(&tree, &author, 10)).expect("the read still succeeds");
        assert_eq!(read.len(), 2, "a partial view is never a wrong view, only a short one");
        assert!(read.iter().all(|r| r.attribution == Attribution::Signed));
    }

    /// **An entry we could not FETCH is not an entry the publisher withdrew.**
    /// The two arrive at the same place — an entry the index names that is not
    /// in hand — and they are opposite facts: one is the author's edit, the
    /// other is an outage. A reader that shortened the feed on the second would
    /// render someone's unreachable posts as posts they deleted.
    ///
    /// It also pins the sentence. Before the review this landed on
    /// `PageMissing { page: 0 }`, which reads *"the head names page 0 and it
    /// does not resolve"* — a report about the index, for a fault in an entry.
    #[test]
    fn an_entry_we_could_not_fetch_is_a_different_report_from_one_that_is_gone() {
        let author = author_id();
        let (tree, report, author) = publish(&entries(&author, 3), 10);
        let victim = report.entry_hashes[1];

        /// The same tree, with one key that answers *"I could not look"* rather
        /// than *"it is not here"*.
        struct Flaky(Tree, String);
        impl FeedSource for Flaky {
            fn get(
                &self,
                relative_key: String,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<Option<Entity>, String>>>,
            > {
                if relative_key == self.1 {
                    return Box::pin(std::future::ready(Err(
                        "the origin dropped the connection".to_string()
                    )));
                }
                self.0.get(relative_key)
            }
        }

        let err = block_on(read_feed(&Flaky(tree, entry_key(&victim)), &author, 10))
            .expect_err("a fault we could not see past is not a short view");
        assert_eq!(
            err,
            FeedReadError::EntryUnreachable {
                entry: victim,
                detail: "the origin dropped the connection".into(),
            }
        );
        assert!(
            err.to_string().contains(&victim.to_hex()),
            "and it names the entry, not a page: {err}"
        );
    }

    /// A head naming a page the publisher does not serve is **not** the same as
    /// an entry going missing: the publisher committed to that page number in a
    /// signed root. Different fact, different outcome.
    #[test]
    fn a_head_naming_a_page_that_does_not_resolve_is_its_own_outcome() {
        let author = author_id();
        let (mut tree, _, author) = publish(&entries(&author, 5), 2);
        tree.0.remove(&index_page_key(1));
        assert!(matches!(
            block_on(read_feed(&tree, &author, 10)),
            Err(FeedReadError::PageMissing { page: 1, .. })
        ));
    }

    // -----------------------------------------------------------------------
    // Two axes, one directory — the constraint any publish verb must satisfy.
    // -----------------------------------------------------------------------

    /// Emit a one-page site into `dir` under `author`, signed, the way `publish`
    /// does. Returns the site-relative manifest key.
    fn emit_signed_site(dir: &Path, site_id: &str) -> String {
        use crate::content_site::format::{SiteManifest, SitePage};
        use crate::content_site::read::OwnedSite;

        let site = OwnedSite {
            peer_id: author_id(),
            site_id: site_id.into(),
            manifest: SiteManifest::new(site_id, "Demo", "index", vec![]),
            pages: vec![("index".to_string(), SitePage::markdown("Index", "hello"))],
            assets: vec![],
            content: Default::default(),
        };
        let mut site_root = RootProjector::new(identity()).unwrap();
        crate::content_site::publish_fixture::emit_owned_sites(
            dir,
            std::slice::from_ref(&site),
            "",
            Some(&mut site_root),
        )
        .expect("the site emits");
        site_root.finish(dir).expect("the site root signs");
        format!("sites/{site_id}/manifest")
    }

    /// Resolve `key` out of `dir` through the real signed consumer.
    fn resolve_at(dir: &Path, key: &str) -> Result<Entity, String> {
        let pin =
            crate::content_site::signed_fetch::PinnedPublisher::from_peer_id("", &author_id())
                .unwrap();
        let session = std::rc::Rc::new(crate::content_site::signed_fetch::SignedSession::new(pin));
        let path = dir.to_path_buf();
        let k = key.to_string();
        block_on(async move {
            session.resolve(&Origin(path), &k).await.map_err(|e| format!("{e:?}"))
        })
    }

    /// ⚠ **THE CONSTRAINT ON ANY FEED PUBLISH VERB, MEASURED — a second axis
    /// signed by its own projector UN-NAMES the first, silently.**
    ///
    /// [`RootProjector::finish`] builds the trie over **`self.bindings`** — the
    /// keys *that projector* recorded — and the module's own doc says so: *"the
    /// root commits to the bytes we projected, not to the tree we read from."*
    /// So two projectors over one directory do not compose: the second `finish`
    /// signs a root naming only its own axis, and everything the first published
    /// stops being reachable through the signed root.
    ///
    /// **The bytes are still on disk.** This is not a delete — it is an
    /// un-naming, and the consumer reports **`Absent`**, i.e. *the publisher
    /// withdrew this site*, about a site that is right there. Same wrong
    /// sentence as `AP54`'s retired-publisher arm, from the opposite direction.
    ///
    /// **What this rules out is a SECOND, INDEPENDENT WRITER putting bytes into
    /// a directory `publish` owns** — i.e. a standalone `feed OUT_DIR` verb.
    ///
    /// ⚠ **It does NOT say `publish` is destructive, and the first write-up of
    /// this test did.** `resolve_publish_source` is
    /// `ingest_path(disk) → tree → read_all_sites(tree) → project`: publish
    /// **translates an input into the tree and then projects the tree**, so the
    /// clean is what a *snapshot* means, and the tree is the authority. The
    /// right shape is **one projection with a third axis read out of the tree**
    /// — a `read_all_feeds` beside `read_all_sites`, through this same
    /// projector. `AGENTS.md` already prescribed it (*what ENTERS the projection
    /// is a hardcoded enumeration of two L5 conventions, not a policy over the
    /// tree*); this is the measurement that makes it binding rather than a
    /// preference.
    #[test]
    fn a_second_axis_signed_by_its_own_projector_un_names_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let manifest_key = emit_signed_site(dir.path(), "demo");

        // Anti-vacuity: the site must resolve BEFORE the feed publish, or this
        // test measures a site that was never there.
        assert!(
            resolve_at(dir.path(), &manifest_key).is_ok(),
            "precondition: the site resolves after its own publish"
        );

        let mut feed_root = RootProjector::new(identity()).unwrap();
        publish_feed(dir.path(), &mut feed_root, &entries(&author_id(), 3), &[], 2, NOW)
            .expect("the feed publishes");
        feed_root.finish(dir.path()).expect("the feed root signs");

        // The feed arrived…
        assert!(
            resolve_at(dir.path(), index_head_key()).is_ok(),
            "the feed head resolves after the feed publish"
        );
        // …and took the site's name with it.
        assert_eq!(
            resolve_at(dir.path(), &manifest_key).map(|_| "resolves").map_err(|e| e),
            Err("Absent".to_string()),
            "the site should have been un-named by the second root — if this now \
             RESOLVES, the projection composes and the standalone-verb constraint \
             this test records has been lifted"
        );
        // And the un-naming is not a delete: the bytes are untouched on disk,
        // which is what makes the `Absent` report wrong rather than merely bad.
        assert!(
            dir.path().join(author_id()).join("sites/demo/manifest.bin").exists(),
            "the site's bytes are still on disk — this is an un-naming, not a delete"
        );
    }

    /// **The other direction — and this one is NOT a defect. It is the cheapest
    /// statement of why the second writer is the mistake.**
    ///
    /// A site publish cleans `{base}/{peer_id}/` wholesale
    /// (`content_site::publish`, the `clean` list) and a feed's entities live at
    /// `{base}/{peer_id}/app/feed/…`, inside it. Read as *"publish deletes the
    /// feed"* that sounds destructive; it is not. **A projection carries what is
    /// in the tree it read**, and a feed written into the out-dir behind
    /// publish's back was never in that tree. Re-projecting is the point — the
    /// alternative is an output that has drifted from the tree it claims to
    /// snapshot.
    ///
    /// So the pair says one thing, not two: **the out-dir is publish's to
    /// write, and anything that wants to be in the signed root has to be in the
    /// tree publish projects.**
    ///
    /// Pinned as a **path fact** rather than by driving the CLI, because the
    /// clean is inside `run()` behind full argument parsing — and the fact that
    /// decides it is only *where the bytes are*.
    #[test]
    fn a_feeds_bytes_live_inside_the_subtree_a_site_publish_cleans() {
        let dir = tempfile::tempdir().unwrap();
        let author = author_id();
        let mut root = RootProjector::new(identity()).unwrap();
        publish_feed(dir.path(), &mut root, &entries(&author, 3), &[], 2, NOW).unwrap();
        root.finish(dir.path()).unwrap();

        // `publish`'s clean is `clean.push(base.join(peer_id))`.
        let cleaned = dir.path().join(&author);
        let head = cleaned.join(format!("{}.bin", index_head_key()));
        assert!(head.exists(), "precondition: the head is where we think it is ({})", head.display());
        assert!(
            head.starts_with(&cleaned),
            "a feed's head is inside the subtree a site publish removes wholesale"
        );
    }
}
