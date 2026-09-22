//! `peer_probe` — the one expression of *"send the cheapest thing that goes
//! through the full dispatch ladder"*.
//!
//! ## Why this is a module and not two copies of four lines
//!
//! Two surfaces need to poke a remote peer for reasons that have nothing to do
//! with the payload: [`reach_keeper`](crate::reach_keeper) (be present at the
//! rendezvous while we are *not* connected) and [`wake_probe`](crate::wake_probe)
//! (find out whether a connection we *believe* is live survived a suspend). They
//! want the same dispatch and they want it for opposite reasons, which is exactly
//! the shape that drifts: one of them grows a resource target, the other keeps a
//! wider one, and the "cheap" probe stops being cheap on one path only. C15's
//! rule — one expression, call sites gated against it — applied on the way in.
//!
//! ## What the probe is, and what it is *for*
//!
//! `system/tree:get` against a narrow prefix on the remote. **The result is
//! discarded, and that is not sloppiness — the round trip is the entire point.**
//! Two different mechanisms hang off it, one per caller:
//!
//! - While no path exists, issuing it *consults the §10.3 establish ladder* and
//!   puts us at the rendezvous where the counterpart is depositing.
//! - While a path is *believed* to exist, issuing it is the only way to learn
//!   that the belief is false. A dead WebRTC data channel is discovered when a
//!   send is attempted and not before (`PortPump::drain` sees
//!   `ready_state != Open` and calls `fail()`); an idle dead channel is
//!   invisible. The transport error that comes back is what drives the kernel's
//!   §A1 seam — `demote_peer_on_transport_error` at `core/peer`'s §10 step-1
//!   dispatch site — which evicts the pooled binding and writes `suspect` in one
//!   event.
//!
//! **So this module never writes liveness and must never learn how.** It sends;
//! the kernel observes the send failing and owns the demotion. That is the §A1
//! seam discipline (*"the demotion write happens at the dispatch caller that both
//! observes the send error and holds `peer_id` — never buried inside a transport
//! primitive"*), and an app that evicted bindings itself would be the fourth
//! parallel liveness store AP12/D8 exists to refuse.

use entity_entity::Entity;
use entity_handler::ExecuteOptions;

use crate::peers::Peers;

/// Build the one probe dispatch at `remote`, as `local`.
///
/// `None` when `local` has no dispatch handle — the caller has nothing to await
/// and should treat it as a probe that did not go out (**not** as one that
/// failed; those are different facts, and only the second says anything about
/// the remote).
///
/// The returned future resolves when the probe lands, whatever the outcome. A
/// failure is the *expected* case on both call paths, so the outcome is
/// deliberately not reported: `reach_keeper` fires this while no path exists,
/// and `wake_probe` fires it precisely because it suspects the path is gone.
/// What matters to both is that the dispatch was attempted, because that is what
/// the kernel needs in order to observe the truth.
pub fn probe(
    peers: &Peers,
    local: &str,
    remote: &str,
) -> Option<impl std::future::Future<Output = ()> + 'static> {
    let dispatch = peers.dispatch_handle(local)?;
    let params = Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null))
        .expect("system/empty Null is well-formed");
    let opts = ExecuteOptions {
        resource: Some(entity_capability::ResourceTarget {
            targets: vec![crate::app_paths::offers_prefix(
                crate::app_paths::APP_ID,
                remote,
            )],
            exclude: vec![],
        }),
        ..Default::default()
    };
    let fut = dispatch.execute(
        format!("entity://{remote}/system/tree"),
        "get".to_string(),
        params,
        opts,
    );
    Some(async move {
        let _ = fut.await;
    })
}

/// Spawn on the local runtime — the arm split every spawned probe needs.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    tokio::spawn(f);
}

/// Spawn on the local runtime — the arm split every spawned probe needs.
#[cfg(target_arch = "wasm32")]
pub fn spawn<F: std::future::Future<Output = ()> + 'static>(f: F) {
    wasm_bindgen_futures::spawn_local(f);
}
