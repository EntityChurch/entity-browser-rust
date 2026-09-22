//! **Corridor ① — a feed `entity-workbench-go` published, walked by our
//! reader**, with no shared code on the path.
//!
//! The first run in this ecosystem pointing a real publisher at a real reader.
//! `J-4` compared two **encoders** over authored input, which is a different
//! claim: per `[ADR-0012]`, two producers agreeing is not evidence that a
//! consumer is right.
//!
//! ## The fixture is a real Go emission, vendored, and it is CUT TWICE
//!
//! `tests/fixtures/crossimpl-go-feed/` is the byte-for-byte output of
//! workbench-go's `publish/cmd/crossimpl-feed` at workbench-go `779616d`.
//! Deterministic — a pinned seed, a pinned publish instant, and a pinned
//! per-entry `created_at` (entry *n* is epoch + *n* minutes), which matters
//! more here than for the site fixture: an entry's `created_at` is inside its
//! hashed bytes, so an unpinned clock moves all 34 entry hashes **and** all 34
//! signature paths. Re-cut:
//!
//! ```text
//! cd ../entity-workbench-go && go run ./publish/cmd/crossimpl-feed -out /tmp/corridor
//! ```
//!
//! | cut | declared prefix | bindings | entities | entry signatures committed |
//! |---|---|---|---|---|
//! | `peer-root/` | `/` | 438 | 459 | **34 / 34** |
//! | `feed-only/` | `app/feed/` | 37 | 42 | **0 / 34** |
//!
//! ⚠ **Two rules to check, not a recommended shape and a deprecated one.** The
//! pair is `FEED-14` from arch's `A-36` proposal §4.5. `peer-root` is the
//! `A-36` fix measured, and measuring it is what produced their `A-38` — the
//! same widening that commits the signatures also commits that peer's device
//! declarations and local folder paths. **A single-prefix fixture passes
//! against both rules and measures neither**, which is the same observation we
//! made about single-*page* feed fixtures one axis over.
//!
//! **34 entries against a 32-entry page size** is the `B-13` shape, so the
//! oldest entry lives on a page the head does not name and `FEED-R12` is
//! falsifiable — it had been unfalsifiable on both seats simultaneously, because
//! every feed fixture either seat had ever cut was one page.
//!
//! ## ⭐ What building this found, and neither defect was visible from inside
//!
//! Both are in [`crate::feed_gather::PublishedTree`], the source `publish
//! --gather` uses, and **both blocked the walk outright** — this is the first
//! time anything in this tree read a feed somebody else published.
//!
//! 1. **The layout was derived, not read.** We pinned
//!    `{peer}/system/peer/published-root`; they serve `{out}/manifest`. Hop 0,
//!    for every conformant publisher whose layout is not ours.
//! 2. **§4.2's pinned address was sent verbatim to a prefix-relative resolve.**
//!    The narrow cut commits `index`, we asked for `app/feed/index`. This is the
//!    reader trap `entity-workbench-go` filed and arch took into
//!    `GUIDE-APPLICATION-DEVELOPMENT`, arriving on our side within the week.
//!
//! **No gate here could have seen either**, and the reason is one sentence:
//! every `--gather` fixture in this tree is a directory *our own publisher
//! wrote*, so the layout the gatherer assumed and the layout the fixture used
//! were the same expression, and our emitter has never declared a narrow
//! prefix. *A test population you generated cannot contain the shape you are
//! missing* — where here the missing shape is **another implementation**.
//!
//! ## ⭐ `B-14` — report which path answered, not the answer
//!
//! Their ask, and the finding that earned it: reading their own fixture back,
//! their reader returned **all 34 entries, correctly attributed, with no error —
//! and every index lookup had missed.** §4.3 rule 6's enumeration fallback did
//! exactly what the convention requires and hid a completely broken primary
//! path. *A conformance fallback built to survive a hostile publisher will
//! equally survive your own broken primary.* No assertion phrased over the
//! **answer** can fail.
//!
//! See [`the_index_is_what_answered_and_no_fallback_could_have_rescued_it`] for
//! our number and why it is structural rather than lucky.
#![cfg(all(test, not(target_arch = "wasm32")))]

use std::path::PathBuf;

use crate::feed_gather::PublishedTree;
use crate::feed_read::{block_on, read_feed_from, FeedSource, FeedWindow, Resumed, Unattributed};

/// The fixture's publisher, pinned by their `-seed`. **Its own seed, distinct
/// from their site fixture's**, so the two are two peers: a reader consuming
/// both against one peer-id could not tell a namespace rule from a coincidence,
/// and `FEED-R1` is exactly a namespace rule.
const PEER_ID: &str = "2KAoCfAP6ZZyLmS9wYz4rUmpehd4JMeek32NLN58R3ehpi";

/// Authored entry count, deliberately larger than their pinned 32-entry page
/// size.
const ENTRIES: usize = 34;

fn cut(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/crossimpl-go-feed")
        .join(name)
}

fn open(name: &str) -> PublishedTree {
    PublishedTree::open(cut(name), PEER_ID)
        .expect("their fixture peer-id is Ed25519 canonical, so it carries its own key")
}

/// Read one cut through the shipped consumer — [`PublishedTree`] is the source
/// `publish --gather` uses, not a double built for this file. That is the whole
/// point: a double would have supplied the layout we forgot to ask for.
fn walk(name: &str) -> FeedWindow {
    block_on(read_feed_from(&open(name), PEER_ID, 100, None))
        .unwrap_or_else(|e| panic!("{name}: their published feed did not read back: {e:?}"))
}

/// Each entry's authored text, which their emitter numbers `0..33` in publish
/// order. Read off the `fallback`, which `EMBED` §3 makes mandatory and
/// non-empty, so it is the one field guaranteed to be there whatever arm the
/// payload took.
fn bodies(w: &FeedWindow) -> Vec<String> {
    w.entries.iter().map(|e| e.entry.body.data.fallback.clone()).collect()
}

fn attributed(w: &FeedWindow) -> usize {
    w.entries.iter().filter(|e| e.attribution.may_name_the_author()).count()
}

// ---------------------------------------------------------------------------
// `FEED-14`, positive arm
// ---------------------------------------------------------------------------

/// **The corridor.** Their publisher, our consumer: the manifest decodes, the
/// root verifies against the key their peer-id carries, the trie walks, §4.2's
/// index leads to both pages, every entry's two-hop body arrives, and
/// `FEED-R4`'s verdict lands on each one separately.
#[test]
fn a_feed_the_other_seat_published_reads_back_whole_and_attributed() {
    let w = walk("peer-root");

    assert_eq!(w.entries.len(), ENTRIES, "every authored entry came back");
    assert_eq!(
        attributed(&w),
        ENTRIES,
        "`FEED-14` positive arm: the signatures are inside the committed set, so \
         every entry names its author. Unattributed: {:?}",
        w.entries
            .iter()
            .filter(|e| !e.attribution.may_name_the_author())
            .map(|e| (e.hash.to_hex(), e.attribution.clone()))
            .collect::<Vec<_>>()
    );
}

/// **`FEED-R12` — the walk crosses a page boundary**, which is what 34-against-32
/// was chosen for. The oldest entry is on a page the head's `current` does not
/// name, so a reader that stopped at the named page returns 2 and passes every
/// count-free assertion.
///
/// Ordering is asserted at **both ends and in full**: §4.5 makes the order
/// authored rather than derived, so a reader that sorted by `created_at` would
/// agree with this fixture by luck and disagree with a backdated post.
#[test]
fn the_walk_reaches_both_ends_of_an_archive_that_does_not_fit_on_one_page() {
    let got = bodies(&walk("peer-root"));

    let expected: Vec<String> = (0..ENTRIES)
        .rev()
        .map(|n| format!("corridor entry {n} — authored by the Go arm for the joint read"))
        .collect();
    assert_eq!(got, expected, "newest-first, whole, across both index pages");
}

// ---------------------------------------------------------------------------
// `FEED-14`, negative arm
// ---------------------------------------------------------------------------

/// **`FEED-14`'s negative arm, and the reason the fixture is cut twice.** The
/// same 34 entries published over `app/feed/`, which **excludes**
/// `system/signature/`. A static reader with no second channel must reach every
/// entry and be able to name nobody for any of them.
///
/// ⚠ **The refusal is `NoSignature` specifically**, not merely *not attributed*.
/// `FEED-R4`'s named case is *the author never signed*; a reader that reported
/// these as `BadSignature` or `SignerIsNotTheAuthor` would be making a statement
/// about the author to cover a **publisher's** scoping choice — which is the
/// `A-36` argument for the rule in the first place: with obligation 3
/// unspecified, *absent* has two causes and a conformant reader publishes a
/// falsehood manufactured by a third party.
#[test]
fn a_feed_published_over_a_prefix_that_excludes_the_signatures_can_name_nobody() {
    let w = walk("feed-only");

    assert_eq!(
        w.entries.len(),
        ENTRIES,
        "the entries ARE in this cut — only their signatures are outside it"
    );
    assert_eq!(bodies(&w), bodies(&walk("peer-root")), "same feed, same order");
    assert_eq!(attributed(&w), 0, "`FEED-14` negative arm");

    for e in &w.entries {
        assert!(
            matches!(
                e.attribution,
                crate::feed_read::Attribution::Unattributed(Unattributed::NoSignature)
            ),
            "{}: a signature the PUBLISHER did not commit must read as unsigned, \
             never as a fault of the author's — got {:?}",
            e.hash.to_hex(),
            e.attribution
        );
    }
}

// ---------------------------------------------------------------------------
// `B-14`
// ---------------------------------------------------------------------------

/// **`B-14`: which path produced the answer.**
///
/// Their number was `0` by index and `34` by enumeration, with every
/// caller-visible field correct. Ours is **34 by index and 0 by enumeration on
/// both cuts** — and the `0` is *structural*, which is a stronger statement than
/// *"it did not happen"*:
///
/// - [`Resumed`] is the per-walk witness. Anything other than
///   [`Resumed::Unpositioned`] means §4.2's index answered, because that variant
///   is the only thing `read_feed_from` returns when it takes §4.3 rule 6's
///   branch — there are no pages on that path, so there is no `{page, applied}`
///   to report.
/// - [`FeedSource::list`] **defaults to `Ok(None)`** — *this source cannot
///   enumerate* — and [`PublishedTree`] does not override it. So on the
///   published leg the fallback is not merely unused, it is unavailable: an
///   index miss is a loud [`crate::feed_read::FeedReadError::NoIndex`] and can
///   never be a quiet success.
///
/// ⭐ **That is not a virtue we designed and it is worth saying so.** It is the
/// consequence of `SignedSession::enumerate` not being wired to this trait yet,
/// and `feed_read`'s own doc records it as a gap. It does mean this seat cannot
/// have had their defect **on this leg** — and `feed_peer`, the live one, *does*
/// implement `list`, so it can. The assertion is written here rather than in a
/// comment so the day somebody wires enumeration in, the claim is re-measured
/// rather than inherited.
#[test]
fn the_index_is_what_answered_and_no_fallback_could_have_rescued_it() {
    for name in ["peer-root", "feed-only"] {
        let w = walk(name);
        assert_ne!(
            w.resumed,
            Resumed::Unpositioned,
            "{name}: §4.2's index answered, not §4.3 rule 6's enumeration"
        );
        assert_eq!(
            w.entries.len(),
            ENTRIES,
            "{name}: {ENTRIES} via the index, 0 via enumeration"
        );

        let src = open(name);
        assert_eq!(
            block_on(src.list(crate::feed::entry_prefix().to_string())),
            Ok(None),
            "{name}: the published leg cannot enumerate at all, so a broken index \
             cannot be masked by a fallback here"
        );
    }
}

