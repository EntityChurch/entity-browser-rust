//! The mount machinery: materialize → seed → tick, arm-agnostic.
//!
//! Every function here builds its dispatch futures **synchronously from
//! `&Peers`** (the `Sdk::execute` contract: creation is eager, dispatch
//! is at first poll) and returns one owning future the caller awaits —
//! `spawn_local` in the window, plain `.await` in the native oracle test.
//! Ordering inside a tick is explicit: the state write is awaited
//! (`WriterHandle::put_wait`) before the projection future is first
//! polled, so its `lookup/tree` reads the new state on both arms.
//!
//! The host stays program-blind: it never decodes program state — it
//! moves entities eval produced (Finding B's rule applied at the base
//! contract: "put an entity an eval produced, never decode it").

use std::future::Future;
use std::pin::Pin;

use entity_entity::Entity;
use entity_handler::HandlerResult;

use crate::peers::Peers;

use super::descriptor::ProgramDescriptor;

/// Non-Send owning future — awaited on the main thread (wasm) or a
/// current-thread test runtime (native).
pub type HostFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>>>>;

/// Poll `fut` to completion, converting a **panic during a poll** into `Err` —
/// so a bad tick (a debug-build arithmetic/hash overflow in the evaluator, a
/// malformed-but-typed program) DEGRADES to a visible fault + a stopped clock
/// instead of unwinding out of the driving `spawn_local` task and freezing the
/// board with no marker (D13/AP3;
/// `AUDIT-L5-COMPUTE-HOST-FOUNDATION-2026-08-01` finding #1). This is the tick
/// loop's twin of the rAF loop's C1 guard (`main.rs`): under the release
/// `panic = "unwind"` profile the panic is caught here; under dev/abort (wasm)
/// a panic is fatal regardless — the same whole-app limit, documented there.
/// Native builds always unwind, so the unit tests exercise the catch. A future
/// that panicked mid-poll is never polled again (the caller drops it on `Err`).
pub async fn guarded(fut: HostFuture<()>) -> Result<(), String> {
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::task::Poll;
    let mut fut = fut;
    std::future::poll_fn(move |cx| {
        match catch_unwind(AssertUnwindSafe(|| fut.as_mut().poll(cx))) {
            Ok(poll) => poll,
            Err(_) => Poll::Ready(Err(
                // Diagnostic fault reason (the panic itself already printed via
                // the panic hook) — same class as `with_timeout`'s message.
                "tick panicked — clock stopped (see the panic above)".to_string(),
            )),
        }
    })
    .await
}

/// Namespace-qualify a program-relative path (`app/life/state` →
/// `/{ns}/app/life/state`). `ns` is the program's ORIGIN namespace (the
/// authoring peer's id from the bundle), not necessarily the hosting
/// peer: the Go builder bakes origin-qualified absolute paths into the
/// IR's `lookup/tree` nodes, so the whole program must live under that
/// namespace in the local store for the evaluator's reads to resolve —
/// the same foreign-natural-path caching shape the Apps window uses.
/// (Finding routed upstream; if workbench moves the builders to
/// `relative` lookups this collapses back to the hosting peer's ns.)
pub fn qualify(ns: &str, rel: &str) -> String {
    if rel.starts_with('/') {
        rel.to_string()
    } else {
        format!("/{ns}/{rel}")
    }
}

/// One `system/compute:eval` dispatch future for the expression at
/// `expr_path` (program-relative), handler defaults for budget.
fn eval_future(
    peers: &Peers,
    peer_id: &str,
    ns: &str,
    expr_path: &str,
) -> Pin<Box<dyn Future<Output = Result<HandlerResult, String>>>> {
    let (params, opts) = entity_sdk::compute::eval_request(
        qualify(ns, expr_path),
        entity_sdk::compute::EvalOptions::default(),
    );
    peers.execute(
        peer_id,
        entity_sdk::compute::HANDLER.to_string(),
        entity_sdk::compute::OP_EVAL.to_string(),
        params,
        opts,
    )
}

/// Map an eval dispatch result to the produced value entity.
/// `compute/error` arrives at HTTP 200 (F10) and is a fault to the host.
pub fn eval_result_entity(
    r: Result<HandlerResult, String>,
    what: &str,
) -> Result<Entity, String> {
    let hr = r.map_err(|e| format!("eval {what}: dispatch: {e}"))?;
    if hr.status != 200 {
        return Err(format!(
            "eval {what}: status {} (type={})",
            hr.status, hr.result.entity_type
        ));
    }
    if hr.result.entity_type == "compute/error" {
        let (code, message) = decode_compute_error(&hr.result);
        return Err(format!("eval {what}: compute/error {code}: {message}"));
    }
    Ok(hr.result)
}

fn decode_compute_error(entity: &Entity) -> (String, String) {
    let mut code = String::from("?");
    let mut message = String::new();
    if let Ok(ciborium::Value::Map(map)) =
        ciborium::from_reader::<ciborium::Value, _>(entity.data.as_slice())
    {
        for (k, v) in &map {
            match k.as_text() {
                Some("code") => code = v.as_text().unwrap_or("?").to_string(),
                Some("message") => message = v.as_text().unwrap_or("").to_string(),
                _ => {}
            }
        }
    }
    (code, message)
}

/// Materialize verified bundle entities at their program-relative paths,
/// sequentially awaited (each write confirmed by the owning store).
/// `on_progress(done, total)` fires per entity — the D13 surface.
pub fn materialize_future(
    peers: &Peers,
    peer_id: &str,
    ns: &str,
    entities: Vec<(String, Entity)>,
    on_progress: impl Fn(usize, usize) + 'static,
) -> HostFuture<()> {
    let Some(writer) = peers.writer_handle_for(peer_id) else {
        return Box::pin(async { Err("no writer handle (unrouted peer)".to_string()) });
    };
    let ns = ns.to_string();
    Box::pin(async move {
        let total = entities.len();
        // Chunked-concurrent: every entity's write is individually
        // confirmed (no silent-drop window), but a chunk's put_waits are
        // in flight together, so the Worker arm pays ~one round-trip
        // latency per CHUNK instead of per entity (88-entity Life took
        // ~2.5 s fully serialized; 441-entity Asteroids would be ~15 s).
        const CHUNK: usize = 32;
        let mut done = 0usize;
        let mut iter = entities.into_iter().peekable();
        while iter.peek().is_some() {
            let chunk: Vec<(String, HostFuture<()>)> = iter
                .by_ref()
                .take(CHUNK)
                .map(|(path, entity)| {
                    let fut = writer.put_wait(qualify(&ns, &path), entity);
                    (path, fut)
                })
                .collect();
            done += chunk.len();
            join_writes(chunk).await?;
            on_progress(done, total);
        }
        Ok(())
    })
}

/// Await a batch of write futures concurrently (a minimal join_all —
/// the crate deliberately has no `futures` dependency). Fails fast on
/// the first error, naming the path.
async fn join_writes(mut slots: Vec<(String, HostFuture<()>)>) -> Result<(), String> {
    use std::task::Poll;
    let mut live: Vec<bool> = vec![true; slots.len()];
    std::future::poll_fn(move |cx| {
        let mut pending = false;
        for (i, (path, fut)) in slots.iter_mut().enumerate() {
            if !live[i] {
                continue;
            }
            match fut.as_mut().poll(cx) {
                Poll::Ready(Ok(())) => live[i] = false,
                Poll::Ready(Err(e)) => {
                    return Poll::Ready(Err(format!("materialize {path}: {e}")))
                }
                Poll::Pending => pending = true,
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    })
    .await
}

/// Seed per F-E1: copy `initial_state` → `state_path`, and each input
/// port's `initial` → its `path`. Also used by Restart (reseed).
pub fn seed_future(
    peers: &Peers,
    peer_id: &str,
    ns: &str,
    desc: &ProgramDescriptor,
) -> HostFuture<()> {
    let Some(writer) = peers.writer_handle_for(peer_id) else {
        return Box::pin(async { Err("no writer handle (unrouted peer)".to_string()) });
    };
    // (src get-future, dst path, label) triples; gets created eagerly.
    let mut copies = Vec::new();
    copies.push((
        peers.get_entity_async(peer_id, &qualify(ns, &desc.initial_state)),
        qualify(ns, &desc.state_path),
        desc.initial_state.clone(),
    ));
    for p in &desc.input_ports {
        let initial = p.initial.clone().expect("validated: input port has initial");
        copies.push((
            peers.get_entity_async(peer_id, &qualify(ns, &initial)),
            qualify(ns, &p.path),
            initial,
        ));
    }
    Box::pin(async move {
        for (get, dst, label) in copies {
            let entity = get
                .await
                .map_err(|e| format!("seed: get {label}: {e}"))?
                .ok_or_else(|| format!("seed: {label} missing (not materialized?)"))?;
            writer
                .put_wait(dst, entity)
                .await
                .map_err(|e| format!("seed: {e}"))?;
        }
        Ok(())
    })
}

/// One mount-contract tick:
/// `eval(step)` → put `state_path` → for each `source`-bearing output
/// port: `eval(source)` → put `path`. Projection type-checked against
/// the port's declared `type_ref` (the blank-board bug's cheap guard).
pub fn tick_future(
    peers: &Peers,
    peer_id: &str,
    ns: &str,
    desc: &ProgramDescriptor,
) -> HostFuture<()> {
    let Some(writer) = peers.writer_handle_for(peer_id) else {
        return Box::pin(async { Err("no writer handle (unrouted peer)".to_string()) });
    };
    let step_fut = eval_future(peers, peer_id, ns, &desc.step);
    let state_path = qualify(ns, &desc.state_path);
    // Projection futures created eagerly (dispatch happens at first
    // poll — AFTER the state write below is awaited).
    let ports: Vec<_> = desc
        .output_ports
        .iter()
        .filter_map(|p| {
            p.source.as_ref().map(|source| {
                (
                    p.name.clone(),
                    p.type_ref.clone(),
                    qualify(ns, &p.path),
                    eval_future(peers, peer_id, ns, source),
                )
            })
        })
        .collect();
    Box::pin(async move {
        tracing::debug!("tick stage: step eval dispatch");
        let state = eval_result_entity(step_fut.await.map_err(|e| e.to_string()), "step")
            .map_err(|e| format!("step: {e}"))?;
        tracing::debug!("tick stage: step done, putting state");
        writer
            .put_wait(state_path, state)
            .await
            .map_err(|e| format!("put state: {e}"))?;
        tracing::debug!("tick stage: state put, refreshing ports");
        for (name, type_ref, path, fut) in ports {
            let ent = eval_result_entity(fut.await.map_err(|e| e.to_string()), &name)
                .map_err(|e| format!("port {name}: {e}"))?;
            tracing::debug!(port = %name, "tick stage: port eval done");
            if ent.entity_type != type_ref {
                return Err(format!(
                    "port {name}: produced type {} != declared type_ref {}",
                    ent.entity_type, type_ref
                ));
            }
            writer
                .put_wait(path, ent)
                .await
                .map_err(|e| format!("port {name}: {e}"))?;
        }
        Ok(())
    })
}

/// Write an input-port entity (a driver write; awaited so a subsequent
/// tick's `lookup/tree` observes it).
pub fn input_future(
    peers: &Peers,
    peer_id: &str,
    ns: &str,
    port_path: &str,
    entity: Entity,
) -> HostFuture<()> {
    let Some(writer) = peers.writer_handle_for(peer_id) else {
        return Box::pin(async { Err("no writer handle (unrouted peer)".to_string()) });
    };
    let path = qualify(ns, port_path);
    Box::pin(async move { writer.put_wait(path, entity).await })
}

#[cfg(test)]
mod tests {
    use super::*;

    // `guarded` contains a panicking tick (native = unwind) so the loop sees an
    // `Err` and can degrade visibly, instead of the panic killing the task.
    #[tokio::test]
    async fn guarded_converts_a_panicking_tick_to_err() {
        let fut: HostFuture<()> = Box::pin(async { panic!("boom") });
        assert!(guarded(fut).await.is_err());
    }

    // A clean tick and a normally-faulting tick both pass through untouched.
    #[tokio::test]
    async fn guarded_passes_through_ok_and_err() {
        let ok: HostFuture<()> = Box::pin(async { Ok(()) });
        assert_eq!(guarded(ok).await, Ok(()));
        let err: HostFuture<()> = Box::pin(async { Err("faulted".to_string()) });
        assert_eq!(guarded(err).await, Err("faulted".to_string()));
    }
}
