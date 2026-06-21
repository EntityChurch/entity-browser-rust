//! Query Console model — mirrored shape.
//!
//! Per-window state (filter fields) is mirrored in `Arc<Mutex<_>>`
//! and persisted to the tree. The renderer pulls the current state
//! plus a snapshot of the shared event log on each render.

use std::sync::{Arc, Mutex};

use entity_entity::Entity;
use crate::peers::Peers;

use crate::views::{EventCategory, EventEntry};
use crate::window::WindowId;

use super::output::{QueryConsoleOutput, QueryFields};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryState {
    pub type_filter: String,
    pub path_prefix: String,
    pub ref_filter: String,
    pub path_filter: String,
    pub limit: String,
    pub include_entities: bool,
}

impl Default for QueryState {
    fn default() -> Self {
        Self {
            type_filter: "*".into(),
            path_prefix: String::new(),
            ref_filter: String::new(),
            path_filter: String::new(),
            limit: "100".into(),
            include_entities: false,
        }
    }
}

impl QueryState {
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
                Some("type_filter") => {
                    if let Some(s) = v.as_text() {
                        state.type_filter = s.to_string();
                    }
                }
                Some("path_prefix") => {
                    if let Some(s) = v.as_text() {
                        state.path_prefix = s.to_string();
                    }
                }
                Some("ref_filter") => {
                    if let Some(s) = v.as_text() {
                        state.ref_filter = s.to_string();
                    }
                }
                Some("path_filter") => {
                    if let Some(s) = v.as_text() {
                        state.path_filter = s.to_string();
                    }
                }
                Some("limit") => {
                    if let Some(s) = v.as_text() {
                        state.limit = s.to_string();
                    }
                }
                Some("include_entities") => {
                    if let Some(b) = v.as_bool() {
                        state.include_entities = b;
                    }
                }
                _ => {}
            }
        }
        state
    }

    pub fn to_entity(&self) -> Entity {
        let mut pairs = vec![
            (entity_ecf::Value::Text("type_filter".into()), entity_ecf::text(&self.type_filter)),
            (entity_ecf::Value::Text("path_prefix".into()), entity_ecf::text(&self.path_prefix)),
            (entity_ecf::Value::Text("ref_filter".into()), entity_ecf::text(&self.ref_filter)),
            (entity_ecf::Value::Text("path_filter".into()), entity_ecf::text(&self.path_filter)),
            (entity_ecf::Value::Text("limit".into()), entity_ecf::text(&self.limit)),
            (
                entity_ecf::Value::Text("include_entities".into()),
                entity_ecf::bool_val(self.include_entities),
            ),
        ];
        // Drop empty string fields to keep entity clean.
        pairs.retain(|(_, v)| match v {
            entity_ecf::Value::Text(s) => !s.is_empty(),
            _ => true,
        });
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        Entity::new("app/state/query_console", data).unwrap()
    }
}

/// Build a `system/query/expression` entity from raw field values.
/// Pure — used both at execute-click time (with live DOM values) and
/// in tests.
#[allow(dead_code)]
pub fn build_expression_from_fields(
    type_filter: &str,
    path_prefix: &str,
    ref_filter: &str,
    path_filter: &str,
    limit: &str,
    include_entities: bool,
) -> Entity {
    let mut pairs: Vec<(entity_ecf::Value, entity_ecf::Value)> = Vec::new();

    if !type_filter.is_empty() {
        pairs.push((
            entity_ecf::Value::Text("type_filter".into()),
            entity_ecf::text(type_filter),
        ));
    }
    if !path_prefix.is_empty() {
        pairs.push((
            entity_ecf::Value::Text("path_prefix".into()),
            entity_ecf::text(path_prefix),
        ));
    }
    if !ref_filter.is_empty() {
        if let Some(bytes) = hex_to_bytes(ref_filter) {
            pairs.push((
                entity_ecf::Value::Text("ref_filter".into()),
                entity_ecf::Value::Bytes(bytes),
            ));
        }
    }
    if !path_filter.is_empty() {
        pairs.push((
            entity_ecf::Value::Text("path_filter".into()),
            entity_ecf::text(path_filter),
        ));
    }
    if let Ok(n) = limit.parse::<i64>() {
        if n > 0 {
            pairs.push((
                entity_ecf::Value::Text("limit".into()),
                entity_ecf::integer(n),
            ));
        }
    }
    if include_entities {
        pairs.push((
            entity_ecf::Value::Text("include_entities".into()),
            entity_ecf::bool_val(true),
        ));
    }

    let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
    Entity::new("system/query/expression", data).unwrap()
}

#[allow(dead_code)]
pub fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    let hex = hex.trim();
    if hex.is_empty() || !hex.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16).ok()?;
        bytes.push(byte);
    }
    Some(bytes)
}

#[derive(Debug)]
pub struct QueryConsoleModel {
    window_id: WindowId,
    peer_id: String,
    inner: Arc<Mutex<QueryState>>,
    event_log: crate::event_log_cache::EventLogCache,
}

impl QueryConsoleModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(QueryState::default())),
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

    fn state_path(&self, _peers: &Peers) -> String {
        crate::app_paths::window_state_path(crate::app_paths::APP_ID, &self.peer_id, self.window_id)
    }

    fn ensure_state_in_tree(&self, peers: &Peers) {
        let path = self.state_path(peers);
        if peers.get_entity(&self.peer_id, &path).is_none() {
            peers.dispatch_write(&self.peer_id, path, QueryState::default().to_entity());
        }
    }

    fn read_window_state(&self, peers: &Peers) -> QueryState {
        let path = self.state_path(peers);
        peers
            .get_entity(&self.peer_id, &path)
            .map(|e| QueryState::from_entity(&e))
            .unwrap_or_default()
    }

    fn persist_state(&self, peers: &Peers) {
        let entity = self.inner.lock().unwrap().to_entity();
        let path = self.state_path(peers);
        peers.dispatch_write(&self.peer_id, path, entity);
    }

    // -- Action methods --

    pub fn set_type_filter(&self, value: &str) {
        self.inner.lock().unwrap().type_filter = value.to_string();
    }

    pub fn set_path_prefix(&self, value: &str) {
        self.inner.lock().unwrap().path_prefix = value.to_string();
    }

    pub fn set_ref_filter(&self, value: &str) {
        self.inner.lock().unwrap().ref_filter = value.to_string();
    }

    pub fn set_path_filter(&self, value: &str) {
        self.inner.lock().unwrap().path_filter = value.to_string();
    }

    pub fn set_limit(&self, value: &str) {
        self.inner.lock().unwrap().limit = value.to_string();
    }

    pub fn toggle_include_entities(&self) {
        let mut s = self.inner.lock().unwrap();
        s.include_entities = !s.include_entities;
    }

    pub fn save_state(&self, peers: &Peers) {
        self.persist_state(peers);
    }

    // -- Pure read API --

    #[allow(dead_code)] // called from WASM render path
    pub fn render_output(&self, peers: &Peers) -> QueryConsoleOutput {
        let state = self.inner.lock().unwrap().clone();

        let events: Vec<EventEntry> = self
            .event_log
            .messages(peers)
            .into_iter()
            .map(|m| EventEntry {
                category: EventCategory::classify(&m),
                message: m,
            })
            .collect();

        QueryConsoleOutput {
            window_id: self.window_id,
            peer_id: self.peer_id.clone(),
            fields: QueryFields {
                type_filter: state.type_filter,
                path_prefix: state.path_prefix,
                ref_filter: state.ref_filter,
                path_filter: state.path_filter,
                limit: state.limit,
                include_entities: state.include_entities,
            },
            events,
        }
    }

    #[cfg(test)]
    pub fn state_snapshot(&self) -> QueryState {
        self.inner.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm() -> Peers {
        Peers::new_direct()
    }

    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[test]
    fn state_round_trip() {
        let state = QueryState {
            type_filter: "app/user".into(),
            path_prefix: "users/".into(),
            include_entities: true,
            ..QueryState::default()
        };
        let entity = state.to_entity();
        let decoded = QueryState::from_entity(&entity);
        assert_eq!(decoded.type_filter, "app/user");
        assert_eq!(decoded.path_prefix, "users/");
        assert!(decoded.include_entities);
    }

    #[tokio::test]
    async fn set_type_filter_persists() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut model = QueryConsoleModel::new(1, pid.clone());
        model.initialize(&pm);
        model.set_type_filter("app/user");
        model.save_state(&pm);
        flush_writes().await;

        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 1);
        let entity = pm.get_entity(&pid, &path).unwrap();
        assert_eq!(QueryState::from_entity(&entity).type_filter, "app/user");
    }

    #[test]
    fn build_expression_with_filters() {
        let expr = build_expression_from_fields("app/user", "users/", "", "", "50", true);
        assert_eq!(expr.entity_type, "system/query/expression");
        let val: ciborium::Value = ciborium::from_reader(expr.data.as_slice()).unwrap();
        let map = val.as_map().unwrap();
        let tf = map
            .iter()
            .find(|(k, _)| k.as_text() == Some("type_filter"))
            .unwrap();
        assert_eq!(tf.1.as_text(), Some("app/user"));
    }

    #[test]
    fn build_expression_with_ref_filter() {
        let expr = build_expression_from_fields("", "", "00aabb", "", "100", false);
        let val: ciborium::Value = ciborium::from_reader(expr.data.as_slice()).unwrap();
        let map = val.as_map().unwrap();
        let rf = map
            .iter()
            .find(|(k, _)| k.as_text() == Some("ref_filter"))
            .unwrap();
        let expected = vec![0x00u8, 0xaa, 0xbb];
        assert_eq!(rf.1.as_bytes(), Some(&expected));
    }

    #[test]
    fn hex_to_bytes_valid() {
        assert_eq!(hex_to_bytes("00aabb"), Some(vec![0x00, 0xaa, 0xbb]));
    }

    #[test]
    fn hex_to_bytes_invalid() {
        assert_eq!(hex_to_bytes(""), None);
        assert_eq!(hex_to_bytes("0"), None);
        assert_eq!(hex_to_bytes("zz"), None);
    }
}
