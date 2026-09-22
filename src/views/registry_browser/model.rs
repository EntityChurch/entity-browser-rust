//! Registry Browser model — pinned registries, a **signed** name listing, and
//! what a resolve actually established.
//!
//! ## Why this window exists
//!
//! Everything under the naming chain was reachable from exactly one surface: the
//! Shell's `name` verb, which prints a preview into a scrollback. This is the
//! product surface for the same chain — *which registries am I pointed at, what
//! do they carry, and what was checked when I resolved one of them.*
//!
//! ## Where the state lives, and why it is not in the tree
//!
//! Window structs hold `window_id` + `peer_id` and the tree is the source of
//! truth — for **durable** state. Everything here is a *transient result of an
//! action in flight*: a listing recovered this session, the outcome of the last
//! resolve. Persisting it would be the `connection_health` mistake in new
//! clothes (a second, staler opinion about something the signed root already
//! answers), and a reload should re-derive it from the root rather than believe
//! a cache of ours.
//!
//! So it is in-memory with an explicit **change flag** ([`Self::take_changed`]),
//! which is the `MeetSession` shape and is load-bearing for the reason AP21
//! records: the results land **between** frames, off the frame loop, touching no
//! tree path, so no `WindowWatch` fires and a diff taken around a pump is equal
//! exactly when something just changed. Every mutation site sets the flag,
//! including inside the spawned landings.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use super::output::{
    NameListing, Phase, PinOrigin, PinnedRegistry, Publications, RegistryBrowserOutput,
    ResolvedName,
};
use crate::peers::Peers;
use crate::window::WindowId;

/// The trie prefix a registry publishes its by-name index under. The trie's key
/// set under this prefix **is** the name set (`EXTENSION-TREE` §3.1: a leaf is
/// `[key, value_hash]`).
const BY_NAME_PREFIX: &str = "system/registry/binding/by-name/";

pub struct RegistryBrowserModel {
    #[allow(dead_code)]
    window_id: WindowId,
    listing: Rc<RefCell<Phase<NameListing>>>,
    resolved: Rc<RefCell<Phase<ResolvedName>>>,
    changed: Rc<Cell<bool>>,
    /// Why the last pin attempt was refused. In memory and dropped on reload —
    /// it describes a keystroke, not state worth persisting.
    pin_error: Rc<RefCell<Option<String>>>,
    /// What the resolved publisher publishes, probed from **their** signed
    /// root. Session-only: it is an answer about one publisher at one moment,
    /// and a durable copy would be a record of a remote assertion with no
    /// trigger to re-check it (AP30/D24). Re-asked on every resolve.
    published: Rc<RefCell<Phase<Publications>>>,
}

impl RegistryBrowserModel {
    pub fn new(window_id: WindowId) -> Self {
        Self {
            window_id,
            listing: Rc::new(RefCell::new(Phase::Idle)),
            resolved: Rc::new(RefCell::new(Phase::Idle)),
            changed: Rc::new(Cell::new(false)),
            pin_error: Rc::new(RefCell::new(None)),
            published: Rc::new(RefCell::new(Phase::Idle)),
        }
    }

    /// Consume the change flag. **The surface must call this rather than diff
    /// its own output** — see the module docs: an async landing between frames
    /// is invisible to a before/after comparison taken around the pump.
    pub fn take_changed(&self) -> bool {
        self.changed.replace(false)
    }

    fn mark(&self) {
        self.changed.set(true);
    }

    /// The registry in force, and **where it came from**.
    ///
    /// Deployment-seeded and user-typed are deliberately distinguished all the
    /// way to the pixel: a name resolving through a registry the user never
    /// chose is the point of a seeded pin *and* exactly the thing that must not
    /// be silent [AP25].
    pub fn pinned(&self) -> Option<PinnedRegistry> {
        // ONE shared pin, read through the one expression of its precedence.
        // This used to read only the deployment seed while `name pin` wrote a
        // `Mutex` on the Shell window — so pinning in one surface left the other
        // reporting "no registry pinned", with no way to fix it from here at
        // all. Both write `session_config::set_user_registry_pin` now.
        crate::session_config::pinned_registry().map(|(p, src)| PinnedRegistry {
            peer_id: p.peer_id,
            origin: p.origin,
            source: match src {
                crate::session_config::PinSource::User => PinOrigin::User,
                crate::session_config::PinSource::Deployment => PinOrigin::Deployment,
            },
        })
    }

    /// Pin a registry from this window.
    ///
    /// **Refuses a non-canonical peer-id, with its reason.** A pin is consumed
    /// by this client, which derives the verification key out of the peer-id
    /// itself (`PinnedPublisher::from_peer_id`) — the SHA-256 legacy form
    /// carries no key, so pinning it produces a registry that can never verify
    /// anything, and the failure would surface later as an unresolvable name.
    /// Same bar and same reason as the emitter's `--registry-pin`.
    ///
    /// An **empty origin is legitimate** — it means same-origin, exactly as in
    /// `DeploymentConfig`'s `origins` map. An empty *peer-id* is not: pinning an
    /// origin would trust the origin.
    pub fn pin(&self, peer_id: &str, origin: &str) -> Result<(), String> {
        let peer_id = peer_id.trim();
        let origin = origin.trim();
        if peer_id.is_empty() {
            let e = crate::i18n::t("registry.pin_needs_peer_id", &[]);
            *self.pin_error.borrow_mut() = Some(e.clone());
            self.mark();
            return Err(e);
        }
        // `from_peer_id` is the "user typed an origin, there is no endpoint
        // document to read" constructor — exactly this case — and it returns
        // `None` precisely when the peer-id carries no key.
        if crate::content_site::signed_fetch::PinnedPublisher::from_peer_id(origin, peer_id)
            .is_none()
        {
            let e = crate::i18n::t("registry.pin_not_canonical", &[]);
            *self.pin_error.borrow_mut() = Some(e.clone());
            self.mark();
            return Err(e);
        }
        crate::session_config::set_user_registry_pin(Some(
            crate::session_config::RegistryPin {
                peer_id: peer_id.to_string(),
                origin: origin.to_string(),
            },
        ));
        *self.pin_error.borrow_mut() = None;
        self.clear_for_new_registry();
        Ok(())
    }

    /// Drop the user's pin, falling back to whatever the deployment seeded.
    pub fn unpin(&self) {
        crate::session_config::set_user_registry_pin(None);
        *self.pin_error.borrow_mut() = None;
        self.clear_for_new_registry();
    }

    /// Reset what belongs to the *previous* registry.
    ///
    /// The listing and the resolved name are that registry's answers; leaving
    /// them on screen under a new pin's identity row would attribute one
    /// registry's names to another — the same class of quiet mis-statement as a
    /// truncated listing that does not announce itself.
    fn clear_for_new_registry(&self) {
        *self.listing.borrow_mut() = Phase::Idle;
        *self.resolved.borrow_mut() = Phase::Idle;
        // The probe is an answer about the publisher the OLD registry named.
        // Every slot this function clears has the same reason; the only way to
        // keep that true is to add to it (`every_registry_scoped_slot_is_cleared`).
        *self.published.borrow_mut() = Phase::Idle;
        self.mark();
    }

    pub fn render_output(&self, peers: &Peers) -> RegistryBrowserOutput {
        RegistryBrowserOutput {
            pinned: self.pinned(),
            listing: self.listing.borrow().clone(),
            resolved: self.resolved.borrow().clone(),
            sessions: crate::content_site::session_cache::len(),
            browser_only: cfg!(not(target_arch = "wasm32")),
            // The store `open_in_site_browser` writes into, and so the only one
            // an opened Site Browser can read those sites from. **One accessor,
            // not a second `system_peer_id()` call** — the write and the window
            // binding agreeing is the whole correctness argument here, and two
            // independent expressions that happen to be equal is a coincidence
            // a future edit gets to break silently (AP44).
            local_peer: Self::reader_peer(peers),
            published: self.published.borrow().clone(),
            pin_error: self.pin_error.borrow().clone(),
        }
    }

    /// Enumerate the pinned registry's names **from its signed root**.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn browse(&self) {
        *self.listing.borrow_mut() =
            Phase::Failed(crate::i18n::t("registry.needs_browser", &[]));
        self.mark();
    }

    /// Enumerate the pinned registry's names **from its signed root** — not from
    /// the host-served `by-name.list`, which commits to nothing.
    ///
    /// The listing is bounded ([`DEFAULT_ENUMERATION_BUDGET`]) and carries
    /// whether it finished. A registry is allowed to be large and a browser must
    /// not download one to discover that; a partial answer is useful **provided
    /// it says it is partial**, which is what `complete` is for.
    ///
    /// [`DEFAULT_ENUMERATION_BUDGET`]: crate::content_site::signed_fetch::DEFAULT_ENUMERATION_BUDGET
    #[cfg(target_arch = "wasm32")]
    pub fn browse(&self) {
        let Some(pin) = self.pinned() else {
            *self.listing.borrow_mut() = Phase::Failed(crate::i18n::t("registry.no_pin", &[]));
            self.mark();
            return;
        };
        let Some(session) =
            crate::content_site::session_cache::session_for(&pin.peer_id, &pin.origin)
        else {
            *self.listing.borrow_mut() = Phase::Failed(crate::i18n::t("registry.no_key", &[]));
            self.mark();
            return;
        };

        *self.listing.borrow_mut() = Phase::Running;
        self.mark();

        let slot = self.listing.clone();
        let changed = self.changed.clone();
        crate::views::shell::model::spawn_task(async move {
            let src = crate::content_site::http_poll::FetchBinSource;
            let out = session
                .enumerate_bounded(
                    &src,
                    BY_NAME_PREFIX,
                    crate::content_site::signed_fetch::DEFAULT_ENUMERATION_BUDGET,
                )
                .await;
            *slot.borrow_mut() = match out {
                Ok(e) => Phase::Done(NameListing {
                    names: e
                        .keys
                        .iter()
                        .filter_map(|k| k.rsplit('/').next())
                        .filter(|n| !n.is_empty())
                        .map(|n| n.to_string())
                        .collect(),
                    complete: e.complete,
                    nodes_walked: e.nodes_walked,
                }),
                // **A failed walk is reported as a failure, never as an empty
                // registry.** `IncompleteWalk` in particular means the origin
                // withheld something its own signed root declares — which is a
                // statement about the origin, not about how many names exist.
                Err(e) => Phase::Failed(e.to_string()),
            };
            changed.set(true);
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn resolve(&self, _name: &str) {
        *self.resolved.borrow_mut() =
            Phase::Failed(crate::i18n::t("registry.needs_browser", &[]));
        *self.published.borrow_mut() = Phase::Idle;
        self.mark();
    }

    /// Resolve one name and record **what was checked**, not merely the answer.
    #[cfg(target_arch = "wasm32")]
    pub fn resolve(&self, name: &str) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        let Some(pin) = self.pinned() else {
            *self.resolved.borrow_mut() = Phase::Failed(crate::i18n::t("registry.no_pin", &[]));
            self.mark();
            return;
        };
        let Some(session) =
            crate::content_site::session_cache::session_for(&pin.peer_id, &pin.origin)
        else {
            *self.resolved.borrow_mut() = Phase::Failed(crate::i18n::t("registry.no_key", &[]));
            self.mark();
            return;
        };

        *self.resolved.borrow_mut() = Phase::Running;
        // A finding is an answer about ONE publisher. Carrying the previous
        // one forward while a new name resolves would attach the last
        // publisher's conventions to this one, on screen, for as long as the
        // probe takes — the stalest possible version of the guess this change
        // exists to retire.
        *self.published.borrow_mut() = Phase::Idle;
        self.mark();

        let slot = self.resolved.clone();
        let published = self.published.clone();
        let changed = self.changed.clone();
        crate::views::shell::model::spawn_task(async move {
            let src = crate::content_site::http_poll::FetchBinSource;
            let now_ms = js_sys::Date::now() as u64;
            // The §6a resolver-side ceiling is a REQUIRED argument, deliberately:
            // an overload that defaulted it is how the protecting half gets
            // skipped at the one call site that needed it.
            let policy = crate::content_site::named_site::ResolverPolicy {
                max_ttl_ms: crate::session_config::active_resolver_ceiling_ms(),
            };
            *slot.borrow_mut() = match crate::content_site::named_site::resolve_name(
                &src, &session, &name, now_ms, &policy,
            )
            .await
            {
                Ok(t) => {
                    let ev = &t.evidence;
                    Phase::Done(ResolvedName {
                        name: t.name.clone(),
                        peer_id: t.peer_id.clone(),
                        origin: t.origin.clone(),
                        association_committed: ev.association_committed,
                        name_checked: ev.name_checked,
                        revocation_checked: ev.revocation_checked,
                        expires_at_ms: ev.expires_at_ms,
                        clamped: ev
                            .ttl_was_clamped()
                            .then_some((ev.issued_ttl_ms, ev.effective_ttl_ms)),
                    })
                }
                Err(e) => Phase::Failed(e.to_string()),
            };
            changed.set(true);

            // ── and then ask the publisher what they publish ──────────────
            //
            // Chained rather than spawned beside the resolve, because the probe
            // needs the peer-id the resolve produced. A failed resolve probes
            // nothing: there is no publisher to ask.
            let Phase::Done(target) = slot.borrow().clone() else { return };
            let Some(origin) = target.origin.clone() else {
                // Resolved WHO but not WHERE. **Not `Idle`** — a reader who
                // sees nothing here would read it as "not asked yet" and wait
                // for an answer that is never coming.
                *published.borrow_mut() =
                    Phase::Failed(crate::i18n::t("registry.published_no_origin", &[]));
                changed.set(true);
                return;
            };
            // The publisher's own session, NOT the registry's — the whole claim
            // is that this is the publisher's signed root. `session_for` is
            // keyed by peer-id so this shares the `seq` floor with every other
            // surface reading them, which is the point of that cache.
            let Some(theirs) =
                crate::content_site::session_cache::session_for(&target.peer_id, &origin)
            else {
                *published.borrow_mut() =
                    Phase::Failed(crate::i18n::t("registry.published_no_key", &[]));
                changed.set(true);
                return;
            };
            *published.borrow_mut() = Phase::Running;
            changed.set(true);
            let found = crate::publication_probe::probe_all(
                &crate::content_site::http_poll::FetchBinSource,
                &theirs,
                &target.peer_id,
            )
            .await;
            *published.borrow_mut() = Phase::Done(found);
            changed.set(true);
        });
    }

    /// Hand a resolved name to the Site Browser.
    ///
    /// **What this does and does not establish, because the gap is the point.**
    /// The origin recorded here came out of a **registry-signed binding**, which
    /// is strictly better than the deployment document's list — a hostile host
    /// cannot invent it. What it does *not* do is make the pages the Site Browser
    /// then fetches verified: that path reads no signed root, so the origin still
    /// chooses which content each path points to, and the Site Browser correctly
    /// labels the result *not verified* (`GUIDE-SERVING-MODE` §8).
    ///
    /// So this wire is honest and incomplete on purpose. Closing it is backlog
    /// B-3, and it must go through `session_cache::session_for_layout` — keyed on
    /// the **publisher**, or a second entry point means a second `seq` floor of
    /// zero and the rollback defence is gone.
    /// **Registering an origin says WHERE a peer is; it does not say WHAT it
    /// hosts, and the Site Browser's directory reads the second.** Without the
    /// warm below, this opened a correctly-bound Site Browser onto *"No external
    /// sites cached"* — measured against the live registry, resolving a real
    /// name to a real publisher. The reader has no site id to type, so an empty
    /// directory is a dead end that looks like the publisher has nothing.
    ///
    /// `warm_peer_sites` already exists for exactly this and had **one** call
    /// site: `boot_load`, over the origins `/entity-deployment.json` declares.
    /// So the enumeration only ever ran for peers the deployment named at
    /// startup, and a peer learned mid-session from a signed binding — which is
    /// the entire point of the naming chain — was never enumerated at all. Its
    /// own comment says the alternative is *"only after a manual navigate"*,
    /// which is unavailable here for want of a site id.
    ///
    /// Fire-and-forget and manifest-pinned, as at boot: a publisher that is down
    /// leaves the rail empty rather than blocking the open.
    /// The peer whose store an *Open in Site Browser* writes into **and** which
    /// the spawned window is bound to. Both must be this one value; see
    /// `local_peer` on the output, and
    /// `the_open_writes_into_the_same_store_the_spawned_window_reads`.
    ///
    /// It is the **system** peer, not the target: the sites are cached in our
    /// tree, not the publisher's. A window bound to any other local peer reads
    /// its own empty registry — which is real, and is what
    /// [`crate::content_site::origins::mirror_origins`] repairs at spawn for the
    /// windows that get bound elsewhere.
    fn reader_peer(peers: &Peers) -> String {
        peers.system_peer_id().to_string()
    }

    pub fn open_in_site_browser(&self, peers: &Peers, target: &ResolvedName) -> Option<String> {
        let origin = target.origin.clone()?;
        let system_pid = Self::reader_peer(peers);
        crate::content_site::origins::set_origin(peers, &system_pid, &target.peer_id, &origin);
        crate::content_site::discovery::warm_peer_sites(
            peers,
            &system_pid,
            vec![(target.peer_id.clone(), origin)],
        );
        self.mark();
        Some(target.peer_id.clone())
    }

    /// Fill every registry-scoped slot with something that is not `Idle`, so
    /// [`tests::every_registry_scoped_slot_is_cleared`] can measure the clear
    /// rather than measure a fresh model.
    ///
    /// A test-only setter and not a widened field: the slots are `Rc<RefCell<…>>`
    /// precisely so the spawned landings own them, and making them writable from
    /// outside is how a second writer appears.
    #[cfg(test)]
    fn seed_every_slot_for_test(&self) {
        *self.listing.borrow_mut() =
            Phase::Done(NameListing { names: vec!["a".into()], complete: true, nodes_walked: 1 });
        *self.resolved.borrow_mut() = Phase::Running;
        *self.published.borrow_mut() = Phase::Done(Vec::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::registry_browser::output::Phase;

    /// **"Nothing asked yet" and "asked, and the answer was empty" are different
    /// states**, and a surface that renders them the same way tells a user a
    /// registry is empty when nothing has been fetched.
    ///
    /// This is the same distinction the whole naming arc keeps re-learning at the
    /// protocol layer — absent vs withheld — arriving at the UI layer, where the
    /// cost is a person concluding a registry carries no names.
    #[test]
    fn idle_is_not_an_empty_result() {
        let m = RegistryBrowserModel::new(1);
        assert!(matches!(m.listing.borrow().clone(), Phase::Idle));
        assert!(matches!(m.resolved.borrow().clone(), Phase::Idle));

        let empty_but_asked: Phase<NameListing> = Phase::Done(NameListing {
            names: vec![],
            complete: true,
            nodes_walked: 1,
        });
        assert_ne!(
            empty_but_asked,
            Phase::Idle,
            "a completed walk that found no names must not compare equal to having asked nothing"
        );
    }

    /// ⭐ **Every slot that belongs to the OLD registry is cleared when the pin
    /// moves — and this is a census, because the rule needs the word "every".**
    ///
    /// `clear_for_new_registry` is a *"and also do X here"* hook, which is
    /// AP44's shape: correct the day it lands and one field behind on the first
    /// slot somebody adds. The probe result was exactly that field — it is an
    /// answer about the publisher the *previous* registry named, and left on
    /// screen under a new pin's identity row it attributes one registry's
    /// publisher to another.
    ///
    /// The destructure is what makes it a census rather than three assertions:
    /// `#[deny(unused_variables)]` plus naming every field means a new one is a
    /// **compile error** at this site, so whoever adds it has to decide whether
    /// it survives a pin change. That is the shape
    /// `every_deployment_declared_field_says_who_owns_it` had to be repaired
    /// into after it spent months counting a literal.
    #[test]
    #[deny(unused_variables)]
    fn every_registry_scoped_slot_is_cleared() {
        let peers = Peers::new_direct();
        let m = RegistryBrowserModel::new(1);
        m.seed_every_slot_for_test();

        // Anti-vacuity: if the seed did not take, the clear below proves nothing.
        let before = m.render_output(&peers);
        assert_ne!(before.listing, Phase::Idle);
        assert_ne!(before.resolved, Phase::Idle);
        assert_ne!(before.published, Phase::Idle);

        m.clear_for_new_registry();
        let RegistryBrowserOutput {
            listing,
            resolved,
            published,
            // Not registry-scoped — named so the destructure stays exhaustive
            // and a new field cannot slip past by being ignored.
            pinned: _,
            sessions: _,
            browser_only: _,
            local_peer: _,
            pin_error: _,
        } = m.render_output(&peers);
        assert_eq!(listing, Phase::Idle, "the old registry's names survived the pin change");
        assert_eq!(resolved, Phase::Idle, "the old registry's resolve survived the pin change");
        assert_eq!(
            published,
            Phase::Idle,
            "the previous publisher's conventions are still on screen under a new registry"
        );
    }

    /// The change flag is consumed, not merely readable — a surface that peeked
    /// without clearing would repaint forever, and one that never set it would
    /// freeze on a stale answer (AP21, and the bug that froze `meet` on
    /// "Searching…").
    #[test]
    fn the_change_flag_is_set_on_mutation_and_consumed_once() {
        let m = RegistryBrowserModel::new(1);
        assert!(!m.take_changed(), "a fresh model has nothing to repaint");
        m.mark();
        assert!(m.take_changed(), "a mutation must be announced");
        assert!(!m.take_changed(), "and consumed exactly once");
    }

    /// Native builds say so rather than rendering an empty panel that reads as a
    /// broken registry. The fetch is `window.fetch`, whose futures are `!Send`.
    #[test]
    fn the_native_build_reports_that_it_cannot_fetch() {
        let m = RegistryBrowserModel::new(1);
        m.browse();
        let phase = m.listing.borrow().clone();
        match phase {
            Phase::Failed(msg) => assert!(
                msg.to_lowercase().contains("browser"),
                "the native stub must name the reason, got {msg:?}"
            ),
            other => panic!("native browse must fail loudly, got {other:?}"),
        }
    }

    /// **The bug this window had for its whole life.** `name pin` in the Shell
    /// wrote a `Mutex` on the Shell's own model; this window read only the
    /// deployment seed. So pinning a registry left the Registry Browser saying
    /// "no registry pinned", with no field here to fix it and no indication the
    /// pin existed a few hundred lines away.
    ///
    /// The gate is that a pin made *here* is visible *there* and vice versa —
    /// which is only meaningful because both now go through one function. Note
    /// what it does NOT do: assert on a field of either model, since that is
    /// exactly the shape that let two slots drift apart.
    #[test]
    fn a_pin_set_in_one_surface_is_the_pin_every_surface_reads() {
        let m = RegistryBrowserModel::new(1);
        crate::session_config::set_user_registry_pin(None);
        crate::session_config::set_active_registry_pin(None);

        assert!(m.pinned().is_none(), "fail closed: nothing seeded, nothing typed");

        // A real canonical-form peer-id — the check derives a key from it, so a
        // made-up string would exercise the refusal instead of the success.
        let pid = entity_crypto::Keypair::generate().peer_id().to_string();
        m.pin(&pid, "https://reg.example").expect("a canonical peer-id pins");

        let p = m.pinned().expect("this window sees its own pin");
        assert_eq!(p.peer_id, pid);
        assert_eq!(p.source, PinOrigin::User);
        // …and the Shell resolves through the same one, without a second slot.
        assert_eq!(
            crate::session_config::pinned_registry().map(|(p, s)| (p.peer_id, s)),
            Some((pid.clone(), crate::session_config::PinSource::User)),
        );

        // Unpinning falls back to the deployment's seed rather than to nothing —
        // which is why the two are separate slots.
        crate::session_config::set_active_registry_pin(Some(
            crate::session_config::RegistryPin {
                peer_id: "2KDeployment".into(),
                origin: "https://seeded.example".into(),
            },
        ));
        m.unpin();
        let p = m.pinned().expect("the deployment's seed survived the user's pin");
        assert_eq!(p.peer_id, "2KDeployment");
        assert_eq!(p.source, PinOrigin::Deployment);

        crate::session_config::set_active_registry_pin(None);
    }

    /// A pin is a key, and the two refusals are the two ways it can fail to be
    /// one. Both must come back as **text** — a Pin button that silently does
    /// nothing tells the one person who could fix it nothing at all, which is
    /// the operator-surface failure this repo keeps meeting.
    #[test]
    fn a_peer_id_that_carries_no_key_is_refused_with_its_reason() {
        let m = RegistryBrowserModel::new(1);
        crate::session_config::set_user_registry_pin(None);
        crate::session_config::set_active_registry_pin(None);

        let e = m.pin("   ", "https://reg.example").expect_err("an origin alone is not a pin");
        assert!(!e.is_empty(), "the refusal must say something");

        let e = m
            .pin("not-a-peer-id", "https://reg.example")
            .expect_err("a string that carries no key cannot be pinned");
        assert!(!e.is_empty());
        assert!(m.pinned().is_none(), "and neither refusal may leave a pin behind");

        // An EMPTY origin is legitimate — same-origin, exactly as in the
        // deployment's `origins` map. Only an empty peer-id is refused.
        let pid = entity_crypto::Keypair::generate().peer_id().to_string();
        m.pin(&pid, "").expect("same-origin is a valid place for a registry to live");
        assert_eq!(m.pinned().map(|p| p.origin), Some(String::new()));

        crate::session_config::set_user_registry_pin(None);
    }

    /// **The whole correctness argument of *Open in Site Browser*, in one
    /// assertion.** The click writes the resolved origin into one peer's store
    /// and binds the new window to another value; if those two ever name
    /// different peers, the window opens onto an empty registry and reports a
    /// site that resolved perfectly as unreachable — measured once already, as
    /// `the_rail_reads_my_store_so_a_foreign_bound_window_sees_nothing`.
    ///
    /// They agree today because both come from `reader_peer`. This asserts the
    /// *observable* pair — the origin actually landed in the store the output
    /// names — so it survives either side being rewritten, which a test
    /// comparing two calls to `system_peer_id()` would not.
    ///
    /// **Falsified:** hard-coding `local_peer` to another peer reds it.
    #[test]
    fn the_open_writes_into_the_same_store_the_spawned_window_reads() {
        let peers = Peers::new_direct();
        let m = RegistryBrowserModel::new(1);
        let target = ResolvedName {
            name: "pub.example".to_string(),
            peer_id: "PUBPEER".to_string(),
            origin: Some("https://pub.example".to_string()),
            association_committed: true,
            name_checked: true,
            revocation_checked: true,
            expires_at_ms: 0,
            clamped: None,
        };

        assert!(m.open_in_site_browser(&peers, &target).is_some(), "the open proceeds");

        let bound_to = m.render_output(&peers).local_peer;
        assert_eq!(
            crate::content_site::origins::get_origin(&peers, &bound_to, "PUBPEER").as_deref(),
            Some("https://pub.example"),
            "the window is bound to a peer whose registry does not carry the origin \
             the click just resolved — the site will report as unreachable"
        );
    }

    /// ⭐⭐ **THE SAME-ORIGIN BINDING — the shape every single-domain deployment
    /// publishes, and the one this surface's tests did not contain.**
    ///
    /// Found in production on 2026-09-17: resolve a name, press *Open*, and the
    /// publisher was **un-named for the whole profile, permanently**. The Feed
    /// window said *"this deployment does not know where they are hosted"* and
    /// the Site Browser rail said **"No sites yet"**, on the same boot that had
    /// just cached all seven of that publisher's manifests. Two reloads did not
    /// heal it, and nothing in the product could: the row this click writes is
    /// marked [`SOURCE_USER`](crate::content_site::origins::SOURCE_USER), so
    /// boot's adoption correctly refused to touch it —
    /// `outcome = KeptUserOverride { theirs: "" }`, in the log, every time.
    ///
    /// The cause was one line in `origins::decode_origin`, which read a
    /// **recorded** empty origin as *no record*. `""` is same-origin — this
    /// codebase says so in four places and every URL builder already treats it
    /// that way — so the write landed and its own reader could not see it.
    ///
    /// ⇒ ***the sibling above passes with the defect fully present, because its
    /// fixture names a concrete host.*** `origin: Some("https://pub.example")`
    /// is the one shape that cannot exhibit this, and it was the only shape
    /// tested. *A test population you generated cannot contain the shape you are
    /// missing* — and here the missing shape is not an encoding or another
    /// implementation, it is **the ordinary deployment**: one domain serving its
    /// own publisher, which is what `--bind=<name>=<peer>@/` emits and what
    /// `/entity-deployment.json` means by `"origins": {"<peer>": ""}`.
    ///
    /// Asserted through `get_origin` **and** the roster, because the two
    /// surfaces that went dark read different accessors: the Feed panel routes
    /// through `get_origin` and both site rails enumerate `list_origins`.
    #[test]
    fn a_publisher_bound_same_origin_is_still_routable_after_the_open() {
        let peers = Peers::new_direct();
        let m = RegistryBrowserModel::new(1);
        let target = ResolvedName {
            name: "billslab.com".to_string(),
            peer_id: "PUBPEER".to_string(),
            // What a single-domain registry publish actually binds. `"/"` is
            // the emitted spelling; it is stored trimmed, so this is the
            // production value byte for byte.
            origin: Some("/".to_string()),
            association_committed: true,
            name_checked: true,
            revocation_checked: true,
            expires_at_ms: 0,
            clamped: None,
        };

        assert!(m.open_in_site_browser(&peers, &target).is_some(), "the open proceeds");
        let bound_to = m.render_output(&peers).local_peer;

        assert_eq!(
            crate::content_site::origins::get_origin(&peers, &bound_to, "PUBPEER").as_deref(),
            Some(""),
            "a same-origin publisher was un-named by the click that opened them — \
             the Feed window renders `no_route` about a publisher one relative URL away"
        );
        assert!(
            crate::content_site::origins::list_origins(&peers, &bound_to)
                .iter()
                .any(|(p, _)| p == "PUBPEER"),
            "the roster lost the publisher, so every surface that ENUMERATES rather \
             than resolves goes dark too — this is the Site Browser's \"No sites yet\" \
             over a store holding that publisher's manifests"
        );
    }
}
