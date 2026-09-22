//! Durable window-state hydration — the shared half of AP41's repair.
//!
//! Every window model in the AP41 class does the same three things wrong in the
//! same way, and the same three things right in the same way. This module owns
//! the *right* half so that six call sites cannot each get it subtly different,
//! and leaves exactly one decision at each call site: **what "adopt" means for
//! this surface.**
//!
//! # Why a helper here, when AP41's charter entry says "a hook, not a helper"
//!
//! That sentence is about a helper that would let an author **skip** the traps
//! — *"a shared helper that skipped this would reintroduce the user-themes
//! resurrection race in six places at once."* This one does the opposite: the
//! traps are inside it and cannot be skipped, and what is left outside is the
//! one thing genuinely specific to a surface. The refinement is worth naming
//! rather than silently contradicting: **factor the invariants, not the
//! decision.**
//!
//! # The three traps, all handled here
//!
//! * **An errored round-trip is not an answer** — [`Hydration::Unheard`],
//!   change nothing. A read that cannot answer must never reset a surface to
//!   its default (AP30 corollary (a)).
//! * **A change that lands during the await is newer than the read** —
//!   [`Hydration::Superseded`]. Guarded by a **witness**, not a counter: the
//!   caller supplies a closure returning the current persisted-half bytes, and
//!   they are compared before and after. A counter needs every mutator to
//!   announce itself and the first one written here already missed one (AP44).
//! * **Someone else's state may be in our slot** — window ids are reused across
//!   a reload, so a decode by field name adopts a foreign window type's entity
//!   (AP42). The `entity_type` discriminator is checked *here*, and a mismatch
//!   reports as [`Hydration::NonePersisted`] — "no state of ours", the weakest
//!   claim, rather than something specific and wrong (AP40).
//!
//! # The one decision this cannot make for you: MERGE or ASSIGN
//!
//! `adopt` receives the entity and applies it however the surface requires.
//! Measured over the eight models in the class, there are two answers and the
//! split is not obvious from the outside:
//!
//! * **Assign** is safe where the model's inner struct is *fully persisted* —
//!   every field appears in `to_entity`, so a decoded state is a complete
//!   state. `query_console`, `execute_console`, `peer_connections` and
//!   `chain_trace` are all like this (verified field-by-field). So is
//!   `knowledge_base`, where the assign is already scoped to the `state`
//!   sub-field and its session-only siblings (`cached`, `root`, `known_slugs`)
//!   sit outside it.
//! * **Merge** is required where persisted and session-only state share one
//!   struct behind one decode path. `shell` holds `scrollback` there, and
//!   `entity_tree` holds `root` / `visible_rows` / `selected_entity` / `known`.
//!   For those, assigning a decoded state adopts the persisted half **and
//!   destroys the screen**.
//!
//! # And the witness is not always `to_entity()`
//!
//! `entity_tree` is the counter-example that proves the closure has to be the
//! caller's: its persisted `expanded_paths` is derived from the live tree
//! (`collect_expanded(&inner.root)`), which changes as subscription events
//! arrive *during* the await. A witness over the whole persisted form would
//! therefore differ almost every time and report `Superseded` forever, never
//! hydrating. Its witness covers only the user-intent fields. **A witness must
//! cover what the USER can change, not everything that happens to be written
//! down.**
//!
//! [AP30, AP40, AP41, AP42, AP44, `window.rs` `WindowView::hydrate_durable`,
//!  `tests/window_hydration_census.rs`]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use entity_entity::Entity;

use crate::peers::Peers;
use crate::window::Hydration;

/// The D13 line for the whole AP41 class: **exactly one line per surface per
/// resolution, naming which of the five outcomes it was.**
///
/// # Why this exists as a function instead of five `tracing!` calls
///
/// Because there were five `tracing!` calls, and they did not cover the
/// outcomes. Measured 2026-08-31, before this: `NonePersisted` logged
/// **nothing**, and the construction-read short-circuit — *the entire happy path
/// on the shipped Direct arm* — logged nothing either. So a Site Browser window
/// that resolved its location correctly said so on the Worker arm and was
/// completely silent on the default one, and an incident could not distinguish
/// *"we restored your page"* from *"you never had one"* from *"we never looked"*.
/// That is the same silence the navigation audit's telemetry-gap map called a
/// D13 violation *regardless of the cause* — fixed for the overlay by
/// `boot_load`'s own line, and left unfixed for every window.
///
/// The overlay keeps its `boot_load` line (it reports the awaited outcome at the
/// boot step, which is where an incident reads it); this is the per-surface one
/// underneath, and it fires for windows too, which have no boot step of their
/// own.
///
/// `surface` is the model's own name, not the window type: two windows of the
/// same type resolve separately and are told apart by `path`.
pub fn report(surface: &str, path: &str, outcome: Hydration) -> Hydration {
    // One level for all five. A `warn` for `Unheard` would be right on its own
    // merits, but splitting the level splits the grep, and the field is what
    // carries the severity — the caller that cares about `unheard` is looking
    // for that word, not for a level.
    tracing::info!(
        surface = %surface,
        path = %path,
        outcome = outcome.label(),
        "window state resolved against the durable tree"
    );
    outcome
}

/// Build the durable-hydration future for one window surface, or `None` if the
/// surface is already hydrated and there is nothing to correct.
///
/// Returns `None` when the construction-time synchronous read already answered
/// — the Direct arm whenever the state is genuinely present. Without that
/// short-circuit this would put an L1 round-trip on every window spawn that
/// never needed one.
///
/// Everything the future needs is owned before it is created, so it holds no
/// borrow of the model or of [`Peers`] across the round-trip. That is what lets
/// the fire-and-forget form (`spawn_local`) exist at all.
///
/// # Parameters that are easy to get wrong
///
/// * `state_type` — this surface's `STATE_TYPE` constant, the AP42
///   discriminator. Not the *path*: two window types sharing one state type is
///   a bug the discriminator is blind to (it separates readers that disagree
///   about their type, never two that agree).
/// * `witness` — returns the bytes of the persisted half **as the user can
///   affect it**. Called twice: once before the round-trip, once after. See the
///   module docs for why `entity_tree`'s is narrower than its `to_entity`.
/// * `adopt` — applies the decoded entity. Merge or assign; see the module
///   docs. Runs only when the read produced *this surface's* state and nothing
///   changed underneath it.
pub fn durable_hydration_job<W, A>(
    peers: &Peers,
    peer_id: &str,
    path: String,
    state_type: &'static str,
    hydrated: Arc<AtomicBool>,
    witness: W,
    adopt: A,
) -> Option<impl std::future::Future<Output = Hydration> + 'static>
where
    W: Fn() -> Vec<u8> + 'static,
    A: FnOnce(&Entity) + 'static,
{
    if hydrated.load(Ordering::SeqCst) {
        // The sync read already answered (Direct arm). Reported rather than
        // returned silently: this is the happy path on the shipped arm, and it
        // was the one outcome no surface said anything about.
        report(state_type, &path, Hydration::AlreadyResolved);
        return None;
    }
    let before = witness();
    // Created here, from `&Peers`, and `'static` once boxed. Creating a future
    // does not start it, so this costs nothing on the path that returned above.
    let read_fut = peers.get_entity_async(peer_id, &path);
    Some(async move {
        let read = read_fut.await;
        if witness() != before {
            // Something the user drove changed across the await. What is on
            // screen is newer than this read; leave it alone. Deliberately NOT
            // marked hydrated — the change is in memory, so there is nothing
            // left to adopt.
            return report(state_type, &path, Hydration::Superseded);
        }
        let outcome = match read {
            Ok(Some(entity)) if entity.entity_type == state_type => {
                adopt(&entity);
                hydrated.store(true, Ordering::SeqCst);
                Hydration::Adopted
            }
            Ok(_) => {
                // A real answer, and it covers two cases that are the same fact
                // for us: the tree holds nothing here, or it holds another
                // window type's state at our reused id (AP42). Either way this
                // surface has no persisted state. The default stands and we
                // write nothing — "you have no history" is not a fact to
                // record.
                hydrated.store(true, Ordering::SeqCst);
                Hydration::NonePersisted
            }
            Err(e) => {
                // NOT an answer. Change nothing, and say so — a silent
                // best-effort here is how a transient round-trip failure reads
                // as "this profile has no history". The error text is its own
                // line because `report` carries the outcome, not the cause.
                tracing::warn!(
                    path = %path,
                    error = %e,
                    "durable window-state read failed — keeping the current state rather \
                     than falling back to the default"
                );
                Hydration::Unheard
            }
        };
        report(state_type, &path, outcome)
    })
}

/// Drive a hydration job to completion in the background.
///
/// The window factory cannot await: a window is constructed long after
/// `boot_load` finished awaiting things, inside the synchronous frame loop.
#[cfg(target_arch = "wasm32")]
pub fn spawn_hydration(job: Option<impl std::future::Future<Output = Hydration> + 'static>) {
    if let Some(job) = job {
        wasm_bindgen_futures::spawn_local(async move {
            job.await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::chain_trace::model::{ChainTraceState, STATE_TYPE};

    // The decision table for the shared machinery, tested ONCE here rather than
    // six times across the models that use it. `ChainTraceState` is the vehicle
    // — the smallest real persisted state in the class — and nothing below
    // depends on which one it is.
    //
    // Direct-only on the native target, where the sync and async reads hit the
    // SAME in-process store, so the arm-split that PRODUCES the AP41 defect is
    // not reproducible here. What IS reachable is where the ways to get this
    // wrong live: adopting on a miss, wiping on an error, clobbering a change
    // that beat the read home, and adopting someone else's state.
    //
    // **`Hydration::Unheard` has no test, and that is a stated gap.**
    // `Peers::get_entity_async` on the Direct arm wraps a store read in a ready
    // `Ok(..)` and cannot fail, so the error arm is unreachable natively. It is
    // held by the `?` -> `Err` path in `WorkerPeerStore::get_entity_async` and
    // by review, not by a gate. Asserting something adjacent here to claim
    // coverage would be AP31.

    fn path(peer: &str) -> String {
        crate::app_paths::window_state_path(crate::app_paths::APP_ID, peer, 1)
    }

    fn state(id: &str) -> ChainTraceState {
        ChainTraceState {
            chain_id: id.to_string(),
        }
    }

    /// Build a job over a fresh cell, with the standard witness and an
    /// assigning adopt — the shape five of the six models use.
    fn job_over(
        peers: &Peers,
        peer: &str,
        cell: &Arc<std::sync::Mutex<ChainTraceState>>,
        hydrated: &Arc<AtomicBool>,
    ) -> Option<impl std::future::Future<Output = Hydration> + 'static> {
        let adopt = cell.clone();
        let witness = cell.clone();
        durable_hydration_job(
            peers,
            peer,
            path(peer),
            STATE_TYPE,
            hydrated.clone(),
            move || witness.lock().unwrap().to_entity().data,
            move |e| *adopt.lock().unwrap() = ChainTraceState::from_entity(e),
        )
    }

    /// **P** — state in the durable tree is adopted by a surface that has not
    /// read one.
    #[tokio::test]
    async fn a_persisted_state_is_adopted() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let cell = Arc::new(std::sync::Mutex::new(ChainTraceState::default()));
        let hydrated = Arc::new(AtomicBool::new(false));
        // `seed_write`, not `dispatch_write`: on Direct it is a sync L0 put, so
        // the value is there before the read. `dispatch_write` spawns and the
        // read would race it — which would take this through `NonePersisted`
        // and prove nothing.
        peers.seed_write(&pid, path(&pid), state("chain-7").to_entity());

        let job = job_over(&peers, &pid, &cell, &hydrated).expect("un-hydrated surface has a job");
        assert_eq!(job.await, Hydration::Adopted);
        assert_eq!(cell.lock().unwrap().chain_id, "chain-7");
        assert!(hydrated.load(Ordering::SeqCst));
    }

    /// **N2 — absence of evidence changes nothing**, and it does not write.
    #[tokio::test]
    async fn nothing_persisted_keeps_the_default_and_writes_nothing() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let cell = Arc::new(std::sync::Mutex::new(state("in-memory-only")));
        let hydrated = Arc::new(AtomicBool::new(false));

        let job = job_over(&peers, &pid, &cell, &hydrated).expect("job");
        assert_eq!(job.await, Hydration::NonePersisted);
        assert_eq!(
            cell.lock().unwrap().chain_id,
            "in-memory-only",
            "a read that answered NOTHING must not reset the surface"
        );
        assert!(
            peers.get_entity(&pid, &path(&pid)).is_none(),
            "hydration must not WRITE — it is a read that corrects an earlier read"
        );
    }

    /// **N1 — a change that lands during the round-trip wins.** The durable
    /// value the read is carrying is older than the screen.
    #[tokio::test]
    async fn a_change_during_the_read_is_not_clobbered() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let cell = Arc::new(std::sync::Mutex::new(ChainTraceState::default()));
        let hydrated = Arc::new(AtomicBool::new(false));
        peers.seed_write(&pid, path(&pid), state("stale").to_entity());

        // The round-trip starts here...
        let job = job_over(&peers, &pid, &cell, &hydrated).expect("job");
        // ...and the user changes something before it lands.
        *cell.lock().unwrap() = state("what-the-user-just-typed");

        assert_eq!(job.await, Hydration::Superseded);
        assert_eq!(
            cell.lock().unwrap().chain_id,
            "what-the-user-just-typed",
            "the older durable read must not drag the surface backwards"
        );
        assert!(
            !hydrated.load(Ordering::SeqCst),
            "a superseded read leaves nothing adopted, so the surface is not hydrated"
        );
    }

    /// **AP42** — window ids are reused, so the entity in our slot may have been
    /// written by another window type. That is *no state of ours*, reported as
    /// the weakest claim rather than something specific and wrong (AP40).
    ///
    /// The check has to be here and not left to the decoder: every
    /// `from_entity` in this codebase reports a type mismatch as `Default`,
    /// which is indistinguishable from a successful read of a default-valued
    /// state — so a decoder-only guard would still let `Adopted` be reported,
    /// and would mark the surface hydrated on someone else's entity.
    #[tokio::test]
    async fn another_window_types_state_in_our_slot_is_not_adopted() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let cell = Arc::new(std::sync::Mutex::new(state("ours")));
        let hydrated = Arc::new(AtomicBool::new(false));
        let foreign = Entity::new(
            crate::views::knowledge_base::model::STATE_TYPE,
            state("theirs").to_entity().data,
        )
        .expect("foreign entity well-formed");
        peers.seed_write(&pid, path(&pid), foreign);

        let job = job_over(&peers, &pid, &cell, &hydrated).expect("job");
        assert_eq!(job.await, Hydration::NonePersisted);
        assert_eq!(
            cell.lock().unwrap().chain_id,
            "ours",
            "a foreign window type's entity must not reach this surface"
        );
    }

    /// A surface whose sync read already answered skips the round-trip. Without
    /// it, the fix puts an L1 round-trip on every window spawn that never
    /// needed one.
    #[tokio::test]
    async fn an_already_hydrated_surface_schedules_no_read() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let cell = Arc::new(std::sync::Mutex::new(ChainTraceState::default()));
        let hydrated = Arc::new(AtomicBool::new(true));
        assert!(
            job_over(&peers, &pid, &cell, &hydrated).is_none(),
            "an already-hydrated surface must not schedule a durable read"
        );
    }
}
