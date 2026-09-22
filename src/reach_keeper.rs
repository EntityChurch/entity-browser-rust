//! `reach_keeper` — **presence**: a browser peer's substitute for a listening
//! socket, so peers that have met can actually connect.
//!
//! ## What this really is (the framing matters)
//!
//! A peer with a listener accepts inbound connections passively — the socket is
//! open, the OS does the waiting. **A browser has no listener.** Its only path
//! to a connection is `EXTENSION-SIGNALING` §6.5's *rendezvous-driven* trigger,
//! which the spec states plainly: two peers agree a key out of band, **meet at
//! it, and both drive `establish_live` off that key**. §6.5's offerer rule even
//! has the `hi` peer *suppress* its offer and wait for `lo`'s — waiting that
//! only works if `hi` is at the bucket collecting.
//!
//! So being reachable is not a state a browser peer has; it is an **activity it
//! performs**. This module is that activity. It is the accept loop, spelled as
//! presence at a rendezvous, because that is the only spelling the substrate
//! offers.
//!
//! An earlier version of this note called the mutuality a *finding* — "both
//! sides must be trying" — as though it were a surprise. It is the specified
//! flow. What we were actually missing was the presence half, and the reason we
//! got away without it for so long is that `ChatDelivery` polls every
//! participant at 5 Hz: chat's presence is a side effect of its delivery loop.
//! The first exchange that did not poll — a file offer, where the serving peer
//! dispatches nothing — had no presence at all, and a user would have read that
//! as "the other person's browser is broken."
//!
//! Measured before this module existed (`make e2e-webrtc-file`): the puller
//! deposited **52** offers under the pair key, the serving side issued **0**
//! collects on it, every negotiation dying `sdp_exchange=INCOMPLETE, fed=0`.
//! Caller-side retry did not help — nobody was at the meeting point. With
//! presence, the channel opens in **~1s**.
//!
//! ## Exchange-agnostic on purpose
//!
//! Nothing here knows about files, chat, or any other payload. It answers one
//! question — *is this peer connectable right now* — and every exchange built on
//! top inherits the answer. A future surface (an inbox, a shared album, a call)
//! should register intent here rather than grow its own poll, and chat's own
//! 5 Hz loop is a candidate to eventually *retire* onto this (see the A3 finding
//! before touching it — that poll is load-bearing in two other ways).
//!
//! ## What it does, and what it deliberately is not
//!
//! `want(local, remote)` records an **intent**: "I have met this peer / I expect
//! to exchange something with them; I should be reachable to them." While that
//! peer is not `Connected`, the keeper dispatches one cheap read at it on a slow
//! cadence. The read is not the point — **being present at the rendezvous while
//! the ladder consults is**. Once the kernel read-model says `Connected`, the
//! keeper costs one enum comparison per pump.
//!
//! - **Not a liveness store.** It records nothing *about* a remote. Liveness is
//!   read from `peer_liveness` (kernel-owned) on every pump and is the thing
//!   that switches this off. AP12/D8's "never a fourth parallel liveness store"
//!   is intact.
//! - **Not a retry counter for §6.5.** How hard establishment tries is bounded
//!   one layer down (`core/peer`'s per-peer consultation backoff, keyed
//!   correctly). This decides only *whether we are still interested in that
//!   peer at all*; the two cadences below are about our own dispatch cost, not
//!   about negotiation.
//! - **Not durable.** Intent is per-session, like a dial marker: it starts empty
//!   every load and is rebuilt from what the user does. Writing it to the tree
//!   would re-grow a stale mirror nobody clears.
//!
//! ## Why a stranger still cannot reach us, stated precisely
//!
//! It is tempting to say *"pair mode needs both peer-ids, so a stranger holding
//! only ours cannot derive the bucket."* **That is wrong, and the correct
//! version is the argument for fixing it.** `pair_key` **sorts** its two
//! arguments, so a stranger `S` holding our id `P` holds *both* inputs and can
//! derive `pair_key(S,P)` and deposit there right now. Nothing is secret.
//!
//! What is missing is on **our** side: to be present for any possible `S` we
//! would have to stand at `pair_key(X,P)` for every `X` — unbounded. So the
//! deficiency is **responder-side enumerability, not key derivability**, and
//! that is exactly the property a listening socket has and this one lacks: a
//! listener has *one* well-known address, while `pair` mode gives us **one
//! address per counterpart**. A fifth rendezvous mode keyed on our own peer-id
//! alone — one bucket, derivable by anyone holding `P` — is the rendezvous
//! spelling of a well-known port, and is the minimal fix rather than merely a
//! convenient one. Routed to arch as `ROUTING-2026-08-20-e` §1.1.
//!
//! ## Why it never gives up
//!
//! A standing offer is a standing invitation. A peer that is offline now may be
//! back in an hour, and the whole point of the serving side is that it does not
//! need to be poked. So the fast cadence decays to a slow one and stops there —
//! it does not stop trying. The slow rate (~30s) is two orders of magnitude
//! below chat's poll, and it applies only while a peer is *not* connected.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use crate::peers::Peers;

/// Frames between probes while a peer is still fresh (~3s at 60fps).
const FAST_EVERY: u32 = 180;
/// Frames between probes once the fast phase is spent (~30s at 60fps).
const SLOW_EVERY: u32 = 1_800;
/// Probes at the fast cadence before decaying to the slow one (~1 minute).
const FAST_PROBES: u32 = 20;

#[derive(Default)]
struct Intent {
    /// Frames until the next probe. Counted down per pump.
    countdown: u32,
    probes: u32,
    in_flight: bool,
    /// Whether this peer has been written into the "ever connected" registry
    /// since it last came up. One write per connect, not one per frame.
    remembered: bool,
}

impl Intent {
    fn interval(&self) -> u32 {
        if self.probes < FAST_PROBES {
            FAST_EVERY
        } else {
            SLOW_EVERY
        }
    }
}

/// Cheap-to-clone handle to the app's reach intents, keyed by
/// `(local_peer, remote_peer)`.
#[derive(Clone, Default)]
pub struct ReachKeeper {
    inner: Arc<Mutex<HashMap<(String, String), Intent>>>,
}

impl ReachKeeper {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record intent to be reachable to/from `remote`, as `local`. Idempotent:
    /// re-registering an existing pair does **not** reset its cadence, so a
    /// surface that calls this every frame cannot turn the slow phase back into
    /// a fast one.
    pub fn want(&self, local: &str, remote: &str) {
        if local.is_empty() || remote.is_empty() || local == remote {
            return;
        }
        let Ok(mut map) = self.inner.lock() else { return };
        map.entry((local.to_string(), remote.to_string())).or_insert_with(|| Intent {
            // Probe on the first pump rather than after a wait: the meet that
            // caused this has just happened, and the counterpart is dispatching
            // at us right now.
            countdown: 0,
            ..Intent::default()
        });
    }

    /// Drop the intent — the conversation ended, the peer was forgotten.
    pub fn forget(&self, local: &str, remote: &str) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(&(local.to_string(), remote.to_string()));
        }
    }

    /// Drop **every** local peer's intent toward `remote`, and report how many
    /// went. This is the teardown half of [`want`](Self::want), and it is
    /// remote-scoped for the same reason
    /// [`Peers::forget_routes_to`](crate::peers::Peers::forget_routes_to) is:
    /// intent is keyed `(local, remote)` and *any* local peer may hold one, so
    /// a pair-scoped drop silently leaves the others probing.
    ///
    /// **Presence is an advertisement, so withdrawing it is not optional.**
    /// `Action::ForgetConnection` already sweeps the registry row, the
    /// published transport route and the dial marker — the route's own comment
    /// is *"or Forget doesn't forget"*. The intent is the fourth thing that
    /// outlives the user's dismissal, and the loudest: while a forgotten peer
    /// is not `Connected` the keeper keeps dispatching at it **forever** on the
    /// slow cadence, because [never giving up](self) is deliberate. Presence at
    /// a rendezvous *is* how this peer form says "I am reachable" — so an
    /// intent nobody can withdraw is a standing invitation to a peer the user
    /// has told us to forget.
    pub fn forget_remote(&self, remote: &str) -> usize {
        let Ok(mut map) = self.inner.lock() else { return 0 };
        let before = map.len();
        map.retain(|(_, r), _| r != remote);
        before - map.len()
    }

    #[cfg(test)]
    fn probes(&self, local: &str, remote: &str) -> u32 {
        self.inner
            .lock()
            .unwrap()
            .get(&(local.to_string(), remote.to_string()))
            .map(|i| i.probes)
            .unwrap_or(0)
    }

    /// Tick every intent and return the pairs due for a probe **this frame**.
    ///
    /// Separated from [`pump`](Self::pump) so the decision is testable without a
    /// runtime: everything that decides *whether* to dispatch is here, and the
    /// dispatch itself is the two lines that need a spawn.
    ///
    /// A `Connected` peer is skipped **and its cadence is left untouched**, so a
    /// peer that connects and later drops resumes at the rate it had rather
    /// than restarting a fast burst on every reconnect.
    fn due(&self, peers: &Peers) -> Vec<(String, String)> {
        let Ok(mut map) = self.inner.lock() else {
            return Vec::new();
        };
        let mut due = Vec::new();
        let mut remember: Vec<String> = Vec::new();
        for ((local, remote), intent) in map.iter_mut() {
            // Time passes whether or not we are allowed to fire — so the
            // countdown drains even while a probe is in flight. Otherwise a
            // slow round-trip would silently *add* its own duration to the
            // interval, and the pacing would drift with the network.
            intent.countdown = intent.countdown.saturating_sub(1);
            if intent.in_flight {
                continue;
            }
            // The kernel read-model is the authority on whether anything is
            // needed at all. This is the switch-off, and it is a read, never a
            // fact we keep.
            if crate::peer_liveness::liveness_of(peers, remote).is_connected() {
                // **Remember a peer we actually reached.** The registry means
                // "we have connected at least once" — and until now only the
                // manual Connect button wrote it, so a peer met by NAME and
                // reached over WebRTC existed nowhere the UI looks: the File
                // Transfer target list, which reads this registry, was empty
                // for exactly the peer the whole rendezvous path produces.
                // Once per connect (not per frame — this is a tree write).
                if !intent.remembered {
                    intent.remembered = true;
                    remember.push(remote.clone());
                }
                continue;
            }
            // Dropped: a later reconnect should refresh `last_seen`.
            intent.remembered = false;
            if intent.countdown > 0 {
                continue;
            }
            intent.probes = intent.probes.saturating_add(1);
            intent.countdown = intent.interval();
            intent.in_flight = true;
            due.push((local.clone(), remote.clone()));
        }
        drop(map); // never hold the lock across a write
        if !remember.is_empty() {
            let writer = crate::connections::ConnectionsWriter::new(peers);
            for remote in remember {
                writer.add(&remote);
            }
        }
        due
    }

    fn landed(&self, local: &str, remote: &str) {
        if let Ok(mut map) = self.inner.lock() {
            if let Some(i) = map.get_mut(&(local.to_string(), remote.to_string())) {
                i.in_flight = false;
            }
        }
    }

    /// The device woke: make every standing intent due on the next pump, and
    /// report how many were refreshed.
    ///
    /// **The countdown is measured in FRAMES, and frames do not advance while
    /// the device is suspended** — so on wake every intent is exactly where it
    /// was when we went to sleep. For a peer on the decayed cadence that is up
    /// to a further ~30 s of silence at precisely the moment the counterpart is
    /// back and dispatching at us. Resetting the countdown costs one probe per
    /// wanted peer and buys back that window.
    ///
    /// **`probes` is deliberately NOT reset.** That counter is what decays the
    /// fast cadence into the slow one, and re-arming a fast burst on every wake
    /// would let a laptop that sleeps twice an hour hold an unreachable peer at
    /// the fast rate forever — the pacing this module exists to bound. We are
    /// re-asking *now*, not starting over.
    pub fn wake(&self) -> usize {
        let Ok(mut map) = self.inner.lock() else {
            return 0;
        };
        let mut refreshed = 0usize;
        for intent in map.values_mut() {
            // An in-flight probe is left alone: its `landed` will clear the
            // guard, and forcing a second one on top is the doubling-up that
            // `in_flight` exists to prevent.
            if intent.in_flight {
                continue;
            }
            intent.countdown = 0;
            refreshed += 1;
        }
        refreshed
    }

    /// Called once per frame. Dispatches one cheap read per due pair.
    ///
    /// The probe itself lives in [`crate::peer_probe`] — the same dispatch the
    /// wake path sends, kept as one expression so the two cannot drift into
    /// different ideas of "cheap". Its *result is discarded*: a failure is the
    /// expected case while no path exists, and the consultation it triggered is
    /// the entire purpose.
    pub fn pump(&self, peers: &Peers) {
        for (local, remote) in self.due(peers) {
            let Some(fut) = crate::peer_probe::probe(peers, &local, &remote) else {
                self.landed(&local, &remote);
                continue;
            };
            let (keeper, l, r) = (self.clone(), local.clone(), remote.clone());
            crate::peer_probe::spawn(async move {
                fut.await;
                keeper.landed(&l, &r);
            });
        }
    }

    /// Every remote we currently hold intent for, for diagnostics.
    #[allow(dead_code)]
    pub fn targets(&self) -> BTreeSet<String> {
        self.inner
            .lock()
            .map(|m| m.keys().map(|(_, r)| r.clone()).collect())
            .unwrap_or_default()
    }
}

/// The one process-global keeper — same shape and rationale as
/// [`crate::access_log_store::global`]. It is a global because the surfaces
/// that *learn* an intent (the Shell's `meet` pump, the transfer verbs) are
/// several windows away from the app that pumps it, and threading a handle
/// through every one of them buys nothing: there is exactly one app per
/// process, and the state is per-session runtime control, not data.
pub fn global() -> &'static ReachKeeper {
    static KEEPER: OnceLock<ReachKeeper> = OnceLock::new();
    KEEPER.get_or_init(ReachKeeper::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer nobody can reach: `Peers::new_direct` holds one local peer, and
    /// any other id has no status entity, so `liveness_of` reads `Unknown` —
    /// which is exactly the state a just-met peer is in.
    fn unreachable_pair() -> (Peers, String, String) {
        let peers = Peers::new_direct();
        let local = peers.primary_peer_id().to_string();
        (peers, local, "2KStrangerWeJustMetAtANameXXXXXXXXXXXXXXXXXX".to_string())
    }

    #[test]
    fn a_wanted_peer_is_due_immediately_then_paced() {
        let (peers, local, remote) = unreachable_pair();
        let k = ReachKeeper::new();
        k.want(&local, &remote);

        // Immediately, because the counterpart is dispatching at us NOW — the
        // meet that caused this just happened.
        assert_eq!(k.due(&peers), vec![(local.clone(), remote.clone())]);
        k.landed(&local, &remote);

        // Then paced: nothing for the next FAST_EVERY-1 frames.
        for _ in 0..(FAST_EVERY - 1) {
            assert!(k.due(&peers).is_empty(), "must not probe every frame");
            k.landed(&local, &remote);
        }
        assert_eq!(k.due(&peers).len(), 1, "and due again once the interval elapses");
    }

    #[test]
    fn an_in_flight_probe_is_never_doubled_up() {
        let (peers, local, remote) = unreachable_pair();
        let k = ReachKeeper::new();
        k.want(&local, &remote);
        assert_eq!(k.due(&peers).len(), 1);
        // No `landed` — the first probe is still out there. Even after a full
        // interval, nothing new fires: a slow round-trip must not stack.
        for _ in 0..(FAST_EVERY + 5) {
            assert!(k.due(&peers).is_empty(), "a probe in flight blocks the next");
        }
        k.landed(&local, &remote);
        assert_eq!(k.due(&peers).len(), 1, "and resumes once it lands");
    }

    #[test]
    fn the_cadence_decays_but_never_stops() {
        let (peers, local, remote) = unreachable_pair();
        let k = ReachKeeper::new();
        k.want(&local, &remote);

        // Run the fast phase out.
        let mut frames = 0u32;
        while k.probes(&local, &remote) < FAST_PROBES {
            for (l, r) in k.due(&peers) {
                k.landed(&l, &r);
            }
            frames += 1;
            assert!(frames < FAST_EVERY * FAST_PROBES + 10, "fast phase must terminate");
        }
        // Now it is slow — nothing for a whole fast interval...
        for _ in 0..FAST_EVERY {
            assert!(k.due(&peers).is_empty(), "the fast cadence must have decayed");
        }
        // ...but it does NOT stop. A standing offer is a standing invitation,
        // and a peer that is offline now may be back in an hour.
        let mut fired = false;
        for _ in 0..SLOW_EVERY {
            if !k.due(&peers).is_empty() {
                fired = true;
                break;
            }
        }
        assert!(fired, "the keeper must keep trying, slowly, forever");
    }

    #[test]
    fn a_connected_peer_costs_nothing_and_forget_removes_it() {
        // `liveness_of` for our OWN peer id reads Connected-by-definition? No —
        // it reads the kernel status, and a self-pair is refused at `want`, so
        // this asserts the two guards that do not need a live kernel.
        let (_peers, local, remote) = unreachable_pair();
        let k = ReachKeeper::new();
        k.want(&local, &local);
        k.want("", &remote);
        assert!(k.targets().is_empty(), "a self-pair or an empty id is not an intent");

        k.want(&local, &remote);
        assert_eq!(k.targets().len(), 1);
        k.forget(&local, &remote);
        assert!(k.targets().is_empty(), "forget drops the intent");
    }

    /// The wake case: frames stop while the device is suspended, so a peer on
    /// the decayed cadence is exactly where it was — up to ~30 s of further
    /// silence at the moment the counterpart is back. `wake` makes it due now.
    #[test]
    fn a_wake_makes_a_decayed_intent_due_immediately() {
        let (peers, local, remote) = unreachable_pair();
        let k = ReachKeeper::new();
        k.want(&local, &remote);

        // Spend the first probe, then sit well inside the interval — nothing is
        // due, which is the state a sleeping laptop freezes in.
        for (l, r) in k.due(&peers) {
            k.landed(&l, &r);
        }
        assert!(k.due(&peers).is_empty(), "paced: nothing due mid-interval");
        k.landed(&local, &remote);

        assert_eq!(k.wake(), 1, "the wake refreshes the intent and says how many");
        assert_eq!(
            k.due(&peers),
            vec![(local.clone(), remote.clone())],
            "and the peer is due on the very next pump"
        );
    }

    /// `probes` is what decays the fast cadence into the slow one. A wake
    /// re-asks *now*; it does not start over. Otherwise a laptop that sleeps
    /// twice an hour would hold an unreachable peer at the fast rate forever —
    /// the pacing this module exists to bound.
    ///
    /// Mutation check: reset `probes` in `wake` and this fails, with the peer
    /// back on `FAST_EVERY`.
    #[test]
    fn a_wake_does_not_re_arm_the_fast_cadence() {
        let (peers, local, remote) = unreachable_pair();
        let k = ReachKeeper::new();
        k.want(&local, &remote);

        // Run the fast phase out, so the intent is on the slow cadence.
        while k.probes(&local, &remote) < FAST_PROBES {
            for (l, r) in k.due(&peers) {
                k.landed(&l, &r);
            }
        }

        k.wake();
        // Due immediately (that is the point) — but the interval it then takes
        // is still the slow one.
        assert_eq!(k.due(&peers).len(), 1, "a wake re-asks now");
        k.landed(&local, &remote);
        for _ in 0..FAST_EVERY {
            assert!(
                k.due(&peers).is_empty(),
                "a wake must not re-arm the fast burst"
            );
        }
    }

    /// An in-flight probe already covers the question a wake would ask, and
    /// `in_flight` exists precisely so a slow round trip cannot stack. A wake
    /// that ignored it would double up at the worst moment — right after a
    /// resume, when round trips are slowest.
    #[test]
    fn a_wake_does_not_double_up_on_an_in_flight_probe() {
        let (peers, local, remote) = unreachable_pair();
        let k = ReachKeeper::new();
        k.want(&local, &remote);
        assert_eq!(k.due(&peers).len(), 1);
        // No `landed` — that probe is still out there.
        assert_eq!(k.wake(), 0, "an in-flight probe is left alone, and is not counted");
        assert!(k.due(&peers).is_empty(), "and nothing is stacked on top of it");
    }

    /// `Action::ForgetConnection` is remote-scoped — the user dismissed a
    /// *peer*, not one of our local peers' relationships with it. Intent is
    /// keyed `(local, remote)`, so a forget that names a single local silently
    /// leaves every other local peer standing at the rendezvous for someone the
    /// user told us to forget.
    ///
    /// Mutation check: swap `forget_remote` for a pair-scoped
    /// `forget(&local_a, &remote)` and the second assertion fails with the
    /// `frontend-idb` peer's intent still live.
    #[test]
    fn forgetting_a_peer_withdraws_every_local_peers_reach_intent() {
        let k = ReachKeeper::new();
        let (local_a, local_b) = ("2KPrimaryLocalPeerXXXXXXXXXXXXXXXXXXXXXXXXXX", "2KDurableIdbLocalPeerXXXXXXXXXXXXXXXXXXXXXXX");
        let forgotten = "2KTheStrangerWeAreDismissingXXXXXXXXXXXXXXXX";
        let kept = "2KSomebodyWeStillTalkToXXXXXXXXXXXXXXXXXXXXX";

        // Two local peers hold intent toward the same remote — the ordinary
        // shape once a durable this-tab peer lands beside the primary.
        k.want(local_a, forgotten);
        k.want(local_b, forgotten);
        k.want(local_a, kept);
        assert_eq!(k.targets().len(), 2, "two distinct remotes are wanted");

        assert_eq!(k.forget_remote(forgotten), 2, "both locals' intents go, and it says how many");

        let left = k.targets();
        assert!(!left.contains(forgotten), "a forgotten peer must not still be wanted by anyone");
        assert!(left.contains(kept), "and forgetting one peer must not disturb another");

        // Idempotent: dismissing an already-forgotten peer is not an error and
        // reports honestly that there was nothing to withdraw.
        assert_eq!(k.forget_remote(forgotten), 0);
    }
}
