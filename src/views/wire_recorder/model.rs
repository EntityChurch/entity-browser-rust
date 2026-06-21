//! Wire Recorder — observational ring buffer over `InspectFact::Wire`
//! events fired on the bound peer.
//!
//! Same shape as `views::path_tap::model` — sink-fed ring + per-variant
//! cumulative counter. Filters for `Wire` rows; other variants increment
//! the counter strip only, so a quiet wire stream alongside busy
//! dispatch/binding traffic is visible at a glance.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use entity_sdk::InspectFact;

use crate::views::path_tap::model::VariantCounts;
use crate::views::wire_recorder::output::{WireDirection, WireRecorderOutput, WireRow};
use crate::window::WindowId;

/// Maximum wire rows retained. Older rows are dropped from the front
/// on overflow. Same cap as Path Tap.
pub const RING_CAP: usize = 200;

#[derive(Clone, Default)]
pub struct WireRing {
    inner: Arc<Mutex<VecDeque<WireRow>>>,
    counts: Arc<Mutex<VariantCounts>>,
}

impl WireRing {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, fact: &InspectFact) {
        match fact {
            InspectFact::Wire {
                direction,
                peer_remote,
                frame_kind,
                bytes,
                request_id,
            } => {
                self.counts.lock().unwrap().wire += 1;
                let row = WireRow {
                    direction: match direction {
                        entity_sdk::InspectWireFrameDirection::Inbound => WireDirection::Inbound,
                        entity_sdk::InspectWireFrameDirection::Outbound => WireDirection::Outbound,
                    },
                    peer_remote: peer_remote.clone(),
                    frame_kind: frame_kind.clone(),
                    bytes: *bytes,
                    request_id: request_id.clone(),
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
            InspectFact::Binding { .. } => {
                self.counts.lock().unwrap().binding += 1;
            }
        }
    }

    pub fn variant_counts(&self) -> VariantCounts {
        self.counts.lock().unwrap().clone()
    }

    pub fn snapshot_newest_first(&self) -> Vec<WireRow> {
        let g = self.inner.lock().unwrap();
        g.iter().rev().cloned().collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

pub struct WireRecorderModel {
    ring: WireRing,
    routing_active: bool,
}

impl WireRecorderModel {
    // `window_id`/`peer_id` accepted for factory-signature parity, not
    // stored — passive sink-fed window, renderer reads no identity.
    pub fn new(_window_id: WindowId, _peer_id: String) -> Self {
        Self {
            ring: WireRing::new(),
            routing_active: false,
        }
    }

    pub fn ring(&self) -> WireRing {
        self.ring.clone()
    }

    pub fn mark_routing_active(&mut self) {
        self.routing_active = true;
    }

    pub fn render_output(&self) -> WireRecorderOutput {
        WireRecorderOutput {
            routing_active: self.routing_active,
            rows: self.ring.snapshot_newest_first(),
            counts: self.ring.variant_counts(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire_frame(bytes: u32) -> InspectFact {
        InspectFact::Wire {
            direction: entity_sdk::InspectWireFrameDirection::Outbound,
            peer_remote: Some("remote-1".into()),
            frame_kind: "execute".into(),
            bytes,
            request_id: None,
        }
    }

    #[test]
    fn pushes_wire_facts() {
        let ring = WireRing::new();
        ring.push(&wire_frame(10));
        ring.push(&wire_frame(20));
        assert_eq!(ring.len(), 2);
        let counts = ring.variant_counts();
        assert_eq!(counts.wire, 2);
        assert_eq!(counts.dispatch, 0);
        assert_eq!(counts.binding, 0);
    }

    #[test]
    fn ring_caps_at_max() {
        let ring = WireRing::new();
        for _ in 0..(RING_CAP + 25) {
            ring.push(&wire_frame(1));
        }
        assert_eq!(ring.len(), RING_CAP);
    }

    #[test]
    fn non_wire_variants_are_counted_not_stored() {
        let ring = WireRing::new();
        ring.push(&InspectFact::Dispatch {
            request_id: "r1".into(),
            handler_uri: "x".into(),
            operation: "y".into(),
            status: 200,
            elapsed_micros: None,
            chain_id: None,
        });
        ring.push(&InspectFact::Binding {
            kind: entity_sdk::InspectBindingKind::Put,
            path: "/p".into(),
            entity_type: None,
            content_hash: None,
            is_new: true,
        });
        assert_eq!(ring.len(), 0);
        let counts = ring.variant_counts();
        assert_eq!(counts.dispatch, 1);
        assert_eq!(counts.binding, 1);
        assert_eq!(counts.wire, 0);
    }

    #[test]
    fn snapshot_is_newest_first() {
        let ring = WireRing::new();
        for i in 1..=3 {
            ring.push(&wire_frame(i));
        }
        let rows = ring.snapshot_newest_first();
        assert_eq!(rows[0].bytes, 3);
        assert_eq!(rows[1].bytes, 2);
        assert_eq!(rows[2].bytes, 1);
    }
}
