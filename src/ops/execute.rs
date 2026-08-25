//! `ops::execute` — typed wrapper around `Peers::execute`.
//!
//! Lifted from `app.rs::handle_execute` so the shell `exec` verb (and
//! any future caller) can run a handler op without producing an
//! `Action::Execute`. The Action path now goes Action → handle_execute
//! → `ops::execute` → log; the shell verb path goes shell → `ops::execute`
//! → scrollback line. Both share the same param/opts assembly and
//! result-format logic.

use std::future::Future;
use std::pin::Pin;

use entity_entity::Entity;
use entity_handler::{ExecuteOptions, HandlerResult};

use crate::peers::Peers;

/// Renderer-neutral request shape for one L1 execute call.
#[derive(Debug, Clone)]
pub struct ExecuteRequest {
    /// The peer to dispatch against. For a genuinely remote target,
    /// rewrite `handler_uri` to `entity://{remote}/...` and pass the
    /// originating local peer here — the connection pool resolves it.
    pub peer_id: String,
    pub handler_uri: String,
    pub operation: String,
    /// Optional params entity. Absent → `system/empty` Null.
    pub params: Option<Entity>,
    /// Optional resource path. Wraps into `ExecuteOptions.resource`.
    pub resource: Option<String>,
}

/// Result of one execute call. `summary` is a pre-formatted text line
/// ready to drop into an event-log row or shell scrollback; consumers
/// that want structured data read `result` directly. (`HandlerResult`
/// is neither `Debug` nor `Clone`, so the struct stays move-only.)
pub struct ExecuteResponse {
    /// Full handler result. Currently only the shell `exec` verb
    /// (Phase 4) consumes the structured payload — `handle_execute`
    /// reads `summary` only. Kept on the type so consumers can pull
    /// status / type / included data without re-parsing the summary.
    #[allow(dead_code)]
    pub result: HandlerResult,
    pub summary: String,
}

#[cfg(not(target_arch = "wasm32"))]
type OpFuture = Pin<Box<dyn Future<Output = Result<ExecuteResponse, String>> + Send>>;
#[cfg(target_arch = "wasm32")]
type OpFuture = Pin<Box<dyn Future<Output = Result<ExecuteResponse, String>>>>;

/// One L1 execute with self-healing for a **remote** target: on a remote `403`
/// (stale capability — the granter re-mints at a fresh handshake) or a
/// transport `Err` (stale pooled connection — a dead conn only surfaces here,
/// there is no liveness signal), evict + reconnect through the peer's
/// remembered listen address and re-dispatch ONCE (`BUGLOG-2026-07-14` B1;
/// AP14 — recovery must key on the failure class that actually occurs, which
/// for a stale pool is the transport `Err`, not the 403).
///
/// This is the app's one dispatch chokepoint, so every caller heals — no
/// hand-picked coverage. A local or address-less target builds no heal future
/// and passes straight through. The single retry re-sends the op: fine for
/// this tier's ops (list/read are pure; write is a full-content overwrite).
///
/// All futures are pre-built: the sync frame-loop caller can't re-borrow
/// `peers` across the reconnect await.
pub fn execute(peers: &Peers, req: ExecuteRequest) -> OpFuture {
    let heal = heal_future(peers, &req);
    let retry = heal.as_ref().map(|_| execute_once(peers, req.clone()));
    let first = execute_once(peers, req);
    Box::pin(async move {
        let resp = first.await;
        let must_heal =
            matches!(&resp, Ok(r) if r.result.status == 403) || resp.is_err();
        if must_heal {
            if let (Some(heal), Some(retry)) = (heal, retry) {
                tracing::info!(
                    "remote dispatch failed (403/transport) → evict + reconnect + retry once"
                );
                let _ = heal.await;
                return retry.await;
            }
        }
        resp
    })
}

fn execute_once(peers: &Peers, req: ExecuteRequest) -> OpFuture {
    let (params, opts) = build_params_and_opts(&req);
    let seed = AccessSeed::from(&req);
    let fut = peers.execute(&req.peer_id, req.handler_uri, req.operation, params, opts);
    Box::pin(async move {
        let outcome = fut.await;
        seed.record(&outcome);
        let result = outcome?;
        let summary = crate::format::format_handler_result(&result);
        Ok(ExecuteResponse { result, summary })
    })
}

/// Evict-and-reconnect future for `req`'s target, if it is a remote peer we
/// hold a listen address for. `None` for a local target or a peer with no
/// remembered address — nothing to reconnect through.
fn heal_future(
    peers: &Peers,
    req: &ExecuteRequest,
) -> Option<crate::peers::ConnectPeerFuture<'static>> {
    let (remote, _) = crate::access_log_store::parse_target(&req.handler_uri);
    let remote = remote?;
    let addr = peers
        .peer_metadata(&remote)?
        .listen_addresses
        .first()
        .map(|a| crate::views::peer_connections::model::rewrite_for_browser(a))?;
    Some(peers.reconnect_peer(&req.peer_id, &remote, addr))
}

/// The (actor, target, operation, resource) tuple captured *before* the request
/// routes, so it survives `req` being consumed into the execute future. On
/// completion it records one access row into the app-tier
/// [`crate::access_log_store`] for a **remote** execute — the outbound half of
/// the access log that the per-peer inspect sink structurally can't see (a
/// remote execute fires its `Dispatch` fact on the *remote* peer, so the local
/// caller only ever emits a `Wire` frame). Local dispatches (queries, counts,
/// local executes) are captured by the inspect sink instead — recording them
/// here too would double-count.
struct AccessSeed {
    actor: String,
    handler_uri: String,
    operation: String,
    resource: Option<String>,
}

impl AccessSeed {
    fn from(req: &ExecuteRequest) -> Self {
        Self {
            actor: req.peer_id.clone(),
            handler_uri: req.handler_uri.clone(),
            operation: req.operation.clone(),
            resource: req.resource.clone(),
        }
    }

    /// Fold the resolved execute outcome into an access row. A returned
    /// `HandlerResult` carries the real status (403 comes back as `Ok`, not
    /// `Err` — only transport/dispatch failures are `Err`), so allow/deny is
    /// classified honestly; an `Err` is a genuine failure → `Error`.
    fn record(self, outcome: &Result<HandlerResult, String>) {
        use crate::access_log_store::{self, AccessEntry, AccessOutcome};
        let (target_peer, handler) = access_log_store::parse_target(&self.handler_uri);
        // Remote-only: a local dispatch is captured by the inspect sink (which
        // sees its `Dispatch` fact); recording it here would double-count.
        if target_peer.is_none() {
            return;
        }
        let (outcome_kind, detail) = match outcome {
            Ok(r) => (access_log_store::classify(r.status), format!("status {}", r.status)),
            Err(e) => (AccessOutcome::Error, e.clone()),
        };
        access_log_store::global().record(AccessEntry {
            direction: access_log_store::AccessDirection::Outbound,
            actor: self.actor,
            target_peer,
            handler,
            operation: self.operation,
            resource: self.resource,
            outcome: outcome_kind,
            detail,
        });
    }
}

fn build_params_and_opts(req: &ExecuteRequest) -> (Entity, ExecuteOptions) {
    let params = req.params.clone().unwrap_or_else(|| {
        Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null))
            .expect("system/empty Null is well-formed")
    });
    let opts = match req.resource.as_deref() {
        Some(path) if !path.is_empty() => ExecuteOptions {
            resource: Some(entity_capability::ResourceTarget {
                targets: vec![path.to_string()],
                exclude: vec![],
            }),
            ..Default::default()
        },
        _ => ExecuteOptions::default(),
    };
    (params, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn execute_against_local_system_tree_get_succeeds() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let req = ExecuteRequest {
            peer_id: pid,
            handler_uri: "system/tree".into(),
            operation: "get".into(),
            params: None,
            resource: None,
        };
        let resp = execute(&peers, req)
            .await
            .expect("local system/tree get should succeed");
        assert!(!resp.summary.is_empty());
    }

    #[test]
    fn build_opts_includes_resource_when_present() {
        let req = ExecuteRequest {
            peer_id: "p".into(),
            handler_uri: "system/tree".into(),
            operation: "get".into(),
            params: None,
            resource: Some("docs/test".into()),
        };
        let (_, opts) = build_params_and_opts(&req);
        let target = opts.resource.as_ref().unwrap();
        assert_eq!(target.targets, vec!["docs/test".to_string()]);
    }

    #[test]
    fn build_opts_omits_resource_when_empty() {
        let req = ExecuteRequest {
            peer_id: "p".into(),
            handler_uri: "system/tree".into(),
            operation: "get".into(),
            params: None,
            resource: Some(String::new()),
        };
        let (_, opts) = build_params_and_opts(&req);
        assert!(opts.resource.is_none());
    }

    #[test]
    fn access_seed_records_remote_execute_only() {
        // Local dispatches are captured by the inspect sink; recording them here
        // too would double-count, so `record` must skip a bare (local) handler.
        let store = crate::access_log_store::global();
        let uniq = "op-remote-only-guard-test";
        let entity = || {
            Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null)).unwrap()
        };

        // Remote target (entity://) → recorded, with actor + target + outcome.
        AccessSeed {
            actor: "PEER_S".into(),
            handler_uri: "entity://PEER_B/local/files".into(),
            operation: uniq.into(),
            resource: Some("shared/a.txt".into()),
        }
        .record(&Ok(HandlerResult::error(403, entity())));

        // Local target (bare handler) → skipped.
        AccessSeed {
            actor: "PEER_S".into(),
            handler_uri: "system/tree".into(),
            operation: uniq.into(),
            resource: None,
        }
        .record(&Ok(HandlerResult::ok(entity())));

        let rows: Vec<_> = store
            .snapshot_newest_first()
            .into_iter()
            .filter(|e| e.operation == uniq)
            .collect();
        assert_eq!(rows.len(), 1, "only the remote execute is recorded");
        assert_eq!(rows[0].actor, "PEER_S");
        assert_eq!(rows[0].target_peer.as_deref(), Some("PEER_B"));
        assert_eq!(rows[0].resource.as_deref(), Some("shared/a.txt"));
        assert_eq!(rows[0].outcome, crate::access_log_store::AccessOutcome::Denied);
    }
}
