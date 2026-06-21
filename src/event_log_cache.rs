//! Shared event-log local-mirror cache.
//!
//! Three windows display app event-log scrollback today: Event Log,
//! Query Console, and Execute Console. Pre-Stage-C they each did a
//! full `tree_listing` of the event-log prefix plus a `get_entity`
//! per entry, per render (via the old `event_log_writer::read_events`,
//! since removed). Linear in log size, paid per render.
//!
//! This module provides a per-window-instance local mirror seeded
//! via `observe_with_events` on the event-log prefix. After the seed
//! completes, render reads from `Vec<String>` in memory; zero
//! `tree_listing` / `get_entity` calls per render.
//!
//! Each window holds its own `EventLogCache` instance and installs
//! its own subscription. Multiple instances mean each window pays
//! its own seed cost + memory (~50 bytes × LOG_CAP=1000 = 50KB per
//! window); the alternative (singleton with shared subscription +
//! per-window dirty-flag fanout) is more code for negligible
//! benefit at our scale.
//!
//! Each window's controller calls [`EventLogCache::apply_change`]
//! from the subscription callback. [`EventLogCache::messages`] reads
//! the decoded list, lazily refetching any missing decoded entries
//! via `get_entity`.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use crate::event_log_writer::decode_event_message;
use crate::peers::{ChangeOp, Peers};

/// Local mirror of the event log. Cheap to clone (Arc).
#[derive(Clone, Default, Debug)]
pub struct EventLogCache {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default, Debug)]
pub struct Inner {
    /// All known event log paths. BTreeSet so iteration is sorted —
    /// paths are zero-padded `event_log_entry_path` strings, so
    /// lexical order = chronological order.
    known_paths: BTreeSet<String>,
    /// Decoded message per path. Missing entries are lazily filled
    /// in `messages` via `get_entity`.
    decoded: HashMap<String, String>,
    /// Worker overflow recovery flag.
    needs_resync: bool,
}

impl EventLogCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow the inner state behind the `Arc<Mutex>`. Window
    /// factories pass this to the `observe_with_events` callback.
    pub fn inner_arc(&self) -> Arc<Mutex<Inner>> {
        self.inner.clone()
    }

    /// Read all messages in chronological order. Resyncs and refills
    /// lazily if needed. After the subscription seed completes,
    /// steady-state calls are O(known) memory walk + 0 peer reads.
    pub fn messages(&self, peers: &Peers) -> Vec<String> {
        let needs_resync = self.inner.lock().unwrap().needs_resync;
        if needs_resync {
            self.resync(peers);
        }
        self.refill(peers);
        let inner = self.inner.lock().unwrap();
        inner
            .known_paths
            .iter()
            .filter_map(|p| inner.decoded.get(p).cloned())
            .collect()
    }

    /// Resync path: wipe local mirror and rebuild from `tree_listing`.
    /// Called by `messages` when `ChangeOp::Resync` fired (Worker arm
    /// overflow recovery).
    fn resync(&self, peers: &Peers) {
        let pid = peers.system_peer_id();
        let prefix = crate::app_paths::event_log_prefix(crate::app_paths::APP_ID, pid);
        let entries = peers.tree_listing(pid, &prefix);
        let mut inner = self.inner.lock().unwrap();
        inner.known_paths.clear();
        inner.decoded.clear();
        for entry in entries {
            inner.known_paths.insert(entry.path);
        }
        inner.needs_resync = false;
    }

    /// Refill any decoded entries that are missing. Costs O(missing)
    /// `get_entity` calls — typically 0 after seed completes.
    fn refill(&self, peers: &Peers) {
        let to_fill: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            inner
                .known_paths
                .iter()
                .filter(|p| !inner.decoded.contains_key(p.as_str()))
                .cloned()
                .collect()
        };
        if to_fill.is_empty() {
            return;
        }
        let pid = peers.system_peer_id().to_string();
        for path in to_fill {
            if let Some(entity) = peers.get_entity(&pid, &path) {
                let msg = decode_event_message(&entity);
                self.inner.lock().unwrap().decoded.insert(path, msg);
            }
        }
    }
}

/// Apply one [`ChangeOp`] to the cache. Called from the per-event
/// subscription callback set up by the window factory.
///
/// Put: register the path, invalidate any decoded entry so render
/// refetches. Remove: drop both. Resync: flag for full rebuild.
pub fn apply_change(inner: &Mutex<Inner>, op: ChangeOp) {
    let mut guard = inner.lock().unwrap();
    match op {
        ChangeOp::Put { path } => {
            guard.known_paths.insert(path.clone());
            guard.decoded.remove(&path);
        }
        ChangeOp::Remove { path } => {
            guard.known_paths.remove(&path);
            guard.decoded.remove(&path);
        }
        ChangeOp::Resync => {
            guard.needs_resync = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_log_writer::EventLogWriter;

    #[tokio::test]
    async fn cache_populates_via_subscription() {
        let pm = Peers::new_direct();
        let writer = EventLogWriter::new(&pm);
        writer.log("first");
        writer.log("second");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let cache = EventLogCache::new();
        let inner = cache.inner_arc();
        let mut watch = crate::window_watch::WindowWatch::new();
        let pid = pm.primary_peer_id().to_string();
        let prefix = crate::app_paths::event_log_prefix(crate::app_paths::APP_ID, &pid);
        pm.observe_with_events(&mut watch, &pid, prefix, move |op| {
            apply_change(&inner, op)
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        let msgs = cache.messages(&pm);
        assert_eq!(msgs, vec!["first".to_string(), "second".to_string()]);
        // Keep watch alive until end of test.
        drop(watch);
    }

    #[tokio::test]
    async fn cache_observes_subsequent_appends() {
        let pm = Peers::new_direct();
        let cache = EventLogCache::new();
        let inner = cache.inner_arc();
        let mut watch = crate::window_watch::WindowWatch::new();
        let pid = pm.primary_peer_id().to_string();
        let prefix = crate::app_paths::event_log_prefix(crate::app_paths::APP_ID, &pid);
        pm.observe_with_events(&mut watch, &pid, prefix, move |op| {
            apply_change(&inner, op)
        });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(cache.messages(&pm).is_empty());

        let writer = EventLogWriter::new(&pm);
        writer.log("post-seed-1");
        writer.log("post-seed-2");
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        let msgs = cache.messages(&pm);
        assert_eq!(msgs, vec!["post-seed-1".to_string(), "post-seed-2".to_string()]);
        drop(watch);
    }
}
