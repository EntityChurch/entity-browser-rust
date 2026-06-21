//! Runtime environment capability detection.
//!
//! Stage 1B: minimal surface — just probes whether a Web
//! Worker can be constructed in this environment. The boot path uses
//! this to pick Worker vs Direct mode automatically, with `?worker=`
//! URL param as an override.
//!
//! Future stages will extend this to cover:
//! - Worker-side OPFS availability (needs Ready handshake change to
//!   `Response::Ready` — see RUNTIME-CONFIG-ARCHITECTURE §12 U1).
//! - Main-thread OPFS availability (for an eventual Direct+OPFS mode).
//! - Cross-origin-isolation (needed only if SharedArrayBuffer enters
//!   the picture; the current worker is `Rc`-based and doesn't use it).

/// Probed runtime capabilities. Cheap to compute; safe to call from
/// the WASM entry point before any peers exist.
#[cfg(target_arch = "wasm32")]
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    /// `Worker` constructor exists on `window`. False in environments
    /// like Node tests, certain embedded WebViews, or extension
    /// background contexts that lack it. Spawning may still fail at
    /// runtime even when this is true (origin/COEP/network issues);
    /// the boot path catches that and falls back.
    pub worker_constructor: bool,
}

#[cfg(target_arch = "wasm32")]
impl Capabilities {
    /// Synchronous probe. No DOM mutation, no async; safe at boot.
    pub fn detect() -> Self {
        Self {
            worker_constructor: worker_constructor_exists(),
        }
    }

    /// Is the Worker mode supportable as a *preference*? `detect()` is
    /// optimistic — `true` here means we should try Worker mode; the
    /// boot path is still responsible for catching spawn/init failures
    /// and falling back to Direct.
    pub fn prefers_worker(&self) -> bool {
        self.worker_constructor
    }
}

#[cfg(target_arch = "wasm32")]
fn worker_constructor_exists() -> bool {
    let Some(window) = web_sys::window() else { return false };
    js_sys::Reflect::has(window.as_ref(), &wasm_bindgen::JsValue::from_str("Worker"))
        .unwrap_or(false)
}
