//! Tree-backed event log writer.
//!
//! The app's event log lives in the system peer's tree under
//! `/{system_peer}/app/entity-browser/event-log/{seq}`. Each entry is an
//! `app/entity-browser/event` entity with a single `message` field.
//!
//! Windows that show event scrollback subscribe to the prefix via
//! `ctx.store().subscribe(...)` and read entries with `ctx.store().list(...)`.
//!
//! The writer holds a clonable [`WriterHandle`] so it can be moved into
//! spawned async tasks (connect/execute/etc.) that produce log lines
//! from background work. The handle owns whichever arm's transport is
//! active, so this module doesn't branch on Direct/Worker.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use entity_ecf::{cbor_map, text, to_ecf};
use entity_entity::Entity;
use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

use crate::app_paths;

/// Cap on the number of retained log entries. Beyond this, the oldest is
/// trimmed on each new write.
const LOG_CAP: u64 = 1000;

/// Entity type name for individual event log entries.
pub const EVENT_TYPE: &str = "app/entity-browser/event";

/// Clonable writer for app event log entries. Cheap to clone — just
/// the seq counter and the writer handle. Move clones into spawned
/// futures and call [`log`](Self::log) from there.
#[derive(Clone)]
pub struct EventLogWriter {
    system_peer_id: String,
    handle: Option<WriterHandle>,
    seq: Arc<AtomicU64>,
}

impl EventLogWriter {
    /// Build a writer bound to the manager's primary (system) peer.
    /// Works in both arms — the underlying `WriterHandle` dispatches
    /// to whichever arm's transport is wired.
    pub fn new(peers: &Peers) -> Self {
        Self {
            system_peer_id: peers.system_peer_id().to_string(),
            handle: peers.writer_handle(),
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Append one event entry. Trims the oldest entry beyond [`LOG_CAP`].
    pub fn log(&self, message: impl Into<String>) {
        let Some(handle) = &self.handle else {
            tracing::trace!("event_log: no writer handle (no arm wired)");
            return;
        };
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let path = app_paths::event_log_entry_path(app_paths::APP_ID, &self.system_peer_id, n);
        handle.put(path, make_event_entity(&message.into()));

        if n >= LOG_CAP {
            let trim = n - LOG_CAP;
            let trim_path = app_paths::event_log_entry_path(app_paths::APP_ID, &self.system_peer_id, trim);
            handle.remove(trim_path);
        }
    }

    /// Drop every entry currently in the log. Direct-only today —
    /// worker-mode clear needs a prefix-list-then-remove and the
    /// `WriterHandle` deliberately doesn't expose reads. Wire if a
    /// consumer needs it.
    pub fn clear(&self) {
        let Some(handle) = &self.handle else { return };
        match handle {
            WriterHandle::Direct(shared) => {
                let prefix = app_paths::event_log_prefix(app_paths::APP_ID, &self.system_peer_id);
                let entries = shared.tree.list(&prefix);
                for entry in entries {
                    shared.tree.remove(&entry.path);
                }
            }
            #[cfg(target_arch = "wasm32")]
            WriterHandle::Worker { .. } => {
                tracing::trace!("event_log: clear unimplemented for worker mode");
            }
        }
    }
}

fn make_event_entity(message: &str) -> Entity {
    let data = to_ecf(&cbor_map! {
        "message" => text(message)
    });
    Entity::new(EVENT_TYPE, data).expect("event entity construction is infallible")
}

/// Decode an event entity's `message` field. Returns an empty string on
/// malformed input rather than failing — log entries are best-effort.
pub fn decode_event_message(entity: &Entity) -> String {
    let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    let map = match value.as_map() {
        Some(m) => m,
        None => return String::new(),
    };
    for (k, v) in map {
        if let Some(key) = k.as_text() {
            if key == "message" {
                if let Some(s) = v.as_text() {
                    return s.to_string();
                }
            }
        }
    }
    String::new()
}

// `read_events` (full tree-listing read per call) was removed —
// superseded by the Stage-C per-event subscription in
// `event_log_cache`. Its tests went with it; EventLogWriter's
// log/clear/trim behaviour is now covered end-to-end by the
// event-log round-trip phases in `tests/e2e_worker.rs`.
