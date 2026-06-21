//! Multi-tab single-writer guard (stabilization sprint #1).
//!
//! ## The hazard (TRIAGE §4.1; charter D16 / D9-persistence)
//! Two tabs of the app on one origin share `localStorage` (the keypair →
//! the same primary `peer_id`) and therefore the same OPFS root
//! `workers/{peer_id}`. OPFS `createSyncAccessHandle` is exclusive per
//! file, so a second tab's worker cannot journal. Before this guard the
//! second tab silently downgraded to ephemeral Direct and *looked* normal
//! — a data-loss papercut: the user edits in a tab that never saves.
//!
//! ## The guard
//! A Web Locks (`navigator.locks`) leader election keyed on the peer_id.
//! The first tab acquires an exclusive lock and holds it for its lifetime
//! (the lock auto-releases when the tab is closed); a later tab finds the
//! lock held (`{ifAvailable: true}` → callback gets `null`) and knows it is
//! the **secondary**, so the caller can stay ephemeral *intentionally* and
//! say so, instead of fighting for the OPFS handle. Deterministic — no
//! claim/timeout race a `BroadcastChannel` ping would need.
//!
//! Web Locks is broadly supported (Chrome 69+, Firefox 96+, Safari 15.4+),
//! correcting the handoff's stale "Chromium-only" note (charter D8 — surface
//! drift, don't normalize). Where it is absent we return "not secondary" and
//! fall through to the existing OPFS-handle-exclusivity backstop (the second
//! worker's init fails → `DowngradedToDirect`) — data-safe, just without the
//! specific message.
//!
//! Reached via `js_sys::Reflect` (same idiom as `storage_durability`) so no
//! web-sys unstable-API feature and no `eval`/CSP dependency. The lock
//! callback is a `Closure` kept alive for the session in a thread-local
//! (charter D12: owned and droppable, never `forget()`).

use std::cell::RefCell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

thread_local! {
    /// Holds the lock-request callback for the session when this tab is the
    /// owner. The Web Locks manager invokes it once and keeps the lock held
    /// while the callback's returned promise is pending; we keep the
    /// `Closure` here so it isn't dropped (charter D12 — owned, never
    /// `forget()`). One slot suffices: `detect_secondary` runs once at boot.
    static OWNER_GUARD: RefCell<Option<Closure<dyn FnMut(JsValue) -> JsValue>>> =
        const { RefCell::new(None) };
}

/// Returns `true` when another live tab already owns `peer_id`'s durable
/// storage — this tab should stay ephemeral and say so. Returns `false`
/// when this tab is the owner (lock acquired + held for the session) or
/// when Web Locks is unavailable (backstop applies).
///
/// Resolves promptly: Web Locks `ifAvailable` reports immediately, no
/// timeout window.
pub async fn detect_secondary(peer_id: &str) -> bool {
    let Some(locks) = navigator_locks() else {
        tracing::info!(
            "multi-tab guard: navigator.locks unavailable; relying on the \
             OPFS-handle-exclusivity backstop"
        );
        return false;
    };
    let lock_name = format!("entity-primary:{peer_id}");

    // Outer promise resolves to "owner" / "secondary" from inside the lock
    // callback. We can't await `request()`'s own promise: for the owner it
    // never resolves (the callback returns a pending promise to hold the
    // lock for the tab's lifetime).
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let resolve_for_cb = resolve.clone();
        let cb = Closure::wrap(Box::new(move |lock: JsValue| -> JsValue {
            if lock.is_null() {
                // `ifAvailable` gave us no lock → another tab holds it.
                let _ =
                    resolve_for_cb.call1(&JsValue::UNDEFINED, &JsValue::from_str("secondary"));
                JsValue::UNDEFINED
            } else {
                // We acquired it → we are the owner. Signal it, then hold the
                // lock for this tab's lifetime by returning a pending promise
                // (the manager releases the lock when this tab is destroyed).
                let _ = resolve_for_cb.call1(&JsValue::UNDEFINED, &JsValue::from_str("owner"));
                js_sys::Promise::new(&mut |_r, _j| {}).into()
            }
        }) as Box<dyn FnMut(JsValue) -> JsValue>);

        // navigator.locks.request(name, { ifAvailable: true }, cb)
        let opts = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&opts, &"ifAvailable".into(), &JsValue::TRUE);
        let name_val = JsValue::from_str(&lock_name);
        let request_fn = js_sys::Reflect::get(&locks, &"request".into())
            .ok()
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok());
        match request_fn {
            Some(request_fn)
                if request_fn
                    .call3(&locks, &name_val, opts.as_ref(), cb.as_ref())
                    .is_ok() => {}
            // request() missing or threw → can't determine; treat as owner
            // (the OPFS backstop still protects the journal).
            _ => {
                let _ = resolve.call1(&JsValue::UNDEFINED, &JsValue::from_str("owner"));
            }
        }
        // Keep the callback alive for the session: the owner case holds the
        // lock through it; harmless for the secondary case.
        OWNER_GUARD.with(|g| *g.borrow_mut() = Some(cb));
    });

    match JsFuture::from(promise).await {
        Ok(v) if v.as_string().as_deref() == Some("secondary") => {
            tracing::warn!(
                "multi-tab guard: another tab owns this peer's durable storage — \
                 staying ephemeral (this tab will not save)"
            );
            true
        }
        Ok(_) => {
            tracing::info!("multi-tab guard: this tab owns the durable primary (lock held)");
            false
        }
        Err(_) => false,
    }
}

fn navigator_locks() -> Option<JsValue> {
    let global = js_sys::global();
    let navigator = js_sys::Reflect::get(&global, &"navigator".into()).ok()?;
    let locks = js_sys::Reflect::get(&navigator, &"locks".into()).ok()?;
    if locks.is_undefined() || locks.is_null() {
        None
    } else {
        Some(locks)
    }
}
