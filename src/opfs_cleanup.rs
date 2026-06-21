//! Boot-time OPFS subdirectory cleanup for Stage 2C
//! Backend(OPFS)-peer deletions.
//!
//! When a user deletes a Backend(OPFS) peer at runtime, the peer's
//! dedicated worker is still alive and holding sync access handles on
//! `entities.log` / `locations.log` inside its `workers/{peer_id}/`
//! subdirectory. `FileSystemDirectoryHandle.removeEntry(recursive: true)`
//! fails while those handles are held — and the upstream
//! `entity-wasm-worker-proxy` doesn't expose `worker.terminate()`, so
//! we can't release them on demand.
//!
//! Solution: `persistence::mark_opfs_for_cleanup(peer_id)` writes the
//! peer-id into a `entity_opfs_tombstones` localStorage list. At the
//! next page boot, before *any* worker spawns, [`run_at_boot`] drains
//! that list and removes each subdir. Boot is the only point where no
//! sync access handles are held, so the cleanup is race-free.
//!
//! Failures (e.g. transient OPFS errors) leave the failed peer-id in
//! the tombstone list for retry on a subsequent boot.

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// Drain the OPFS tombstone list and remove each `workers/{peer_id}/`
/// subdirectory in OPFS. Must be called BEFORE any worker spawn — once
/// a worker takes sync access handles inside `workers/{primary_peer_id}`,
/// removing sibling subdirs is still safe but we want the simplest
/// timing invariant.
///
/// Returns immediately when there's nothing to clean up — cheap to
/// call on every boot.
pub async fn run_at_boot() {
    let tombstones = crate::persistence::load_opfs_tombstones();
    if tombstones.is_empty() {
        return;
    }
    tracing::info!(count = tombstones.len(), "OPFS cleanup: draining tombstones");

    let workers_dir = match open_workers_dir().await {
        Some(d) => d,
        None => {
            // No OPFS or no `workers/` subdir → nothing on disk to
            // clean. Clear the tombstones so we don't retry forever.
            tracing::info!("OPFS cleanup: no workers/ dir; clearing tombstones");
            crate::persistence::set_opfs_tombstones(&[]);
            return;
        }
    };

    let opts = web_sys::FileSystemRemoveOptions::new();
    opts.set_recursive(true);

    let mut surviving: Vec<String> = Vec::new();
    for peer_id in tombstones {
        let promise = workers_dir.remove_entry_with_options(&peer_id, &opts);
        match JsFuture::from(promise).await {
            Ok(_) => {
                tracing::info!(peer_id = %peer_id, "OPFS cleanup: removed workers/{peer_id}/");
            }
            Err(e) => {
                // NotFoundError is fine — the dir was already gone (e.g.
                // the peer never reached the point of creating OPFS
                // state). Drop the tombstone in that case so it doesn't
                // accumulate.
                let msg = format!("{e:?}");
                if msg.contains("NotFoundError") {
                    tracing::info!(peer_id = %peer_id, "OPFS cleanup: subdir already gone");
                } else {
                    tracing::warn!(peer_id = %peer_id, error = %msg, "OPFS cleanup: removeEntry failed; will retry next boot");
                    surviving.push(peer_id);
                }
            }
        }
    }
    crate::persistence::set_opfs_tombstones(&surviving);
}

/// Open OPFS root → `workers/` subdirectory. Returns `None` if OPFS is
/// unavailable in this environment or the `workers/` dir doesn't exist
/// yet (no Backend(OPFS) peer has ever been created on this origin).
async fn open_workers_dir() -> Option<web_sys::FileSystemDirectoryHandle> {
    let global = js_sys::global();
    let navigator = js_sys::Reflect::get(&global, &"navigator".into()).ok()?;
    let storage = js_sys::Reflect::get(&navigator, &"storage".into()).ok()?;
    let get_dir_fn = js_sys::Reflect::get(&storage, &"getDirectory".into()).ok()?;
    let get_dir_fn: js_sys::Function = get_dir_fn.dyn_into().ok()?;
    let dir_promise = get_dir_fn.call0(&storage).ok()?;
    let root_js = JsFuture::from(js_sys::Promise::from(dir_promise)).await.ok()?;
    let root: web_sys::FileSystemDirectoryHandle = root_js.dyn_into().ok()?;

    // `create: false` because we don't want to instantiate the dir
    // when there's nothing to clean. NotFoundError means no Backend(OPFS)
    // peer has ever been created.
    let opts = web_sys::FileSystemGetDirectoryOptions::new();
    opts.set_create(false);
    let workers_promise = root.get_directory_handle_with_options("workers", &opts);
    let workers_js = JsFuture::from(workers_promise).await.ok()?;
    workers_js.dyn_into().ok()
}
