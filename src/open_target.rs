//! **What a viewer is aimed at** — the read side of [`crate::publish_axes`].
//!
//! `publish_axes` is THE list of what enters a projection: a convention is a row
//! there and the compiler enforces the obligations. This module is the same list
//! read backwards — *given an address, which viewer shows it* — and it exists for
//! the same reason: the alternative is a literal window name at each call site,
//! which is what `views/registry_browser/output.rs:open_target` was.
//!
//! # The finding this was built from
//!
//! `DESIGN-2026-09-12-BROWSING-WITHOUT-PRIVILEGING-A-CONVENTION` §0:
//!
//! > **There is no expression in this codebase for *"open this thing"*, only for
//! > *"open this window, bound to this peer"*.**
//!
//! So a caller who knew exactly what a reader wanted to look at had nowhere to
//! put it, and the two callers that had one used a side channel: the Registry
//! Browser spawned a hard-coded `("Site Browser", my_peer)` and warmed the
//! publisher's manifests *as a side effect*, and a deployment could name a window
//! type but not what it was aimed at. **The privileging was never in the UI; it
//! was in one type carrying no address.**
//!
//! # The address is [`EntityRef`], and that is not a new invention
//!
//! `APP-CONVENTION-REFERENCE`'s atom already is *who publishes it* + *the address
//! of record*, tagged, with a string form that round-trips (§3.2 makes that a
//! MUST in both directions). A deployment document and a persisted session config
//! both need a **string**, so the round trip is load-bearing rather than tidy.
//!
//! ⭐ **A pin routes to nothing, and that is a fact about pins rather than a gap
//! here.** `EntityRef::Pinned` names bytes; a viewer is chosen by *where a thing
//! lives*, and a hash says nothing about that. [`Routing::Unaddressed`] is its own
//! outcome for the AP40 reason: *"we cannot show you this"* and *"nobody handles
//! this kind of thing"* send a person to different places.
//!
//! # The subject peer and the bound peer are DIFFERENT, and conflating them is a
//! shipped bug
//!
//! `open_target`'s own doc narrates it at length: **a window's bound peer is the
//! store it READS**, never the subject it is looking at. Cached foreign content
//! lives at `/{foreign}/…` in *my* store, so binding a Site Browser to the
//! publisher makes every read answer `UnknownPeer` and produces a real window with
//! an empty rail. That is why an aim carries the subject and
//! [`crate::action::Action::SpawnWindow`] still carries the binding — two fields,
//! because they are two facts.
//!
//! # What this does NOT answer: discovery
//!
//! *Which viewer handles this address* is answerable from the address. *What does
//! this peer publish* is not, and nothing here invents an answer — the design
//! doc's §4 is explicit that a generic **open** needs no discovery (the caller
//! already knows the target) and a generic **browse** does. The one caller that
//! still guesses is the Registry Browser, which resolves a name to a publisher and
//! has nothing in the binding saying what they publish; its guess is now a single
//! visible target rather than a window name in a literal.

use crate::entity_ref::EntityRef;

/// One viewer that can be aimed, and the peer-relative tree segment it reads.
///
/// `segment` is deliberately a *tree* segment and not a convention name: the
/// tree is the universal namespace (`paths.rs`: *"the universal tree carries the
/// partition"*), so *where a thing lives* is the one thing every convention
/// already agrees about. A fourth viewer is a row here plus a
/// [`crate::window::WindowView::aim`] override.
pub struct Viewer {
    /// The **identity key** from `window_registry`, not UI text — a wrong string
    /// here resolves to no factory and silently opens nothing, which is the same
    /// shape as the retired `Games` key.
    pub window_type: &'static str,
    /// How a reader asks a publisher *"do you publish this at all?"* — see
    /// [`EntryPoint`], and read its doc before adding a row, because the two
    /// arms are a fact about the conventions and not a convenience here.
    pub entry: EntryPoint,
    /// The peer-relative prefix this viewer's addresses live under, **leading
    /// slash, no trailing one** — see [`FEED_SEGMENT`] for why the leading one is
    /// load-bearing. Pinned against the owning convention's own path builders by
    /// [`the_segments_are_pinned_against_the_conventions_own_paths`], so a
    /// convention that moves its subtree reds here instead of silently routing
    /// nothing.
    pub segment: &'static str,
    /// Why this viewer claims that segment — read by nobody, which is the point:
    /// a row that cannot say what it is for is a row nobody can audit.
    pub why: &'static str,
}

/// How a reader asks a publisher **whether they publish this convention at
/// all** — the one question a name binding cannot answer.
///
/// ⭐ **The two arms are not an implementation detail; they are the namespace
/// asymmetry the conventions themselves carry, and it is worth knowing which
/// one you are in before designing anything that browses.**
///
/// - `APP-CONVENTION-FEED` §4.2 pins **two tree paths** by hand — the head at
///   `/{peer}/app/feed/index` and pages beneath it — precisely because §4.1
///   argues a reader holding no reference *has to start somewhere*, and that
///   discovering what is new under a prefix costs the whole trie. So the feed
///   probe is **one keyed read**.
/// - `APP-CONVENTION-SEMANTIC-CONTENT-SITE` v0.5 §2 makes a site **"a free
///   subgraph at any publisher-chosen tree path"** and pins no tree path at
///   all; what it registers (§11) is a *URL projection prefix* at
///   `EXTENSION-NETWORK` §6.5.6's demux, explicitly *"a publish-time
///   projection, not a tree-storage rule"*. So there is no key to ask for, the
///   probe is a **bounded walk**, and `/sites` is **this publisher's** choice
///   of placement rather than the convention's.
///
/// **A convention that pins an entry point can be probed in one round trip; one
/// that does not has to be walked.** That is the whole cost difference, and it
/// is the argument for pinning one.
/// **Both arms are spelled with a LEADING SLASH**, like [`Viewer::segment`] and
/// for the same two reasons: it is what makes them read as *paths* rather than
/// type tags to a human and to `spec vocab` alike, and it keeps one spelling
/// convention across the row. [`EntryPoint::relative`] strips it — the signed
/// session takes a peer-relative key with no leading slash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPoint {
    /// One key whose presence in the publisher's **signed root** is the answer.
    Key(&'static str),
    /// A prefix (trailing slash) to walk, because the convention pins no entry
    /// point. Bounded at the call site; a walk that ran out of budget reports
    /// itself rather than answering short.
    Prefix(&'static str),
}

impl EntryPoint {
    /// The peer-relative form a `SignedSession` takes — this spelling with the
    /// leading slash removed.
    pub fn relative(&self) -> &'static str {
        let s = match self {
            EntryPoint::Key(k) => k,
            EntryPoint::Prefix(p) => p,
        };
        s.strip_prefix('/').unwrap_or(s)
    }
}

/// THE table. Two implementors, which is the minimum that makes a table
/// trustworthy — `publish_axes`' own rule: *"a registration table with one
/// implementor and two special cases is a table nobody can trust."*
pub fn viewers() -> &'static [Viewer] {
    &[
        Viewer {
            window_type: SITE_BROWSER,
            entry: EntryPoint::Prefix(SITES_ENTRY_PREFIX),
            segment: SITES_SEGMENT,
            why: "APP-CONVENTION-SEMANTIC-CONTENT-SITE — /{peer}/sites/{site}/pages/{page}",
        },
        Viewer {
            window_type: FEED,
            entry: EntryPoint::Key(FEED_ENTRY_KEY),
            segment: FEED_SEGMENT,
            why: "APP-CONVENTION-FEED §4.2 — /{peer}/app/feed/index and what hangs under it",
        },
    ]
}

/// Where this publisher places sites — **ours, not the convention's.** SITE
/// v0.5 §2 leaves placement free, so this is a choice we made and the test
/// below is what keeps it honest against `content_site::paths`.
const SITES_ENTRY_PREFIX: &str = "/sites/";

/// `APP-CONVENTION-FEED` §4.2's index head — **the convention's**, pinned
/// against `feed::index_head_key` by
/// [`the_entry_points_are_pinned_against_the_conventions_own_paths`] so the two
/// spellings of §4.2 in this crate cannot drift.
const FEED_ENTRY_KEY: &str = "/app/feed/index";

/// The Site Browser's identity key.
///
/// The keys and the segments below are public so the table **and** each viewer's
/// [`crate::window::WindowView::aim`] read one expression of each. Two spellings
/// of *"which addresses are mine"* is C15's drift with a routing symptom: the
/// table sends a target to a window whose own decode then answers `NotMine`.
pub const SITE_BROWSER: &str = "Site Browser"; // i18n-ignore — identity key
/// The Feed window's identity key.
pub const FEED: &str = "Feed"; // i18n-ignore — identity key
/// Where `APP-CONVENTION-SEMANTIC-CONTENT-SITE` addresses live under a peer.
pub const SITES_SEGMENT: &str = "/sites";
/// Where `APP-CONVENTION-FEED` addresses live under a peer.
///
/// ⚠ **Spelled with a LEADING SLASH, and that is not cosmetic.** `spec vocab`
/// classifies a source literal by shape: a quoted `app/…` with no trailing slash
/// is a type tag, and this segment written bare is shaped exactly like one.
/// `make lint` went red on **`implemented-undeclared app/feed`** — a type no spec
/// declares and that we do not emit. That is the **third** instance of the
/// analyzer's path/tag confusion here (after the share-records prefix and the
/// feed index key), and the same local answer applies: spell the path so it
/// cannot be mistaken for a tag. It does not fix the analyzer, and the next bare
/// `app/…` path literal anywhere will trip it again.
///
/// ⭐ **Fourth instance, found the same hour: the sentence above used to quote
/// the bare form to explain the problem, and the analyzer read the COMMENT as a
/// literal.** It scans raw lines, so prose about a tag and an emission of one are
/// the same bytes to it — which is the cheapest possible demonstration that the
/// discriminator has to be the corpus and not the shape. Routed as an
/// observation; do not write a quoted `app/…` into a comment in this repo.
pub const FEED_SEGMENT: &str = "/app/feed";

/// Which viewer shows this address — **three outcomes, and the two refusals are
/// different facts.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routing {
    /// This address is under a registered viewer's segment.
    Viewer(&'static str),
    /// A **pin**: it names bytes and no place, so nothing in it says which viewer.
    /// Not a failure of the table.
    Unaddressed,
    /// A live address under no viewer's segment. Carries what it asked for,
    /// because *"nobody shows `/apps/…`"* is a roadmap fact somebody can act on
    /// and a bare `None` is not.
    NoViewer { path: String },
}

/// Route an address to a viewer.
///
/// First match wins and the segments are disjoint today; if two ever overlap the
/// longer one must come first, which is why this iterates the table in order
/// rather than collecting matches.
pub fn route(target: &EntityRef) -> Routing {
    let EntityRef::Live { path, .. } = target else {
        return Routing::Unaddressed;
    };
    for v in viewers() {
        if path_is_under(path, v.segment) {
            return Routing::Viewer(v.window_type);
        }
    }
    Routing::NoViewer { path: path.clone() }
}

/// Is this peer-relative path the segment itself, or something beneath it?
///
/// **Both, deliberately.** `/sites` is *"this publisher's sites"* — a real thing
/// to be pointed at even though it names no particular site — and a viewer that
/// only matched `/sites/…` would refuse the one address the Registry Browser can
/// honestly build. The exact-match arm is what makes the difference between
/// *routable* and *aimable* expressible at all.
fn path_is_under(path: &str, segment: &str) -> bool {
    match path.strip_prefix(segment) {
        Some("") => true,
        Some(r) => r.starts_with('/'),
        None => false,
    }
}

/// The part of a target's path **beneath** a viewer's segment, with no leading
/// slash — what a viewer decodes as its own address payload. `None` when the
/// target is not this viewer's at all.
///
/// Exists so an aim override never re-parses the whole path: the outer address is
/// this module's, the payload is the viewer's, and §3's *"each viewer's address
/// type is different and that is mostly fine"* only holds if the split is made
/// once.
pub fn payload<'a>(target: &'a EntityRef, segment: &str) -> Option<&'a str> {
    let EntityRef::Live { path, .. } = target else { return None };
    match path.strip_prefix(segment) {
        Some("") => Some(""),
        Some(r) => r.strip_prefix('/'),
        None => None,
    }
}

/// A target naming one page of one site. `page` empty = the site's root page.
pub fn site(peer: &str, site_id: &str, page: &str) -> EntityRef {
    let path = if page.is_empty() {
        format!("/sites/{site_id}")
    } else {
        format!("/sites/{site_id}/pages/{page}")
    };
    EntityRef::live(peer, path)
}

/// A target naming a publisher's **sites** and not which one — the honest target
/// for a name that resolved to a peer and an origin and nothing else.
pub fn site_directory(peer: &str) -> EntityRef {
    EntityRef::live(peer, "/sites")
}

/// *Everything of this convention that this publisher has* — the viewer's own
/// segment under their peer.
///
/// **One expression instead of a per-convention builder**, which matters
/// because a caller holding a `Viewer` (a probe result, a browse row) would
/// otherwise need a `match` on the window type to pick between
/// [`site_directory`] and [`feed`] — the literal this module exists to retire,
/// reintroduced one level up. It works because [`path_is_under`]'s exact-match
/// arm makes a bare segment routable, which is the difference between
/// *routable* and *aimable* that arm was added for.
pub fn directory(peer: &str, viewer: &Viewer) -> EntityRef {
    EntityRef::live(peer, viewer.segment)
}

/// A target naming a publisher's feed — `APP-CONVENTION-FEED` §4.2's index head,
/// which is where a reader holding no reference has to start.
pub fn feed(peer: &str) -> EntityRef {
    EntityRef::live(peer, format!("/{}", crate::feed::index_head_key()))
}

/// Parse a target out of its §3.1 string form, or say why not.
///
/// A thin wrapper so callers (the deployment document, the persisted session
/// config, a shell verb) share one spelling of *what a target looks like written
/// down* — and so the empty string is **absence** rather than a parse failure at
/// every one of them.
pub fn parse(s: &str) -> Option<EntityRef> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    match EntityRef::parse_uri(s) {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!(
                target_uri = %s,
                error = ?e,
                "open target is not a readable entity reference; ignoring it"
            );
            None
        }
    }
}

/// Write a target down. `None` → the empty string, which every reader treats as
/// *no target declared* rather than as a malformed one.
pub fn write(target: Option<&EntityRef>) -> String {
    target.and_then(|t| t.to_uri().ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity_hash::Hash;

    /// A real self-describing hash — `entity_ref`'s own test rule: never a
    /// hand-written fixed width.
    fn h() -> Hash {
        Hash::compute("test/note", b"open-target")
    }

    /// ⭐ **The table's segments are the conventions' own, and this is what says
    /// so.** Deriving them by string surgery from `paths::manifest_path` /
    /// `feed::index_head_path` would be cute and unreadable; pinning them is the
    /// same protection with the relationship written where a reader looks. A
    /// convention that moves its subtree reds here rather than quietly routing
    /// nothing — which would render as *"nobody shows this"* about a viewer that
    /// is sitting right there.
    #[test]
    fn the_segments_are_pinned_against_the_conventions_own_paths() {
        let site_seg = viewers().iter().find(|v| v.window_type == SITE_BROWSER).unwrap().segment;
        assert!(
            crate::content_site::paths::manifest_path("PEER", "demo")
                .starts_with(&format!("/PEER{site_seg}/")),
            "the site viewer's segment no longer matches content_site::paths"
        );
        let feed_seg = viewers().iter().find(|v| v.window_type == FEED).unwrap().segment;
        assert!(
            crate::feed::index_head_path("PEER").starts_with(&format!("/PEER{feed_seg}/")),
            "the feed viewer's segment no longer matches APP-CONVENTION-FEED §4.2"
        );
    }

    /// ⭐ **The entry points are the conventions' own too, and one of them is
    /// not a convention's at all.**
    ///
    /// The feed's is `APP-CONVENTION-FEED` §4.2's pinned head, so it is checked
    /// against `feed::index_head_key` — two spellings of one normative path in
    /// one crate is C15's drift with a routing symptom.
    ///
    /// The site's is checked against `content_site::paths` and **nothing
    /// normative**, because SITE v0.5 §2 makes placement free: *"a site is a
    /// free subgraph at any publisher-chosen tree path"*. That asymmetry is the
    /// point of [`EntryPoint`] having two arms, and this test is where it is
    /// stated in code rather than in prose — a probe that assumed every
    /// convention pins an entry point would answer *"this publisher has no
    /// sites"* about every publisher who placed theirs somewhere else.
    #[test]
    fn the_entry_points_are_pinned_against_the_conventions_own_paths() {
        let site = viewers().iter().find(|v| v.window_type == SITE_BROWSER).unwrap();
        let EntryPoint::Prefix(site_entry) = site.entry else {
            panic!("a convention that pins no tree path cannot have a keyed entry point");
        };
        assert!(
            crate::content_site::paths::manifest_path("PEER", "demo")
                .starts_with(&format!("/PEER{site_entry}")),
            "the site viewer's entry prefix no longer matches content_site::paths"
        );

        let feed = viewers().iter().find(|v| v.window_type == FEED).unwrap();
        let EntryPoint::Key(feed_entry) = feed.entry else {
            panic!("FEED §4.2 pins the head by hand; a prefix walk is not what it asks for");
        };
        assert_eq!(
            feed.entry.relative(),
            crate::feed::index_head_key(),
            "the feed viewer's entry key no longer matches APP-CONVENTION-FEED §4.2"
        );
        assert!(
            feed_entry.starts_with('/') && site_entry.starts_with('/'),
            "entry points are spelled with a leading slash so they read as paths, not tags"
        );
    }

    /// ⭐ **`directory` must agree with the hand-written builders, and it must
    /// route back to the viewer it was built from.**
    ///
    /// The first half stops two spellings of *"this publisher's sites"*; the
    /// second is the property that makes a generic builder safe at all — a
    /// segment that did not round-trip through [`route`] would hand a caller an
    /// address the table refuses, i.e. an Open button that opens nothing.
    #[test]
    fn a_directory_address_agrees_with_the_builders_and_routes_home() {
        for v in viewers() {
            assert_eq!(
                route(&directory("PEER", v)),
                Routing::Viewer(v.window_type),
                "`{}`'s own segment does not route back to it",
                v.window_type
            );
        }
        let site = viewers().iter().find(|v| v.window_type == SITE_BROWSER).unwrap();
        assert_eq!(directory("PEER", site), site_directory("PEER"));
        let feed_v = viewers().iter().find(|v| v.window_type == FEED).unwrap();
        assert_eq!(
            payload(&directory("PEER", feed_v), FEED_SEGMENT),
            Some(""),
            "the Feed window's aim reads a payload, and a bare segment must give it the empty one"
        );
    }

    /// Every window type in the table is a registered one. A row naming a window
    /// that does not exist routes a person to a factory that is not there, and
    /// `WindowManager::spawn` answers `None` — an *open* button that opens
    /// nothing, which is the exact defect `open_target` was built to fix.
    #[test]
    fn every_viewer_in_the_table_is_a_registered_window_type() {
        let roster = crate::window_registry::standard_window_type_meta();
        for v in viewers() {
            assert!(
                roster.iter().any(|(name, _)| *name == v.window_type),
                "the table routes to `{}`, which is not a registered window type",
                v.window_type
            );
        }
    }

    #[test]
    fn a_site_page_routes_to_the_site_browser() {
        assert_eq!(
            route(&site("PEER", "demo", "about")),
            Routing::Viewer(SITE_BROWSER)
        );
        assert_eq!(route(&site("PEER", "demo", "")), Routing::Viewer(SITE_BROWSER));
    }

    /// A publisher's sites with no site named still routes — see
    /// [`path_is_under`]. *Routable* and *aimable* are different claims and this
    /// is the address that separates them.
    #[test]
    fn a_publishers_site_directory_routes_even_though_it_names_no_site() {
        assert_eq!(route(&site_directory("PEER")), Routing::Viewer(SITE_BROWSER));
        assert_eq!(payload(&site_directory("PEER"), SITES_SEGMENT), Some(""));
    }

    #[test]
    fn a_feed_routes_to_the_feed_window() {
        assert_eq!(route(&feed("PEER")), Routing::Viewer(FEED));
    }

    /// ⭐ **A pin is not an unroutable address; it is not an address.** Kept apart
    /// from `NoViewer` because they license different sentences: one is *we do not
    /// have a viewer for that*, the other is *that reference does not say where it
    /// lives*. A single `None` would render both as the first.
    #[test]
    fn a_pin_names_bytes_and_therefore_no_viewer() {
        assert_eq!(route(&EntityRef::pin("PEER", h())), Routing::Unaddressed);
        assert_eq!(payload(&EntityRef::pin("PEER", h()), SITES_SEGMENT), None);
    }

    #[test]
    fn an_address_under_no_viewers_segment_says_what_it_asked_for() {
        let r = route(&EntityRef::live("PEER", "/apps/arcade/pong"));
        assert_eq!(r, Routing::NoViewer { path: "/apps/arcade/pong".into() });
    }

    /// A near-miss must not match. `/sitesomething` shares a prefix with `sites`
    /// and is a different subtree; a `starts_with` on the bare segment would route
    /// it to the Site Browser, which would then decode a site id out of nothing.
    #[test]
    fn a_segment_that_merely_shares_a_prefix_is_not_a_match() {
        assert!(matches!(
            route(&EntityRef::live("PEER", "/sitesomething/x")),
            Routing::NoViewer { .. }
        ));
        assert_eq!(payload(&EntityRef::live("PEER", "/sitesomething/x"), SITES_SEGMENT), None);
    }

    #[test]
    fn payload_is_the_part_beneath_the_segment() {
        assert_eq!(payload(&site("PEER", "demo", "about"), SITES_SEGMENT), Some("demo/pages/about"));
        assert_eq!(payload(&feed("PEER"), FEED_SEGMENT), Some("index"));
    }

    /// The string form round-trips, which is what a deployment document and a
    /// persisted config both depend on.
    #[test]
    fn a_target_survives_being_written_down_and_read_back() {
        let t = site("2KBjEXAMPLE", "demo", "about");
        let written = write(Some(&t));
        assert!(written.starts_with("entity+ref://2KBjEXAMPLE/sites/"), "{written}");
        assert_eq!(parse(&written), Some(t));
    }

    /// **Absence and garbage are different facts.** An empty declaration is a
    /// deployment that named no target; an unparseable one is a deployment that
    /// tried and got it wrong, and only the second is worth a line in the log.
    #[test]
    fn nothing_written_down_is_absence_and_not_a_parse_failure() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("   "), None);
        assert_eq!(parse("not a reference"), None);
        assert_eq!(write(None), "");
    }
}
