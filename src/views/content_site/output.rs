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
}
