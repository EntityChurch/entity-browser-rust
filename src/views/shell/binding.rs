//! Egui-side adapters implementing the `entity_shell` crate's
//! `PeerBinding` + `SelectionSink` traits.
//!
//! The crate stays peer-router-agnostic; this module is the seam where
//! our multi-SDK `Peers` type meets the trait surface verbs see.

use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;

use entity_shell::{
    AppActionSink, EntityRead, PeerBinding, PeerMode, QueryMatch, QueryResults,
    SelectionSink, ShellRequest, TailInfo, TreeListingEntry,
};

use crate::peers::Peers;
use crate::window::WindowId;
use super::model::TailEntry;

/// `PeerBinding` impl backed by `&Peers`. Constructed per verb call;
/// holds borrowed references and the bound peer id.
///
/// Connected-peer enumeration goes through the app-tier
/// `connections::read_connected` registry — that is the source of
/// truth for "currently connected remotes" in this app (the SDK pool
/// is internal).
pub struct PeersBinding<'a> {
    peers: &'a Peers,
    bound_peer: &'a str,
}

impl<'a> PeersBinding<'a> {
    pub fn new(peers: &'a Peers, bound_peer: &'a str) -> Self {
        Self { peers, bound_peer }
    }
}

impl<'a> PeerBinding for PeersBinding<'a> {
    fn peer_id(&self) -> &str {
        self.bound_peer
    }

    fn primary_peer_id(&self) -> String {
        self.peers.primary_peer_id().to_string()
    }

    fn peer_ids(&self) -> Vec<String> {
        self.peers.peer_ids()
    }

    fn connected_peers(&self) -> Vec<String> {
        crate::connections::read_connected(self.peers)
    }

    fn peer_label(&self, peer_id: &str) -> Option<String> {
        self.peers
            .peer_metadata(peer_id)
            .and_then(|m| m.label)
    }

    fn tree_listing(&self, peer_id: &str, prefix: &str) -> Vec<TreeListingEntry> {
        self.peers
            .tree_listing(peer_id, prefix)
            .into_iter()
            .map(|e| TreeListingEntry { path: e.path })
            .collect()
    }

    fn get_entity(&self, peer_id: &str, path: &str) -> Option<EntityRead> {
        self.peers
            .get_entity(peer_id, path)
            .map(|e| EntityRead {
                entity_type: e.entity_type,
                data: e.data,
                content_hash: e.content_hash.to_string(),
            })
    }

    fn primary_arm(&self) -> &'static str {
        if self.peers.primary_as_direct().is_some() {
            "Direct"
        } else {
            "Worker"
        }
    }

    fn remove_connection(&self, peer_id: &str) {
        let connections = crate::connections::ConnectionsWriter::new(self.peers);
        connections.remove(peer_id);
    }

    fn connect_peer(
        &self,
        from_peer: &str,
        address: String,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<String, String>> {
        // Wrap `Peers::connect_peer` so success updates the local
        // connections registry before returning the remote pid (the
        // existing verb did this inline; crate-side connect verb just
        // forwards the success).
        let connect_fut = self.peers.connect_peer(from_peer, address);
        let connections = crate::connections::ConnectionsWriter::new(self.peers);
        Box::pin(async move {
            match connect_fut.await {
                Ok(remote_pid) => {
                    connections.add(&remote_pid);
                    Ok(remote_pid)
                }
                Err(e) => Err(e),
            }
        })
    }

    fn put_entity(
        &self,
        peer_id: &str,
        path: &str,
        entity_type: &str,
        params_text: Option<String>,
    ) -> Result<(), String> {
        // Body: parsed JSON → CBOR bytes (embedding owns the parser).
        // Empty body becomes a CBOR null.
        let data = match params_text {
            Some(text) => super::model::parse_json_to_ecf(&text)
                .map_err(|e| format!("invalid JSON body: {}", e))?,
            None => entity_ecf::to_ecf(&entity_ecf::Value::Null),
        };
        let entity = entity_entity::Entity::new(entity_type, data)
            .map_err(|e| format!("entity construction failed: {}", e))?;
        self.peers.dispatch_write(peer_id, path.to_string(), entity);
        Ok(())
    }

    fn remove_entity(&self, peer_id: &str, path: &str) {
        self.peers.dispatch_remove(peer_id, path.to_string());
    }

    fn query(
        &self,
        peer_id: &str,
        type_filter: &str,
        limit: usize,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<QueryResults, String>> {
        let expr = crate::views::query_console::model::build_expression_from_fields(
            type_filter,
            "",
            "",
            "",
            &limit.to_string(),
            false,
        );
        let fut = self.peers.query(peer_id, expr);
        Box::pin(async move {
            match fut.await {
                Ok(results) => Ok(QueryResults {
                    matches: results
                        .matches
                        .into_iter()
                        .map(|m| QueryMatch {
                            path: m.path,
                            entity_type: m.entity_type,
                        })
                        .collect(),
                    total: results.total as usize,
                    has_more: results.has_more,
                }),
                Err(e) => Err(e),
            }
        })
    }

    fn count(
        &self,
        peer_id: &str,
        type_filter: &str,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<usize, String>> {
        let expr = crate::views::query_console::model::build_expression_from_fields(
            type_filter, "", "", "", "", false,
        );
        let fut = self.peers.count(peer_id, expr);
        Box::pin(async move {
            match fut.await {
                Ok(n) => Ok(n as usize),
                Err(e) => Err(e),
            }
        })
    }

    fn execute(
        &self,
        peer_id: &str,
        handler_uri: String,
        operation: String,
        params_text: Option<String>,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<String, String>> {
        // Parse JSON params here (embedding-side, per the crate's
        // PeerBinding::execute contract). Failures become user-facing
        // strings; the verb wraps these in a `Dispatch::Failed` chunk.
        let params = match params_text {
            None => None,
            Some(text) => match super::model::parse_json_params(&text) {
                Ok(entity) => Some(entity),
                Err(e) => {
                    return Box::pin(async move { Err(format!("invalid JSON params: {}", e)) });
                }
            },
        };
        let req = crate::ops::ExecuteRequest {
            peer_id: peer_id.to_string(),
            handler_uri,
            operation,
            params,
            resource: None,
        };
        let fut = crate::ops::execute(self.peers, req);
        Box::pin(async move {
            match fut.await {
                Ok(resp) => Ok(resp.summary),
                Err(e) => Err(e),
            }
        })
    }

    // -------------------------------------------------------------------
    // Compute wire-up — per the core-Rust shell-verb guide.
    // Arm-agnostic: all five verbs route through the `Peers` L1 router
    // (`execute`/`query`/`get_entity_async`), so they work on a Worker-arm
    // peer (durable default) as well as Direct. The compute wire contract
    // (param/opts build + result decode) lives in `entity_sdk::compute` —
    // the same source of truth the typed Direct-arm `ComputeOps` uses, so
    // there is no app-side reimplementation of the decode. eval/install/
    // uninstall dispatch EXECUTE; list runs an L1 query for the subgraph
    // metadata; show reads the one metadata entity on demand.
    // -------------------------------------------------------------------

    fn compute_eval(
        &self,
        peer_id: &str,
        expr_path: String,
        budget: Option<u64>,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<String, String>> {
        let (params, opts) =
            entity_sdk::compute::eval_request(expr_path, entity_sdk::EvalOptions { budget });
        let fut = self.peers.execute(
            peer_id,
            entity_sdk::compute::HANDLER.to_string(),
            entity_sdk::compute::OP_EVAL.to_string(),
            params,
            opts,
        );
        Box::pin(async move {
            let result = fut.await?;
            let eval = entity_sdk::compute::finish_eval(result).map_err(|e| e.to_string())?;
            Ok(format_compute_eval_result(eval))
        })
    }

    fn compute_install(
        &self,
        peer_id: &str,
        root_expression_path: String,
        result_path: Option<String>,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<(String, String), String>> {
        let (params, opts) = entity_sdk::compute::install_request(
            root_expression_path,
            entity_sdk::InstallOptions { result_path },
        );
        let fut = self.peers.execute(
            peer_id,
            entity_sdk::compute::HANDLER.to_string(),
            entity_sdk::compute::OP_INSTALL.to_string(),
            params,
            opts,
        );
        Box::pin(async move {
            let result = fut.await?;
            let r = entity_sdk::compute::finish_install(result).map_err(|e| e.to_string())?;
            Ok((r.subgraph_path, r.result_path))
        })
    }

    fn compute_uninstall(
        &self,
        peer_id: &str,
        subgraph_path: String,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<(), String>> {
        let (params, opts) = entity_sdk::compute::uninstall_request(subgraph_path);
        let fut = self.peers.execute(
            peer_id,
            entity_sdk::compute::HANDLER.to_string(),
            entity_sdk::compute::OP_UNINSTALL.to_string(),
            params,
            opts,
        );
        Box::pin(async move {
            let result = fut.await?;
            entity_sdk::compute::finish_uninstall(result).map_err(|e| e.to_string())
        })
    }

    fn compute_list(
        &self,
        peer_id: &str,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<Vec<(String, String, String)>, String>>
    {
        // Worker-arm `list` can't sync-read the store, so resolve it as an
        // L1 query for the subgraph metadata entities the install handler
        // writes under `system/compute/processes/`. `include_entities`
        // brings each metadata entity back inline (one round-trip) so we
        // decode via the SDK's `decode_subgraph_entity` — same parse the
        // Direct-arm `ComputeOps::list` uses.
        let prefix = format!("/{}/{}", peer_id, entity_sdk::compute::PROCESSES_PREFIX);
        let expr = crate::views::query_console::model::build_expression_from_fields(
            "system/compute/subgraph", // type_filter
            &prefix,                   // path_prefix
            "",                        // ref_filter
            "",                        // path_filter
            "1000",                    // limit
            true,                      // include_entities
        );
        let fut = self.peers.query(peer_id, expr);
        Box::pin(async move {
            let results = fut.await?;
            let rows = results
                .matches
                .into_iter()
                .filter_map(|m| {
                    let entity = m.entity?;
                    let s = entity_sdk::compute::decode_subgraph_entity(&m.path, &entity)?;
                    Some((s.subgraph_path, s.root_expression_path, s.status))
                })
                .collect();
            Ok(rows)
        })
    }

    fn compute_show(
        &self,
        peer_id: &str,
        subgraph_path: String,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<Option<Vec<(String, String)>>, String>>
    {
        let qualified = if subgraph_path.starts_with('/') {
            subgraph_path
        } else {
            format!("/{}/{}", peer_id, subgraph_path)
        };
        let fut = self.peers.get_entity_async(peer_id, &qualified);
        Box::pin(async move {
            let Some(entity) = fut.await? else {
                return Ok(None);
            };
            let Some(s) = entity_sdk::compute::decode_subgraph_entity(&qualified, &entity) else {
                return Ok(None);
            };
            Ok(Some(vec![
                ("subgraph".into(), s.subgraph_path),
                ("root expression".into(), s.root_expression_path),
                ("result path".into(), s.result_path),
                ("status".into(), s.status),
                ("installed by".into(), short_hash(&s.installed_by)),
                (
                    "installation grant".into(),
                    short_hash(&s.installation_grant),
                ),
            ]))
        })
    }

    fn bootstrap_identity(
        &self,
        peer_id: &str,
        threshold: usize,
        label: Option<String>,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<Vec<(String, String)>, String>>
    {
        let ctx = match self.peers.direct_peer_context(peer_id) {
            Ok(c) => c,
            Err(_) => {
                return Box::pin(async move {
                    Err("bootstrap: not supported on Worker-arm peer".into())
                })
            }
        };
        let opts = entity_sdk::BootstrapOptions {
            quorum_threshold: threshold,
            label,
            ..entity_sdk::BootstrapOptions::default()
        };
        let fut = ctx.identity().bootstrap(opts);
        Box::pin(async move {
            fut.await
                .map(format_bootstrap_result)
                .map_err(|e| e.to_string())
        })
    }

    fn bootstrap_status(&self, peer_id: &str) -> Vec<(String, String)> {
        let Ok(ctx) = self.peers.direct_peer_context(peer_id) else {
            return Vec::new();
        };
        let s = ctx.identity().bootstrap_status();
        let mut rows = vec![
            ("bootstrapped".into(), s.bootstrapped.to_string()),
            ("identity".into(), short_hash(&s.identity_hash)),
        ];
        if let Some(q) = s.quorum_id {
            rows.push(("quorum".into(), short_hash(&q)));
        }
        if let Some(p) = s.peer_config_path {
            rows.push(("peer config".into(), p));
        }
        rows
    }

    fn export_identity_bundle(&self, peer_id: &str) -> Result<Vec<u8>, String> {
        let ctx = self
            .peers
            .direct_peer_context(peer_id)
            .map_err(|_| "bootstrap export: not supported on Worker-arm peer".to_string())?;
        let bundle = ctx.identity().export_bundle().map_err(|e| e.to_string())?;
        bundle.to_cbor().map_err(|e| e.to_string())
    }

    fn restore_identity_bundle(
        &self,
        peer_id: &str,
        bundle_cbor: Vec<u8>,
    ) -> entity_shell::runtime::BoxFuture<'static, Result<Vec<(String, String)>, String>>
    {
        let bundle = match entity_sdk::IdentityBundle::from_cbor(&bundle_cbor) {
            Ok(b) => b,
            Err(e) => {
                return Box::pin(async move { Err(format!("bundle decode: {}", e)) })
            }
        };
        let ctx = match self.peers.direct_peer_context(peer_id) {
            Ok(c) => c,
            Err(_) => {
                return Box::pin(async move {
                    Err("bootstrap import: not supported on Worker-arm peer".into())
                })
            }
        };
        let fut = ctx.identity().restore_from_bundle(&bundle);
        Box::pin(async move {
            fut.await
                .map(format_bootstrap_result)
                .map_err(|e| e.to_string())
        })
    }
}

fn short_hash(h: &entity_hash::Hash) -> String {
    let hex = hex_encode(h.to_bytes());
    if hex.len() >= 8 {
        format!("{}…{}", &hex[..4], &hex[hex.len() - 4..])
    } else {
        hex
    }
}

fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn format_compute_eval_result(r: entity_sdk::ComputeEvalResult) -> String {
    use entity_sdk::ComputeValue;
    let v = match &r.value {
        ComputeValue::Null => "Null".to_string(),
        ComputeValue::Bool(b) => format!("Bool({})", b),
        ComputeValue::Int(n) => format!("Int({})", n),
        ComputeValue::Uint(n) => format!("Uint({})", n),
        ComputeValue::Float(f) => format!("Float({})", f),
        ComputeValue::Bytes(b) => format!("Bytes({} bytes)", b.len()),
        ComputeValue::Text(s) => format!("Text({:?})", s),
        ComputeValue::Hash(h) => format!("Hash({})", short_hash(h)),
        ComputeValue::Array(a) => format!("Array({} elems)", a.len()),
        ComputeValue::Map(m) => format!("Map({} pairs)", m.len()),
        ComputeValue::Entity(e) => format!("Entity({})", e.entity_type),
        ComputeValue::Closure(e) => format!("Closure({})", e.entity_type),
        ComputeValue::Error(e) => format!("Error({})", e.entity_type),
    };
    format!(
        "  value: {}\n  entity type: {}",
        v, r.result_entity.entity_type
    )
}

fn format_bootstrap_result(r: entity_sdk::BootstrapResult) -> Vec<(String, String)> {
    use entity_sdk::BootstrapResult::*;
    match r {
        AlreadyBootstrapped { identity_hash, quorum_id } => vec![
            ("status".into(), "already bootstrapped".into()),
            ("identity".into(), short_hash(&identity_hash)),
            ("quorum".into(), short_hash(&quorum_id)),
        ],
        Bootstrapped {
            identity_hash,
            quorum_id,
            controller_cert,
            peer_config_path,
            issued_caps,
        } => {
            let mut rows = vec![
                ("status".into(), "bootstrapped".into()),
                ("identity".into(), short_hash(&identity_hash)),
                ("quorum".into(), short_hash(&quorum_id)),
                ("controller cert".into(), short_hash(&controller_cert)),
                ("peer config".into(), peer_config_path),
            ];
            if !issued_caps.is_empty() {
                rows.push((
                    "issued caps".into(),
                    issued_caps
                        .iter()
                        .map(short_hash)
                        .collect::<Vec<_>>()
                        .join(", "),
                ));
            }
            rows
        }
    }
}

/// `SelectionSink` impl that publishes through the app's
/// `selection::publish_entity_selection` (writes both per-panel and
/// app-aggregate slots).
pub struct PanelSelectionSink<'a> {
    peers: &'a Peers,
    peer_id: &'a str,
    window_id: WindowId,
}

impl<'a> PanelSelectionSink<'a> {
    pub fn new(peers: &'a Peers, peer_id: &'a str, window_id: WindowId) -> Self {
        Self { peers, peer_id, window_id }
    }
}

impl<'a> SelectionSink for PanelSelectionSink<'a> {
    fn publish(&self, path: &str) {
        crate::selection::publish_entity_selection(
            self.peers,
            self.peer_id,
            self.window_id,
            path,
        );
    }
}

/// `AppActionSink` impl that pushes `ShellRequest` variants onto the
/// shell window's `pending_out` queue as the embedding-side `Action`
/// variants. Also exposes the WINDOW_TYPES catalog for `open` and
/// reads `ShellModel::tails` for `tails`.
pub struct ShellActionSink<'a> {
    pending: &'a Mutex<Vec<crate::action::Action>>,
    window_id: WindowId,
    tails_handle: Arc<Mutex<Vec<TailEntry>>>,
}

impl<'a> ShellActionSink<'a> {
    pub fn new(
        pending: &'a Mutex<Vec<crate::action::Action>>,
        window_id: WindowId,
        tails_handle: Arc<Mutex<Vec<TailEntry>>>,
    ) -> Self {
        Self { pending, window_id, tails_handle }
    }
}

impl<'a> AppActionSink for ShellActionSink<'a> {
    fn submit(&self, request: ShellRequest) {
        let action = match request {
            ShellRequest::SpawnWindow { type_name, peer_id } => {
                // SpawnWindow's `type_name` is `&'static str` — look
                // up the crate-resolved name against our static list.
                let resolved = resolve_static_window_name(&type_name)
                    .unwrap_or("Shell");
                crate::action::Action::SpawnWindow {
                    type_name: resolved,
                    peer_id,
                }
            }
            ShellRequest::CreatePeer { mode, label } => {
                let app_mode = match mode {
                    PeerMode::Frontend => crate::peer_mode::PeerMode::Frontend,
                    PeerMode::BackendMemory => crate::peer_mode::PeerMode::BackendMemory,
                    PeerMode::BackendOpfs => crate::peer_mode::PeerMode::BackendOpfs,
                };
                crate::action::Action::CreatePeerWithMode { label, mode: app_mode }
            }
            ShellRequest::DeletePeer { peer_id } => {
                crate::action::Action::DeletePeer(peer_id)
            }
            ShellRequest::RenamePeer { peer_id, label } => {
                crate::action::Action::RenamePeer { peer_id, label }
            }
            ShellRequest::InstallTail { prefix } => crate::action::Action::ShellTail {
                window_id: self.window_id,
                prefix,
            },
            ShellRequest::UninstallTail { target } => {
                // Flip active flags inline on the tails list; no
                // app-tier Action is needed because subscription
                // teardown is signaled by the atomic the callback
                // observes.
                let entries = self.tails_handle.lock().unwrap();
                for t in entries.iter() {
                    if target == "all" || t.prefix == target {
                        t.active.store(false, Ordering::Relaxed);
                    }
                }
                return;
            }
        };
        self.pending.lock().unwrap().push(action);
    }

    fn list_tails(&self) -> Vec<TailInfo> {
        self.tails_handle
            .lock()
            .unwrap()
            .iter()
            .map(|t| TailInfo {
                prefix: t.prefix.clone(),
                active: t.active.load(Ordering::Relaxed),
            })
            .collect()
    }

    fn available_windows(&self) -> Vec<String> {
        super::model::WINDOW_TYPES.iter().map(|s| s.to_string()).collect()
    }

    fn resolve_window_name(&self, input: &str) -> Option<String> {
        super::model::resolve_window_name(input).map(|s| s.to_string())
    }
}

fn resolve_static_window_name(name: &str) -> Option<&'static str> {
    super::model::WINDOW_TYPES.iter().copied().find(|s| *s == name)
}
