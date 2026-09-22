//! Pure policy for the frozen-frame watchdog (`watchdog.rs`, wasm-only).
//!
//! Split out from the wasm runtime so the false-positive suppression — the
//! load-bearing decision — is unit-testable on the native target without any
//! DOM / worker / clock. The runtime plumbing stays in `watchdog.rs`.

/// After the tab returns to the foreground, ignore freeze reports for this long.
/// The watcher worker can race the `resume` message — its overdue `setInterval`
/// tick fires and reports the whole *backgrounded* gap before it processes
/// `resume` — so a report landing right after we become visible is that race,
/// not a real in-page stall. Comfortably covers the worker's ~1 Hz tick + the
/// postMessage round-trip back to the main thread.
pub const RESUME_GRACE_MS: f64 = 4000.0;

/// A genuinely *recoverable* frozen frame is seconds long (the main thread got
/// stuck then unstuck — detected at the 5 s default threshold, reported at
/// ~threshold). A gap this large is the environment (device sleep / suspend /
/// bfcache) where no `visibilitychange` fired to pause us — not something a
/// reload prompt should fire on. Backstop for the no-event suspend path.
pub const MAX_PLAUSIBLE_FREEZE_MS: f64 = 60_000.0;

/// What a watcher freeze report actually means. **Four facts, not a bool.**
///
/// The three suppression guards were merged into one `true` and one log line
/// that read *"tab backgrounded / device sleep — not a real freeze"* — naming
/// three different causes at once and committing to none. The one that matters
/// is [`EnvironmentGap`](FreezeVerdict::EnvironmentGap): it is the strongest
/// signal the substrate offers that **every NAT mapping and DTLS session this
/// profile holds is dead**, and it was being spent on suppressing a banner
/// (AP54 — the product diagnosing a condition correctly and telling a status
/// surface instead of the resolver).
///
/// **What separates a suspend from a wedge is not the duration — it is which
/// thread stopped.** The watcher lives off the main thread and resets its own
/// clock after every report (`last = Date.now()`), so while the main thread is
/// wedged it keeps ticking and emits a *stream* of reports each about one
/// threshold. A single gap far larger than the threshold therefore means the
/// **watcher itself** stopped running — which is the whole process being
/// suspended, not a frame stalling. That is why the large-gap arm is named for
/// the environment rather than for its size, and why a wedge is still reported:
/// its reports are threshold-sized and land on [`Freeze`](FreezeVerdict::Freeze).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeVerdict {
    /// The tab is backgrounded right now. `requestAnimationFrame` is paused, so
    /// the beats legitimately stopped.
    Backgrounded,
    /// We returned to the foreground moments ago and the watcher's overdue tick
    /// reported the *backgrounded* gap before it processed `resume`. The race,
    /// not a stall (see [`RESUME_GRACE_MS`]).
    ResumeRace,
    /// The whole process stopped: device sleep / suspend / bfcache, with no
    /// `visibilitychange` to pause us. Not a freeze — and a **wake signal**,
    /// which `wake_probe` acts on from the frame loop.
    EnvironmentGap,
    /// A real in-page stall: the main thread stopped rendering while the watcher
    /// kept running. This is the one the watchdog exists to catch.
    Freeze,
}

impl FreezeVerdict {
    /// Whether this is a real in-page freeze, as opposed to something the
    /// environment did to us.
    ///
    /// Stated **positively** on purpose: a `!matches!(self, Freeze)` spelling
    /// would silently absorb every variant added later into "suppressed", which
    /// is the same defect `AppServerView::is_serving` shipped one subsystem over.
    pub fn is_freeze(self) -> bool {
        matches!(self, FreezeVerdict::Freeze)
    }
}

/// Classify a watcher freeze report. Pure: `hidden` = the tab is backgrounded
/// right now; `ms_since_resume` = how long ago we returned to the foreground;
/// `gap_ms` = the reported silent gap.
///
/// Order is cheapest-first and the arms are mutually exclusive by construction;
/// where more than one holds, the more specific claim wins — being backgrounded
/// explains the gap without needing to reason about its size.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn classify_freeze_report(hidden: bool, ms_since_resume: f64, gap_ms: f64) -> FreezeVerdict {
    if hidden {
        FreezeVerdict::Backgrounded
    } else if ms_since_resume < RESUME_GRACE_MS {
        FreezeVerdict::ResumeRace
    } else if gap_ms > MAX_PLAUSIBLE_FREEZE_MS {
        FreezeVerdict::EnvironmentGap
    } else {
        FreezeVerdict::Freeze
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backgrounded_tab_is_not_a_freeze() {
        // Even a long, plausible-looking gap is the background if we're hidden.
        assert_eq!(
            classify_freeze_report(true, 999_999.0, 7000.0),
            FreezeVerdict::Backgrounded
        );
    }

    #[test]
    fn a_report_racing_the_resume_is_the_race() {
        assert_eq!(
            classify_freeze_report(false, 100.0, 30_000.0),
            FreezeVerdict::ResumeRace
        );
        assert_eq!(
            classify_freeze_report(false, RESUME_GRACE_MS - 1.0, 8000.0),
            FreezeVerdict::ResumeRace
        );
    }

    #[test]
    fn an_implausibly_large_gap_is_the_environment_stopping_us() {
        // Device sleep/suspend with no visibilitychange: huge gap, not recent.
        // Only reachable when the WATCHER stopped too — see the enum's doc.
        assert_eq!(
            classify_freeze_report(false, 999_999.0, MAX_PLAUSIBLE_FREEZE_MS + 1.0),
            FreezeVerdict::EnvironmentGap
        );
    }

    #[test]
    fn reports_a_real_recoverable_stall() {
        // Visible, settled, plausible-length stall → a real freeze, surface it.
        assert_eq!(
            classify_freeze_report(false, 999_999.0, 6000.0),
            FreezeVerdict::Freeze
        );
    }

    /// A wedged main thread does **not** produce one enormous gap: the watcher
    /// resets its clock after each report, so it emits a stream of
    /// threshold-sized ones. Each of those must classify as a real freeze — this
    /// is what stops the large-gap arm from swallowing the case the watchdog
    /// exists for.
    #[test]
    fn a_wedged_main_thread_still_reports_because_its_gaps_are_threshold_sized() {
        for report in [5_100.0, 6_000.0, 5_050.0, 12_000.0] {
            assert_eq!(
                classify_freeze_report(false, 999_999.0, report),
                FreezeVerdict::Freeze,
                "a {report}ms report from a still-ticking watcher is a real freeze"
            );
        }
    }

    /// Exactly one verdict is a freeze. Asserted as a property over the whole
    /// set rather than as a count of variants, so adding a fifth environmental
    /// cause cannot quietly start rendering as a stall.
    #[test]
    fn only_a_real_stall_counts_as_a_freeze() {
        let all = [
            FreezeVerdict::Backgrounded,
            FreezeVerdict::ResumeRace,
            FreezeVerdict::EnvironmentGap,
            FreezeVerdict::Freeze,
        ];
        assert_eq!(all.iter().filter(|v| v.is_freeze()).count(), 1);
        assert!(FreezeVerdict::Freeze.is_freeze());
        assert!(
            !FreezeVerdict::EnvironmentGap.is_freeze(),
            "a suspend must never be reported to the user as the app freezing"
        );
    }
}
