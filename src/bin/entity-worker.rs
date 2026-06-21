//! WASM worker binary hosting the entity-core peer for the entity-browser frontend.
//!
//! This is the `entity-worker` bin; Trunk builds it as a separate worker
//! bundle alongside the `entity-browser` app bundle from `index.html`
//! (`<link data-trunk rel="rust" data-bin="entity-worker" data-type="worker">`),
//! so a single `make wasm` produces both. The whole file is
//! `#[cfg(target_arch = "wasm32")]` — it only compiles on the wasm32 target.
//!
//! Phase 3.0 — `Vec::new()` for handler factories: SDK bootstrap
//! handlers cover everything Settings + Event Log pilots need. Phase 1.x
//! revisits when a custom-handler consumer materializes.

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn worker_main() {
    console_error_panic_hook::set_once();
    init_worker_tracing();
    install_worker_error_capture();
    entity_wasm_worker_host::run_worker(Vec::new());
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    /// Worker `error`/`unhandledrejection` listeners, held for the worker's
    /// lifetime — owned here, never `forget()`'d (charter D12 / AP1).
    static WORKER_ERR_HANDLERS: std::cell::RefCell<Vec<Closure<dyn FnMut(JsValue)>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Sprint #4: capture uncaught errors + unhandled rejections in the worker
/// context and route them to `tracing::error!` (forwarded to the main
/// console). The worker is a separate bin crate with no shared lib and no
/// Event Log writer, so this is a small inline routine — in-app forwarding
/// of worker errors over the wire is a noted follow-on. Modest scope.
#[cfg(target_arch = "wasm32")]
fn install_worker_error_capture() {
    use wasm_bindgen::JsCast;
    let Ok(target) = js_sys::global().dyn_into::<web_sys::EventTarget>() else {
        return;
    };
    for kind in ["error", "unhandledrejection"] {
        let cb = Closure::wrap(Box::new(move |event: JsValue| {
            let detail = js_sys::Reflect::get(&event, &"message".into())
                .ok()
                .and_then(|v| v.as_string())
                .or_else(|| {
                    js_sys::Reflect::get(&event, &"reason".into())
                        .ok()
                        .map(|r| r.as_string().unwrap_or_else(|| format!("{r:?}")))
                })
                .unwrap_or_default();
            tracing::error!(target: "browser_diagnostics", "⚠ uncaught worker {kind}: {detail}");
        }) as Box<dyn FnMut(JsValue)>);
        if target
            .add_event_listener_with_callback(kind, cb.as_ref().unchecked_ref())
            .is_ok()
        {
            WORKER_ERR_HANDLERS.with(|h| h.borrow_mut().push(cb));
        }
    }
}

/// Configure `tracing_wasm` in the worker context.
///
/// Without this call every `tracing::*` invocation inside the worker
/// (including `entity_wasm_worker_host`, `entity_sdk`, `entity_peer`,
/// `entity_store::opfs`, etc.) is a silent no-op — there is no
/// subscriber wired up. We mirror the main-thread default (DEBUG in
/// dev builds, INFO in release) and honour an inherited `?log=…` URL
/// param the main thread propagates via the loader spawn URL.
#[cfg(target_arch = "wasm32")]
fn init_worker_tracing() {
    let level = worker_log_level();
    let mut builder = tracing_wasm::WASMLayerConfigBuilder::new();
    builder.set_max_level(level);
    tracing_wasm::set_as_global_default_with_config(builder.build());
    tracing::info!(level = %level, "entity-worker: tracing initialized");
}

#[cfg(target_arch = "wasm32")]
fn worker_log_level() -> tracing::Level {
    let default = if cfg!(debug_assertions) {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    log_level_from_worker_url().unwrap_or(default)
}

#[cfg(target_arch = "wasm32")]
fn log_level_from_worker_url() -> Option<tracing::Level> {
    use wasm_bindgen::JsCast;
    let scope: web_sys::WorkerGlobalScope = js_sys::global().dyn_into().ok()?;
    let search = scope.location().search();
    let q = search.trim_start_matches('?');
    for pair in q.split('&') {
        let pair: &str = pair;
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some("log") {
            let raw = parts.next().unwrap_or("");
            return match raw.to_ascii_lowercase().as_str() {
                "trace" => Some(tracing::Level::TRACE),
                "debug" => Some(tracing::Level::DEBUG),
                "info" => Some(tracing::Level::INFO),
                "warn" | "warning" => Some(tracing::Level::WARN),
                "error" | "off" => Some(tracing::Level::ERROR),
                _ => None,
            };
        }
    }
    None
}

// The bin target requires fn main(); wasm-bindgen handles entry via
// `#[wasm_bindgen(start)]` on wasm32. Native main is a no-op stub.
fn main() {}
