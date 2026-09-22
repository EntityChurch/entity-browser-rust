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
    /// Catalog key for the control's caption — [`crate::open_target::Viewer`]'s,
    /// because the act differs per viewer and one of them writes a durable
    /// follow. See that field's doc.
    pub open_key: &'static str,
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

/// ⭐ **What the resolved publisher actually publishes — and the reason this
/// surface no longer guesses.**
///
/// A resolve establishes *who* and *where*. It cannot establish *what*: a
/// `system/registry/binding` carries `name`, `target_peer_id` and `transports`
/// and nothing about the publisher's content, so `open_target` below picked
/// `sites` and said so in its own doc. [`crate::publication_probe`] asks the
/// publisher's signed root instead, and the answer it produces includes a
/// **verified negative** — something no registry field could supply.
///
/// `Phase` for the same reason the listing is: *nothing asked yet* and *asked,
/// and they publish nothing* are different states, and rendering them alike is
/// how a reader concludes a publisher is empty.
pub type Publications = Vec<crate::publication_probe::Finding>;

/// One *Open* per convention the publisher **demonstrably** carries.
///
/// # This replaced `open_target`, which guessed — 2026-09-15
///
/// That function returned a Site Browser for **every** resolved name with an
/// origin, and its own doc said why: *"a name binding carries a peer and an
/// origin and says nothing about what that peer publishes … the day a binding
/// can say 'I publish a feed' this is the line that changes."* The line changed
/// — not because a binding learned to say it, but because the publisher can be
/// asked directly ([`crate::publication_probe`]).
///
/// # The bound peer is MINE, not the publisher's — and getting that backwards
/// emptied the window
///
/// **A window's bound peer is the store it READS**, never the subject it is
/// looking at. Every read in a viewer passes it to `Peers` as a selector, and
/// cached foreign content lives at `/{foreign}/…` in **my** store (V7 §1.4: the
/// path's peer-segment carries whose content it is, the selector says whose
/// store to look in). A publisher's peer-id is hosted by no local SDK, so
/// `Peers::sdk_for` answers `UnknownPeer` and every read collapses to its empty
/// value — a real window, a plausible title, and a rail that says *"nothing
/// cached"* about a manifest sitting in that very store.
///
/// The failure was total, silent and network-independent, which is why the
/// publishing side measured green (`ROUTING-2026-08-24-REGISTRY-OPEN…`: all
/// three hops 200 with CORS) while the rail stayed empty. Pinned from both
/// sides by `the_rail_reads_my_store_so_a_foreign_bound_window_sees_nothing`.
///
/// **What a native test still cannot see is whether the renderer calls this at
/// all** — that needs a browser, and this repo has no gate that drives the
/// Registry Browser. Said plainly rather than implied.
///
/// ⛔ **A refusal offers nothing, and that is the whole behaviour change.** The
/// retired `open_target` returned a Site Browser for every resolved name with
/// an origin — so a publisher who carries only a feed got a window whose rail
/// is empty by construction, which reads as *"this publisher has nothing"*
/// about a publisher with an archive. Only [`Publishes::Yes`] licenses a
/// button; the other seven outcomes are rendered as what they are.
///
/// [`Publishes::Yes`]: crate::publication_probe::Publishes::Yes
pub fn opens(
    published: &Publications,
    resolved: &Phase<ResolvedName>,
    local_peer: &str,
) -> Vec<Open> {
    // ⚠ **Takes the PHASE, not the resolved name, deliberately.** The retired
    // `open_target` did too, and its `an_unresolved_phase_opens_nothing` is the
    // only place that guard is checkable: the renderer is wasm-only, so a rule
    // living there is unreachable from `make test`. Narrowing the parameter to
    // `&ResolvedName` would move a real rule into the one file no gate reads.
    let Phase::Done(target) = resolved else { return Vec::new() };
    // Resolved WHO but not WHERE. Belt-and-braces — with no origin the probe
    // cannot have run — but it is the guard on the DECISION rather than on the
    // acquisition, so it keeps holding if a future caller probes by some other
    // route.
    if target.origin.is_none() {
        return Vec::new();
    }
    published
        .iter()
        .filter(|f| f.outcome.is_offerable())
        .filter_map(|f| {
            let viewer = crate::open_target::viewers().iter().find(|v| v.window_type == f.window_type)?;
            Some(Open {
                window_type: viewer.window_type,
                bind_peer: local_peer.to_string(),
                target: crate::open_target::directory(&target.peer_id, viewer),
                open_key: viewer.open_key,
            })
        })
        .collect()
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
    /// What the resolved publisher publishes — see [`Publications`]. `Idle`
    /// until a name resolves; reset on every new resolve, because a finding is
    /// an answer about **one** publisher and carrying it forward would attach
    /// the last publisher's conventions to this one.
    pub published: Phase<Publications>,
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
    use crate::publication_probe::{Finding, Publishes, Unknown};

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

    /// ⭐ **The SUBJECT is the publisher and the BINDING is my store** — two
    /// peers, two facts, and collapsing them is the bug that shipped.
    ///
    /// Binding the window to the publisher is the reading that looks obvious
    /// (*open a browser onto that peer*) and it produces a real window with a
    /// plausible title and an empty rail — a working-looking button reporting
    /// that the publisher has nothing. Asserting only "some peer" or only "not
    /// the default" would both pass it, which is why both peers are named here.
    #[test]
    fn the_subject_is_the_publisher_and_the_binding_is_mine() {
        let r = resolved(Some("https://billslab.com"));
        let found = vec![finding(crate::open_target::SITE_BROWSER, Publishes::Yes { units: Some(1) })];
        let got = opens(&found, &Phase::Done(r.clone()), "2KMYLOCALPEER");
        let got = got.first().expect("a demonstrated convention is offered");
        assert_eq!(got.window_type, crate::open_target::SITE_BROWSER);
        assert_eq!(got.bind_peer, "2KMYLOCALPEER");
        assert_eq!(got.target.peer(), r.peer_id, "the target names the publisher");
        assert_ne!(
            got.bind_peer, r.peer_id,
            "binding the window to the publisher is the bug: no local SDK hosts \
             that id, so every read in the window is empty"
        );
        assert_eq!(
            crate::open_target::route(&got.target),
            crate::open_target::Routing::Viewer(crate::open_target::SITE_BROWSER),
            "the window type must come from the table, not from a literal here"
        );
    }

    /// Nothing resolved yet is not something to open. Guards the arm a renderer
    /// would otherwise reach by unwrapping an in-flight phase — and it is the
    /// reason [`opens`] takes the `Phase` rather than the resolved name.
    #[test]
    fn an_unresolved_phase_opens_nothing() {
        let found = vec![finding(crate::open_target::FEED, Publishes::Yes { units: None })];
        assert!(opens(&found, &Phase::Idle, "2KMYLOCALPEER").is_empty());
        assert!(opens(&found, &Phase::Running, "2KMYLOCALPEER").is_empty());
        assert!(opens(&found, &Phase::Failed("no such name".into()), "2KMYLOCALPEER").is_empty());
    }

    // -- what the probe replaced the guess with ----------------------------

    fn finding(window_type: &'static str, outcome: Publishes) -> Finding {
        let v = crate::open_target::viewers()
            .iter()
            .find(|v| v.window_type == window_type)
            .expect("a fixture naming a viewer the table does not have measures nothing");
        Finding { window_type: v.window_type, entry: v.entry, outcome }
    }

    /// ⭐⭐ **The behaviour change, stated as the case that was wrong.**
    ///
    /// A publisher who carries a feed and no sites used to get an *Open in Site
    /// Browser* button — the only button there was — and it opened a window
    /// whose rail is empty by construction. So the surface reported *"this
    /// publisher has nothing"* about somebody with an archive, from a registry
    /// that had answered perfectly.
    ///
    /// **Both halves asserted:** the feed IS offered and the site is NOT. A test
    /// checking only the first passes for an implementation that offers
    /// everything, which is the defect.
    #[test]
    fn a_feed_only_publisher_is_offered_a_feed_and_not_a_site_browser() {
        let r = resolved(Some("https://billslab.com"));
        let found = vec![
            finding(crate::open_target::SITE_BROWSER, Publishes::No),
            finding(crate::open_target::FEED, Publishes::Yes { units: None }),
        ];
        let got = opens(&found, &Phase::Done(r.clone()), "2KMYLOCALPEER");
        assert_eq!(got.len(), 1, "exactly one convention was demonstrated: {got:?}");
        assert_eq!(got[0].window_type, crate::open_target::FEED);
        assert_eq!(got[0].target.peer(), r.peer_id, "the subject is the publisher");
        assert_eq!(got[0].bind_peer, "2KMYLOCALPEER", "the binding is my store");
    }

    /// A publisher carrying both is offered both, in table order — so a reader
    /// sees the whole of what this peer has rather than whichever convention
    /// the code happened to privilege.
    #[test]
    fn a_publisher_carrying_both_is_offered_both() {
        let r = resolved(Some("https://billslab.com"));
        let found = vec![
            finding(crate::open_target::SITE_BROWSER, Publishes::Yes { units: Some(2) }),
            finding(crate::open_target::FEED, Publishes::Yes { units: None }),
        ];
        let got = opens(&found, &Phase::Done(r.clone()), "2KMYLOCALPEER");
        assert_eq!(
            got.iter().map(|o| o.window_type).collect::<Vec<_>>(),
            vec![crate::open_target::SITE_BROWSER, crate::open_target::FEED],
        );
    }

    /// ⛔ **A refusal offers nothing** — and a *"we could not tell"* offers
    /// nothing either, which is the arm a tidy version gets wrong by treating
    /// unknown as *probably yes, let them try*. A window opened on a hunch can
    /// only render an empty rail, and the reader cannot tell that from an
    /// answer.
    #[test]
    fn nothing_we_could_not_establish_is_offered() {
        let r = resolved(Some("https://billslab.com"));
        for outcome in [
            Publishes::No,
            Publishes::Partial { nodes_walked: 32 },
            Publishes::Unknown(Unknown::Unreachable("down".into())),
            Publishes::Unknown(Unknown::Withheld("cut".into())),
            Publishes::Unknown(Unknown::Unproven("bad sig".into())),
            Publishes::Unknown(Unknown::OurFloor("seq rollback".into())),
            Publishes::Unknown(Unknown::Exhausted),
        ] {
            let found = vec![finding(crate::open_target::SITE_BROWSER, outcome.clone())];
            assert!(
                opens(&found, &Phase::Done(r.clone()), "2KMYLOCALPEER").is_empty(),
                "`{}` is not a demonstration that there is anything to open",
                outcome.word()
            );
        }
    }

    /// Resolved WHO but not WHERE still opens nothing, even if findings somehow
    /// exist. The guard is on the decision, not on the acquisition.
    #[test]
    fn a_publisher_with_no_origin_is_offered_nothing_whatever_the_findings_say() {
        let found = vec![finding(crate::open_target::FEED, Publishes::Yes { units: None })];
        assert!(opens(&found, &Phase::Done(resolved(None)), "2KMYLOCALPEER").is_empty());
    }

    /// Nothing probed yet is not *"they publish nothing"*. The empty finding
    /// list and a list of refusals both offer nothing, and the **renderer** is
    /// what must keep them apart — asserted here so a later simplification that
    /// collapses `Phase::Idle` into an empty vec has something to red against.
    #[test]
    fn an_unprobed_publisher_offers_nothing_and_that_is_not_an_answer() {
        let r = resolved(Some("https://billslab.com"));
        assert!(opens(&Vec::new(), &Phase::Done(r), "2KMYLOCALPEER").is_empty());
        let idle: Phase<Publications> = Phase::Idle;
        assert_ne!(
            idle,
            Phase::Done(Vec::new()),
            "nothing asked and asked-and-empty must not compare equal"
        );
    }
}
