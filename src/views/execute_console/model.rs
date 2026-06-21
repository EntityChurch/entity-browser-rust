//! Execute Console model — mirrored shape.
//!
//! Per-window state (mode, selected peer, selected handler/operation,
//! resource, raw URI/operation) is mirrored in `Arc<Mutex<_>>` and
//! persisted to the tree. Render-time data (handler discovery,
//! connected peers, event log) comes from `Peers` on demand.

use std::sync::{Arc, Mutex};

use entity_entity::Entity;
use crate::peers::Peers;

use crate::views::{EventCategory, EventEntry};
use crate::window::WindowId;

use super::output::{
    ExecuteConsoleOutput, ExecuteMode, GuidedView, HandlerOption, OperationOption, PeerOption,
    RawView, ResolvedExecute,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecuteState {
    pub mode: String, // "guided" or "raw"
    pub selected_peer: String,
    pub selected_handler: usize,
    pub selected_operation: usize,
    pub resource: String,
    pub raw_handler_uri: String,
    pub raw_operation: String,
}

impl Default for ExecuteState {
    fn default() -> Self {
        Self {
            mode: "guided".into(),
            selected_peer: "local".into(),
            selected_handler: 0,
            selected_operation: 0,
            resource: String::new(),
            raw_handler_uri: "system/tree".into(),
            raw_operation: "get".into(),
        }
    }
}

impl ExecuteState {
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };
        let mut state = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("mode") => {
                    if let Some(s) = v.as_text() {
                        state.mode = s.to_string();
                    }
                }
                Some("selected_peer") => {
                    if let Some(s) = v.as_text() {
                        state.selected_peer = s.to_string();
                    }
                }
                Some("selected_handler") => {
                    if let Some(i) = v.as_integer() {
                        let n: i128 = i.into();
                        state.selected_handler = n as usize;
                    }
                }
                Some("selected_operation") => {
                    if let Some(i) = v.as_integer() {
                        let n: i128 = i.into();
                        state.selected_operation = n as usize;
                    }
                }
                Some("resource") => {
                    if let Some(s) = v.as_text() {
                        state.resource = s.to_string();
                    }
                }
                Some("raw_handler_uri") => {
                    if let Some(s) = v.as_text() {
                        state.raw_handler_uri = s.to_string();
                    }
                }
                Some("raw_operation") => {
                    if let Some(s) = v.as_text() {
                        state.raw_operation = s.to_string();
                    }
                }
                _ => {}
            }
        }
        state
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "mode" => entity_ecf::text(&self.mode),
            "selected_peer" => entity_ecf::text(&self.selected_peer),
            "selected_handler" => entity_ecf::integer(self.selected_handler as i64),
            "selected_operation" => entity_ecf::integer(self.selected_operation as i64),
            "resource" => entity_ecf::text(&self.resource),
            "raw_handler_uri" => entity_ecf::text(&self.raw_handler_uri),
            "raw_operation" => entity_ecf::text(&self.raw_operation)
        });
        Entity::new("app/state/execute_console", data).unwrap()
    }

    pub fn is_guided(&self) -> bool {
        self.mode != "raw"
    }

    /// The peer the console currently targets. `"local"` resolves to
    /// the window's *bound* peer (`bound_peer`) — NOT the primary; a
    /// console can be palette-bound to a non-primary backend peer, and
    /// silently discovering/executing against the primary's tree
    /// instead is the §4.3 defect class. A remote selection is its
    /// own peer-id (resolved via the bound peer's connection pool).
    fn active_peer_id<'a>(&'a self, bound_peer: &'a str) -> &'a str {
        if self.selected_peer == "local" {
            bound_peer
        } else {
            &self.selected_peer
        }
    }
}

#[derive(Debug)]
pub struct ExecuteConsoleModel {
    window_id: WindowId,
    peer_id: String,
    inner: Arc<Mutex<ExecuteState>>,
    /// Cached handler list keyed by which peer it was fetched against.
    /// Direct mode populates this synchronously via `refresh_handlers`;
    /// Worker mode populates it via an async dispatch through the
    /// proxy. Render reads from this cache regardless of arm so the
    /// codepath is uniform.
    handlers: Arc<Mutex<Vec<entity_sdk::HandlerInfo>>>,
    handlers_for_peer: Arc<Mutex<Option<String>>>,
    event_log: crate::event_log_cache::EventLogCache,
}

impl ExecuteConsoleModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(ExecuteState::default())),
            handlers: Arc::new(Mutex::new(Vec::new())),
            handlers_for_peer: Arc::new(Mutex::new(None)),
            event_log: crate::event_log_cache::EventLogCache::new(),
        }
    }

    /// Borrow the event-log cache so the factory can install the
    /// per-event subscription on it.
    pub fn event_log_cache(&self) -> &crate::event_log_cache::EventLogCache {
        &self.event_log
    }

    pub fn initialize(&mut self, peers: &Peers) {
        self.ensure_state_in_tree(peers);
        let state = self.read_window_state(peers);
        *self.inner.lock().unwrap() = state;
    }

    /// Refresh the cached handler list for the currently-active peer.
    /// Async — kicks off the dispatch and updates the cache when it
    /// completes. `dirty` is signaled on completion so the window
    /// re-renders with the fresh list. Idempotent: if a refresh for
    /// the same peer is already in flight, this still spawns another;
    /// the last write wins (which is fine for handler lists since
    /// they're slow-changing).
    pub fn refresh_handlers(&self, peers: &Peers, dirty: crate::window_watch::DirtyFlag) {
        let pid = self.inner.lock().unwrap().active_peer_id(&self.peer_id).to_string();
        let fut = peers.discover_handlers_async(&pid);
        let handlers_slot = self.handlers.clone();
        let for_peer_slot = self.handlers_for_peer.clone();
        let task = async move {
            match fut.await {
                Ok(list) => {
                    *handlers_slot.lock().unwrap() = list;
                    *for_peer_slot.lock().unwrap() = Some(pid);
                    dirty.mark();
                }
                Err(e) => {
                    tracing::warn!(error = %e, "execute_console: discover_handlers failed");
                }
            }
        };
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(task);
        #[cfg(not(target_arch = "wasm32"))]
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(task);
        }
    }

    fn state_path(&self, _peers: &Peers) -> String {
        crate::app_paths::window_state_path(crate::app_paths::APP_ID, &self.peer_id, self.window_id)
    }

    fn ensure_state_in_tree(&self, peers: &Peers) {
        let path = self.state_path(peers);
        if peers.get_entity(&self.peer_id, &path).is_none() {
            peers.dispatch_write(&self.peer_id, path, ExecuteState::default().to_entity());
        }
    }

    fn read_window_state(&self, peers: &Peers) -> ExecuteState {
        let path = self.state_path(peers);
        peers
            .get_entity(&self.peer_id, &path)
            .map(|e| ExecuteState::from_entity(&e))
            .unwrap_or_default()
    }

    fn persist_state(&self, peers: &Peers) {
        let entity = self.inner.lock().unwrap().to_entity();
        let path = self.state_path(peers);
        peers.dispatch_write(&self.peer_id, path, entity);
    }

    // -- Action methods --

    pub fn set_mode(&self, value: &str) {
        self.inner.lock().unwrap().mode = value.to_string();
    }

    pub fn select_peer(&self, value: &str) {
        let mut s = self.inner.lock().unwrap();
        s.selected_peer = value.to_string();
        s.selected_handler = 0;
        s.selected_operation = 0;
    }

    pub fn select_handler(&self, value: &str) {
        let mut s = self.inner.lock().unwrap();
        s.selected_handler = value.parse().unwrap_or(0);
        s.selected_operation = 0;
    }

    pub fn select_operation(&self, value: &str) {
        self.inner.lock().unwrap().selected_operation = value.parse().unwrap_or(0);
    }

    pub fn set_resource(&self, value: &str) {
        self.inner.lock().unwrap().resource = value.to_string();
    }

    pub fn set_raw_uri(&self, value: &str) {
        self.inner.lock().unwrap().raw_handler_uri = value.to_string();
    }

    pub fn set_raw_operation(&self, value: &str) {
        self.inner.lock().unwrap().raw_operation = value.to_string();
    }

    pub fn save_state(&self, peers: &Peers) {
        self.persist_state(peers);
    }

    // -- Pure read API --

    #[allow(dead_code)] // called from WASM render path
    pub fn render_output(&self, peers: &Peers) -> ExecuteConsoleOutput {
        let state = self.inner.lock().unwrap().clone();
        let mode = if state.is_guided() {
            ExecuteMode::Guided
        } else {
            ExecuteMode::Raw
        };

        // Peer selector options.
        let mut peer_options = vec![PeerOption {
            value: "local".into(),
            label: format!("Local ({})", crate::views::display_name(peers, &self.peer_id)),
            selected: state.selected_peer == "local",
        }];
        let connected: Vec<String> = crate::connections::read_connected(peers);
        for rpid in &connected {
            peer_options.push(PeerOption {
                value: rpid.clone(),
                label: format!("Remote: {}", crate::views::display_name(peers, rpid)),
                selected: state.selected_peer == *rpid,
            });
        }

        // Handler list comes from the model's cached state, populated
        // asynchronously via `refresh_handlers` (called on
        // initialize and on `select_peer`). Direct and Worker arms
        // both fill the same cache; render reads from it uniformly.
        let handlers = self.handlers.lock().unwrap().clone();
        let guided = if mode == ExecuteMode::Guided {
            let handler_options: Vec<HandlerOption> = handlers
                .iter()
                .enumerate()
                .map(|(i, h)| HandlerOption {
                    index: i,
                    label: format!("{} ({})", h.name, h.pattern),
                    selected: i == state.selected_handler,
                })
                .collect();
            let operations: Vec<OperationOption> = handlers
                .get(state.selected_handler)
                .map(|h| {
                    h.operations
                        .iter()
                        .enumerate()
                        .map(|(i, op)| OperationOption {
                            index: i,
                            name: op.clone(),
                            selected: i == state.selected_operation,
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(GuidedView {
                handlers: handler_options,
                operations,
            })
        } else {
            None
        };

        let raw = if mode == ExecuteMode::Raw {
            Some(RawView {
                handler_uri_initial: state.raw_handler_uri.clone(),
                operation_initial: state.raw_operation.clone(),
            })
        } else {
            None
        };

        // Resolve the execute target — used by the click handler when
        // the live-DOM raw fields can't be read.
        let resolved_uri = if state.is_guided() {
            handlers
                .get(state.selected_handler)
                .map(|h| h.pattern.clone())
                .unwrap_or_else(|| "system/tree".into())
        } else {
            state.raw_handler_uri.clone()
        };
        let resolved_uri = if state.selected_peer == "local" || resolved_uri.starts_with("entity://") {
            resolved_uri
        } else {
            format!("entity://{}/{}", state.selected_peer, resolved_uri)
        };
        let resolved_op = if state.is_guided() {
            handlers
                .get(state.selected_handler)
                .and_then(|h| h.operations.get(state.selected_operation))
                .cloned()
                .unwrap_or_else(|| "get".into())
        } else {
            state.raw_operation.clone()
        };

        // Event log snapshot — read from the local cache mirror.
        let events: Vec<EventEntry> = self
            .event_log
            .messages(peers)
            .into_iter()
            .map(|m| EventEntry {
                category: EventCategory::classify(&m),
                message: m,
            })
            .collect();

        ExecuteConsoleOutput {
            peer_id: self.peer_id.clone(),
            mode,
            peer_options,
            guided,
            raw,
            resource_initial: state.resource.clone(),
            resolved: ResolvedExecute {
                handler_uri: resolved_uri,
                operation: resolved_op,
            },
            events,
        }
    }

    #[cfg(test)]
    pub fn state_snapshot(&self) -> ExecuteState {
        self.inner.lock().unwrap().clone()
    }
}
