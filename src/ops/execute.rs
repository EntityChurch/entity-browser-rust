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
pub fn execute(
    peers: &Peers,
    req: ExecuteRequest,
) -> Pin<Box<dyn Future<Output = Result<ExecuteResponse, String>> + Send>> {
    let (params, opts) = build_params_and_opts(&req);
    let fut = peers.execute(&req.peer_id, req.handler_uri, req.operation, params, opts);
    Box::pin(async move {
        let result = fut.await?;
        let summary = crate::format::format_handler_result(&result);
        Ok(ExecuteResponse { result, summary })
    })
}

#[cfg(target_arch = "wasm32")]
pub fn execute(
    peers: &Peers,
    req: ExecuteRequest,
) -> Pin<Box<dyn Future<Output = Result<ExecuteResponse, String>>>> {
    let (params, opts) = build_params_and_opts(&req);
    let fut = peers.execute(&req.peer_id, req.handler_uri, req.operation, params, opts);
    Box::pin(async move {
        let result = fut.await?;
        let summary = crate::format::format_handler_result(&result);
        Ok(ExecuteResponse { result, summary })
    })
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
}
