//! The boot surface — what the page shows between WASM start and the frame loop.
//!
//! **Why this exists.** `boot_load` awaits ~14 steps and only five of them are
//! fundamental (boot audit `AUDIT-BOOT-PATH-2026-08-27.md` §1); the frame loop
//! and the frozen-frame watchdog both install *after* it returns. So every
//! application-tier boot step runs with nothing behind it — and until this
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
//! **What this is NOT.** It does not make an application-tier step non-fatal.
//! That is boot audit **B-1**, the two-phase boot, and it is blocked on a
//! substrate constraint rather than on effort: `boot_load` takes `&mut self`,
//! the rAF closure `try_borrow_mut()`s the same `Rc<RefCell<EntityApp>>` every
//! frame, and nearly every application step needs `&Peers` — which is owned by
//! value inside that cell, is not `Clone`, and carries ~20 `&mut self` methods.
//! Spawning `boot_load` behind the loop without first making the router
//! shareable reproduces the blank page exactly, with a `FRAME SKIP` line under
//! it. This module closes the **observability** half only: a stalled boot is
//! now visible, named, and has a way out, instead of being indistinguishable
//! from a brick.
//!
//! **English, deliberately — and it adds no locale keys.** `index.html` carries
//! no i18n at all. This surface is the same tier as the L1 recovery console:
//! it has to render when the app cannot, which is precisely when the catalog
//! machinery is the thing that might not be there. [`step`] writes a short
//! technical token for a bug report, not prose; the headline the user reads
//! stays the one already in the markup.
//!
//! **The load-bearing signal is [`armed`], not [`step`].** `armed` has exactly
//! one call site — the rAF arm in `start()` — so *"did boot finish"* cannot
//! rot, and the stall timer in `index.html` keys off nothing more than whether
//! this surface is still up. [`step`] is best-effort by design: a boot step
//! added without a call here costs one line in a bug report and nothing else.
//! That split is deliberate (AP44) — the half that has to be right is the half
//! that is structural.

use wasm_bindgen::JsCast;

/// The full-screen boot surface in `index.html`, and the two lines inside it.
const ROOT_ID: &str = "loading";
const SUBSTEP_ID: &str = "loading-substep";

fn set_text(id: &str, text: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    {
        el.set_text_content(Some(text));
    }
}

/// Name the boot step now running, on the surface the user is looking at.
///
/// Best-effort and deliberately terse — this is the line a bug report quotes
/// when someone says *"it just sits there"*, and before it existed the honest
/// answer was that nobody could tell which of fourteen awaits it sat on. A
/// missing call costs that line and nothing else; see the module note on why
/// this half is not structural and [`armed`] is.
pub fn step(label: &str) {
    set_text(SUBSTEP_ID, label);
}

/// The rAF loop is armed and the app owns the page — take the boot surface down.
///
/// **One call site, and that is the design.** Everything else about this module
/// is best-effort; this is the signal the stall timer in `index.html` waits on,
/// so it is called from the rAF arm in `start()` and nowhere else. If boot dies
/// before reaching it the surface deliberately **stays up**, carrying the
/// recovery link — a page that never finished booting should not look like one
/// that did.
pub fn armed() {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(ROOT_ID))
    {
        let _ = el
            .dyn_ref::<web_sys::HtmlElement>()
            .map(|e| e.style().set_property("display", "none"));
    }
}
