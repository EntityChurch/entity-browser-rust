//! **What enters a publish's projection** — the one list, and the obligations a
//! convention takes on by joining it.
//!
//! `AGENTS.md` has recorded the defect this replaces for weeks: *"what ENTERS
//! the projection is a hardcoded enumeration of two L5 conventions, not a policy
//! over the tree"* — `emit_owned_sites(sites)` followed by
//! `for set in app_sets { emit_app_set }`, spelled out inside `run_projection`,
//! with a third convention costing an edit in five places. It also recorded when
//! to fix it: ***structurally WITH the third axis, not speculatively before***,
//! because a registration table with one implementor and two special cases is a
//! table nobody can trust. The feed is that third axis.
//!
//! ## What a row buys, and what it does not
//!
//! A row is the **`.bin` projection** — read a subgraph out of the peer's tree,
//! record it into the one [`RootProjector`] whose `finish` signs the publish.
//! That is the half all three conventions share, and it is the half a fourth
//! gets for free: implement [`PublishAxis`], add a line to [`axes`], and the
//! compiler will not let you skip an obligation.
//!
//! **It is deliberately not everything a publish does**, and the residue is
//! named here rather than left for the next reader to discover:
//!
//! | still per-convention | why |
//! |---|---|
//! | the legacy-web `.html` export | **only sites have one.** A projection to HTML is a site concern by construction — `static_export` renders pages and nav, and a feed's analogue would be a different artifact answering a different question |
//! | `--bare-root` | one site at a domain root; there is no "bare-root feed" |
//! | `--plan`'s per-unit add/remove naming | the value of the plan is naming *which site* disappears, and a uniform "N units" term would be strictly worse reporting than what sites and apps have. Every axis owes a term (see [`PublishAxis::incoming`]); only the naming is bespoke |
//! | the `http_poll` URL builders | consumer-side, and the feed's already exist ([`crate::feed_fetch`]) |
//!
//! ## The clean is wholesale, so [`PublishAxis::tree_prefix`] is load-bearing
//!
//! `run_projection` removes `{base}/{peer}/` in one `remove_dir_all`. Anything
//! under a peer's prefix is therefore **in scope by construction** — a publish
//! that forgets to re-emit an axis does not leave it alone, it deletes it. That
//! is what made the apps blind spot expensive (*"plan a content fix, forget
//! `--ingest-apps`, publish"* reported *"nothing would be removed"* and then
//! deleted every app bundle on the domain), and it is why a row states its
//! prefix: the prefix is what the accounting is *about*.

// Native-only: every emitter below writes a directory, which is what
// `RootProjector`, `publish_fixture` and `feed_publish` are all gated on.
#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use crate::content_site::read::OwnedSite;
use crate::content_site::signed_root::RootProjector;
use crate::content_site::{paths, publish_fixture};
use crate::feed_tree::OwnedFeed;

/// One L5 convention's `.bin` half of a publish.
///
/// Implement it, add a line to [`axes`], and the four obligations below are
/// enforced by the compiler rather than by whoever remembers to grep
/// `run_projection`.
pub trait PublishAxis {
    /// What an operator sees this called in a report or a refusal.
    fn name(&self) -> &'static str;

    /// The **peer-relative** tree prefix this convention owns — what the
    /// wholesale clean removes, and therefore what a publish carrying none of
    /// this axis silently deletes. See the module doc.
    fn tree_prefix(&self) -> &'static str;

    /// How many units this publish carries: sites, app bundles, posts. Zero
    /// means the axis contributes nothing — which is a real answer and not an
    /// error, since most publishers use one convention.
    fn incoming(&self) -> usize;

    /// Record every entity into `root`. Returns a report line when there is
    /// something to say.
    ///
    /// **`out_dir` is the un-prefixed output root**, because that is what two of
    /// the three emitters take; an implementor whose emitter wants the prefixed
    /// base joins it itself ([`paths::prefixed_root`]). Getting that backwards
    /// publishes a whole axis *outside* the hosting scope it belongs to, where
    /// no consumer resolving `{origin}/{prefix}/…` will ever look — gated by
    /// `a_prefixed_publish_puts_every_axis_inside_the_prefix`.
    fn project(
        &self,
        out_dir: &Path,
        peer_id: &str,
        prefix: &str,
        root: &mut RootProjector,
    ) -> Result<Option<String>, String>;
}

/// **The list.** A fourth convention is a line here and an `impl` above.
///
/// Order is report order, not a dependency: the root is a trie over every
/// binding and does not care which arrived first.
pub fn axes<'a>(
    peer_id: &'a str,
    sites: &'a [OwnedSite],
    app_sets: &'a crate::apps::ingest::IngestedSets,
    feed: Option<&'a OwnedFeed>,
) -> Vec<Box<dyn PublishAxis + 'a>> {
    vec![
        Box::new(SiteAxis { sites }),
        Box::new(AppsAxis { peer_id, sets: app_sets }),
        Box::new(FeedAxis { feed }),
    ]
}

// ---------------------------------------------------------------------------
// sites — `APP-CONVENTION-SEMANTIC-CONTENT-SITE`
// ---------------------------------------------------------------------------

pub struct SiteAxis<'a> {
    pub sites: &'a [OwnedSite],
}

impl PublishAxis for SiteAxis<'_> {
    fn name(&self) -> &'static str {
        "sites"
    }
    fn tree_prefix(&self) -> &'static str {
        "sites/"
    }
    fn incoming(&self) -> usize {
        self.sites.len()
    }
    fn project(
        &self,
        out_dir: &Path,
        _peer_id: &str,
        prefix: &str,
        root: &mut RootProjector,
    ) -> Result<Option<String>, String> {
        publish_fixture::emit_owned_sites(out_dir, self.sites, prefix, Some(root))
            .map_err(|e| e.to_string())?;
        // Sites are reported by `report_projection`, which names each one and
        // its page count — richer than a line here could be, and it has to run
        // on the `--html-only` path too where this axis never executes.
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// apps — the app-set subgraph
// ---------------------------------------------------------------------------

pub struct AppsAxis<'a> {
    pub peer_id: &'a str,
    pub sets: &'a crate::apps::ingest::IngestedSets,
}

impl PublishAxis for AppsAxis<'_> {
    fn name(&self) -> &'static str {
        "apps"
    }
    fn tree_prefix(&self) -> &'static str {
        "apps/"
    }
    fn incoming(&self) -> usize {
        self.sets.values().map(|i| i.catalog.entries.len()).sum()
    }
    fn project(
        &self,
        out_dir: &Path,
        peer_id: &str,
        prefix: &str,
        root: &mut RootProjector,
    ) -> Result<Option<String>, String> {
        let mut lines = Vec::new();
        for (set, ing) in self.sets {
            if ing.catalog.entries.is_empty() {
                continue;
            }
            let n = publish_fixture::emit_app_set(
                out_dir,
                peer_id,
                set,
                &ing.catalog,
                &ing.bundles,
                prefix,
                Some(root),
            )
            .map_err(|e| format!("app-set '{set}': {e}"))?;
            lines.push(format!("apps[{set}]: {n} bundle(s) → {peer_id}/apps/{set}/"));
        }
        Ok((!lines.is_empty()).then(|| lines.join("\n  ")))
    }
}

// ---------------------------------------------------------------------------
// feed — `APP-CONVENTION-FEED`
// ---------------------------------------------------------------------------

pub struct FeedAxis<'a> {
    pub feed: Option<&'a OwnedFeed>,
}

impl PublishAxis for FeedAxis<'_> {
    fn name(&self) -> &'static str {
        "feed"
    }
    fn tree_prefix(&self) -> &'static str {
        // §4.2's pinned index and, beneath it, the pages. Our entry keys
        // (`app/feed/entries/`) sit alongside; both are under `app/feed/`.
        "app/feed/"
    }
    fn incoming(&self) -> usize {
        self.feed.map_or(0, |f| f.entries.len())
    }
    fn project(
        &self,
        out_dir: &Path,
        _peer_id: &str,
        prefix: &str,
        root: &mut RootProjector,
    ) -> Result<Option<String>, String> {
        let Some(feed) = self.feed else { return Ok(None) };
        // **The prefixed base, because `publish_feed` writes `{dir}/{peer}/…`
        // directly** — unlike the two emitters above it takes no `prefix`. A
        // feed written to the un-prefixed root would sit outside the hosting
        // scope its own signed root is served from.
        let base = paths::prefixed_root(out_dir, prefix);
        let report = crate::feed_publish::publish_feed(
            &base,
            root,
            &feed.entries,
            &feed.content,
            crate::feed_publish::DEFAULT_PAGE_SIZE,
            head_clock(feed),
        )?;
        Ok(Some(format!(
            "feed: {} post(s) over {} page(s) → {}/app/feed/",
            report.entry_count, report.page_count, feed.peer_id
        )))
    }
}

/// The instant the feed's §4.2 head is stamped with.
///
/// ⚠ **`SystemTime::now()` is the obvious answer and it would make every publish
/// of this tree produce a different signed root.** `plan_index` gives the head
/// the *publish* instant deliberately — it is the one small mutable pointer a
/// reader polls, and a page's stamp is a witness of its own entries so that an
/// untouched page keeps its bytes (`A-41`). But **who supplies that instant is
/// the caller's decision**, and a wall clock is what turns a projection into
/// something that is not a function of the tree it read: the head lands in the
/// trie, so the site root moves on every run, and `G-PIN-4`'s whole comparand —
/// *one fixture, two publishers, identical site root* — becomes unreachable for
/// any tree carrying a feed. It is exactly the defect
/// `system/peer/published-root`'s `published_at` already has, and which
/// `EXTENSION-TREE` §3.2's determinism rule 3 states as *"no timestamp — a
/// snapshot is pure structural data."*
///
/// So the publisher hands it the feed's own **high-water mark**: the newest
/// authored `created_at`. It still moves whenever the feed changes, which is
/// everything a poller needs, and it never moves when it has not.
fn head_clock(feed: &OwnedFeed) -> u64 {
    feed.entries.iter().map(|e| e.created_at).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A publish's axes are enumerated in one place, and every one of them
    /// states the tree prefix the wholesale clean removes.** The census is the
    /// point: the clean deletes `{base}/{peer}/` entire, so an axis nobody
    /// listed is not an axis that is left alone — it is one that is silently
    /// deleted the first time somebody publishes without it.
    #[test]
    fn every_axis_names_the_tree_prefix_the_clean_would_remove() {
        let sets = crate::apps::ingest::IngestedSets::new();
        let all = axes("QmPeer", &[], &sets, None);
        let rows: Vec<(&str, &str)> =
            all.iter().map(|a| (a.name(), a.tree_prefix())).collect();
        assert_eq!(
            rows,
            vec![("sites", "sites/"), ("apps", "apps/"), ("feed", "app/feed/")],
            "a fourth L5 convention is a row in `axes` — if this fails because you added \
             one, add it here and check `run_plan` gives it a term"
        );
        // Nothing to publish is not an error on any axis; most publishers use
        // one convention.
        assert!(all.iter().all(|a| a.incoming() == 0));
    }

    /// **The head's clock is the feed's own high-water mark, not the wall
    /// clock** — so two publishes of one unchanged tree produce one root. See
    /// [`head_clock`].
    #[test]
    fn the_feeds_head_is_stamped_from_its_entries_and_not_from_the_wall_clock() {
        use crate::embed::{EmbedData, EmbedNode, EmbedPayload};
        use crate::feed::FeedEntry;

        let node = |s: &str| {
            EmbedNode::new(
                "text/plain",
                EmbedData::new(EmbedPayload::Inline(s.as_bytes().to_vec()), s),
            )
        };
        let feed = OwnedFeed {
            peer_id: "QmPeer".into(),
            entries: vec![
                FeedEntry::new("QmPeer", 1_000, node("first")),
                // Out of order on purpose: `max`, not `last`. §4.5 makes the
                // order authored, so a backdated post at the end must not drag
                // the high-water mark backwards.
                FeedEntry::new("QmPeer", 9_000, node("newest")),
                FeedEntry::new("QmPeer", 2_000, node("backdated")),
            ],
            content: Vec::new(),
        };
        assert_eq!(head_clock(&feed), 9_000);

        let empty =
            OwnedFeed { peer_id: "QmPeer".into(), entries: Vec::new(), content: Vec::new() };
        assert_eq!(head_clock(&empty), 0, "and it never reaches for a clock it does not have");
    }
}
