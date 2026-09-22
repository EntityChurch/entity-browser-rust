//! `wake_probe` — after the device wakes, find out whether the connections we
//! *believe* in survived, instead of waiting ~130 s to be told.
//!
//! ## The failure this exists for
//!
//! Two machines chatting over §6.5 WebRTC. Both sleep. On wake both show the
//! counterpart offline, and nothing recovers until the page is reloaded — which
//! recovers it in ~1 s, with no need to re-`meet`. The reload was never fixing
//! corrupted state; it was **skipping a slow detection path**.
//!
//! Everything downstream of the liveness transition is already correct and
//! already reactive: `system/peer/status/{peer}` flips, the §4.1 `on-disconnect`
//! continuation dispatches `reconnect`, `on-reconnect-backoff` re-EXECUTEs
//! `maintain-peer` on §2.2 backoff, the §10.3 ladder is consulted, and
//! `reach_keeper` has kept us present at the rendezvous. **The entire defect is
//! how fast, and whether, that transition is written.** So this module feeds the
//! existing machine at that one point rather than building a second recovery
//! path beside it.
//!
//! ## Why it takes ~130 s without this, and ~10 s with it
//!
//! `KeepaliveConfig::default()` is `interval_ms 30_000`, `timeout_ms 10_000`,
//! `max_missed 3` — so `3 × (30 s + 10 s) + 10 s` of grace before the binding is
//! **evicted**. Until eviction, dispatch keeps using the dead pooled route and
//! the §10.3 ladder is never consulted, so re-establishment cannot even begin.
//! That is the number behind *"it just sat there"*.
//!
//! Worse, the app's own recovery machinery is switched **off** for exactly that
//! window: `reach_keeper::due` skips any peer the kernel calls `Connected`, and
//! after a suspend that is every peer. So the keeper is silent, the keepalive is
//! slow, and nothing dispatches — a mutual deadlock in which each mechanism is
//! individually behaving as designed.
//!
//! The keepalive's own ping does **not** short-circuit this: a missed ping only
//! increments a counter (`missed`), and it takes `max_missed` of them. An
//! ordinary **app dispatch** is different — a transport error on a connection we
//! believed active runs the kernel's §A1 seam
//! (`liveness::demote_peer_on_transport_error` at `core/peer`'s §10 step-1 site),
//! which evicts the pooled binding and writes `suspect` **in one event, on the
//! first failure**. The §5.4a escalation then completes `suspect → disconnected`
//! after the 10 s grace, and the eviction alone is already enough for
//! `reach_keeper` to wake up and the ladder to be consulted.
//!
//! **So the whole fix is: on wake, send something.** We write no liveness and
//! evict nothing — the kernel observes the send failing and owns the demotion.
//!
//! ## Probe, don't evict
//!
//! Tearing a connection down on wake would cost a full re-establish on every
//! wake where the link was fine (a short lid-close, a hub that kept its state).
//! Probing costs one round trip and never destroys a link that survived. The
//! value was never in *which* action we take — it is in **not waiting up to 30 s
//! for the next scheduled tick**, and both options capture that, so the cheap
//! non-destructive one wins.
//!
//! ## Why the frame loop is the wake signal
//!
//! `requestAnimationFrame` does not advance while the device is suspended or the
//! tab is backgrounded, so a large wall-clock gap between consecutive frames *is*
//! the resume event — with no worker, no `visibilitychange` (which a suspend does
//! not always fire), and no dependence on the frozen-frame watchdog, which the
//! user can switch off with `?watchdog=0`. The watchdog reaches the same
//! conclusion by its own route and reports it (`watchdog_policy::FreezeVerdict::
//! EnvironmentGap`); it deliberately does **not** also act on it, because two
//! detectors driving one repair is the parallel mechanism this design refuses.
//!
//! A backgrounded tab returning to the foreground produces the same large gap and
//! gets the same probe. That is correct rather than merely tolerable: the
//! question is never *did the device sleep*, it is *did time pass without us
//! observing any traffic* — and the answer decides the same thing either way.

/// What a wall-clock gap between two consecutive frames means. Three outcomes,
/// kept apart because they license different statements: *nothing happened*,
/// *time passed and we are going to check*, and *time passed and we are
/// deliberately not checking again yet*. Merging the last two into "did not
/// probe" is what would make a debounced wake indistinguishable from an ordinary
/// frame in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeVerdict {
    /// Less than one keepalive interval passed. Our `connected` beliefs are no
    /// older than the kernel's own idea of "recently active", so there is
    /// nothing to re-check.
    NoGap { gap_ms: u64 },
    /// Long enough that every `connected` belief predates a full keepalive
    /// interval — probe each one.
    Resumed { gap_ms: u64 },
    /// A qualifying gap, but a probe already went out inside the debounce
    /// window. A frame loop that stalls repeatedly must not turn into a probe
    /// storm.
    RecentlyProbed {
        gap_ms: u64,
        ms_since_last_probe: u64,
    },
}

/// The gap at which a `connected` belief stops being worth trusting, **derived
/// from the kernel's own keepalive interval rather than picked**.
///
/// §5.4's adaptive suppression skips the ping when the connection exchanged
/// anything within `interval_ms` — that is the kernel's own threshold for
/// "recently active". A gap at least that long means even the kernel would no
/// longer count us as active, which makes it the honest place to stop trusting
/// the belief. Deriving it here also means a §12.4 retune of the keepalive moves
/// this with it instead of leaving two numbers to drift apart (C15's rule).
///
/// It doubles as the debounce window, and that bounds the cost exactly: at most
/// one wake probe per keepalive interval per peer is strictly cheaper than the
/// keepalive traffic already flowing, so this can never more than double it.
pub fn wake_gap_threshold_ms() -> u64 {
    entity_peer::keepalive::KeepaliveConfig::default().interval_ms
}

/// Pure: decide what a frame-to-frame gap means. `ms_since_last_probe` is `None`
/// when no wake probe has fired this session.
pub fn decide(gap_ms: u64, ms_since_last_probe: Option<u64>, threshold_ms: u64) -> WakeVerdict {
    if gap_ms < threshold_ms {
        return WakeVerdict::NoGap { gap_ms };
    }
    match ms_since_last_probe {
        Some(since) if since < threshold_ms => WakeVerdict::RecentlyProbed {
            gap_ms,
            ms_since_last_probe: since,
        },
        _ => WakeVerdict::Resumed { gap_ms },
    }
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// Wall-clock ms of the previous frame. `None` until the second frame —
    /// there is no gap to measure across the first one.
    static LAST_FRAME_MS: std::cell::Cell<Option<f64>> = const { std::cell::Cell::new(None) };
    /// Wall-clock ms of the last wake probe, for the debounce.
    static LAST_PROBE_MS: std::cell::Cell<Option<f64>> = const { std::cell::Cell::new(None) };
}

/// Called at the top of every frame. Measures the wall-clock gap since the
/// previous frame and, on a resume, probes every peer we believe we are
/// connected to.
///
/// Cheap on the ordinary path: one `Date::now()`, one `Cell` swap and an
/// integer compare. The tree is read only on the frames that follow a resume.
#[cfg(target_arch = "wasm32")]
pub fn note_frame(peers: &crate::peers::Peers) {
    let now = js_sys::Date::now();
    let Some(previous) = LAST_FRAME_MS.with(|c| c.replace(Some(now))) else {
        return; // first frame — nothing to measure against
    };
    let gap_ms = (now - previous).max(0.0) as u64;
    let since_probe = LAST_PROBE_MS
        .with(|c| c.get())
        .map(|t| (now - t).max(0.0) as u64);

    match decide(gap_ms, since_probe, wake_gap_threshold_ms()) {
        WakeVerdict::NoGap { .. } => {}
        WakeVerdict::RecentlyProbed {
            gap_ms,
            ms_since_last_probe,
        } => {
            tracing::info!(
                gap_ms,
                ms_since_last_probe,
                "wake: frames resumed, but a wake probe already went out inside the \
                 debounce window"
            );
        }
        WakeVerdict::Resumed { gap_ms } => {
            LAST_PROBE_MS.with(|c| c.set(Some(now)));
            probe_after_resume(peers, gap_ms);
        }
    }
}

/// Send one cheap dispatch at every remote a local vantage believes it is
/// connected to, and put every standing reach intent back on its fast cadence.
///
/// The two halves cover opposite states deliberately: the liveness read covers
/// peers we think are **up** (whose belief the suspend invalidated), and the
/// keeper covers peers we already know are **down** (whose retry countdown is
/// measured in frames and therefore did not advance while we were asleep — so
/// without this they would sit out up to a further 30 s of slow cadence at the
/// exact moment the counterpart is back).
///
/// **Reports even when it sends nothing.** A repair whose only evidence is a side
/// effect cannot be told from one that never ran.
#[cfg(target_arch = "wasm32")]
fn probe_after_resume(peers: &crate::peers::Peers, gap_ms: u64) {
    let mut believed_connected = 0usize;
    let mut probes_sent = 0usize;
    for vantage in peers.peer_ids() {
        // Per vantage, not merged: a `connected` belief belongs to the local
        // peer that holds the binding, and the probe has to go out from that
        // one or it is not testing the belief.
        for row in crate::peer_liveness::read_peer_liveness(peers, &vantage) {
            if !row.status.is_connected() {
                continue;
            }
            believed_connected += 1;
            if row.remote_pid.is_empty() || row.remote_pid == vantage {
                continue;
            }
            if let Some(fut) = crate::peer_probe::probe(peers, &vantage, &row.remote_pid) {
                crate::peer_probe::spawn(fut);
                probes_sent += 1;
            }
        }
    }
    let intents_refreshed = crate::reach_keeper::global().wake();
    tracing::info!(
        gap_ms,
        believed_connected,
        probes_sent,
        intents_refreshed,
        "wake: frames resumed after a gap; re-checking every connection we believe in"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The steady state: an ordinary 16 ms frame concludes nothing.
    #[test]
    fn an_ordinary_frame_is_not_a_wake() {
        assert_eq!(
            decide(16, None, 30_000),
            WakeVerdict::NoGap { gap_ms: 16 }
        );
    }

    /// A gap shorter than the keepalive interval is left to the keepalive: our
    /// belief is no older than the kernel's own "recently active" window, so
    /// there is nothing this could establish that the next tick will not.
    #[test]
    fn a_gap_under_one_keepalive_interval_is_left_to_the_keepalive() {
        assert_eq!(
            decide(29_999, None, 30_000),
            WakeVerdict::NoGap { gap_ms: 29_999 }
        );
        // The boundary is inclusive on the probe side — at exactly one interval
        // the kernel would already have stopped counting us active.
        assert_eq!(
            decide(30_000, None, 30_000),
            WakeVerdict::Resumed { gap_ms: 30_000 }
        );
    }

    /// The reported case: a lid closed for ten minutes.
    #[test]
    fn a_long_suspend_probes() {
        assert_eq!(
            decide(600_000, None, 30_000),
            WakeVerdict::Resumed { gap_ms: 600_000 }
        );
    }

    /// A frame loop that stalls repeatedly must not become a probe storm — and
    /// the debounced case must be *its own* outcome, not silently indistinguishable
    /// from an ordinary frame.
    #[test]
    fn a_repeatedly_stalling_frame_loop_is_debounced_and_says_so() {
        assert_eq!(
            decide(90_000, Some(5_000), 30_000),
            WakeVerdict::RecentlyProbed {
                gap_ms: 90_000,
                ms_since_last_probe: 5_000
            }
        );
        // Once the window is spent, a fresh gap probes again — a device that
        // sleeps twice must be re-checked twice.
        assert_eq!(
            decide(90_000, Some(30_000), 30_000),
            WakeVerdict::Resumed { gap_ms: 90_000 }
        );
    }

    /// The threshold is *derived*, not typed in — a §12.4 retune of the kernel
    /// keepalive must move this with it rather than leaving two numbers to
    /// drift. Asserting the derivation (not the literal 30_000) is the whole
    /// point: a test spelled as the constant follows any change and can never
    /// catch one.
    #[test]
    fn the_threshold_follows_the_kernel_keepalive_interval() {
        assert_eq!(
            wake_gap_threshold_ms(),
            entity_peer::keepalive::KeepaliveConfig::default().interval_ms,
            "the wake threshold is the kernel's own 'recently active' window"
        );
    }
}
