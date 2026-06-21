//! Frozen-frame watchdog (stabilization sprint #3).
//!
//! ## What it is — and isn't (read this before touching it)
//! The app re-renders on the `requestAnimationFrame` loop (`main.rs`). The
//! acute "a frame panic kills the loop" freeze is **already fixed** (C1:
//! reschedule-next-frame *before* `frame()`, `main.rs`). This watchdog
//! catches the *other*, rarer freeze: a frame that gets stuck in a long /
//! infinite computation, where the loop is alive but not progressing.
//!
//! A main-thread timer can't detect a main-thread stall (it can't fire while
//! the thread is stuck), so the watcher lives **off the main thread** — a
//! tiny dedicated worker (no SDK, no protocol; spawned from a Blob URL). The
//! rAF loop posts it a heartbeat (throttled to ~1 Hz — cheap); if the worker
//! sees no beat for `threshold_ms`, the UI froze, and it reports back.
//!
//! **Honest scope (charter: promote on evidence, not speculation — this is
//! the most speculative sprint item, so it stays pure cheap observability):**
//! - ✅ *Visibility* — a freeze becomes a logged entry in the in-app
//!   diagnostics sink (`crate::diagnostics`, the Event Log). Today we are
//!   blind to freezes in the wild; this is the real value.
//! - ✅ *Recovery for recoverable freezes* — when the main thread unsticks and
//!   processes the report, an **opt-in** "hit a snag — Reload?" banner offers a
//!   clean reload (durable Worker tree intact). **Off by default**
//!   (`?watchdog-banner=1` to enable): a recoverable stall self-recovers, and
//!   popping the banner for an ordinary heavy-frame hitch is more alarming than
//!   the hitch. The freeze is logged either way — the log is the real value.
//! - ❌ It does **not** un-stick a *permanently* hung tab (nothing outside the
//!   main thread can) and does not prevent freezes. Smoke detector, not
//!   extinguisher.
//!
//! Cost: one `postMessage` per second + a sliver of a worker. Off via
//! `?watchdog=0`. Listener `Closure` held in a thread-local, never
//! `forget()`'d (charter D12 / AP1). Event data read via `js_sys::Reflect`
//! (codebase idiom) so no extra web-sys feature.

use crate::watchdog_policy::freeze_report_suppressed;
use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

struct State {
    worker: web_sys::Worker,
    last_beat_ms: f64,
    /// Wall-clock ms of the most recent return to the foreground
    /// (`visibilitychange` → visible). Used to suppress the resume-race false
    /// positive (see [`crate::watchdog_policy::RESUME_GRACE_MS`]).
    last_resume_ms: f64,
    /// Whether a detected freeze surfaces the user-facing "hit a snag — Reload?"
    /// banner. **Default off** — the banner is alarming for a normal user and a
    /// genuine recoverable stall self-recovers; the freeze is always recorded in
    /// the diagnostics sink regardless (the watchdog's real value is the *log*,
    /// not the prompt). Opt in with `?watchdog-banner=1` (also used by the e2e).
    show_banner: bool,
    /// Held so the onmessage closure lives for the session (D12).
    _onmsg: Closure<dyn FnMut(JsValue)>,
    /// Held so the visibilitychange closure lives for the session (D12).
    _onvis: Closure<dyn FnMut()>,
    /// Held so the window blur/focus closures live for the session (D12).
    /// These cover switching to another **window/app** — which throttles rAF
    /// but, unlike a tab switch, fires NO `visibilitychange` (the tab stays
    /// "visible"), so without them the paused heartbeat looked like a freeze.
    _onblur: Closure<dyn FnMut()>,
    _onfocus: Closure<dyn FnMut()>,
}

/// Pause or resume the watcher worker (resetting its clock on resume, so the
/// idle gap is discarded), and on resume stamp the return-to-foreground time so
/// a report racing the resume is caught by the grace window in
/// [`on_freeze_detected`]. Shared by the visibility + window blur/focus paths.
fn mark_paused(worker: &web_sys::Worker, paused: bool) {
    let _ = worker.post_message(&JsValue::from_str(if paused { "pause" } else { "resume" }));
    if !paused {
        let now = js_sys::Date::now();
        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.last_resume_ms = now;
            }
        });
    }
}

/// Inline watcher-worker source. Tracks the last heartbeat; once a second it
/// reports back the gap if the main thread has gone silent past the
/// threshold (no rendered frame → frozen). Resets after a report so it
/// doesn't spam while still frozen.
///
/// `pause`/`resume` gate the check: the browser pauses `requestAnimationFrame`
/// while the tab is hidden, so beats legitimately stop — that is NOT a freeze.
/// The main thread sends `pause` on `visibilitychange→hidden` and `resume`
/// (which resets the clock, discarding the hidden gap) on becoming visible.
fn worker_source(threshold_ms: u32) -> String {
    format!(
        "let last = Date.now(); let paused = false;\
         self.onmessage = (e) => {{\
            const d = e.data;\
            if (d === 'beat') last = Date.now();\
            else if (d === 'pause') paused = true;\
            else if (d === 'resume') {{ paused = false; last = Date.now(); }}\
         }};\
         setInterval(() => {{\
            if (paused) return;\
            const gap = Date.now() - last;\
            if (gap > {threshold_ms}) {{ self.postMessage(gap); last = Date.now(); }}\
         }}, 1000);"
    )
}

/// Spawn the watcher worker and wire its freeze report to the diagnostics
/// sink (always) + an optional reload-offer banner (`show_banner`). Call once
/// at boot, after diagnostics are installed. No-op on any failure (the app must
/// boot regardless).
pub fn install(threshold_ms: u32, show_banner: bool) {
    let Some(worker) = spawn_watcher(threshold_ms) else {
        tracing::warn!("frozen-frame watchdog: watcher worker spawn failed; skipping");
        return;
    };

    let onmsg = Closure::wrap(Box::new(move |event: JsValue| {
        let gap_ms = js_sys::Reflect::get(&event, &"data".into())
            .ok()
            .and_then(|d| d.as_f64())
            .unwrap_or(0.0);
        on_freeze_detected(gap_ms);
    }) as Box<dyn FnMut(JsValue)>);
    worker.set_onmessage(Some(onmsg.as_ref().unchecked_ref()));

    // Pause the watcher whenever rAF legitimately stops, so the silent beat
    // isn't mistaken for a freeze. TWO distinct browser behaviors stop rAF:
    //   * a TAB switch → `document.hidden` flips → `visibilitychange` (on doc);
    //   * a WINDOW / app switch → the window blurs but the tab stays "visible"
    //     (NO `visibilitychange`) → rAF still throttles/pauses. This is the
    //     case the visibility-only guard MISSED, and the actual source of the
    //     spurious "hit a snag" banner on alt-tab / clicking another window.
    // We pause on either signal and resume on the opposite.
    let onvis = {
        let worker = worker.clone();
        Closure::wrap(Box::new(move || mark_paused(&worker, document_hidden())) as Box<dyn FnMut()>)
    };
    let onblur = {
        let worker = worker.clone();
        Closure::wrap(Box::new(move || mark_paused(&worker, true)) as Box<dyn FnMut()>)
    };
    let onfocus = {
        let worker = worker.clone();
        Closure::wrap(Box::new(move || mark_paused(&worker, false)) as Box<dyn FnMut()>)
    };
    if let Some(win) = web_sys::window() {
        if let Some(doc) = win.document() {
            let _ = doc.add_event_listener_with_callback(
                "visibilitychange",
                onvis.as_ref().unchecked_ref(),
            );
        }
        let _ = win.add_event_listener_with_callback("blur", onblur.as_ref().unchecked_ref());
        let _ = win.add_event_listener_with_callback("focus", onfocus.as_ref().unchecked_ref());
        // Seed the initial state in case we boot hidden (prerender / bg tab). We
        // deliberately do NOT seed-pause on unfocused-at-boot: `document.hasFocus()`
        // is unreliable for headless/automation contexts (it can report unfocused
        // for the active window), which would wedge the watcher paused. A real
        // window blur after boot fires the `blur` listener regardless.
        if document_hidden() {
            let _ = worker.post_message(&JsValue::from_str("pause"));
        }
    }

    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            worker,
            last_beat_ms: js_sys::Date::now(),
            last_resume_ms: js_sys::Date::now(),
            show_banner,
            _onmsg: onmsg,
            _onvis: onvis,
            _onblur: onblur,
            _onfocus: onfocus,
        });
    });
    tracing::info!(threshold_ms, show_banner, "frozen-frame watchdog installed");
}

/// `document.hidden`, read via `Reflect` (file idiom — keeps the watchdog
/// free of an extra web-sys feature).
fn document_hidden() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| js_sys::Reflect::get(&d, &"hidden".into()).ok())
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn spawn_watcher(threshold_ms: u32) -> Option<web_sys::Worker> {
    let parts = js_sys::Array::new();
    parts.push(&JsValue::from_str(&worker_source(threshold_ms)));
    let blob = web_sys::Blob::new_with_str_sequence(&parts).ok()?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).ok()?;
    let worker = web_sys::Worker::new(&url).ok();
    let _ = web_sys::Url::revoke_object_url(&url);
    worker
}

/// Called once per frame from the rAF loop. Throttled to ~1 Hz internally,
/// so this is cheap to call every frame. No-op when the watchdog is off.
pub fn beat() {
    STATE.with(|s| {
        if let Some(state) = s.borrow_mut().as_mut() {
            let now = js_sys::Date::now();
            if now - state.last_beat_ms >= 1000.0 {
                state.last_beat_ms = now;
                let _ = state.worker.post_message(&JsValue::from_str("beat"));
            }
        }
    });
}

fn on_freeze_detected(gap_ms: f64) {
    let secs = (gap_ms / 1000.0).round().max(1.0) as u64;

    // Suppress environmental false positives (the user-facing bug: switching
    // tabs / device sleep popped the "hit a snag — Reload?" banner). A backgrounded
    // tab pauses rAF, so beats legitimately stop — that is NOT a freeze. We pause
    // the watcher on `visibilitychange→hidden`, but the worker can race the
    // `resume` and report the backgrounded gap anyway. Three guards, cheapest
    // first:
    //   1. currently hidden — definitely backgrounded, not a user-visible freeze;
    //   2. just returned to the foreground — the resume race (see RESUME_GRACE_MS);
    //   3. implausibly long gap — device sleep/suspend with no visibilitychange.
    let now = js_sys::Date::now();
    let ms_since_resume = STATE.with(|s| {
        s.borrow()
            .as_ref()
            .map(|st| now - st.last_resume_ms)
            .unwrap_or(f64::INFINITY)
    });
    if freeze_report_suppressed(document_hidden(), ms_since_resume, gap_ms) {
        crate::diagnostics::note(format!(
            "frozen-frame watchdog: ignored a ~{secs}s gap (tab backgrounded / \
             device sleep — not a real freeze)."
        ));
        return;
    }

    // Visibility: land it in the same in-app diagnostics sink as #4. This is
    // the watchdog's primary value and ALWAYS happens (invisible unless the
    // user opens the Event Log).
    crate::diagnostics::note(format!(
        "frozen-frame watchdog: the UI stopped rendering for ~{secs}s (a frame \
         stalled); it has since recovered."
    ));

    // The user-facing "hit a snag — Reload?" banner is OPT-IN (`?watchdog-banner=1`).
    // Off by default: a recoverable stall self-recovers, and popping an alarming
    // banner during normal use (a heavy render / big tree op crossing the
    // threshold) is worse than the brief hitch it reports.
    let show_banner = STATE.with(|s| s.borrow().as_ref().map(|st| st.show_banner).unwrap_or(false));
    if show_banner {
        show_reload_banner();
    }
}

/// A dismissible "hit a snag — Reload" banner. Reload boots clean (durable
/// Worker tree intact). Inline-onclick handlers — no Rust `Closure` to leak
/// (D12 / AP1), same pattern as `storage_durability`.
fn show_reload_banner() {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if doc.get_element_by_id("watchdog-banner").is_some() {
        return; // never stack
    }
    let Ok(banner) = doc.create_element("div") else {
        return;
    };
    banner.set_id("watchdog-banner");
    let _ = banner.set_attribute(
        "style",
        "display:flex;align-items:center;gap:12px;padding:8px 14px;background:#3a1e2e;\
         color:#eee;border-bottom:1px solid #9a4d7d;\
         font:13px/1.4 system-ui,-apple-system,sans-serif;",
    );
    if let Ok(text) = doc.create_element("span") {
        text.set_text_content(Some(
            "The app hit a snag and briefly stopped responding. Reload to get back \
             to a clean state — your saved data is kept.",
        ));
        let _ = text.set_attribute("style", "flex:1;");
        let _ = banner.append_child(&text);
    }
    if let Ok(btn) = doc.create_element("button") {
        btn.set_text_content(Some("Reload"));
        let _ = btn.set_attribute(
            "style",
            "padding:3px 10px;cursor:pointer;background:transparent;color:#eee;\
             border:1px solid currentColor;border-radius:4px;font-size:12px;",
        );
        let _ = btn.set_attribute("onclick", "location.reload()");
        let _ = banner.append_child(&btn);
    }
    if let Ok(btn) = doc.create_element("button") {
        btn.set_text_content(Some("Dismiss"));
        let _ = btn.set_attribute(
            "style",
            "padding:3px 10px;cursor:pointer;background:transparent;color:#eee;\
             border:1px solid currentColor;border-radius:4px;font-size:12px;",
        );
        let _ = btn.set_attribute("onclick", "this.closest('#watchdog-banner').remove()");
        let _ = banner.append_child(&btn);
    }
    if let Some(layout) = doc.get_element_by_id("app-layout") {
        let _ = layout.insert_before(&banner, layout.first_child().as_ref());
    } else if let Some(body) = doc.body() {
        let _ = body.insert_before(&banner, body.first_child().as_ref());
    }
}
