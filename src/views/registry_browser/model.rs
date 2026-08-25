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
    NameListing, Phase, PinOrigin, PinnedRegistry, RegistryBrowserOutput, ResolvedName,
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
}

impl RegistryBrowserModel {
    pub fn new(window_id: WindowId) -> Self {
        Self {
            window_id,
            listing: Rc::new(RefCell::new(Phase::Idle)),
            resolved: Rc::new(RefCell::new(Phase::Idle)),
            changed: Rc::new(Cell::new(false)),
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
        crate::session_config::active_registry_pin().map(|p| PinnedRegistry {
            peer_id: p.peer_id,
            origin: p.origin,
            // The window reads the durable/deployment pin. A tab-scoped `name
            // pin` lives in the Shell's own slot; wiring the two together is a
            // shared-state question and is deliberately not answered by
            // duplicating the slot here.
            source: PinOrigin::Deployment,
        })
    }

    pub fn render_output(&self, _peers: &Peers) -> RegistryBrowserOutput {
        RegistryBrowserOutput {
            pinned: self.pinned(),
            listing: self.listing.borrow().clone(),
            resolved: self.resolved.borrow().clone(),
            sessions: crate::content_site::session_cache::len(),
            browser_only: cfg!(not(target_arch = "wasm32")),
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
        self.mark();

        let slot = self.resolved.clone();
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
    pub fn open_in_site_browser(&self, peers: &Peers, target: &ResolvedName) -> Option<String> {
        let origin = target.origin.clone()?;
        let system_pid = peers.system_peer_id().to_string();
        crate::content_site::origins::set_origin(peers, &system_pid, &target.peer_id, &origin);
        self.mark();
        Some(target.peer_id.clone())
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
}
