//! `DispatchHandle` — a cloneable, arm-agnostic **dispatch** handle for
//! app-tier flows that make several L1 calls in a row from a spawned task.
//!
//! [`crate::writer_handle::WriterHandle`] is the write-side twin and the reason
//! this shape is already blessed here: an app-tier writer that must survive
//! being moved into a future cannot hold `&Peers`, so it owns the transport
//! instead (`Arc<PeerShared>` on Direct, `Rc<WorkerProxy>` on Worker). That
//! handle is deliberately **fire-and-forget and write-only**, which is enough
//! for an event-log append and not enough for a *conversation*: a flow that
//! reads a result and then dispatches again — list, then read each row; fetch a
//! blob, then fetch the chunks it names — has to await, and it has to still be
//! dispatching after the first `.await` point.
//!
//! Every `Peers` L1 method already returns an **owning** future; what none of
//! them gives you is a way to make the *second* call, because that needs
//! `&Peers` again and the borrow is long gone. Existing multi-step flows work
//! around it by being pumped from the frame loop with a fresh `&Peers` each
//! frame (`MeetSession`, `ChatDelivery`) — a state machine per flow, and the
//! shape AP21 names as a footgun (a pump cannot detect its own changes by
//! diffing around itself). This handle is the alternative for flows that are
//! genuinely sequential rather than reactive: hold it, `.await` in a straight
//! line, and let the arm difference stay in one file.
//!
//! Construct via [`Peers::dispatch_handle`](crate::peers::Peers::dispatch_handle).
//! It does **not** replace `WriterHandle` — a fire-and-forget mirror write
//! should stay fire-and-forget.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use entity_entity::Entity;
use entity_handler::{ExecuteOptions, HandlerResult};

#[cfg(target_arch = "wasm32")]
use std::rc::Rc;
#[cfg(target_arch = "wasm32")]
use entity_wasm_worker_proxy::{WebTransport, WorkerProxy};

/// A future this handle returns. Non-`Send` on wasm (the Worker arm holds
/// `Rc`s and everything runs on the main thread), `Send` on native so a
/// `tokio::spawn` can take it.
#[cfg(not(target_arch = "wasm32"))]
pub type DispatchFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;
#[cfg(target_arch = "wasm32")]
pub type DispatchFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>>>>;

/// Sleep, on whichever runtime this is. Exists for **caller-owned retry**
/// (§7.2.1): the §6.5 establisher runs exactly one negotiation and never
/// retries it, so a caller whose first cross-peer dispatch loses the
/// establishment race must try again — and a retry with no gap just loses it
/// faster. `ChatDelivery` gets this for free from its 5 Hz poll; a one-shot
/// flow has to ask for it.
///
/// Wasm: `setTimeout` via the window. With no window (a worker context — where
/// today's `DispatchHandle::Worker` does not run, since the proxy lives on the
/// main thread) this returns immediately rather than hanging, which degrades a
/// retry loop to a tight one instead of a deadlock.
#[cfg(not(target_arch = "wasm32"))]
pub async fn delay_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

#[cfg(target_arch = "wasm32")]
pub async fn delay_ms(ms: u32) {
    let Some(window) = web_sys::window() else { return };
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32);
    });
    // A dropped rejecting promise reloads the whole app (`index.html`'s
    // `unhandledrejection` guard), so this is consumed, never `let _ = promise`.
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[derive(Clone)]
pub enum DispatchHandle {
    /// Direct arm: the peer's own `PeerContext`, whose L1 methods already
    /// return owning futures.
    Direct(Arc<entity_sdk::PeerContext>),
    /// Worker arm: the proxy plus the peer whose tree/dispatcher the calls
    /// address. Mirrors `WorkerPeerStore`'s own wire conversion — same
    /// `WireEntity`/`WireExecuteOptions` round-trip, so `included` survives
    /// (the worker protocol carries it in both directions, which is what makes
    /// a content pull possible on this arm at all).
    #[cfg(target_arch = "wasm32")]
    Worker {
        proxy: Rc<WorkerProxy<WebTransport>>,
        peer_id: String,
    },
}

impl DispatchHandle {
    /// The local peer these calls act as — the id a counterpart sees, and the
    /// peer whose pool, grants and routes the dispatch rides.
    pub fn local_peer_id(&self) -> String {
        match self {
            DispatchHandle::Direct(ctx) => ctx.peer_id().to_string(),
            #[cfg(target_arch = "wasm32")]
            DispatchHandle::Worker { peer_id, .. } => peer_id.clone(),
        }
    }

    /// One L1 `execute`. A local target is a bare handler URI
    /// (`system/content`); a remote one is `entity://{peer}/system/content`
    /// and resolves through the connection pool — over whatever transport
    /// reaches it.
    pub fn execute(
        &self,
        handler_uri: String,
        operation: String,
        params: Entity,
        opts: ExecuteOptions,
    ) -> DispatchFuture<HandlerResult> {
        match self {
            DispatchHandle::Direct(ctx) => {
                let fut = ctx.execute(handler_uri, operation, params, opts);
                Box::pin(async move { fut.await.map_err(|e| e.to_string()) })
            }
            #[cfg(target_arch = "wasm32")]
            DispatchHandle::Worker { proxy, peer_id } => {
                let (proxy, peer_id) = (proxy.clone(), peer_id.clone());
                Box::pin(async move {
                    let wire_params =
                        entity_wasm_worker_protocol::WireEntity::try_from(params)
                            .map_err(|e| format!("Entity→WireEntity conversion: {e}"))?;
                    let wire_opts =
                        entity_wasm_worker_protocol::WireExecuteOptions::from(&opts);
                    let wire_result = proxy
                        .execute(peer_id, handler_uri, operation, wire_params, wire_opts)
                        .await
                        .map_err(|e| format!("proxy.execute: {e:?}"))?;
                    HandlerResult::try_from(wire_result)
                        .map_err(|e| format!("WireHandlerResult→HandlerResult: {e}"))
                })
            }
        }
    }

    /// Write `entity` at `path` in the **local** peer's tree, awaited. Uses each
    /// arm's own put (not a hand-rolled `system/tree:put` execute) so the
    /// Direct arm's generation bump and the Worker arm's cache-reflection wait
    /// both still happen — the two things a raw dispatch would silently skip.
    pub fn put(&self, path: String, entity: Entity) -> DispatchFuture<()> {
        match self {
            DispatchHandle::Direct(ctx) => {
                let fut = ctx.put(path, entity);
                Box::pin(async move { fut.await.map(|_| ()).map_err(|e| e.to_string()) })
            }
            #[cfg(target_arch = "wasm32")]
            DispatchHandle::Worker { proxy, peer_id } => {
                let (proxy, peer_id) = (proxy.clone(), peer_id.clone());
                Box::pin(async move {
                    let wire = entity_wasm_worker_protocol::WireEntity::try_from(entity)
                        .map_err(|e| format!("Entity→WireEntity conversion: {e}"))?;
                    proxy
                        .put_and_wait_for_cache(peer_id, path, wire, 5_000)
                        .await
                        .map_err(|e| format!("proxy.put_and_wait_for_cache: {e:?}"))?;
                    Ok(())
                })
            }
        }
    }
}
