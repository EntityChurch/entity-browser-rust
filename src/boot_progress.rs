//! The boot surface — what the page shows until the app knows what to render.
//!
//! **Why this exists.** `boot_load` awaits ~14 steps and only five of them are
//! fundamental (boot audit `AUDIT-BOOT-PATH-2026-08-27.md` §1); the frame loop
//! and the frozen-frame watchdog both install *after* it returns. So every
//! application-tier boot step ran with nothing behind it — and until this
//! module existed the page was **blank** for the whole of it. Three facts
//! stacked to make that total rather than a worst case:
//!
//! 1. `start()` hid `#loading` immediately after the DOM-mode class, *before*
//!    peer construction and `boot_load`.
//! 2. The one early-paint path that could have covered the gap,
//!    [`crate::boot_fast_paint`], is `DISABLED_FOR_CONSOLIDATION` — so it
//!    paints nothing on any deployment today.
//! 3. The always-visible *"Stuck here? Open System Recovery →"* hatch lives
//!    inside that same `#loading` div, so it went down with it — the escape
//!    hatch was removed exactly when the boot became unobservable.
//!
//! Bounded worst case before first paint is tens of seconds (one 3 s
//! deployment-document deadline plus four 5 s origin writes). The audit's own
//! phrasing is the point: **"bounded is not the same as observable."**
//!
//! # Two signals, not one — and the split is the 2026-09-02 change
//!
//! This module used to have a single structural call, `armed()`, which meant
//! two things at once: *the rAF loop is live* and *take the surface down*. The
//! two-phase boot separates them, because they became true at different times
//! and conflating them is what forced a choice between a blank page and a
//! flicker:
//!
//! * [`frame_loop_live`] — the rAF loop is running and the app is rendering,
//!   **behind** this surface. From here on an application-tier step that
//!   stalls, panics or hangs costs a late config, not a dead page: there is a
//!   live frame loop, an installed watchdog and a reachable recovery hatch
//!   underneath. One call site, the rAF arm in `start()`.
//! * [`surface_down`] — the app now knows what to show. Called when boot phase
//!   2 finishes (however it finishes), and by this module's own failsafe.
//!
//! **The user still sees exactly one transition**, boot surface → the finished
//! app, which is what makes the deferred order flicker-free. The old choice —
//! take the surface down at `frame_loop_live` and let phase 2 change it
//! underneath the user — is the flicker, and it was never the point of row 8.
//! The point is that the failure is non-fatal, and that is entirely bought by
//! *where the frame loop is armed*, not by *when the surface comes down*.
//!
//! # The failsafe is the guarantee, and it is structural (AP44)
//!
//! Holding the surface until phase 2 reports back would re-create the brick if
//! phase 2 never reports back. So the hold is bounded here, in the module that
//! owns the surface, armed by the same one call site — not by a rule each
//! completion path has to remember. `surface_down` is idempotent, so the two
//! callers cannot fight.
//!
//! **English, deliberately — and it adds no locale keys.** `index.html` carries
//! no i18n at all. This surface is the same tier as the L1 recovery console:
//! it has to render when the app cannot, which is precisely when the catalog
//! machinery is the thing that might not be there. [`step`] writes a short
//! technical token for a bug report, not prose; the headline the user reads
//! stays the one already in the markup.
//!
//! **The load-bearing signals are [`frame_loop_live`] and [`surface_down`], not
//! [`step`].** `step` is best-effort by design: a boot step added without a call
//! here costs one line in a bug report and nothing else. That split is
//! deliberate — the half that has to be right is the half that is structural.

use std::cell::Cell;

use wasm_bindgen::JsCast;

/// The full-screen boot surface in `index.html`, and the two lines inside it.
const ROOT_ID: &str = "loading";
const SUBSTEP_ID: &str = "loading-substep";

/// The longest the boot surface may stay up after the frame loop is live,
/// whatever phase 2 is doing.
///
/// **Chosen to sit ABOVE phase 2's own bounded worst case, not below it.** That
/// worst case is one 3 s deployment-document deadline (D23) plus up to four 5 s
/// origin seeds (`SEED_TIMEOUT_MS`) — about 23 s. A failsafe under that number
/// would fire on a boot that is merely slow and produce the exact flicker this
/// ordering exists to avoid; above it, the only thing that reaches the failsafe
/// is a phase 2 that has hung past every bound it declares, which is the case
/// that used to be an unbounded wait with no way out.
///
/// The user is not left guessing during the hold: `index.html`'s stall reporter
/// escalates at 10 s, names the last [`step`], and promotes the recovery hatch.
const HOLD_FAILSAFE_MS: u32 = 30_000;

thread_local! {
    /// Set once the surface has been taken down, so the failsafe and the phase-2
    /// completion path cannot fight and the log line prints once.
    static SURFACE_IS_DOWN: Cell<bool> = const { Cell::new(false) };
}

fn set_text(id: &str, text: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    {
        el.set_text_content(Some(text));
    }
}

/// `?boothold=<ms>` — override [`HOLD_FAILSAFE_MS`].
///
/// **A test affordance nothing in the product sets**, the same shape and the
/// same justification as `?bootstall=` in `index.html`: without it the failsafe
/// branch is unreachable from the harness, because reaching it honestly means a
/// 30 s gate against an origin that has to stall for all of it. The alternative
/// on the table was shipping the one branch that only runs on the bad day with
/// no test at all.
fn hold_failsafe_ms() -> u32 {
    let Some(window) = web_sys::window() else {
        return HOLD_FAILSAFE_MS;
    };
    let Ok(search) = window.location().search() else {
        return HOLD_FAILSAFE_MS;
    };
    for pair in search.trim_start_matches('?').split('&') {
        if let Some(v) = pair.strip_prefix("boothold=") {
            if let Ok(ms) = v.parse::<u32>() {
                return ms;
            }
        }
    }
    HOLD_FAILSAFE_MS
}

/// Name the boot step now running, on the surface the user is looking at.
///
/// Best-effort and deliberately terse — this is the line a bug report quotes
/// when someone says *"it just sits there"*, and before it existed the honest
/// answer was that nobody could tell which of fourteen awaits it sat on. A
/// missing call costs that line and nothing else; see the module note on why
/// this half is not structural and the other two are.
pub fn step(label: &str) {
    set_text(SUBSTEP_ID, label);
}

/// The rAF loop is live and the app is rendering **behind** this surface.
///
/// **One call site, and that is the design** — the rAF arm in `start()`. It is
/// what makes an application-tier boot step non-fatal: from here on a step that
/// stalls or panics leaves a live frame loop, an installed frozen-frame watchdog
/// and a reachable recovery hatch, instead of a page that never finished.
///
/// It does **not** take the surface down. The app does not yet know what to
/// render — the startup surface is decided in phase 2, behind the deployment
/// document and the supersession adoption, which is a data dependency and not a
/// preference (`rekeyed_domain_heals_on_next_boot_window_surface` is the gate
/// that established it). Painting a guessed surface here and correcting it in
/// phase 2 is the flicker; [`surface_down`] is where the page is handed over.
///
/// Arms the hold failsafe, so the surface always comes down in bounded time even
/// if phase 2 never reports back.
pub fn frame_loop_live() {
    let ms = hold_failsafe_ms();
    wasm_bindgen_futures::spawn_local(async move {
        crate::dispatch_handle::delay_ms(ms).await;
        if !SURFACE_IS_DOWN.with(|d| d.get()) {
            tracing::error!(
                hold_ms = ms,
                "BOOT HOLD FAILSAFE — boot phase 2 has not reported back and the boot \
                 surface is coming down anyway. The frame loop has been live the whole \
                 time, so the app is usable; its deployment config may be unreconciled. \
                 This is the branch that used to be an unbounded wait."
            );
            surface_down("hold failsafe");
        }
    });
}

/// The hand-over, as a guard rather than as a call someone has to remember.
///
/// **Why RAII and not a line at the end of phase 2 (AP44).** A trailing
/// `surface_down()` is reached only when phase 2 returns normally — and the two
/// paths that most need the page handed over are the ones that do not: a panic
/// unwinding out of an await, and an early return added later by an author who
/// did not have this rule in their head. Dropping is the one thing every path
/// does.
///
/// It also keeps the outcome honest. The reason starts at *did not complete* and
/// is promoted by [`HandOver::completed`], so a boot that fell out early cannot
/// report itself as a boot that finished — the same shape as the recovery
/// console deciding by a witness instead of by what a call returned.
pub struct HandOver {
    reason: &'static str,
}

impl HandOver {
    /// Arm the hand-over. Until [`Self::completed`] is called this reports a
    /// phase 2 that did not run to the end.
    pub fn pending() -> Self {
        Self {
            reason: "phase 2 did not complete",
        }
    }

    /// Phase 2 ran to completion — promote the reason the hand-over will report.
    pub fn completed(&mut self) {
        self.reason = "phase 2 complete";
    }
}

impl Drop for HandOver {
    fn drop(&mut self) {
        if self.reason != "phase 2 complete" {
            tracing::error!(
                "BOOT PHASE 2 DID NOT COMPLETE — it panicked or returned early. The \
                 frame loop has been live throughout, so the app is usable with an \
                 unreconciled deployment config; this is the failure that used to be a \
                 page that never painted."
            );
        }
        surface_down(self.reason);
    }
}

/// The app knows what to show — take the boot surface down.
///
/// Idempotent, and it names *why* the page was handed over: `phase 2 complete`
/// on the ordinary path, `phase 2 failed` when the deferred half panicked, and
/// `hold failsafe` when it never reported at all. Those are three different
/// facts about a boot and a log that renders them as one is the AP40 shape; the
/// second and third are the ones a bug report needs.
///
/// If boot dies before anything reaches here the surface deliberately **stays
/// up** until the failsafe, carrying the recovery link — a page that never
/// finished booting should not look like one that did.
pub fn surface_down(reason: &str) {
    if SURFACE_IS_DOWN.with(|d| d.replace(true)) {
        return;
    }
    tracing::info!(reason, "boot surface down — the app owns the page");
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(ROOT_ID))
    {
        let _ = el
            .dyn_ref::<web_sys::HtmlElement>()
            .map(|e| e.style().set_property("display", "none"));
    }
}
