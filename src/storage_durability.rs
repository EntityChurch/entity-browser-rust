//! Boot-time storage-durability hooks (DAG node C5).
//!
//! Three release-scope fixes from the persistence-durability slate
//! (browser-storage-substrate research, charter D16):
//!
//! NOTE: the original C5 framing below predates the IDB-default flip
//! — the default arm is now the **durable main-thread IndexedDB** store
//! (`DurableDirectIdb`), NOT in-memory. The authoritative arm map is
//! [`BootStorageStatus`] in this file; treat it (not this header) as current.
//!
//! - **C5a — [`request_persistent_storage`]**: call `navigator.storage.persist()`
//!   at boot. The durable default tree (IDB main-thread, or Worker+OPFS under
//!   `?worker=1`) *is* durable, but storage is best-effort and evictable
//!   (Safari ~7-day ITP purge + cross-engine LRU) until the origin is granted
//!   *persistent* storage. We never asked. This asks once per boot (idempotent —
//!   checks `persisted()` first). It is an origin-scoped grant, so calling it on
//!   the main thread covers the OPFS that spawned workers use (OPFS is
//!   origin-scoped) AND the main-thread IDB store.
//!
//! - **C5b/C5c — [`show_storage_banner`]**: make the *ephemeral* arms honest.
//!   The durable arms (`DurableDirectIdb` default, `DurableWorker` under
//!   `?worker=1`) get no banner. The in-memory arms — `EphemeralDirect` (no
//!   IDB), `SecondaryTabEphemeral` (a multi-tab non-leader), and the dangerous
//!   silent `DowngradedToDirect` Worker→Direct fallback that orphans a durable
//!   OPFS tree and *looks* wiped (GAP-5 §2.4) — surface a banner (the downgrade
//!   case a stronger, warning-styled one).
//!
//! Reaches `navigator.storage` via `js_sys::Reflect` (same idiom as
//! `opfs_cleanup`) so no `StorageManager` web-sys feature is required.
//!
//! The banner is plain boot-level DOM — a sibling of the loading indicator
//! and the mode class (`main.rs` / `dom::util::set_mode_class`), *not*
//! entity-backed window state. Its dismiss button uses an inline `onclick`
//! (like index.html's "Try Again"), so there is no Rust `Closure` to leak
//! (charter D12 / AP1).

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// C5a — request persistent (non-evictable) storage for this origin.
///
/// Awaited from boot (`spawn_local`). Idempotent: if the origin is already
/// persistent we skip the request. Logs the outcome; never throws, never
/// blocks boot.
///
/// Returns the grant outcome so the caller can make a durable-but-evictable
/// mode honest (D16): `Some(true)` = persistent/eviction-protected,
/// `Some(false)` = best-effort/evictable, `None` = API unavailable (the mode
/// itself then determines durability, e.g. Tauri WebKitGTK).
pub async fn request_persistent_storage() -> Option<bool> {
    // Logged synchronously (before the first await) so the request is
    // observable even if the navigator.storage promise resolves slowly or
    // never (some headless/automation runtimes). The outcome line below
    // follows when/if the promise settles.
    tracing::info!("storage durability: requesting persistent storage (persist())");
    let outcome = try_request().await;
    match outcome {
        Some(true) => tracing::info!(
            "storage durability: origin has persistent storage (eviction-protected)"
        ),
        Some(false) => tracing::warn!(
            "storage durability: persistent storage NOT granted — OPFS/localStorage \
             remain evictable (Safari ITP / cross-engine LRU). Best-effort only."
        ),
        None => tracing::info!(
            "storage durability: navigator.storage.persist() unavailable in this \
             runtime; skipping (mode determines durability)"
        ),
    }
    outcome
}

async fn try_request() -> Option<bool> {
    let global = js_sys::global();
    let navigator = js_sys::Reflect::get(&global, &"navigator".into()).ok()?;
    let storage = js_sys::Reflect::get(&navigator, &"storage".into()).ok()?;
    if storage.is_undefined() || storage.is_null() {
        return None;
    }
    // Already persistent? Don't re-request (avoids any UA prompt).
    if let Some(true) = call_bool_method(&storage, "persisted").await {
        return Some(true);
    }
    call_bool_method(&storage, "persist").await
}

/// Call a zero-arg `StorageManager` method that resolves to a bool
/// (`persist` / `persisted`). Returns `None` if the method is missing or
/// the promise rejects.
async fn call_bool_method(storage: &wasm_bindgen::JsValue, name: &str) -> Option<bool> {
    let f = js_sys::Reflect::get(storage, &name.into()).ok()?;
    let f: js_sys::Function = f.dyn_into().ok()?;
    let promise = f.call0(storage).ok()?;
    let result = JsFuture::from(js_sys::Promise::from(promise)).await.ok()?;
    result.as_bool()
}

/// How durable is this boot's primary tree? Drives the C5b/C5c banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootStorageStatus {
    /// Worker mode bootstrapped — primary tree is OPFS-journaled (durable).
    DurableWorker,
    /// Direct mode with a **main-thread IndexedDB-backed primary** — the
    /// primary tree (settings, window state, content) is durable across
    /// reload via the write-behind IDB journal, independent of any worker.
    /// This is the durable Direct arm (plain browser `?worker=0`, Tauri
    /// WebView). Durability is best-effort on an *abrupt* kill (the last
    /// unflushed ~250ms window), but identity/destructive ops checkpoint-
    /// flush, so it is durable on a normal reload — no "not saved" banner.
    DurableDirectIdb,
    /// Direct mode chosen up front (`?worker=0`, no Worker support, or
    /// Tauri) with no durable backend — tree is in-memory only.
    EphemeralDirect,
    /// Worker was wanted but bootstrap failed and we fell back to Direct —
    /// the durable OPFS tree (if any) is now orphaned this session.
    DowngradedToDirect,
    /// Another live tab already owns this peer's durable storage (Web Locks
    /// leader election, `crate::multitab`). We stayed ephemeral Direct
    /// *intentionally* rather than fight for the exclusive OPFS handle. The
    /// owning tab is durable; this one is not — distinct from a storage
    /// failure, so it gets its own specific message.
    SecondaryTabEphemeral,
}

/// C5b/C5c — surface an honest "not saved" banner for the in-memory modes.
///
/// No-op for [`BootStorageStatus::DurableWorker`]. Suppressed under Tauri:
/// the WebView frontend is forced-Direct but has a separate native-backend
/// persistence story (under investigation — see
/// the release-acceptance standards §4), so a browser-storage
/// "not saved" claim would be misleading there.
pub fn show_storage_banner(status: BootStorageStatus, in_tauri: bool) {
    if in_tauri {
        return;
    }
    let (msg, bg, border) = match status {
        BootStorageStatus::DurableWorker => return,
        // Durable on a normal reload (IDB journal) — no "not saved" banner,
        // same as Worker mode. The tiny abrupt-kill window is covered by
        // checkpoint-flush on identity ops.
        BootStorageStatus::DurableDirectIdb => return,
        BootStorageStatus::EphemeralDirect => (
            "Direct mode: your entity tree lives in memory only and is lost on \
             reload (your identity is preserved).",
            "#3a2e10",
            "#9a7d22",
        ),
        BootStorageStatus::DowngradedToDirect => (
            "Storage unavailable: background (Worker) storage failed to start, so \
             your entity tree won't be saved this session and any previously saved \
             tree isn't loaded. Try reloading.",
            "#4a1e1e",
            "#a23a3a",
        ),
        BootStorageStatus::SecondaryTabEphemeral => (
            "This app is already open in another tab, which owns your saved data. \
             Changes in THIS tab are not being saved. Close the other tab and \
             reload here to edit your saved tree.",
            "#3a2e10",
            "#9a7d22",
        ),
    };
    inject_banner(msg, bg, border);
}

/// C5d — durable-but-evictable honesty (D16).
///
/// Worker mode IS durable on a normal reload (OPFS-journaled), so it gets no
/// "not saved" banner. But until the origin is granted *persistent* storage,
/// that durable tree is still **best-effort**: evictable under storage
/// pressure (all engines, LRU) and on iOS/Safari after ~7 days without a
/// visit (ITP). `request_persistent_storage()` told us whether the grant
/// landed; when it did NOT (`Some(false)` denied, or `None` API-absent in a
/// durable-claiming runtime), saying "saved" without qualification is the
/// lie D16 forbids. Soft amber banner — distinct from the EphemeralDirect
/// "lost on reload" warning: this data survives a reload, it's just not
/// eviction-protected.
///
/// Caller contract: only invoke in `DurableWorker` mode and only when the
/// grant was not `Some(true)`. No-op styling/leak story identical to
/// [`show_storage_banner`] (inline-onclick dismiss, no Rust `Closure`).
pub fn show_evictable_banner() {
    inject_banner(
        "Your data is saved on this device, but the browser hasn't granted \
         persistent storage — it may be cleared if the device runs low on space, \
         or (on iOS/Safari) after about a week without opening this site. \
         Bookmark or install to Home Screen to make it permanent.",
        "#2e2a14",
        "#7d6b22",
    );
}

/// The shared "not saved" / durability banner (one id, since the storage and
/// evictable variants are mutually exclusive).
fn inject_banner(msg: &str, bg: &str, border: &str) {
    inject_banner_with_id("storage-banner", msg, bg, border);
}

/// 1a durability/capability gate (MAP §10): a transient banner shown when
/// `CreatePeerWithMode` is refused, so the failure is never silent (D13). Its
/// own id, so it coexists with a secondary-tab "not saved" banner — which is
/// precisely the situation that triggers it. Red/warning styled (a blocked
/// action, distinct from the amber "still works, just not saved" notices).
pub fn show_action_refused_banner(reason: &str) {
    inject_banner_with_id(
        "create-refused-banner",
        &format!("Peer not created — {reason}"),
        "#4a1e1e",
        "#a23a3a",
    );
}

/// Inject a dismissible boot-level banner with the given DOM `id`. Idempotent
/// per id (never stacks the same banner) — distinct ids let, e.g., a secondary-
/// tab "not saved" banner and a "create refused" banner coexist.
fn inject_banner_with_id(id: &str, msg: &str, bg: &str, border: &str) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    // Idempotent — never stack the same banner.
    if doc.get_element_by_id(id).is_some() {
        return;
    }
    let Ok(banner) = doc.create_element("div") else {
        return;
    };
    banner.set_id(id);
    let _ = banner.set_attribute(
        "style",
        &format!(
            "display:flex;align-items:center;gap:12px;padding:8px 14px;background:{bg};\
             color:#eee;border-bottom:1px solid {border};\
             font:13px/1.4 system-ui,-apple-system,sans-serif;"
        ),
    );

    if let Ok(text) = doc.create_element("span") {
        text.set_text_content(Some(msg));
        let _ = text.set_attribute("style", "flex:1;");
        let _ = banner.append_child(&text);
    }
    if let Ok(btn) = doc.create_element("button") {
        btn.set_text_content(Some("Dismiss"));
        let _ = btn.set_attribute(
            "style",
            "padding:3px 10px;cursor:pointer;background:transparent;color:#eee;\
             border:1px solid currentColor;border-radius:4px;font-size:12px;",
        );
        // Inline JS — no Rust Closure to manage/leak (D12 / AP1). Walks up to
        // the banner div and removes it (works for any banner id).
        let _ = btn.set_attribute(
            "onclick",
            "this.parentNode && this.parentNode.remove()",
        );
        let _ = banner.append_child(&btn);
    }

    // Top of the layout, above the status bar (fall back to <body>).
    if let Some(layout) = doc.get_element_by_id("app-layout") {
        let _ = layout.insert_before(&banner, layout.first_child().as_ref());
    } else if let Some(body) = doc.body() {
        let _ = body.insert_before(&banner, body.first_child().as_ref());
    }
}
