//! Browser-native failure capture (stabilization sprint #4).
//!
//! We have `console_error_panic_hook` but no visibility into *browser-level*
//! failures in production — uncaught exceptions and unhandled promise
//! rejections that originate outside our Rust call stack (a stale-bundle
//! mismatch, a Web API throwing, a detached port). Before this they vanished
//! into the devtools console, which production users never open
//! (`[[feedback_verify_through_service_worker]]` — users hit a stale-bundle
//! panic we couldn't see).
//!
//! This routes those events into **the entity system's own diagnostics
//! surface** — the tree-backed Event Log (`event_log_writer`, shown in the
//! Event Log window) — so a browser-level failure is captured and visible
//! *in-app*, next to internal diagnostics.
//!
//! **Deliberately modest** (charter scope discipline — we are not building a
//! telemetry platform): `error` + `unhandledrejection` only, on the main
//! thread and the worker. No `ReportingObserver`/`PerformanceObserver` suite,
//! no linear-memory gauge, no external SaaS. The frozen-frame watchdog (#3)
//! and any future signal feed the same [`note`] sink.
//!
//! Event fields are read via `js_sys::Reflect` (the codebase idiom for
//! browser APIs — same as `storage_durability` / `multitab`), so no extra
//! web-sys feature is needed. Listener `Closure`s are held for the session in
//! a thread-local, never `forget()`'d (charter D12 / AP1).

use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::event_log_writer::EventLogWriter;

thread_local! {
    /// The in-app diagnostics sink (main thread only). `None` in the worker
    /// context, where [`note`] falls back to `tracing` (forwarded to the
    /// main console).
    static SINK: RefCell<Option<EventLogWriter>> = const { RefCell::new(None) };
    /// Session-held listener closures — owned here so they live as long as
    /// the page without `forget()` (D12).
    static HANDLERS: RefCell<Vec<Closure<dyn FnMut(JsValue)>>> = const { RefCell::new(Vec::new()) };
}

/// Record a browser-level failure: always to `tracing` (a distinct target so
/// it's greppable + survives where the Event Log doesn't), and to the in-app
/// Event Log when a sink is installed (main thread). Safe to call from
/// anywhere, including the frozen-frame watchdog (#3).
pub fn note(message: impl Into<String>) {
    let message = message.into();
    tracing::error!(target: "browser_diagnostics", "{message}");
    SINK.with(|s| {
        if let Some(w) = s.borrow().as_ref() {
            w.log(message);
        }
    });
}

/// Install the main-thread sink + global `error` / `unhandledrejection`
/// listeners on `window`. Call once, after the Event Log writer exists.
pub fn install_main_thread(writer: EventLogWriter) {
    SINK.with(|s| *s.borrow_mut() = Some(writer));
    let Some(window) = web_sys::window() else {
        return;
    };
    let target: &web_sys::EventTarget = window.as_ref();
    attach(target, "error", &error_event_message);
    attach(target, "unhandledrejection", &rejection_event_message);
    tracing::info!("browser diagnostics: main-thread error/rejection capture installed");
}

// The worker runs as a separate bin crate (no shared lib), so its
// `error`/`unhandledrejection` capture is a small inline routine in
// `src/bin/entity-worker.rs` that routes to `tracing` (the worker has no
// Event Log writer; in-app forwarding over the wire is a noted follow-on,
// out of this modest scope).

/// Attach one listener that formats the event via `fmt` and routes it to
/// [`note`]. The kind label (`error`/`unhandledrejection`) prefixes the line.
fn attach(target: &web_sys::EventTarget, kind: &str, fmt: &'static dyn Fn(&JsValue) -> String) {
    let kind_owned = kind.to_string();
    let cb = Closure::wrap(Box::new(move |event: JsValue| {
        note(format!("⚠ uncaught {kind_owned}: {}", fmt(&event)));
    }) as Box<dyn FnMut(JsValue)>);
    if target
        .add_event_listener_with_callback(kind, cb.as_ref().unchecked_ref())
        .is_ok()
    {
        HANDLERS.with(|h| h.borrow_mut().push(cb));
    }
}

/// Best-effort string from an `ErrorEvent` (`message` + `filename:lineno`).
fn error_event_message(event: &JsValue) -> String {
    let msg = reflect_string(event, "message").unwrap_or_else(|| "(no message)".into());
    match (reflect_string(event, "filename"), reflect_string(event, "lineno")) {
        (Some(file), Some(line)) if !file.is_empty() => format!("{msg} ({file}:{line})"),
        _ => msg,
    }
}

/// Best-effort string from a `PromiseRejectionEvent` (`reason`).
fn rejection_event_message(event: &JsValue) -> String {
    js_sys::Reflect::get(event, &"reason".into())
        .ok()
        .map(|reason| {
            reason
                .as_string()
                .or_else(|| reflect_string(&reason, "message"))
                .unwrap_or_else(|| format!("{reason:?}"))
        })
        .unwrap_or_else(|| "(no reason)".into())
}

fn reflect_string(obj: &JsValue, key: &str) -> Option<String> {
    let v = js_sys::Reflect::get(obj, &key.into()).ok()?;
    if v.is_undefined() || v.is_null() {
        return None;
    }
    Some(v.as_string().unwrap_or_else(|| format!("{v:?}")))
}
