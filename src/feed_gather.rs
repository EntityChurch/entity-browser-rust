//! **Gathering** — reading somebody else's published feed so that
//! [`crate::feed_mirror`] can republish it.
//!
//! [`crate::feed_mirror`] is the *decision* (what a gatherer may carry, and what
//! it must refuse) and [`crate::feed_read`] is the *consumer*. Neither of them
//! has a source: every `FeedSource` that reads a real published tree lived in a
//! `#[cfg(test)]` module until this one. That is the gap between *"the gatherer
//! is built"* and *"a person can gather"*, and it is one file wide.
//!
//! ## ⛔ The bound, stated first because it decides what this can be
//!
//! **This tree has no native HTTP client.** `http_poll::FetchBinSource` is
//! `cfg(target_arch = "wasm32")`, and nothing in `Cargo.toml` brings one in —
//! the one native HTTP GET that exists is a raw-socket helper inside
//! `crossimpl_go_live.rs`'s test module, which is `http://` only and has no TLS.
//! So a CLI gather reads a **published tree on disk**, never an origin over the
//! network.
//!
//! That is a real topology and not a stand-in: several publishers share one
//! hosting scope and their trees tell them apart (AP52/AP53), so a gatherer at
//! that origin gathers a sibling with no network at all. It is *also* the
//! smaller half — gathering somebody you follow over the internet needs an HTTP
//! client this repo has decided not to acquire for a test, and acquiring one for
//! a product verb is its own decision. **Do not describe `--gather` as covering
//! the network case.**
//!
//! ## What this module does NOT re-implement
//!
//! The read is [`crate::feed_read::read_feed`], unchanged — through
//! [`SignedSession`], so a gather walks the manifest, the trie, the two-hop
//! pointer fetch and the author's detached signature exactly as a consumer does.
//! A gatherer that read the projection some other way would be republishing
//! bytes nobody verified.

// Native-only: a published tree is a directory, which is what every emitter in
// this arc is gated on.
#![cfg(not(target_arch = "wasm32"))]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;

use entity_entity::Entity;

use crate::content_site::http_poll::{poll_error_for_io, BinSource, Freshness, PollError};
use crate::content_site::signed_fetch::{PinnedPublisher, SignedFetchError, SignedSession};
use crate::feed::MirrorSubject;
use crate::feed_mirror::{plan_mirror, GatherError, MirrorPlan};
use crate::feed_read::{read_feed, FeedReadError, FeedSource, ReadEntry};

/// How many entries one gather carries.
///
/// Bounded because a gather is a *publish*: an unbounded walk of a stranger's
/// archive decides how large our own output directory is, from their side of
/// the wire. §1.3 already makes a short view a legal one.
pub const DEFAULT_GATHER_LIMIT: usize = 256;

// ---------------------------------------------------------------------------
// The source
// ---------------------------------------------------------------------------

/// A published tree on disk, served the way an origin serves one.
///
/// ⚠ **The fourth spelling of this in the crate and the first outside a test
/// module** — `crossimpl_go::OriginDir`, `signed_fetch::DirSource` and
/// `publish_fixture::FsBinSource` are the other three, each with its own
/// origin-matching rule. They are not consolidated here: this one exists
/// because a *product* verb needed it, and folding four test doubles together
/// is a change that should be made by whatever proves the boundary, not by the
/// commit that needs the first one. [`crate::feed_publish`]'s own `Origin` is
/// re-pointed at this, which is the one call site the feed path shares.
pub struct DirOrigin(pub PathBuf);

impl BinSource for DirOrigin {
    fn get(
        &self,
        url: String,
        _freshness: Freshness,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, PollError>>>> {
        let rel = url.trim_start_matches('/').to_string();
        let r = std::fs::read(self.0.join(&rel)).map_err(|e| poll_error_for_io(&rel, &e));
        Box::pin(std::future::ready(r))
    }
}

/// One author's feed, read out of a published tree through the real consumer.
///
/// ⚠ **The `Rc` and the owned `PathBuf` are the async trait doing its job** —
/// [`FeedSource::get`] returns a boxed future, which cannot borrow `self`. The
/// same constraint a `JsFuture`-backed implementation has; see `FeedSource`'s
/// own note on the synchronous trait that could never have been wired.
pub struct PublishedTree {
    base: PathBuf,
    session: Rc<SignedSession>,
}

impl PublishedTree {
    /// Pin `author` at `base` — the directory a publish wrote, i.e. `{out}` or
    /// `{out}/{prefix}`, the level that holds `{peer}/`, `content/` and
    /// `sites/`.
    ///
    /// `None` when the peer-id does not carry its own key: a canonical Ed25519
    /// peer-id embeds the public key, and one that does not (Ed448, the legacy
    /// SHA-256 form) cannot be pinned from the id alone. **That is *"we cannot
    /// check"*, never *"this is not theirs"*** — the same distinction
    /// `feed_publish::attribute` draws, and the caller reports it as its own
    /// refusal rather than as the author's defect.
    pub fn open(base: impl Into<PathBuf>, author: &str) -> Option<Self> {
        let pin = PinnedPublisher::from_peer_id("", author)?;
        Some(Self { base: base.into(), session: Rc::new(SignedSession::new(pin)) })
    }
}

impl PublishedTree {
    /// **Hop 2 on its own: a hash-addressed body out of the shared content
    /// store.**
    ///
    /// A pointer body's blob is bound at no tree key, so [`FeedSource::get`]
    /// cannot reach it and the signed root does not name it — it is reached by
    /// hash, from inside the entry that declares it, and verified against that
    /// hash. `verify_and_decode` is the check; there is no weaker path here and
    /// none is needed, because the address *is* the evidence.
    async fn content(&self, h: &entity_hash::Hash) -> Result<Option<Entity>, String> {
        // ⚠ **D24, and this one deserves more than a ratchet, because a
        // published mirror IS a durable copy of somebody else's bytes.**
        //
        // The discipline's subject is a copy that gets consulted *instead of*
        // asking the origin, so a stale one is served as current. Two things
        // take this out of that subject, and only the second is obvious:
        //
        // 1. **There is no presence check anywhere on this path.** A gather
        //    reads the source fresh on every run — `gather_timeline` has no
        //    *"do I already hold it"* branch and no way to grow one, which is
        //    the thing `ensure_current` exists to stop. D24's own words are
        //    *"`if absent` is not a trigger"*; here there is no `if`.
        // 2. **The durable copy this feeds is a PUBLICATION, not a cache.** It
        //    lands at `{out}/{author}/…` for a third party to read, carries its
        //    own `gathered_at`, and §1.3 makes a short or older view a legal
        //    one a consumer can reason about. Its currency trigger is the
        //    operator re-running `--gather` — a deliberate publish act, exactly
        //    as `--ingest-feed` is for posts.
        //
        // `ensure_current` is also the wrong door mechanically: it writes into
        // `/{me}/{foreign}/…` in our own tree, which is neither where these
        // bytes come from nor where they go.
        use crate::content_site::http_poll::{fetch_content, PollError};
        match fetch_content(&DirOrigin(self.base.clone()), "", h).await {
            Ok(e) => Ok(Some(e)),
            Err(PollError::NotFound(_)) => Ok(None),
            Err(e) => Err(format!("{e}")),
        }
    }
}

impl FeedSource for PublishedTree {
    fn get(
        &self,
        relative_key: String,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Entity>, String>>>> {
        let session = Rc::clone(&self.session);
        let base = self.base.clone();
        Box::pin(async move {
            match session.resolve(&DirOrigin(base), &relative_key).await {
                Ok(e) => Ok(Some(e)),
                Err(SignedFetchError::Absent) => Ok(None),
                Err(e) => Err(format!("{e:?}")),
            }
        })
    }
}

// ---------------------------------------------------------------------------
// The gather
// ---------------------------------------------------------------------------

/// Why a gather could not produce a plan.
#[derive(Debug, Clone, PartialEq)]
pub enum GatherFailure {
    /// The peer-id does not carry its own key, so nothing can be pinned to it.
    /// **Ours, not theirs** — see [`PublishedTree::open`].
    Unpinnable { author: String },
    /// We could not read their feed. Carries the reader's own outcome, because
    /// *no index* / *a page the head names is missing* / *we could not look* are
    /// three different reports and only one of them is about their choices.
    Unread { author: String, source: FeedReadError },
    /// Their bytes do not address to the names they arrived under, so
    /// republishing them would bind bytes under a name that does not address
    /// them. Refused at the gatherer; see [`plan_mirror`].
    Unrepublishable { author: String, source: GatherError },
}

impl std::fmt::Display for GatherFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GatherFailure::Unpinnable { author } => write!(
                f,
                "{author} is not a peer-id that carries its own key, so this tree \
                 cannot be pinned to it — we could not check, which is not the same \
                 as the tree being wrong"
            ),
            GatherFailure::Unread { author, source } => {
                write!(f, "could not read {author}'s feed: {source}")
            }
            GatherFailure::Unrepublishable { author, source } => {
                write!(f, "refusing to republish {author}'s feed: {source}")
            }
        }
    }
}

/// **Gather one author's timeline from a published tree.**
///
/// ## Timeline, not thread — and that is the verb, not a simplification
///
/// [`MirrorSubject`]'s two kinds take different inputs: a *thread* pins one
/// entity many parties contribute to, and naming it means already holding that
/// entity's hash. A *timeline* names a peer. **A CLI flag naming a peer can
/// only produce the second**, and offering the first would mean a flag whose
/// argument is a 64-hex string an operator has no way to obtain. A thread
/// gather belongs to whatever surface can show somebody a thread.
pub async fn gather_timeline(
    base: &Path,
    author: &str,
    gatherer: &str,
    limit: usize,
) -> Result<MirrorPlan, GatherFailure> {
    let Some(src) = PublishedTree::open(base, author) else {
        return Err(GatherFailure::Unpinnable { author: author.to_string() });
    };
    let rows = read_feed(&src, author, limit)
        .await
        .map_err(|source| GatherFailure::Unread { author: author.to_string(), source })?;

    // **The closure beneath the bodies, fetched before anything is planned.**
    // `read_feed` returns entries and their signatures — that is the whole of
    // what a *rendering* consumer needs. A republishing one needs the bytes a
    // pointer body names as well, and `plan_mirror` refuses without them rather
    // than publishing a post with an empty body.
    let mut closure: Vec<Entity> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = Default::default();
    for row in &rows {
        let mut queue = crate::feed_tree::body_blob_hashes(&row.entry.body);
        while let Some(h) = queue.pop() {
            if !seen.insert(h.to_hex()) {
                continue;
            }
            match src.content(&h).await {
                // Absent here is NOT skipped: `plan_mirror` will refuse, which
                // is the honest outcome. A source that cannot produce a body it
                // declares is one we cannot mirror faithfully, and carrying the
                // entry anyway is how a hollow post gets published.
                Ok(None) => {}
                Ok(Some(blob)) => {
                    if blob.entity_type == entity_types::TYPE_CONTENT_BLOB {
                        if let Ok(chunks) =
                            crate::content_site::asset_store::chunk_hashes_of(&blob)
                        {
                            queue.extend(chunks);
                        }
                    }
                    closure.push(blob);
                }
                Err(detail) => {
                    return Err(GatherFailure::Unread {
                        author: author.to_string(),
                        source: crate::feed_read::FeedReadError::EntryUnreachable {
                            entry: row.hash,
                            detail,
                        },
                    })
                }
            }
        }
    }
    plan_from_rows(gatherer, author, &rows, &closure)
}

/// The plan a gathered row set produces — split out so the decision is reachable
/// without a directory, and so the clock rule below has one expression.
pub fn plan_from_rows(
    gatherer: &str,
    author: &str,
    rows: &[ReadEntry],
    closure: &[Entity],
) -> Result<MirrorPlan, GatherFailure> {
    plan_mirror(gatherer, &MirrorSubject::timeline(author), rows, closure, gathered_clock(rows))
        .map_err(|source| GatherFailure::Unrepublishable { author: author.to_string(), source })
}

/// The instant a mirror record is stamped with.
///
/// ⚠ **`SystemTime::now()` is what the field's NAME asks for, and it would make
/// every publish of one unchanged gather produce a different record.** The
/// stamp lands in the entity, the entity lands in the trie, and the signed root
/// moves on every run — which is `EXTENSION-TREE` §3.2 determinism rule 3
/// (*"no timestamp — a snapshot is pure structural data"*), the defect
/// `system/peer/published-root`'s `published_at` already has, and the reason
/// `G-PIN-4`'s *one fixture, two publishers, identical root* comparand would be
/// unreachable for any tree carrying a mirror. It would also invalidate every
/// consumer's cached copy of a view whose content did not change.
///
/// So the publisher supplies the gathered set's own **high-water mark**: the
/// newest `created_at` it carries. It moves whenever the gathered content
/// moves, which is everything a poller needs, and never when it has not.
///
/// **This is the same choice, one convention over, that `publish_axes::
/// head_clock` already makes for §4.2's index head** — where `plan_index`'s own
/// doc still documents the parameter as *the publish instant* and the publisher
/// fills it with a witness instead. `§6` gives `gathered_at` no semantics at all
/// (three CDDL lines, no prose), so which of the two readings is meant is a
/// question for the convention and not one to settle locally: routed, and pinned
/// by a test so a ruling moves one function.
pub fn gathered_clock(rows: &[ReadEntry]) -> u64 {
    rows.iter().map(|r| r.entry.created_at).max().unwrap_or(0)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed_read::block_on;

    /// A gather reads through the **real** signed consumer: a tree published by
    /// a projector, walked by a `SignedSession`, verified against the author's
    /// own key — and every carried body is byte-identical to what was published.
    #[test]
    fn a_gather_reads_a_published_tree_through_the_signed_consumer() {
        let (dir, author) = crate::feed_publish::tests::published_dir(3);
        let plan = block_on(gather_timeline(dir.path(), &author, "2SomeGatherer", 100))
            .expect("the gather plans");

        assert_eq!(plan.entry_count(), 3);
        assert_eq!(plan.attributable(), 3, "every entry travelled with its signature");
        for c in &plan.carried {
            assert_eq!(c.peer, author, "a carried body left its author's namespace");
        }
    }

    /// ⛔ **The anti-vacuity arm: a tree whose signed root does not hold is not
    /// gathered.** Without it *"the gather read something"* is true of any
    /// implementation that reads files out of a directory, which is exactly what
    /// the `SignedSession` is there to stop.
    #[test]
    fn a_tree_whose_root_does_not_verify_is_not_gathered() {
        let (dir, author) = crate::feed_publish::tests::published_dir(2);

        // Move one byte of the signed manifest.
        let manifest = dir
            .path()
            .join(&author)
            .join(crate::content_site::signed_root::PUBLISHED_ROOT_REL);
        let mut bytes = std::fs::read(&manifest).expect("the manifest is there");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&manifest, bytes).unwrap();

        match block_on(gather_timeline(dir.path(), &author, "2SomeGatherer", 100)) {
            Err(GatherFailure::Unread { .. }) => {}
            other => panic!("a tree that does not verify was gathered: {other:?}"),
        }
    }

    /// An author we cannot pin is **our** refusal, and it is its own outcome —
    /// *we could not check* must never render as *their tree is wrong*.
    #[test]
    fn an_unpinnable_author_is_our_refusal_and_not_their_defect() {
        let dir = tempfile::tempdir().unwrap();
        match block_on(gather_timeline(dir.path(), "not-a-peer-id", "2SomeGatherer", 10)) {
            Err(GatherFailure::Unpinnable { author }) => assert_eq!(author, "not-a-peer-id"),
            other => panic!("expected our own refusal, got {other:?}"),
        }
    }

    /// **A publisher with a verifiable tree and no feed is read, and reports
    /// having no feed** — not an error about the tree. The two are different
    /// facts and a gatherer that merged them would tell an operator a healthy
    /// publisher is broken.
    #[test]
    fn a_publisher_with_no_feed_is_read_and_says_so() {
        let (dir, author) = crate::feed_publish::tests::published_dir(0);
        match block_on(gather_timeline(dir.path(), &author, "2SomeGatherer", 10)) {
            // A feed with no posts still publishes a head, so the read succeeds
            // and the plan is simply empty. That is the shape to assert: an
            // empty gather is a gather.
            Ok(plan) => assert_eq!(plan.entry_count(), 0),
            other => panic!("a publisher with an empty feed was reported as a fault: {other:?}"),
        }
    }

    /// **The stamp is the gathered set's high-water mark, not a wall clock** —
    /// so two gathers of one unchanged tree produce one record, byte for byte.
    /// See [`gathered_clock`].
    #[test]
    fn the_stamp_is_the_gathered_sets_high_water_mark_and_not_the_wall_clock() {
        let (dir, author) = crate::feed_publish::tests::published_dir(3);
        let one = block_on(gather_timeline(dir.path(), &author, "2SomeGatherer", 100)).unwrap();
        let two = block_on(gather_timeline(dir.path(), &author, "2SomeGatherer", 100)).unwrap();

        assert_eq!(
            one.record.to_entity().unwrap().content_hash,
            two.record.to_entity().unwrap().content_hash,
            "two gathers of one unchanged tree produced two records — the stamp is \
             reading a clock, and the signed root will move on every publish"
        );
        let newest = one.record.gathered_at;
        assert!(newest > 0, "an empty stamp would make the assertion above vacuous");
        assert_eq!(
            newest,
            crate::feed_publish::tests::newest_created_at(3),
            "the stamp is not the newest post's own time"
        );
    }

    /// A gather carries at most what it was asked for. §1.3 makes a short view
    /// legal, and an unbounded walk of a stranger's archive lets them decide how
    /// large our output directory is.
    #[test]
    fn a_gather_is_bounded_by_what_it_was_asked_for() {
        let (dir, author) = crate::feed_publish::tests::published_dir(5);
        let plan = block_on(gather_timeline(dir.path(), &author, "2SomeGatherer", 2)).unwrap();
        assert_eq!(plan.entry_count(), 2);
    }
}
