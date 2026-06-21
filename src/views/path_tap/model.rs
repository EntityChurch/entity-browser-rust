//! Path Tap — observational ring buffer over `InspectFact::Dispatch`
//! events fired on the bound peer.
//!
//! Unlike Chain Trace (which mirrors path-bound substrate state via
//! L0 subscribe), Path Tap consumes the **live dispatch hook stream**
//! through `Peers::install_inspect_sink`. The sink callback pushes
//! exit-phase facts (status != 0) into a bounded ring; render reads
//! the snapshot. State is intentionally NOT persisted to the tree —
//! the buffer is ephemeral per session.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use entity_sdk::InspectFact;

use crate::views::path_tap::output::{DispatchRow, PathTapOutput};
use crate::window::WindowId;

/// Maximum dispatch rows retained. Older rows are dropped from the
/// front on overflow. Sized for visual scan (a few seconds of busy
/// activity) — consumers wanting full retention should compose with a
/// log writer downstream.
pub const RING_CAP: usize = 200;

#[derive(Clone, Default)]
pub struct PathTapRing {
    inner: Arc<Mutex<VecDeque<DispatchRow>>>,
    /// Diagnostic per-variant total counter (cumulative; not capped).
    /// Exposed via `variant_counts` so renderers + tests can see what
    /// the live stream looks like — particularly useful when the
    /// dispatch ring is empty but other variants are arriving.
    counts: Arc<Mutex<VariantCounts>>,
}

#[derive(Debug, Clone, Default)]
pub struct VariantCounts {
    pub dispatch: u32,
    pub wire: u32,
    pub binding: u32,
}

impl PathTapRing {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a Dispatch fact into the ring (entry OR exit phase).
    /// Non-Dispatch variants are ignored from the row buffer but
    /// counted in `variant_counts` so consumers can see what kinds
    /// of facts are arriving even when the dispatch ring is empty.
    pub fn push(&self, fact: &InspectFact) {
        match fact {
            InspectFact::Dispatch {
                request_id,
                handler_uri,
                operation,
                status,
                ..
            } => {
                self.counts.lock().unwrap().dispatch += 1;
                let row = DispatchRow {
                    request_id: request_id.clone(),
                    handler_uri: handler_uri.clone(),
                    operation: operation.clone(),
                    status: *status,
                };
                let mut g = self.inner.lock().unwrap();
                g.push_back(row);
                while g.len() > RING_CAP {
                    g.pop_front();
                }
            }
            InspectFact::Wire { .. } => {
                self.counts.lock().unwrap().wire += 1;
            }
            InspectFact::Binding { .. } => {
                self.counts.lock().unwrap().binding += 1;
            }
        }
    }

    pub fn variant_counts(&self) -> VariantCounts {
        self.counts.lock().unwrap().clone()
    }

    pub fn snapshot_newest_first(&self) -> Vec<DispatchRow> {
        let g = self.inner.lock().unwrap();
        g.iter().rev().cloned().collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

pub struct PathTapModel {
    ring: PathTapRing,
    /// Set to true at construction once `install_inspect_sink` returns
    /// Ok. Drives the empty-state copy in the output.
    routing_active: bool,
}

impl PathTapModel {
    // `window_id`/`peer_id` accepted for factory-signature parity, not
    // stored — passive sink-fed window, renderer reads no identity.
    pub fn new(_window_id: WindowId, _peer_id: String) -> Self {
        Self {
            ring: PathTapRing::new(),
            routing_active: false,
        }
    }

    pub fn ring(&self) -> PathTapRing {
        self.ring.clone()
    }

    pub fn mark_routing_active(&mut self) {
        self.routing_active = true;
    }

    pub fn render_output(&self) -> PathTapOutput {
        PathTapOutput {
            routing_active: self.routing_active,
            rows: self.ring.snapshot_newest_first(),
            counts: self.ring.variant_counts(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dispatch(status: u32) -> InspectFact {
        InspectFact::Dispatch {
            request_id: "r1".into(),
            handler_uri: "x/y".into(),
            operation: "z".into(),
            status,
            elapsed_micros: None,
            chain_id: None,
        }
    }

    #[test]
    fn entry_phase_is_kept_with_status_zero() {
        let ring = PathTapRing::new();
        ring.push(&dispatch(0));
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn exit_phase_pushes() {
        let ring = PathTapRing::new();
        ring.push(&dispatch(200));
        assert_eq!(ring.len(), 1);
    }

    #[test]
    fn ring_caps_at_max() {
        let ring = PathTapRing::new();
        for _ in 0..(RING_CAP + 50) {
            ring.push(&dispatch(200));
        }
        assert_eq!(ring.len(), RING_CAP);
    }

    #[test]
    fn snapshot_is_newest_first() {
        let ring = PathTapRing::new();
        for i in 1..=3 {
            ring.push(&InspectFact::Dispatch {
                request_id: format!("r{i}"),
                handler_uri: "h".into(),
                operation: "o".into(),
                status: 200,
                elapsed_micros: None,
                chain_id: None,
            });
        }
        let rows = ring.snapshot_newest_first();
        assert_eq!(rows[0].request_id, "r3");
        assert_eq!(rows[1].request_id, "r2");
        assert_eq!(rows[2].request_id, "r1");
    }

    #[test]
    fn non_dispatch_variants_are_ignored() {
        let ring = PathTapRing::new();
        ring.push(&InspectFact::Wire {
            direction: entity_sdk::InspectWireFrameDirection::Inbound,
            peer_remote: None,
            frame_kind: "execute".into(),
            bytes: 10,
            request_id: None,
        });
        ring.push(&InspectFact::Binding {
            kind: entity_sdk::InspectBindingKind::Put,
            path: "/p/x".into(),
            entity_type: None,
            content_hash: None,
            is_new: true,
        });
        assert_eq!(ring.len(), 0);
    }
}
