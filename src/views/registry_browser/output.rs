//! Renderer-neutral output for the Registry Browser.
//!
//! The model builds this; `dom::registry_browser` is a pure consumer. Keeping
//! the shape here (rather than formatting in the renderer) is what lets the
//! honesty rules below be **unit-tested natively** — every one of them is a
//! statement about what a user is told, and none of them is testable through a
//! DOM.

/// Where the registry in force came from. A name resolving through a registry
/// the user never chose must never be silent [AP25].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinOrigin {
    /// Typed here with `name pin` (or this window). Tab-scoped.
    User,
    /// Seeded by `/entity-deployment.json`'s `name_registry_pin`.
    Deployment,
}

/// The registry this window is pointed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedRegistry {
    pub peer_id: String,
    pub origin: String,
    pub source: PinOrigin,
}

/// The name listing recovered from a registry's **signed root**.
///
/// **`complete` is not decoration.** A bounded walk that returns fewer keys than
/// the registry holds is indistinguishable from a small registry unless the
/// surface says so, which is the same defect the walk itself is built to prevent
/// (a shortened list that does not announce itself). The renderer must show the
/// truncation notice whenever this is `false`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameListing {
    pub names: Vec<String>,
    /// `true` when the walk visited every node the signed root declares.
    pub complete: bool,
    /// Interior nodes fetched — the cost, and the honest answer to "why is this
    /// big registry slow".
    pub nodes_walked: usize,
}

/// What a resolve established, and what it checked to establish it.
///
/// Every field here is *evidence*, not a claim: `NameEvidence` records which
/// checks actually ran, and nothing in it is a boolean a caller can set. A
/// resolved name and an unchecked one must not look alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedName {
    pub name: String,
    pub peer_id: String,
    /// Where the binding says that peer publishes. `None` when the binding
    /// carried no consumable `http-poll` transport profile — resolved WHO but
    /// not WHERE, which is a different failure from "no such name" and is
    /// reported as one.
    pub origin: Option<String>,
    pub association_committed: bool,
    pub name_checked: bool,
    pub revocation_checked: bool,
    pub expires_at_ms: u64,
    /// Set when OUR resolver ceiling shortened the lifetime the registry issued.
    /// Announced rather than applied silently [AP25]: a silent clamp makes a
    /// correctly-issued binding look like a registry that mis-set its TTL, and
    /// the operator is the one person who can tell the difference.
    pub clamped: Option<(u64, u64)>,
}

/// The window an *Open in Site Browser* click must spawn, and the peer it must
/// be bound to — `None` when there is nothing to open.
///
/// **This exists because the button did not open anything for its entire life.**
/// The click registered the origin the signed binding carried (real work, which
/// is why it did not feel completely inert) and then marked the window dirty,
/// so the Registry Browser repainted itself and no Site Browser was ever
/// spawned. Reported from a live deployment against a name that resolves
/// perfectly — every layer under the button was correct.
///
/// It is a free function rather than a line in the renderer for one reason: the
/// renderer is wasm-only, so a rule living there is unreachable from
/// `make test`, and *"which window does this open, bound to whom"* is exactly
/// the part that was wrong. **What a native test still cannot see is whether
/// the renderer calls this at all** — that needs a browser, and this repo has
/// no gate that drives the Registry Browser. Said plainly rather than implied.
///
/// The window type is the **identity key** from `window_registry`, not UI text;
/// a wrong string here resolves to no factory and silently opens nothing, which
/// is the same shape as the retired `Games` key.
///
/// # The bound peer is MINE, not the publisher's — and getting that backwards
/// is what emptied the window
///
/// The first version of this function bound the Site Browser to
/// `target.peer_id`, which reads as the obvious answer: *open a browser onto
/// that publisher*. It is wrong, and it is wrong in a way that produces a
/// window rather than an error.
///
/// **A window's bound peer is the store it READS**, never the subject it is
/// looking at. Every read in the Site Browser passes it to `Peers` as a
/// selector — the derived site index, `scan_local_sites`, the origins registry,
/// prefs, provenance — and cached foreign content lives at `/{foreign}/sites/`
/// in **my** store (V7 §1.4: the path's peer-segment carries whose site it is,
/// the selector says whose store to look in). A publisher's peer-id is hosted
/// by no local SDK, so `Peers::sdk_for` answers `UnknownPeer` and every one of
/// those reads collapses to its empty value. The rail then says *"No external
/// sites cached"* about a manifest sitting in that very store.
///
/// So the failure was total, silent and network-independent — which is why the
/// publishing side measured green (`ROUTING-2026-08-24-REGISTRY-OPEN…`: all
/// three hops 200 with CORS) while the rail stayed empty. Pinned from both
/// sides by `the_rail_reads_my_store_so_a_foreign_bound_window_sees_nothing`.
///
/// The publisher is still reached — by the origin registration and the
/// `warm_peer_sites` enumeration that run on the same click — and it appears in
/// the rail as a *cached* entry, which is what it is.
/// # ⭐ The window type is no longer a literal here — 2026-09-12
///
/// It used to be `Some(("Site Browser", local_peer))`, and
/// `DESIGN-2026-09-12-BROWSING-WITHOUT-PRIVILEGING-A-CONVENTION` §1 row 1 names
/// that literal as one of the three places the site convention is privileged.
/// What this builds now is an **address** — *this publisher's sites* — and
/// [`crate::open_target::route`] decides which viewer shows it. The answer is
/// still the Site Browser, and that is the point: the table gives the same
/// answer and a second convention can register beside it without editing this
/// function.
///
/// ⚠ **The remaining guess is `sites`, and it is deliberately visible.** A name
/// binding carries a peer and an origin and says **nothing** about what that peer
/// publishes, so *which convention* is not derivable here — it is the discovery
/// half §4 separates out and refuses to smuggle in with the open. Before, that
/// guess was a window name in a literal; now it is one address on one line, and
/// the day a binding can say *"I publish a feed"* this is the line that changes.
pub fn open_target(resolved: &Phase<ResolvedName>, local_peer: &str) -> Option<Open> {
    let Phase::Done(target) = resolved else { return None };
    // No origin means we resolved WHO but not WHERE. A Site Browser opened for
    // a peer with no registered origin can only fail to fetch, and it would
    // fail as "that site is not there" — a wrong sentence about a registry that
    // answered correctly.
    target.origin.as_ref()?;
    let address = crate::open_target::site_directory(&target.peer_id);
    match crate::open_target::route(&address) {
        crate::open_target::Routing::Viewer(window_type) => Some(Open {
            window_type,
            bind_peer: local_peer.to_string(),
            target: address,
        }),
        // Unreachable while `sites` has a row, and not asserted away: a table
        // with no viewer for this address is a real state, and opening *some*
        // window because one was expected is how a routing hole becomes an empty
        // rail somebody has to debug.
        other => {
            tracing::warn!(
                resolved_peer = %target.peer_id,
                routing = ?other,
                "a resolved name has no viewer to open it in"
            );
            None
        }
    }
}

/// What an *Open* click must do: **which window, whose store it reads, and what
/// it is looking at.**
///
/// The second and third fields are different peers and that is the whole
/// history of this surface — see this module's [`open_target`] doc for the
/// window that shipped bound to the publisher and rendered an empty rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    /// Identity key from `window_registry`, chosen by [`crate::open_target`].
    pub window_type: &'static str,
    /// The peer whose **store** the window reads — mine.
    pub bind_peer: String,
    /// The **subject**: what the window is opened at.
    pub target: crate::entity_ref::EntityRef,
}

/// One in-flight or finished operation, so the surface can distinguish "nothing
/// asked yet" from "asked and empty" — which are the same pixels otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase<T> {
    /// Nothing asked yet. **Not** an empty result.
    Idle,
    Running,
    Done(T),
    Failed(String),
}

/// Everything the Registry Browser renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryBrowserOutput {
    /// `None` = this deployment seeds no pin and the user typed none, so
    /// resolution fails closed. That is a state to *state*, not a blank panel.
    pub pinned: Option<PinnedRegistry>,
    pub listing: Phase<NameListing>,
    pub resolved: Phase<ResolvedName>,
    /// How many publishers this tab holds a `seq` floor for.
    pub sessions: usize,
    /// True on native builds, where the fetch has no implementation. Saying so
    /// beats an empty panel that reads as a broken registry.
    pub browser_only: bool,
    /// The peer whose store an *Open in Site Browser* click must bind that
    /// window to — **the system peer**, which is where `open_in_site_browser`
    /// registers the origin and warms the publisher's manifests, and therefore
    /// the only store in which those sites can be read.
    ///
    /// Carried on the output rather than read in the renderer because the
    /// renderer holds no `Peers`, and because *which peer* is precisely the
    /// thing `open_target` got wrong — see its doc.
    pub local_peer: String,
    /// Why the last pin attempt was refused, if it was.
    ///
    /// Carried to the pixel rather than dropped: the two refusals (no peer-id, a
    /// non-canonical peer-id that carries no key) are both things the person can
    /// fix, and a Pin button that silently does nothing is the operator-surface
    /// failure this repo keeps meeting — the one person who could correct it is
    /// told nothing at all.
    pub pin_error: Option<String>,
}

#[cfg(test)]
mod open_target_tests {
    use super::*;

    fn resolved(origin: Option<&str>) -> ResolvedName {
        ResolvedName {
            name: "billslab.com".into(),
            peer_id: "2KBj64anEXAMPLE".into(),
            origin: origin.map(str::to_string),
            association_committed: true,
            name_checked: true,
            revocation_checked: true,
            expires_at_ms: 0,
            clamped: None,
        }
    }

    /// A resolved name with an origin opens a **Site Browser bound to MY peer**,
    /// which is the store the window reads. The publisher's id appears in that
    /// window as a cached *entry*, not as the binding.
    ///
    /// **The negative half is the load-bearing one**, because binding to the
    /// publisher is the reading that shipped: it produces a real window with a
    /// plausible title and an empty rail, so it looks like a working button
    /// reporting that the publisher has nothing. Asserting only "some peer" or
    /// only "not the default" would both pass it.
    #[test]
    fn a_resolved_name_opens_a_site_browser_bound_to_my_own_store() {
        let r = resolved(Some("https://billslab.com"));
        let got = open_target(&Phase::Done(r.clone()), "2KMYLOCALPEER").expect("an open");
        assert_eq!(got.window_type, "Site Browser");
        assert_eq!(got.bind_peer, "2KMYLOCALPEER");
        assert_ne!(
            got.bind_peer, r.peer_id,
            "binding the window to the publisher is the bug: no local SDK hosts \
             that id, so every read in the window is empty"
        );
    }

    /// ⭐ **…and the SUBJECT is the publisher, which is the same fact from the
    /// other side.** The pair that shipped carried only the binding, so *whose
    /// content this is* travelled as a side effect (the origin registration and
    /// the manifest warm on the same click). Asserting both peers in one test is
    /// what stops a later simplification from collapsing them back — and it is
    /// the one assertion that would fail if somebody "fixed" the binding to point
    /// at the publisher.
    #[test]
    fn the_subject_is_the_publisher_and_the_binding_is_mine() {
        let r = resolved(Some("https://billslab.com"));
        let got = open_target(&Phase::Done(r.clone()), "2KMYLOCALPEER").expect("an open");
        assert_eq!(got.target.peer(), r.peer_id, "the target names the publisher");
        assert_ne!(got.target.peer(), got.bind_peer, "subject and binding are two facts");
        assert_eq!(
            crate::open_target::route(&got.target),
            crate::open_target::Routing::Viewer("Site Browser"),
            "the window type must come from the table, not from a literal here"
        );
    }

    /// **Resolved WHO but not WHERE opens nothing.** A binding carrying no
    /// consumable transport profile gives a peer with no registered origin, and
    /// a Site Browser opened for one can only fail to fetch — reporting "that
    /// site is not there" about a registry that answered correctly. The same
    /// absent-vs-withheld seam this arc keeps meeting, at the UI layer.
    #[test]
    fn resolving_who_but_not_where_opens_nothing() {
        assert_eq!(open_target(&Phase::Done(resolved(None)), "2KMYLOCALPEER"), None);
    }

    /// Nothing resolved yet is not something to open. Guards the arm a
    /// renderer would otherwise reach by unwrapping an in-flight phase.
    #[test]
    fn an_unresolved_phase_opens_nothing() {
        assert_eq!(open_target(&Phase::Idle, "2KMYLOCALPEER"), None);
        assert_eq!(open_target(&Phase::Running, "2KMYLOCALPEER"), None);
    }
}
