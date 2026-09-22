//! Renderer-neutral output for the Content Site window.
//!
//! The model builds this (the page body already rendered into a typed
//! [`PageRender`]); the DOM renderer (`dom/content_site.rs`) mounts it and
//! rewrites the entity-native `<a>` links into nav handlers. Carries enough of
//! the current location (`peer` / `site_id` / `current_page`) for the renderer
//! to classify in-page links relative to where we are.

use crate::content_site::PageRender;

/// One nav-menu entry, with whether it points at the current page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NavLink {
    pub label: String,
    /// Raw entity-native link target (re-classified on click).
    pub target: String,
    pub active: bool,
}

/// One entry in the tree-driven section sidebar. Derived from the live
/// page tree (`.list`), so it reflects the site's actual structure even
/// when the manifest nav is flat. The sidebar shows the top-level entries
/// and expands the active section one level (its child pages).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionLink {
    pub label: String,
    /// Raw nav target (re-classified on click).
    pub target: String,
    /// On the current page, or (for a section header) on its trail.
    pub active: bool,
    /// 0 = top-level, 1 = a child of the open section (indent depth).
    pub depth: u8,
    /// A section (has children) vs a leaf page — a render hint.
    pub is_section: bool,
}

/// One step in the breadcrumb trail to the current page. Derived from the
/// current page slug + the manifest (the format already carries
/// everything needed — purely presentation, no format change).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Crumb {
    pub label: String,
    /// `Some` = a clickable nav target (raw link string, re-classified on
    /// click); `None` = a plain label — the current page ("you are here")
    /// or an intermediate path segment with no known page.
    pub target: Option<String>,
}

/// One row in the site-aware window's **directory rail** — a site my store
/// holds, owned or cached, with its provenance + the preferences that drive
/// the row's affordances. Assembled by the model from the derived site index
/// ([`crate::content_site::discovery::read_site_index`]) + the provenance
/// ledger ([`crate::content_site::cache::read_provenance`]) + the app-tier
/// preferences ([`crate::content_site::prefs::read_prefs`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteEntry {
    /// The owning peer (always concrete — the path partition; for an owned
    /// site this is my own id).
    pub peer: String,
    pub site: String,
    /// Derived from `peer == me` (the path's peer-segment is the signal).
    pub owned: bool,
    /// This row is the window's current location (highlight it).
    pub is_current: bool,
    /// Pinned to the top of the list by the user.
    pub bookmarked: bool,
    /// "Keep offline" — full page-body caching for this site (O3). Off =
    /// manifest-pinned (the default): structure persists, pages re-fetch.
    /// Only meaningful for cached foreign sites (owned sites are always local).
    pub keep_offline: bool,
    /// How many times this window has opened the site (recency hint).
    pub visit_count: u64,
    /// SDK-tier provenance (cached sites only; `0`/empty for owned). Wall-clock
    /// ms the cache was last verified-fresh, and the origin it was fetched from.
    pub last_reconciled: u64,
    pub source_transport: String,
    /// **The `GUIDE-SERVING-MODE` §8 verification state**, which is a different
    /// question from `last_reconciled` and must not be derived from it:
    ///
    /// - `None` — *never checked*. No signature has been verified for this site.
    ///   **This is every foreign site today**, because the Site Browser's fetch
    ///   path reads no signed root: its two-hop check proves a body matches the
    ///   pointer the same origin served, which is real against corruption and
    ///   says nothing about authorship.
    /// - `Some(0)` — *verification failed*. The hostile-origin signal: bad
    ///   signature, `seq` rollback, or an incomplete closure walk.
    /// - `Some(published_at_ms)` — *verified*, and the timestamp is the
    ///   **publisher's** `published_at`, never our fetch time. Rendering it is
    ///   mandatory, not decorative (§8's third row).
    ///
    /// `last_reconciled` answers *"when did WE last fetch"* — a fact about us. A
    /// surface that showed it as freshness would be claiming a bound on the
    /// content's age that it does not have, which §8 names and forbids.
    pub verified_at: Option<u64>,
}

/// The site-aware window's directory: every site this peer holds, bookmarked
/// first then owned, each side alphabetical — a stable, scannable order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteDirectory {
    pub entries: Vec<SiteEntry>,
    /// The active view filter (drives the rail's My/All/External control).
    pub filter: RailFilter,
}

/// Which subset of the directory the rail shows. A session-only view filter
/// over the same assembled entries — `All` (default) is the historical
/// behaviour (owned + cached together), `Mine` narrows to sites I own, and
/// `External` to cached foreign sites.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RailFilter {
    #[default]
    All,
    Mine,
    External,
}

impl RailFilter {
    /// Stable wire token (the `SiteRailFilter` action value).
    pub fn as_str(self) -> &'static str {
        match self {
            RailFilter::All => "all",
            RailFilter::Mine => "mine",
            RailFilter::External => "external",
        }
    }

    /// Parse the wire token; anything unrecognised falls back to `All`.
    pub fn parse(s: &str) -> Self {
        match s {
            "mine" => RailFilter::Mine,
            "external" => RailFilter::External,
            _ => RailFilter::All,
        }
    }

    /// Does an entry (owned or not) pass this filter?
    pub fn keeps(self, owned: bool) -> bool {
        match self {
            RailFilter::All => true,
            RailFilter::Mine => owned,
            RailFilter::External => !owned,
        }
    }
}

/// Everything the renderer needs for one frame of the site view.
///
/// `PartialEq`/`Eq` back the overlay's rebuild guard — the Site Mode
/// overlay re-renders only when this value changes between frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteRenderOutput {
    pub site_title: String,
    pub nav: Vec<NavLink>,
    /// Breadcrumb trail to the current page (root → … → here). Empty on
    /// the site's root page (no trail to show).
    pub breadcrumbs: Vec<Crumb>,
    /// Tree-driven section sidebar (from `.list`). Empty for a flat site
    /// or when listing is unavailable (a remote HTTP site — finding #4),
    /// in which case the renderer keeps the simple single-pane layout.
    pub sidebar: Vec<SectionLink>,
    /// True when there's a previous location to return to (the back
    /// affordance shows only then). Session-scoped — back-history is
    /// in-memory and does not survive a reload.
    pub can_go_back: bool,
    pub page_title: String,
    /// The rendered page body **and how it must be mounted**
    /// ([`PageRender`]). `Markup` is sanitized markdown output — the renderer
    /// `set_inner_html`s it and rewrites `<a>` hrefs and `<img>` srcs.
    /// `Document` is an untrusted `format:html` body that must go to the
    /// sandboxed document frame instead; the rewriters do not run on it (they
    /// cannot reach into an opaque origin, and its links are its own).
    pub body: PageRender,

    // -- current location, for relative link classification --
    pub peer: Option<String>,
    pub site_id: String,
    pub current_page: String,

    /// Set when the current location couldn't be resolved (missing
    /// manifest/page, or a transport not yet wired). Rendered in place
    /// of the page body.
    pub error: Option<String>,
    /// True while an async transport is fetching (P4 HTTP-poll); local
    /// resolution never sets this.
    pub loading: bool,

    /// A link string that navigates to the deployment's **configured home
    /// site** (`home_site`), independent of the *current* location — a
    /// `site:{id}/` (local home) or `entity://{peer}/sites/{id}/` (foreign
    /// home). The overlay's Home button wires to this so it always resets to
    /// the real site even when the current location is unresolvable (the
    /// "stranded on `No site manifest` with no way home" bug). Empty ⇒ the
    /// renderer falls back to `/` (the current site's root). Not part of the
    /// rebuild-guard concern — it's constant for the overlay's lifetime, but
    /// carried on the output so the host-agnostic renderer can reach it.
    pub home_target: String,

    /// The site's **manifest-declared theme**, resolved to container-scoped
    /// `--site-*` declarations (S-T2, DESIGN-MANIFEST-SITE-THEME). `Some`
    /// only when the manifest names a *registered* theme AND the effective
    /// "Site appearance" mode is `"site"` — the mode gate lives here, not in
    /// the renderer, ON PURPOSE: this field is part of the overlay's
    /// rebuild-guard equality, so a Settings mode flip while a themed site
    /// is open changes the output and forces the rebuild that removes the
    /// container vars. (Container properties override inherited `:root`
    /// values — gated renderer-side only, a strict "Always X" override would
    /// be silently defeated by stale vars.) The renderer appends it to the
    /// site wrapper's inline style verbatim; values come exclusively from
    /// the app's own theme table, never from site-supplied bytes.
    pub site_theme_css: Option<String>,
}

// ---------------------------------------------------------------------------
// Why a site would not resolve — the message-selection rule.
// ---------------------------------------------------------------------------

/// Why we could not produce a page for a location whose **manifest** is
/// missing, at the granularity a reader can act on.
///
/// This exists because `ResolveError::ManifestMissing` collapses two very
/// different situations into one sentence, and the sentence it produced —
/// *"No site manifest at 'X' (peer: Y)"* — reads as **"that site does not
/// exist"** in both. Measured against production on 2026-08-24: following a
/// shared link to a site published by *another* peer reports exactly that,
/// while the site is live and perfectly reachable at its own domain. The
/// reader is told the content is missing when what is missing is *this
/// browser's knowledge of where that peer is hosted*.
///
/// The discriminator is **which origin we looked at**, and it is worth
/// spelling out because it is the whole diagnosis: a `?site=` deep link for a
/// foreign peer seeds a *same-origin* entry for that peer at boot
/// (`app.rs::boot_load`, the static→live round-trip case), so the resolver
/// dutifully fetches `{this origin}/{their peer}/sites/…`, gets a 404, and
/// reports a missing manifest. Naming the origin turns an apparently-broken
/// site into an obviously-wrong lookup.
///
/// **We deliberately do NOT mention this tab's storage durability here.** A
/// secondary tab is ephemeral and therefore holds no origins it learned
/// earlier, which is a real contributing factor — and it is *not* the cause:
/// a fresh durable profile following the same link fails identically
/// (measured, both arms). The multi-tab situation already has its own honest
/// banner; asserting it as the cause of *this* failure would be the same
/// mistake this type exists to fix. Grade by the measured consequence, never
/// by an asserted one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingSite<'a> {
    /// The site should have been in our own tree. The original message is
    /// correct here: nothing was published under this peer at that id.
    Local,
    /// Published by another peer, and we looked for it at `origin` — which
    /// answered, and does not carry it. Almost always "that peer lives on a
    /// different domain".
    Foreign { peer: &'a str, origin: &'a str },
    /// Published by another peer we hold no route for at all. The resolver
    /// normally reports this as `Unreachable` before a manifest read is even
    /// attempted; kept so the mapping is total rather than defaulting a
    /// genuinely-unknown host into a claim about an origin.
    ForeignUnknownHost { peer: &'a str },
}

/// Classify a missing-manifest failure.
///
/// `loc_peer` is the peer named by the location (`None` ⇒ our own tree), and
/// `origin` is whatever the site-origin registry holds for that peer *from
/// our vantage*. Pure — no `Peers`, no DOM — so the rule is reachable from
/// `make test`, which the renderer that consumes it is not (wasm-only).
pub fn classify_missing_site<'a>(
    our_peer: &str,
    loc_peer: Option<&'a str>,
    origin: Option<&'a str>,
) -> MissingSite<'a> {
    match loc_peer {
        // No peer named, or it is us: our own tree, our own missing site.
        None => MissingSite::Local,
        Some(p) if p == our_peer => MissingSite::Local,
        Some(p) => match origin {
            // An empty origin string is the same-origin sentinel used
            // throughout the origins registry; it is still an origin we
            // looked at, and the caller renders it as the current location.
            Some(o) => MissingSite::Foreign { peer: p, origin: o },
            None => MissingSite::ForeignUnknownHost { peer: p },
        },
    }
}

// ---------------------------------------------------------------------------
// Why a site is ABSENT — the three-way distinction, and the one bit of context
// that makes the advice true.
// ---------------------------------------------------------------------------

/// Everything the reports need to know about a site that would not resolve.
///
/// [`MissingSite`] above answers *which origin did we look at*. It cannot answer
/// **what happened when we looked**, because until now nothing carried that:
/// `ResolveError::ManifestMissing` was returned whether the origin answered
/// "not here", answered nothing, or was never asked. This type is the second
/// axis, and it exists because the surface was measurably wrong on both counts:
///
///   * a returning visitor to a **withdrawn** site was told *"This site's source
///     is unreachable. Showing its cached outline."* The source answered
///     perfectly; the publisher withdrew the site. (Gate cell #17.)
///   * a first-time visitor was told the site *"belongs to another peer and is
///     probably hosted on its own domain — open it there, or find it in the
///     Registry Browser"* — about **this deployment's own publisher, on the
///     origin they were already looking at**. That sentence is correct for the
///     case it was written for (a `?site=` deep link that seeded a same-origin
///     entry for a genuinely foreign peer) and is reached by a case it was not.
///     AP33, exactly.
///
/// **The discriminator for the advice is `home`, and it is derived rather than
/// stored.** The change map proposed a provenance bit — *did this home come from
/// a deployment document* — and a durable `home_site` schema change to carry it.
/// It is not needed and it is the weaker fact: what makes the Registry Browser
/// advice absurd is not who configured the home, it is that the peer being
/// described is the one this deployment is built around and whose origin the
/// reader is already on. Both halves of that are readable now: the configured
/// home site, and whether the registered origin is the same-origin sentinel.
/// A user who chose that home themselves is owed the same true sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteAbsence<'a> {
    /// Our own tree, our own missing site. Unchanged.
    Local,
    /// The peer named here was **superseded** and no longer publishes. Persisted
    /// navigation resolves a retired peer to its successor at its decode point
    /// (`ContentSiteState::from_entity`), so this is reachable only from a
    /// *fresh* reference — a link, a `?site=` deep link, or a deployment
    /// document that still names the retired identity. Those paths never had a
    /// report for it at all.
    Retired { peer: &'a str, successor: &'a str },
    /// **The origin answered and does not carry this site.** A fact about the
    /// publisher. `home` = this is the site the deployment is built around, so
    /// "go find it on its own domain" is not advice we may give.
    Withdrawn { peer: &'a str, origin: &'a str, home: bool },
    /// **We asked and heard nothing usable.** A fact about the network. Never
    /// evidence about what the publisher carries — which is why this may not
    /// borrow the withdrawn sentence, in either direction.
    Unreachable { peer: &'a str, origin: &'a str },
    /// No route for this peer at all — nothing to ask.
    NoRoute { peer: &'a str },
}

/// Classify an absent site: *which origin* (via [`classify_missing_site`]) plus
/// *what happened when we asked* plus *is this the deployment's own home*.
///
/// Pure — no `Peers`, no DOM, no globals — so every branch is reachable from
/// `make test`, which the renderer that consumes it is not (wasm-only).
///
/// `retired_to` is the successor if the location's peer was superseded, and
/// **it wins over everything else**: if the publisher was replaced, "the origin
/// does not carry this site" is true and useless — of course it does not, it is
/// not that publisher any more.
pub fn classify_site_absence<'a>(
    our_peer: &str,
    loc_peer: Option<&'a str>,
    origin: Option<&'a str>,
    answered: bool,
    home: bool,
    retired_to: Option<&'a str>,
) -> SiteAbsence<'a> {
    match classify_missing_site(our_peer, loc_peer, origin) {
        MissingSite::Local => SiteAbsence::Local,
        MissingSite::ForeignUnknownHost { peer } => match retired_to {
            Some(successor) => SiteAbsence::Retired { peer, successor },
            None => SiteAbsence::NoRoute { peer },
        },
        MissingSite::Foreign { peer, origin } => match retired_to {
            Some(successor) => SiteAbsence::Retired { peer, successor },
            None if answered => SiteAbsence::Withdrawn { peer, origin, home },
            None => SiteAbsence::Unreachable { peer, origin },
        },
    }
}

#[cfg(test)]
mod missing_site_tests {
    use super::*;

    const ME: &str = "2KMineMineMine";
    const THEM: &str = "2KTheirsTheirs";

    #[test]
    fn our_own_tree_keeps_the_original_reading() {
        assert_eq!(classify_missing_site(ME, None, None), MissingSite::Local);
        assert_eq!(classify_missing_site(ME, Some(ME), None), MissingSite::Local);
        // Even if an origin somehow exists for ourselves, a local miss is local.
        assert_eq!(
            classify_missing_site(ME, Some(ME), Some("https://example.org")),
            MissingSite::Local
        );
    }

    /// The shipped bug, as a rule: a foreign peer whose origin resolved to
    /// *this* domain must not be reported as a missing site without naming
    /// the origin we actually asked.
    #[test]
    fn a_foreign_peer_names_the_origin_we_looked_at() {
        assert_eq!(
            classify_missing_site(ME, Some(THEM), Some("https://entitycoreprotocol.org")),
            MissingSite::Foreign { peer: THEM, origin: "https://entitycoreprotocol.org" }
        );
    }

    /// The same-origin sentinel is an origin, not an absence — collapsing it
    /// into `ForeignUnknownHost` would drop the one fact worth printing.
    #[test]
    fn the_same_origin_sentinel_is_still_an_origin_we_looked_at() {
        assert_eq!(
            classify_missing_site(ME, Some(THEM), Some("")),
            MissingSite::Foreign { peer: THEM, origin: "" }
        );
    }

    #[test]
    fn a_foreign_peer_with_no_route_is_not_a_claim_about_an_origin() {
        assert_eq!(
            classify_missing_site(ME, Some(THEM), None),
            MissingSite::ForeignUnknownHost { peer: THEM }
        );
    }

    // ── The three-way distinction (map-C2) ────────────────────────────────

    /// **P — an origin that ANSWERED is a fact about the publisher.** This is
    /// cell #17: a returning visitor to a withdrawn site used to be told the
    /// source was unreachable, while the source was answering perfectly.
    #[test]
    fn an_origin_that_answered_reports_a_withdrawal_not_an_outage() {
        assert_eq!(
            classify_site_absence(ME, Some(THEM), Some("https://them.example"), true, false, None),
            SiteAbsence::Withdrawn {
                peer: THEM,
                origin: "https://them.example",
                home: false
            }
        );
    }

    /// **N — the easy failure in the other direction, and the one to watch
    /// for.** Relabelling every miss as "withdrawn" would be worse than the bug
    /// it replaces: it turns a transient outage into a claim that the publisher
    /// deleted something.
    #[test]
    fn an_origin_that_said_nothing_is_never_reported_as_a_withdrawal() {
        assert_eq!(
            classify_site_absence(ME, Some(THEM), Some("https://them.example"), false, false, None),
            SiteAbsence::Unreachable { peer: THEM, origin: "https://them.example" }
        );
        // And the home bit must not smuggle a withdrawal in either — an
        // unreachable home is still unreachable.
        assert_eq!(
            classify_site_absence(ME, Some(THEM), Some(""), false, true, None),
            SiteAbsence::Unreachable { peer: THEM, origin: "" }
        );
    }

    /// **The home bit is what makes the advice true.** Same publisher, same
    /// answered 404, and the only difference is whether this is the site the
    /// deployment is built around — which is exactly the difference between
    /// "open it on its own domain" being useful and being absurd.
    #[test]
    fn the_deployments_own_home_is_distinguished_from_a_site_you_navigated_to() {
        let visited =
            classify_site_absence(ME, Some(THEM), Some(""), true, false, None);
        let home = classify_site_absence(ME, Some(THEM), Some(""), true, true, None);
        assert_eq!(visited, SiteAbsence::Withdrawn { peer: THEM, origin: "", home: false });
        assert_eq!(home, SiteAbsence::Withdrawn { peer: THEM, origin: "", home: true });
        assert_ne!(visited, home, "the two must not render the same advice");
    }

    /// **A superseded publisher wins over everything.** "The origin does not
    /// carry this site" is true and useless when the origin is not that
    /// publisher any more — and it is the sentence that sent an operator
    /// looking for a deleted site during the re-key.
    #[test]
    fn a_retired_publisher_is_named_as_retired_whatever_the_origin_said() {
        for (origin, answered) in
            [(Some("https://them.example"), true), (Some(""), false), (None, true)]
        {
            assert_eq!(
                classify_site_absence(ME, Some(THEM), origin, answered, false, Some("2KNewOne")),
                SiteAbsence::Retired { peer: THEM, successor: "2KNewOne" },
                "origin={origin:?} answered={answered}"
            );
        }
    }

    /// Our own tree is unaffected by any of it — a local miss is a local miss,
    /// and none of these axes apply.
    #[test]
    fn a_local_miss_is_untouched_by_the_new_axes() {
        assert_eq!(classify_site_absence(ME, None, None, true, true, None), SiteAbsence::Local);
        assert_eq!(
            classify_site_absence(ME, Some(ME), Some(""), false, true, Some("2KX")),
            SiteAbsence::Local
        );
    }

    #[test]
    fn no_route_and_not_retired_stays_a_route_problem() {
        assert_eq!(
            classify_site_absence(ME, Some(THEM), None, false, false, None),
            SiteAbsence::NoRoute { peer: THEM }
        );
    }
}
