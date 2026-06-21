//! Content Stream — observational ring buffer over `InspectFact::Binding`
//! events fired on the bound peer.
//!
//! Sibling to `views::path_tap::model` and `views::wire_recorder::model`.
//! Captures local-store writes (put / remove / snapshot / cache-invalidate)
//! so users can see "what entities just got written, where" without
//! polling the tree.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use entity_sdk::InspectFact;

use crate::views::content_stream::output::{
    BindingKind, BindingRow, ContentStreamOutput,
};
use crate::views::path_tap::model::VariantCounts;
use crate::window::WindowId;

pub const RING_CAP: usize = 200;

#[derive(Clone, Default)]
pub struct ContentRing {
    inner: Arc<Mutex<VecDeque<BindingRow>>>,
    counts: Arc<Mutex<VariantCounts>>,
}

impl ContentRing {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, fact: &InspectFact) {
        match fact {
            InspectFact::Binding {
                kind,
                path,
                entity_type,
                content_hash,
                is_new,
            } => {
                self.counts.lock().unwrap().binding += 1;
                let row = BindingRow {
                    kind: match kind {
                        entity_sdk::InspectBindingKind::Put => BindingKind::Put,
                        entity_sdk::InspectBindingKind::Remove => BindingKind::Remove,
                        entity_sdk::InspectBindingKind::Snapshot => BindingKind::Snapshot,
                        entity_sdk::InspectBindingKind::CacheInvalidate => {
                            BindingKind::CacheInvalidate
                        }
                    },
                    path: path.clone(),
                    entity_type: entity_type.clone(),
                    content_hash: content_hash.clone(),
                    is_new: *is_new,
                };
                let mut g = self.inner.lock().unwrap();
                g.push_back(row);
                while g.len() > RING_CAP {
                    g.pop_front();
                }
            }
            InspectFact::Dispatch { .. } => {
                self.counts.lock().unwrap().dispatch += 1;
            }
            InspectFact::Wire { .. } => {
                self.counts.lock().unwrap().wire += 1;
            }
        }
    }

    pub fn variant_counts(&self) -> VariantCounts {
        self.counts.lock().unwrap().clone()
    }

    pub fn snapshot_newest_first(&self) -> Vec<BindingRow> {
        let g = self.inner.lock().unwrap();
        g.iter().rev().cloned().collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

pub struct ContentStreamModel {
    ring: ContentRing,
    routing_active: bool,
}

impl ContentStreamModel {
    // `window_id`/`peer_id` are accepted for factory-signature parity but
    // not stored — this passive sink-fed window keys off nothing but its
    // ring; the renderer never reads window/peer identity.
    pub fn new(_window_id: WindowId, _peer_id: String) -> Self {
        Self {
            ring: ContentRing::new(),
            routing_active: false,
        }
    }

    pub fn ring(&self) -> ContentRing {
        self.ring.clone()
    }

    pub fn mark_routing_active(&mut self) {
        self.routing_active = true;
    }

    pub fn render_output(&self) -> ContentStreamOutput {
        ContentStreamOutput {
            routing_active: self.routing_active,
            rows: self.ring.snapshot_newest_first(),
            counts: self.ring.variant_counts(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(path: &str, is_new: bool) -> InspectFact {
        InspectFact::Binding {
            kind: entity_sdk::InspectBindingKind::Put,
            path: path.into(),
            entity_type: Some("test/x".into()),
            content_hash: Some("deadbeef".into()),
            is_new,
        }
    }

    #[test]
    fn pushes_binding_facts() {
        let ring = ContentRing::new();
        ring.push(&binding("/a", true));
        ring.push(&binding("/b", false));
        assert_eq!(ring.len(), 2);
        let counts = ring.variant_counts();
        assert_eq!(counts.binding, 2);
        assert_eq!(counts.dispatch, 0);
        assert_eq!(counts.wire, 0);
    }

    #[test]
    fn ring_caps_at_max() {
        let ring = ContentRing::new();
        for i in 0..(RING_CAP + 20) {
            ring.push(&binding(&format!("/p{i}"), false));
        }
        assert_eq!(ring.len(), RING_CAP);
    }

    #[test]
    fn snapshot_is_newest_first() {
        let ring = ContentRing::new();
        ring.push(&binding("/a", true));
        ring.push(&binding("/b", false));
        let rows = ring.snapshot_newest_first();
        assert_eq!(rows[0].path, "/b");
        assert_eq!(rows[1].path, "/a");
    }

    #[test]
    fn maps_all_binding_kinds() {
        let ring = ContentRing::new();
        let kinds = [
            (entity_sdk::InspectBindingKind::Put, BindingKind::Put),
            (entity_sdk::InspectBindingKind::Remove, BindingKind::Remove),
            (entity_sdk::InspectBindingKind::Snapshot, BindingKind::Snapshot),
            (
                entity_sdk::InspectBindingKind::CacheInvalidate,
                BindingKind::CacheInvalidate,
            ),
        ];
        for (sdk_kind, _) in &kinds {
            ring.push(&InspectFact::Binding {
                kind: *sdk_kind,
                path: "/p".into(),
                entity_type: None,
                content_hash: None,
                is_new: false,
            });
        }
        let rows = ring.snapshot_newest_first();
        assert_eq!(rows.len(), 4);
        // Newest-first: kinds reverse order
        assert_eq!(rows[0].kind, BindingKind::CacheInvalidate);
        assert_eq!(rows[3].kind, BindingKind::Put);
    }

    #[test]
    fn non_binding_variants_are_counted_not_stored() {
        let ring = ContentRing::new();
        ring.push(&InspectFact::Dispatch {
            request_id: "r1".into(),
            handler_uri: "x".into(),
            operation: "y".into(),
            status: 200,
            elapsed_micros: None,
            chain_id: None,
        });
        ring.push(&InspectFact::Wire {
            direction: entity_sdk::InspectWireFrameDirection::Inbound,
            peer_remote: None,
            frame_kind: "execute".into(),
            bytes: 10,
            request_id: None,
        });
        assert_eq!(ring.len(), 0);
        let counts = ring.variant_counts();
        assert_eq!(counts.dispatch, 1);
        assert_eq!(counts.wire, 1);
        assert_eq!(counts.binding, 0);
    }
}
