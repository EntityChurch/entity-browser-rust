//! Chain Trace model — peer-scoped viewer keyed by a user-entered
//! `chain_id`. Subscribes to the continuation + chain-error-marker
//! prefixes on the bound peer; renders the filtered trace.
//!
//! Per the inspectability feedback correspondence §3.1: this is the
//! highest-leverage L3 inspect surface and is buildable today because
//! chain state is path-bound substrate state (no L1 hook required).
//! When entity-core-rust surfaces the dispatch hook (A1) through the
//! SDK boundary, this window gains live dispatch event traces; for
//! now the substrate-read view is the v0 surface.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use entity_entity::Entity;

use crate::chain_trace_cache::ChainTraceCache;
use crate::peers::Peers;
use crate::render_policy::RenderPolicy;
use crate::window::WindowId;

use super::output::{ChainTraceOutput, TraceEntry};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChainTraceState {
    pub chain_id: String,
}

impl ChainTraceState {
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
            if let (Some("chain_id"), Some(s)) = (k.as_text(), v.as_text()) {
                state.chain_id = s.to_string();
            }
        }
        state
    }

    pub fn to_entity(&self) -> Entity {
        let mut pairs: Vec<(entity_ecf::Value, entity_ecf::Value)> = Vec::new();
        if !self.chain_id.is_empty() {
            pairs.push((
                entity_ecf::Value::Text("chain_id".into()),
                entity_ecf::text(&self.chain_id),
            ));
        }
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        Entity::new("app/state/chain_trace", data).unwrap()
    }
}

#[derive(Debug)]
pub struct ChainTraceModel {
    window_id: WindowId,
    peer_id: String,
    inner: Arc<Mutex<ChainTraceState>>,
    cache: ChainTraceCache,
}

impl ChainTraceModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(ChainTraceState::default())),
            cache: ChainTraceCache::new(),
        }
    }

    /// Borrow the cache so the window factory can install its two
    /// per-event subscriptions (continuation + chain-errors prefixes).
    pub fn cache(&self) -> &ChainTraceCache {
        &self.cache
    }

    pub fn initialize(&mut self, peers: &Peers) {
        self.ensure_state_in_tree(peers);
        *self.inner.lock().unwrap() = self.read_window_state(peers);
    }

    fn state_path(&self) -> String {
        crate::app_paths::window_state_path(crate::app_paths::APP_ID, &self.peer_id, self.window_id)
    }

    fn ensure_state_in_tree(&self, peers: &Peers) {
        let path = self.state_path();
        if peers.get_entity(&self.peer_id, &path).is_none() {
            peers.dispatch_write(&self.peer_id, path, ChainTraceState::default().to_entity());
        }
    }

    fn read_window_state(&self, peers: &Peers) -> ChainTraceState {
        peers
            .get_entity(&self.peer_id, &self.state_path())
            .map(|e| ChainTraceState::from_entity(&e))
            .unwrap_or_default()
    }

    fn persist_state(&self, peers: &Peers) {
        let entity = self.inner.lock().unwrap().to_entity();
        peers.dispatch_write(&self.peer_id, self.state_path(), entity);
    }

    pub fn set_chain_id(&self, value: &str) {
        self.inner.lock().unwrap().chain_id = value.trim().to_string();
    }

    pub fn save_state(&self, peers: &Peers) {
        self.persist_state(peers);
    }

    /// Pure read API — used by `render_dom`.
    pub fn render_output(&self, peers: &Peers) -> ChainTraceOutput {
        let state = self.inner.lock().unwrap().clone();

        if state.chain_id.is_empty() {
            return ChainTraceOutput {
                window_id: self.window_id,
                peer_id: self.peer_id.clone(),
                chain_id: String::new(),
                chain_known: false,
                continuations: Vec::new(),
                markers: Vec::new(),
            };
        }

        let paths = self.cache.snapshot_for(peers, &self.peer_id, &state.chain_id);
        let chain_known = !paths.continuations.is_empty() || !paths.markers.is_empty();

        let continuations = paths
            .continuations
            .iter()
            .map(|p| build_trace_entry(&self.cache, p, false))
            .collect();
        let markers = paths
            .markers
            .iter()
            .map(|p| build_trace_entry(&self.cache, p, true))
            .collect();

        ChainTraceOutput {
            window_id: self.window_id,
            peer_id: self.peer_id.clone(),
            chain_id: state.chain_id,
            chain_known,
            continuations,
            markers,
        }
    }

    #[cfg(test)]
    pub fn state_snapshot(&self) -> ChainTraceState {
        self.inner.lock().unwrap().clone()
    }
}

/// Build one [`TraceEntry`] from a path + decoded body (if available).
/// `is_marker` distinguishes the path-segment vocabulary so we know
/// whether to fill `kind_label` / `reason_label`.
fn build_trace_entry(cache: &ChainTraceCache, path: &str, is_marker: bool) -> TraceEntry {
    let entity = cache.decoded(path);
    let entity_type = entity
        .as_ref()
        .map(|e| e.entity_type.clone())
        .unwrap_or_default();
    let policy = RenderPolicy::for_entity_type(&entity_type);

    let body_available = entity.is_some();
    let body_display = entity.as_ref().and_then(|e| {
        if policy.permits_body_in_operator_mode() {
            Some(crate::format::format_entity_data(&e.data))
        } else {
            None
        }
    });

    let (kind_label, reason_label) = if is_marker {
        marker_labels(path)
    } else {
        (String::new(), String::new())
    };

    TraceEntry {
        path: path.to_string(),
        entity_type,
        body_available,
        body_display,
        kind_label,
        reason_label,
    }
}

/// Pull the `{lost|rejected}` and `{reason}` segments out of a marker
/// path for display. Returns `(kind, reason)` — empty strings if the
/// path doesn't match the marker shape.
fn marker_labels(path: &str) -> (String, String) {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    // /{peer}/system/runtime/chain-errors/{kind}/{chain_id}/{step}/{reason}/{hash}
    if segments.len() >= 8
        && segments[1] == "system"
        && segments[2] == "runtime"
        && segments[3] == "chain-errors"
    {
        (segments[4].to_string(), segments[7].to_string())
    } else {
        (String::new(), String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip_preserves_chain_id() {
        let state = ChainTraceState { chain_id: "CHAIN_ALPHA".into() };
        let entity = state.to_entity();
        let decoded = ChainTraceState::from_entity(&entity);
        assert_eq!(decoded.chain_id, "CHAIN_ALPHA");
    }

    #[test]
    fn state_empty_round_trip() {
        let state = ChainTraceState::default();
        let entity = state.to_entity();
        let decoded = ChainTraceState::from_entity(&entity);
        assert_eq!(decoded.chain_id, "");
    }

    #[test]
    fn marker_labels_extracts_lost_and_reason() {
        let (kind, reason) = marker_labels(
            "/PEER1/system/runtime/chain-errors/lost/CHAIN_A/0/timeout/0xabc",
        );
        assert_eq!(kind, "lost");
        assert_eq!(reason, "timeout");
    }

    #[test]
    fn marker_labels_extracts_rejected_and_cap_denied() {
        let (kind, reason) = marker_labels(
            "/PEER1/system/runtime/chain-errors/rejected/CHAIN_B/2/cap_denied/0xdef",
        );
        assert_eq!(kind, "rejected");
        assert_eq!(reason, "cap_denied");
    }

    #[test]
    fn marker_labels_empty_for_non_marker_path() {
        let (kind, reason) = marker_labels("/PEER1/system/continuation/CHAIN_A");
        assert_eq!(kind, "");
        assert_eq!(reason, "");
    }

    #[tokio::test]
    async fn set_chain_id_persists() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let mut model = ChainTraceModel::new(1, pid.clone());
        model.initialize(&peers);
        model.set_chain_id("CHAIN_ALPHA");
        model.save_state(&peers);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 1);
        let entity = peers.get_entity(&pid, &path).unwrap();
        assert_eq!(entity.entity_type, "app/state/chain_trace");
        assert_eq!(ChainTraceState::from_entity(&entity).chain_id, "CHAIN_ALPHA");
    }

    #[test]
    fn render_output_when_chain_id_empty_returns_empty_view() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let model = ChainTraceModel::new(1, pid);
        let output = model.render_output(&peers);
        assert_eq!(output.chain_id, "");
        assert!(!output.chain_known);
        assert!(output.continuations.is_empty());
        assert!(output.markers.is_empty());
    }

    #[tokio::test]
    async fn render_output_unknown_chain_id_reports_unknown() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let mut model = ChainTraceModel::new(1, pid);
        model.initialize(&peers);
        model.set_chain_id("UNKNOWN_CHAIN");
        let output = model.render_output(&peers);
        assert_eq!(output.chain_id, "UNKNOWN_CHAIN");
        assert!(!output.chain_known);
    }
}
