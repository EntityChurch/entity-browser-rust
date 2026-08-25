//! Boot-time IndexedDB database cleanup for deleted `frontend-idb` peers.
//!
//! A durable this-tab (`frontend-idb`) peer keeps its tree in its own
//! `entity-peer-{peer_id}` IndexedDB database. When the user deletes such a
//! peer, `persistence::mark_idb_for_cleanup(peer_id)` records the id in the
//! `entity_idb_tombstones` localStorage list; the vault + roster entries are
//! removed synchronously in the same delete. At the next boot, before the peer
//! could ever be reopened (it won't be — it's off the roster), [`run_at_boot`]
//! drains the list and `deleteDatabase`s each store. Boot is the one point at
//! which no connection is held, so the deletion cannot be blocked — the same
//! race-free timing invariant `opfs_cleanup` relies on.
//!
//! Uses `js_sys` reflection rather than web-sys `Idb*` types so the app tier
//! doesn't pull in IndexedDB web-sys features it otherwise never touches (the
//! durable store itself lives in the `entity-store` crate).

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Drain the IDB tombstone list, deleting each deleted `frontend-idb` peer's
/// `entity-peer-{id}` database. Cheap no-op when the list is empty.
pub async fn run_at_boot() {
    let tombstones = crate::persistence::load_idb_tombstones();
    if tombstones.is_empty() {
        return;
    }
    tracing::info!(count = tombstones.len(), "IDB cleanup: draining tombstones");

    let idb = match indexed_db() {
        Some(i) => i,
        None => {
            // No IndexedDB in this environment → nothing to delete; clear the
            // list so we don't retry forever.
            tracing::info!("IDB cleanup: no indexedDB; clearing tombstones");
            crate::persistence::set_idb_tombstones(&[]);
            return;
        }
    };

    let mut surviving: Vec<String> = Vec::new();
    for peer_id in tombstones {
        let db_name = format!("entity-peer-{peer_id}");
        match delete_database(&idb, &db_name).await {
            Ok(()) => {
                tracing::info!(peer_id = %peer_id, "IDB cleanup: deleted {db_name}");
            }
            Err(e) => {
                tracing::warn!(
                    peer_id = %peer_id,
                    error = %e,
                    "IDB cleanup: deleteDatabase failed; will retry next boot"
                );
                surviving.push(peer_id);
            }
        }
    }
    crate::persistence::set_idb_tombstones(&surviving);
}

/// `self.indexedDB` from the global scope (works on the main thread and in a
/// worker). Reflection so we avoid web-sys `Idb*` features.
fn indexed_db() -> Option<js_sys::Object> {
    let global = js_sys::global();
    let idb = js_sys::Reflect::get(&global, &"indexedDB".into()).ok()?;
    if idb.is_undefined() || idb.is_null() {
        return None;
    }
    idb.dyn_into().ok()
}

/// `indexedDB.deleteDatabase(name)`, awaited: the returned `IDBOpenDBRequest`'s
/// `onsuccess`/`onerror` are bridged into a `Promise` we `await`. The closures
/// are held in local slots across the `await` (NOT `forget()`ed — they free on
/// scope exit) so they survive until the event fires. The rejecting-promise
/// path is consumed by the `await`, so it can never become an
/// `unhandledrejection` → `location.reload()`.
async fn delete_database(idb: &js_sys::Object, name: &str) -> Result<(), String> {
    let delete_fn = js_sys::Reflect::get(idb, &"deleteDatabase".into())
        .map_err(|e| format!("{e:?}"))?;
    let delete_fn: js_sys::Function = delete_fn
        .dyn_into()
        .map_err(|_| "indexedDB.deleteDatabase is not a function".to_string())?;
    let req = delete_fn
        .call1(idb, &JsValue::from_str(name))
        .map_err(|e| format!("{e:?}"))?;
    let req: js_sys::Object = req
        .dyn_into()
        .map_err(|_| "deleteDatabase did not return a request".to_string())?;

    // Closures must outlive the executor (they fire asynchronously); keep them
    // in slots owned by this async frame until after the await.
    let mut on_success: Option<Closure<dyn FnMut(JsValue)>> = None;
    let mut on_error: Option<Closure<dyn FnMut(JsValue)>> = None;
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let os = Closure::wrap(Box::new(move |_e: JsValue| {
            let _ = resolve.call0(&JsValue::NULL);
        }) as Box<dyn FnMut(JsValue)>);
        let oe = Closure::wrap(Box::new(move |_e: JsValue| {
            let _ = reject.call0(&JsValue::NULL);
        }) as Box<dyn FnMut(JsValue)>);
        let _ = js_sys::Reflect::set(&req, &"onsuccess".into(), os.as_ref().unchecked_ref());
        let _ = js_sys::Reflect::set(&req, &"onerror".into(), oe.as_ref().unchecked_ref());
        // `onblocked` should never fire (no connection is held at boot), but
        // treat it as an error so a surprise doesn't hang the drain forever.
        let _ = js_sys::Reflect::set(&req, &"onblocked".into(), oe.as_ref().unchecked_ref());
        on_success = Some(os);
        on_error = Some(oe);
    });

    let result = JsFuture::from(promise).await;
    // Keep the request + closures alive until the event has fired.
    drop(on_success);
    drop(on_error);
    drop(req);
    result.map(|_| ()).map_err(|e| format!("{e:?}"))
}
