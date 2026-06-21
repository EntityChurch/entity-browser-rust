//! Cross-impl selection schema — slot writer for the per-panel +
//! app-aggregate selection slots.
//!
//! Mirrors workbench-go's `entitysdk.Selection` (post-Stage-5 cleanup):
//! `{path, type, peer_id, updated_at}` with `paths[]` and
//! `source_window` dropped. Entity type `app/state/selection` per
//! the guide §5 slot table.
//!
//! **Two slots, same schema.** A panel that publishes a navigate /
//! select event writes both:
//! - **Per-panel:** `{peer_id}/app/{aid}/workspace/panels/{panel_id}/selection`
//!   — the panel's own slot. Used for restoring panel state across
//!   sessions and as a stable target for panels that want to subscribe
//!   to a specific panel's selection. (Path uses `panels` per
//!   workbench-go's cross-impl rename; Rust-side type
//!   stays `WindowId` for now.)
//! - **App-aggregate:** `{peer_id}/app/{aid}/workspace/selection` —
//!   the "global" selection that other panels co-orient against. We
//!   use a flat app-aggregate (no screen layer) since we're a
//!   single-screen app today; the schema is forward-compatible with
//!   multi-screen.
//!
//! Path helpers live in `crate::app_paths`. Subscribers read via
//! `ctx.store().on_prefix_change_seeded(prefix, …)` (Direct) /
//! `Peers::observe_with_events(…)` (cross-arm normalized).

#![allow(dead_code)]

use entity_entity::Entity;

/// One selection record. Stored as CBOR at the per-panel and
/// app-aggregate slots. Optional fields are omitted from the CBOR map
/// when empty/zero — matches the guide's "absence = unset"
/// convention.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// The selected tree path. Empty string is legal but unusual —
    /// typically a clear is represented by removing the slot entity
    /// rather than writing an empty-path Selection.
    pub path: String,
    /// What kind of pointee `path` refers to. "entity" today; future
    /// values like "query-result", "event-log-row".
    pub type_: Option<String>,
    /// Which peer's tree contains `path`. Empty when the host peer.
    pub peer_id: Option<String>,
    /// Epoch milliseconds at write time. Staleness signal for
    /// last-writer tie-breaks on the aggregate slot.
    pub updated_at: u64,
}

impl Selection {
    /// Construct an entity-pointer selection. Auto-fills `type` =
    /// `"entity"` and `updated_at` from the current time. Pass
    /// `peer_id = ""` to omit (= host peer).
    pub fn entity(path: impl Into<String>, peer_id: impl Into<String>) -> Self {
        let peer_id = peer_id.into();
        let peer_id = if peer_id.is_empty() { None } else { Some(peer_id) };
        Self {
            path: path.into(),
            type_: Some("entity".into()),
            peer_id,
            updated_at: now_epoch_ms(),
        }
    }

    /// Decode from an entity body. Tolerant of records missing
    /// optional fields. Returns a default `Selection` (empty path,
    /// zero updated_at) when the CBOR shape is unrecognized.
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };
        let mut sel = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("path") => {
                    if let Some(s) = v.as_text() {
                        sel.path = s.to_string();
                    }
                }
                Some("type") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            sel.type_ = Some(s.to_string());
                        }
                    }
                }
                Some("peer_id") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            sel.peer_id = Some(s.to_string());
                        }
                    }
                }
                Some("updated_at") => {
                    sel.updated_at = v.as_integer().and_then(|i| u64::try_from(i).ok()).unwrap_or(0);
                }
                _ => {}
            }
        }
        sel
    }

    /// Encode to an entity body. Optional fields omitted when
    /// empty/zero; `updated_at` always present.
    pub fn to_entity(&self) -> Entity {
        let mut pairs: Vec<(entity_ecf::Value, entity_ecf::Value)> = Vec::new();
        pairs.push((
            entity_ecf::Value::Text("path".into()),
            entity_ecf::text(&self.path),
        ));
        if let Some(ref t) = self.type_ {
            pairs.push((
                entity_ecf::Value::Text("type".into()),
                entity_ecf::text(t),
            ));
        }
        if let Some(ref p) = self.peer_id {
            pairs.push((
                entity_ecf::Value::Text("peer_id".into()),
                entity_ecf::text(p),
            ));
        }
        pairs.push((
            entity_ecf::Value::Text("updated_at".into()),
            entity_ecf::Value::Integer(self.updated_at.into()),
        ));
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        Entity::new("app/state/selection", data).unwrap()
    }
}

/// Write an entity-pointer Selection to both the per-panel and the
/// app-aggregate selection slots. Used by Entity Tree on Navigate and
/// by the shell on `cd`; any future producer panel calls this too.
pub fn publish_entity_selection(
    peers: &crate::peers::Peers,
    peer_id: &str,
    window_id: crate::window::WindowId,
    path: &str,
) {
    let sel = Selection::entity(path, peer_id);
    let panel_path = crate::app_paths::panel_selection_path(
        crate::app_paths::APP_ID,
        peer_id,
        window_id,
    );
    let app_path =
        crate::app_paths::app_selection_path(crate::app_paths::APP_ID, peer_id);
    peers.dispatch_write(peer_id, panel_path, sel.to_entity());
    peers.dispatch_write(peer_id, app_path, sel.to_entity());
}

/// Remove both selection slots — used when the producer panel has
/// "no current selection" (Entity Tree on NavigateUp past the peer
/// root).
pub fn clear_entity_selection(
    peers: &crate::peers::Peers,
    peer_id: &str,
    window_id: crate::window::WindowId,
) {
    let panel_path = crate::app_paths::panel_selection_path(
        crate::app_paths::APP_ID,
        peer_id,
        window_id,
    );
    let app_path =
        crate::app_paths::app_selection_path(crate::app_paths::APP_ID, peer_id);
    peers.dispatch_remove(peer_id, panel_path);
    peers.dispatch_remove(peer_id, app_path);
}

/// Remove only the per-panel selection slot for `window_id`. Used at
/// window-close time — the app-aggregate slot stays put because another
/// open panel may have published there (single-valued slot, last-write
/// wins). Clearing it on every close would race other panels still
/// holding a current selection.
///
/// See the D9 memory-accounting audit §2.B for the rationale.
pub fn clear_panel_selection_on_close(
    peers: &crate::peers::Peers,
    peer_id: &str,
    window_id: crate::window::WindowId,
) {
    let panel_path = crate::app_paths::panel_selection_path(
        crate::app_paths::APP_ID,
        peer_id,
        window_id,
    );
    peers.dispatch_remove(peer_id, panel_path);
}

#[cfg(target_arch = "wasm32")]
fn now_epoch_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
fn now_epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_constructor_fills_defaults() {
        let s = Selection::entity("/p/foo", "");
        assert_eq!(s.path, "/p/foo");
        assert_eq!(s.type_.as_deref(), Some("entity"));
        assert!(s.peer_id.is_none()); // empty string => omitted
        assert!(s.updated_at > 0);
    }

    #[test]
    fn entity_constructor_keeps_peer_id_when_set() {
        let s = Selection::entity("/p/foo", "peerXYZ");
        assert_eq!(s.peer_id.as_deref(), Some("peerXYZ"));
    }

    #[test]
    fn round_trip_full() {
        let original = Selection {
            path: "/p/docs/arch".into(),
            type_: Some("entity".into()),
            peer_id: Some("p".into()),
            updated_at: 1_700_000_000_000,
        };
        let entity = original.to_entity();
        assert_eq!(entity.entity_type, "app/state/selection");
        let decoded = Selection::from_entity(&entity);
        assert_eq!(decoded, original);
    }

    #[test]
    fn round_trip_minimal() {
        let original = Selection {
            path: "/p/x".into(),
            type_: None,
            peer_id: None,
            updated_at: 42,
        };
        let entity = original.to_entity();
        let decoded = Selection::from_entity(&entity);
        assert_eq!(decoded, original);
    }

    #[test]
    fn from_entity_tolerates_unknown_fields() {
        // Build a CBOR map with extra fields that should be ignored
        // (legacy `paths[]`, `source_window`, future additions).
        let pairs: Vec<(entity_ecf::Value, entity_ecf::Value)> = vec![
            (
                entity_ecf::Value::Text("path".into()),
                entity_ecf::text("/p/y"),
            ),
            (
                entity_ecf::Value::Text("source_window".into()),
                entity_ecf::Value::Integer(7.into()),
            ),
            (
                entity_ecf::Value::Text("paths".into()),
                entity_ecf::Value::Array(vec![entity_ecf::text("/p/y"), entity_ecf::text("/p/z")]),
            ),
        ];
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        let entity = Entity::new("app/state/selection", data).unwrap();
        let decoded = Selection::from_entity(&entity);
        assert_eq!(decoded.path, "/p/y");
        // Other fields default.
    }
}
