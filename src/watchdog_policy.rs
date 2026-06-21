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

/// Decide whether a watcher freeze report is an environmental false positive
/// that should be dropped rather than shown as the "hit a snag — Reload?"
/// banner. Pure: `hidden` = the tab is backgrounded right now; `ms_since_resume`
/// = how long ago we returned to the foreground; `gap_ms` = the reported silent
/// gap. Suppress when backgrounded, just-resumed (the resume race), or the gap
/// is implausibly large (sleep/suspend with no visibilitychange to pause us).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn freeze_report_suppressed(hidden: bool, ms_since_resume: f64, gap_ms: f64) -> bool {
    hidden || ms_since_resume < RESUME_GRACE_MS || gap_ms > MAX_PLAUSIBLE_FREEZE_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_while_hidden() {
        // Even a long, plausible-looking gap is ignored if we're backgrounded.
        assert!(freeze_report_suppressed(true, 999_999.0, 7000.0));
    }

    #[test]
    fn suppresses_right_after_resume() {
        // The resume race: a big gap reported just after returning to foreground.
        assert!(freeze_report_suppressed(false, 100.0, 30_000.0));
        assert!(freeze_report_suppressed(
            false,
            RESUME_GRACE_MS - 1.0,
            8000.0
        ));
    }

    #[test]
    fn suppresses_implausibly_large_gap() {
        // Device sleep/suspend with no visibilitychange: huge gap, not recent.
        assert!(freeze_report_suppressed(
            false,
            999_999.0,
            MAX_PLAUSIBLE_FREEZE_MS + 1.0
        ));
    }

    #[test]
    fn reports_a_real_recoverable_stall() {
        // Visible, settled, plausible-length stall → a real freeze, surface it.
        assert!(!freeze_report_suppressed(false, 999_999.0, 6000.0));
    }
}
