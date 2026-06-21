//! Event Log model — reader over the shared `event_log_cache`.
//!
//! Stage C: the model holds an `EventLogCache` that
//! subscribes once to the event-log prefix via `observe_with_events`.
//! Render reads from the cache's in-memory `Vec<String>` — zero
//! `tree_listing` / `get_entity` calls per render once the
//! subscription seed completes.

use crate::event_log_cache::EventLogCache;
use crate::peers::Peers;
use crate::views::{EventCategory, EventEntry};

use super::output::EventLogOutput;

#[derive(Debug, Default)]
pub struct EventLogModel {
    cache: EventLogCache,
}

impl EventLogModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow the underlying cache. The window factory installs the
    /// subscription on this cache's `inner_arc`.
    pub fn cache(&self) -> &EventLogCache {
        &self.cache
    }

    /// Read the event log into a pure output. The renderer
    /// touches no peers state.
    #[allow(dead_code)] // called from WASM render path
    pub fn render_output(&self, peers: &Peers) -> EventLogOutput {
        let events = self
            .cache
            .messages(peers)
            .into_iter()
            .map(|message| EventEntry {
                category: EventCategory::classify(&message),
                message,
            })
            .collect();
        EventLogOutput { events }
    }
}
