//! Consumer-side inspect-sink router. Bridges the two arms of `Peers`:
//!
//! - **Direct arm:** `entity_sdk::PeerContext::install_inspect_sink`
//!   produces an `entity_sdk::InspectSinkHandle` directly.
//! - **Worker arm:** `entity_wasm_worker_proxy::WorkerProxy::install_inspect_sink`
//!   takes the wire-shape `entity_wasm_worker_protocol::InspectFact`.
//!   This module wraps the consumer's callback so the conversion to
//!   `entity_sdk::InspectFact` happens here, and returns the proxy's
//!   `InspectSinkHandle` instead.
//!
//! See the upstream inspect-worker-arm design §7.
//!
//! Both shapes are kept in nominal sync (identical field layout); drift
//! is caught at compile time inside `wire_to_sdk_fact` because the
//! exhaustive `match` arms reference every enum variant.

use entity_sdk::{
    InspectBindingKind, InspectFact, InspectSinkHandle as SdkInspectSinkHandle,
    InspectWireFrameDirection, SdkError,
};

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("unknown peer (not present in the Peers route map)")]
    UnknownPeer,
    #[error("SDK install failed: {0}")]
    Sdk(#[from] SdkError),
}

/// Unified handle returned by [`crate::peers::Peers::install_inspect_sink`].
/// Drop detaches the sink — Direct arm synchronously via the SDK
/// registry; Worker arm via a fire-and-forget
/// `Request::SetInspectEnabled(false)` when it was the last sink.
// The inner handles are never *read* — they are RAII drop-guards: holding
// the variant keeps the sink attached, dropping it detaches it (see the type
// doc above). That IS the contract, so `dead_code` ("field never read") is a
// false positive here.
#[allow(dead_code)]
#[must_use = "dropping the handle detaches the sink"]
pub enum PeersInspectSinkHandle {
    Direct(SdkInspectSinkHandle),
    #[cfg(target_arch = "wasm32")]
    Worker(
        entity_wasm_worker_proxy::InspectSinkHandle<entity_wasm_worker_proxy::WebTransport>,
    ),
}

/// Convert the wire-protocol `InspectFact` (PROTOCOL_VERSION=9) into
/// the SDK-side `InspectFact`. Pure field-by-field mapping. The
/// exhaustive match catches any field drift at compile time.
#[cfg(target_arch = "wasm32")]
pub(crate) fn wire_to_sdk_fact(
    wire: &entity_wasm_worker_protocol::InspectFact,
) -> InspectFact {
    use entity_wasm_worker_protocol::{
        BindingKind as WireBindingKind, InspectFact as WireFact, WireDirection as WireDir,
    };
    match wire {
        WireFact::Dispatch {
            request_id,
            handler_uri,
            operation,
            status,
            elapsed_micros,
            chain_id,
        } => InspectFact::Dispatch {
            request_id: request_id.clone(),
            handler_uri: handler_uri.clone(),
            operation: operation.clone(),
            status: *status,
            elapsed_micros: *elapsed_micros,
            chain_id: chain_id.clone(),
        },
        WireFact::Wire {
            direction,
            peer_remote,
            frame_kind,
            bytes,
            request_id,
        } => InspectFact::Wire {
            direction: match direction {
                WireDir::Inbound => InspectWireFrameDirection::Inbound,
                WireDir::Outbound => InspectWireFrameDirection::Outbound,
            },
            peer_remote: peer_remote.clone(),
            frame_kind: frame_kind.clone(),
            bytes: *bytes,
            request_id: request_id.clone(),
        },
        WireFact::Binding {
            kind,
            path,
            entity_type,
            content_hash,
            is_new,
        } => InspectFact::Binding {
            kind: match kind {
                WireBindingKind::Put => InspectBindingKind::Put,
                WireBindingKind::Remove => InspectBindingKind::Remove,
                WireBindingKind::Snapshot => InspectBindingKind::Snapshot,
                WireBindingKind::CacheInvalidate => InspectBindingKind::CacheInvalidate,
            },
            path: path.clone(),
            entity_type: entity_type.clone(),
            content_hash: content_hash.clone(),
            is_new: *is_new,
        },
    }
}
