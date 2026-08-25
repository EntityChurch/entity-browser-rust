//! File Transfer model.
//!
//! Slice 0 holds only ephemeral session state (selected target + filename)
//! in an `Arc<Mutex<_>>` — no tree persistence yet. What's worth persisting
//! (transfer history, remembered shares, grants) is a later-phase decision
//! (DESIGN-CROSS-DEVICE-FILE-TRANSFER §0). Results surface via the shared
//! event-log cache, populated by an async subscription the factory installs.

use std::sync::{Arc, Mutex};

use crate::peers::Peers;
use crate::views::{EventCategory, EventEntry};
use crate::window::WindowId;

use super::output::{FileTransferOutput, TargetOption};

/// Tree prefix the backend peer exposes its share at. Mirrors the backend's
/// `SHARE_PREFIX` (`src-tauri/src/lib.rs`); kept in one const here on the
/// app side. Always ends with `/`.
pub const SHARE_PREFIX: &str = "local/files/shared/";

/// Default file to pull — the backend seeds this so a fresh pairing always
/// has something to transfer.
pub const DEFAULT_FILENAME: &str = "welcome.txt";

#[derive(Debug, Clone, Default)]
struct FileTransferState {
    /// Selected target peer id, or empty for "auto / first connected".
    selected_target: String,
    filename: String,
}

#[derive(Debug)]
pub struct FileTransferModel {
    /// Retained for the later tree-persisted window state (§0); unused
    /// while Slice 0 keeps state ephemeral.
    #[allow(dead_code)]
    window_id: WindowId,
    peer_id: String,
    inner: Arc<Mutex<FileTransferState>>,
    event_log: crate::event_log_cache::EventLogCache,
}

impl FileTransferModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(FileTransferState {
                selected_target: String::new(),
                filename: DEFAULT_FILENAME.to_string(),
            })),
            event_log: crate::event_log_cache::EventLogCache::new(),
        }
    }

    /// Borrow the event-log cache so the factory can install the per-event
    /// subscription that feeds the results pane.
    pub fn event_log_cache(&self) -> &crate::event_log_cache::EventLogCache {
        &self.event_log
    }

    // -- Action methods --

    pub fn select_target(&self, value: &str) {
        self.inner.lock().unwrap().selected_target = value.to_string();
    }

    pub fn set_filename(&self, value: &str) {
        self.inner.lock().unwrap().filename = value.to_string();
    }

    // -- Pure read API --

    #[allow(dead_code)] // called from the WASM render path
    pub fn render_output(&self, peers: &Peers) -> FileTransferOutput {
        let state = self.inner.lock().unwrap().clone();

        let connected: Vec<String> = crate::connections::read_connected(peers);
        // Resolve the effective target: an explicit selection if it's still
        // connected, else the first connected peer.
        let effective_target = if !state.selected_target.is_empty()
            && connected.iter().any(|p| *p == state.selected_target)
        {
            state.selected_target.clone()
        } else {
            connected.first().cloned().unwrap_or_default()
        };

        let target_options: Vec<TargetOption> = connected
            .iter()
            .map(|rpid| TargetOption {
                value: rpid.clone(),
                label: format!("Remote: {}", crate::views::display_name(peers, rpid)),
                selected: *rpid == effective_target,
            })
            .collect();

        let events: Vec<EventEntry> = self
            .event_log
            .messages(peers)
            .into_iter()
            .map(|m| EventEntry {
                category: EventCategory::classify(&m),
                message: m,
            })
            .collect();

        FileTransferOutput {
            peer_id: self.peer_id.clone(),
            has_target: !target_options.is_empty(),
            selected_target: effective_target,
            target_options,
            share_prefix: SHARE_PREFIX.to_string(),
            filename_initial: state.filename.clone(),
            events,
        }
    }
}
