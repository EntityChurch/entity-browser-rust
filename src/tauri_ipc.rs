//! Tauri IPC bridge — WASM-only module for calling Tauri backend commands.
//!
//! Detects the `__TAURI__` JS global and invokes backend commands for
//! peer lifecycle management. Only compiled on wasm32.

use wasm_bindgen::JsValue;
use wasm_bindgen::JsCast;

/// Response from backend peer IPC commands.
#[derive(Debug, Clone)]
pub struct BackendPeerInfo {
    pub peer_id: String,
    pub label: Option<String>,
    /// Lifecycle state from backend ("running"/"stopped"/etc). Mirrored from
    /// the IPC contract; not yet surfaced in the Rust UI.
    #[allow(dead_code)]
    pub status: String,
    pub ws_addr: Option<String>,
    /// Whether this backend is serving `system/signaling` **right now** — i.e.
    /// whether browsers can rendezvous through it.
    ///
    /// Read from the running peer, not from the persisted setting, because the
    /// handler mounts at build time: a peer started before the toggle was
    /// flipped is configured to serve and is not serving. Reporting the setting
    /// here would put a "Rendezvous: on" row above a node that answers 404.
    pub signaling_node: bool,
    /// The internet-reachable `ws://` address the router is forwarding to this
    /// backend, when a door is actually open.
    ///
    /// **`None` is four different situations, not one** — nobody asked, still
    /// probing, the router refused, or it granted an address behind CGNAT that
    /// nobody can reach. Which one lives in [`Self::port_mapping_note`]; this
    /// is only ever an address that works right now, and it is dropped the
    /// moment a renewal fails.
    pub external_addr: Option<String>,
    /// Whether a lease is being maintained at all. Distinct from
    /// `external_addr` being `Some`: on most routers asking and getting nothing
    /// is the ordinary outcome, and that is not the same as not asking.
    pub port_mapping: bool,
    /// Why there is no external address, already written for a person.
    pub port_mapping_note: Option<String>,
}

impl BackendPeerInfo {
    /// Collect non-None addresses into a Vec for PeerMetadata.
    pub fn listen_addresses(&self) -> Vec<String> {
        let mut addrs = Vec::new();
        if let Some(ref ws) = self.ws_addr {
            addrs.push(ws.clone());
        }
        addrs
    }

    fn from_js(result: &JsValue) -> Option<Self> {
        Some(Self {
            peer_id: get_string(result, "peer_id")?,
            label: get_string(result, "label"),
            status: get_string(result, "status").unwrap_or_else(|| "unknown".into()),
            ws_addr: get_string(result, "ws_addr"),
            // Absent reads as false — an older backend that does not send the
            // field is not serving rendezvous, which is the truthful answer.
            signaling_node: js_sys::Reflect::get(result, &JsValue::from_str("signaling_node"))
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            external_addr: get_string(result, "external_addr"),
            // Absent reads as false for the same reason: a backend that does
            // not send the field is not asking any router for anything.
            port_mapping: js_sys::Reflect::get(result, &JsValue::from_str("port_mapping"))
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            port_mapping_note: get_string(result, "port_mapping_note"),
        })
    }
}

/// Check if we're running inside a Tauri WebView.
pub fn is_tauri() -> bool {
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &JsValue::from_str("__TAURI__")).ok())
        .map(|v| !v.is_undefined() && !v.is_null())
        .unwrap_or(false)
}

/// Call a Tauri IPC command and return the JS result.
async fn invoke(cmd: &str, args: &JsValue) -> Result<JsValue, String> {
    let window = web_sys::window().ok_or("no window object")?;
    let tauri = js_sys::Reflect::get(&window, &JsValue::from_str("__TAURI__"))
        .map_err(|_| "no __TAURI__ global")?;
    let core = js_sys::Reflect::get(&tauri, &JsValue::from_str("core"))
        .map_err(|_| "no __TAURI__.core")?;
    let invoke_fn = js_sys::Reflect::get(&core, &JsValue::from_str("invoke"))
        .map_err(|_| "no __TAURI__.core.invoke")?;
    let invoke_fn = invoke_fn
        .dyn_ref::<js_sys::Function>()
        .ok_or("invoke is not a function")?;

    let promise = invoke_fn
        .call2(&core, &JsValue::from_str(cmd), args)
        .map_err(|e| format!("invoke({}) call failed: {:?}", cmd, e))?;
    let promise = js_sys::Promise::from(promise);

    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("invoke({}) rejected: {:?}", cmd, e))
}

/// Get a string field from a JS object, returning None if missing or not a string.
fn get_string(obj: &JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(obj, &JsValue::from_str(key))
        .ok()
        .and_then(|v| v.as_string())
}

/// Create a backend peer via Tauri IPC. Returns stopped (not started).
pub async fn create_backend_peer(label: Option<String>) -> Result<BackendPeerInfo, String> {
    let args = js_sys::Object::new();
    if let Some(ref l) = label {
        js_sys::Reflect::set(&args, &JsValue::from_str("label"), &JsValue::from_str(l))
            .map_err(|_| "failed to set label arg")?;
    }
    let result = invoke("create_backend_peer", &args.into()).await?;
    BackendPeerInfo::from_js(&result).ok_or("invalid create response".into())
}

/// Start a stopped backend peer — boots Peer + WS listener.
pub async fn start_backend_peer(
    peer_id: &str,
    manager_peer_id: &str,
) -> Result<BackendPeerInfo, String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(&args, &JsValue::from_str("peerId"), &JsValue::from_str(peer_id))
        .map_err(|_| "failed to set peerId arg")?;
    // The managing system peer (S) — B self-seeds its manager grant from this.
    js_sys::Reflect::set(
        &args,
        &JsValue::from_str("managerPeerId"),
        &JsValue::from_str(manager_peer_id),
    )
    .map_err(|_| "failed to set managerPeerId arg")?;
    let result = invoke("start_backend_peer", &args.into()).await?;
    BackendPeerInfo::from_js(&result).ok_or("invalid start response".into())
}

/// Stop a running backend peer — keeps persisted identity.
pub async fn stop_backend_peer(peer_id: &str) -> Result<BackendPeerInfo, String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(&args, &JsValue::from_str("peerId"), &JsValue::from_str(peer_id))
        .map_err(|_| "failed to set peerId arg")?;
    let result = invoke("stop_backend_peer", &args.into()).await?;
    BackendPeerInfo::from_js(&result).ok_or("invalid stop response".into())
}

/// Turn this backend's §6.5 rendezvous on or off, persistently.
///
/// **Restarts the peer when it is running** — `system/signaling` mounts on
/// `PeerBuilder`, so there is no way to add a handler to a live peer. The
/// returned info reports what is actually being served afterwards, so a caller
/// never has to assume the flip took.
pub async fn set_backend_signaling_node(
    peer_id: &str,
    enabled: bool,
) -> Result<BackendPeerInfo, String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(&args, &JsValue::from_str("peerId"), &JsValue::from_str(peer_id))
        .map_err(|_| "failed to set peerId arg")?;
    js_sys::Reflect::set(
        &args,
        &JsValue::from_str("enabled"),
        &JsValue::from_bool(enabled),
    )
    .map_err(|_| "failed to set enabled arg")?;
    let result = invoke("set_backend_signaling_node", &args.into()).await?;
    BackendPeerInfo::from_js(&result).ok_or("invalid set_backend_signaling_node response".into())
}

/// Ask the router to forward this backend's port, or stop asking. Persistent.
///
/// **Restarts the peer**, like the rendezvous toggle and for one more reason:
/// the lease is bound to the port this run actually bound, so a toggle taking
/// effect live would have to reason about a listener that may since have moved.
/// Turning it off releases the mapping on the way through.
///
/// The returned info reports the state afterwards — but **not the answer**: the
/// probe has only just started, so `external_addr` is `None` and the real
/// answer arrives on the next status poll.
pub async fn set_backend_port_mapping(
    peer_id: &str,
    enabled: bool,
) -> Result<BackendPeerInfo, String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(&args, &JsValue::from_str("peerId"), &JsValue::from_str(peer_id))
        .map_err(|_| "failed to set peerId arg")?;
    js_sys::Reflect::set(
        &args,
        &JsValue::from_str("enabled"),
        &JsValue::from_bool(enabled),
    )
    .map_err(|_| "failed to set enabled arg")?;
    let result = invoke("set_backend_port_mapping", &args.into()).await?;
    BackendPeerInfo::from_js(&result).ok_or("invalid set_backend_port_mapping response".into())
}

/// Delete a backend peer entirely — stops + removes from disk.
pub async fn delete_backend_peer(peer_id: &str) -> Result<(), String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(&args, &JsValue::from_str("peerId"), &JsValue::from_str(peer_id))
        .map_err(|_| "failed to set peerId arg")?;
    invoke("delete_backend_peer", &args.into()).await?;
    Ok(())
}

/// Auto-provision the canonical system backend peer (B) and return it, started
/// and listening. Idempotent server-side (stable `system-backend` identity), so
/// the WebView (S) calls it unconditionally at boot. `manager_peer_id` is S —
/// B self-seeds S's manager grant. Replaces the manual create → start dance.
/// See `DESIGN-SYSTEM-BACKEND-PEER.md` §10.
pub async fn ensure_system_backend(manager_peer_id: &str) -> Result<BackendPeerInfo, String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(
        &args,
        &JsValue::from_str("managerPeerId"),
        &JsValue::from_str(manager_peer_id),
    )
    .map_err(|_| "failed to set managerPeerId arg")?;
    let result = invoke("ensure_system_backend", &args.into()).await?;
    BackendPeerInfo::from_js(&result).ok_or("invalid ensure_system_backend response".into())
}

/// The backend's shared-files directory on disk (behind the `local/files/shared/`
/// tree prefix). Shown in the System Backend window so the operator knows where
/// shared files actually live. `None` if the share root couldn't be resolved.
pub async fn system_backend_share_path() -> Result<Option<String>, String> {
    let result = invoke("system_backend_share_path", &JsValue::undefined()).await?;
    Ok(result.as_string())
}

/// Native-store stats for the system backend, so the Storage window can surface
/// B's on-disk store (it otherwise sees only the frontend arms — B is a remote
/// peer over the pool). `entity_count`/`path_count` are `None` when B is stopped;
/// `sqlite_bytes` is the on-disk file size regardless. `None` if no backend.
#[derive(Debug, Clone, Default)]
pub struct BackendStoreStats {
    pub peer_id: String,
    pub running: bool,
    pub sqlite_bytes: Option<u64>,
    pub entity_count: Option<u64>,
    pub path_count: Option<u64>,
}

pub async fn system_backend_store_stats() -> Result<Option<BackendStoreStats>, String> {
    let result = invoke("system_backend_store_stats", &JsValue::undefined()).await?;
    if result.is_null() || result.is_undefined() {
        return Ok(None);
    }
    let num = |key: &str| {
        js_sys::Reflect::get(&result, &JsValue::from_str(key))
            .ok()
            .and_then(|v| v.as_f64())
            .map(|f| f as u64)
    };
    Ok(Some(BackendStoreStats {
        peer_id: get_string(&result, "peer_id").unwrap_or_default(),
        running: js_sys::Reflect::get(&result, &JsValue::from_str("running"))
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        sqlite_bytes: num("sqlite_bytes"),
        entity_count: num("entity_count"),
        path_count: num("path_count"),
    }))
}

/// Set the backend's `tracing` level at runtime (`off`/`error`/…/`trace`) from
/// the System Backend window's level control.
pub async fn set_backend_log_level(level: &str) -> Result<(), String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(
        &args,
        &JsValue::from_str("level"),
        &JsValue::from_str(level),
    )
    .map_err(|_| "failed to set level arg")?;
    invoke("set_backend_log_level", &args.into()).await?;
    Ok(())
}

/// The backend's current `tracing` level, so the window can pre-select its level
/// control on open.
pub async fn get_backend_log_level() -> Result<String, String> {
    let result = invoke("get_backend_log_level", &JsValue::undefined()).await?;
    Ok(result.as_string().unwrap_or_default())
}

/// Result of a `backend_log_tail` poll: the new log lines (text only — the
/// server's per-line `seq` is redundant for the consumer, which advances by
/// `cursor`) plus the cursor to poll with next time (`{ lines, cursor }`).
#[derive(Debug, Clone, Default)]
pub struct BackendLogTail {
    pub lines: Vec<String>,
    pub cursor: u64,
}

/// Tail the backend peer's captured `tracing` output for the System Backend
/// window's live log stream. `after` is the last cursor seen (0 for a fresh
/// poll); pass back the returned `cursor` each time. In-memory only server-side.
pub async fn backend_log_tail(after: u64) -> Result<BackendLogTail, String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(
        &args,
        &JsValue::from_str("after"),
        &JsValue::from_f64(after as f64),
    )
    .map_err(|_| "failed to set after arg")?;
    let result = invoke("backend_log_tail", &args.into()).await?;
    let cursor = js_sys::Reflect::get(&result, &JsValue::from_str("cursor"))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(after as f64) as u64;
    let mut lines = Vec::new();
    if let Some(arr) = js_sys::Reflect::get(&result, &JsValue::from_str("lines"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Array>().ok())
    {
        for entry in arr.iter() {
            if let Some(text) = get_string(&entry, "text") {
                lines.push(text);
            }
        }
    }
    Ok(BackendLogTail { lines, cursor })
}

/// One inbound access row from the native backend: who called, what they hit,
/// the operation, and the handler exit status (for allow/deny classification).
#[derive(Debug, Clone)]
pub struct InboundAccess {
    pub caller: String,
    pub target_uri: String,
    pub operation: String,
    pub status: u32,
}

/// Result of a `backend_access_log_tail` poll: new inbound access rows plus the
/// cursor to poll with next (`{ records, cursor }`). Mirrors `backend_log_tail`.
#[derive(Debug, Clone, Default)]
pub struct BackendAccessTail {
    pub records: Vec<InboundAccess>,
    pub cursor: u64,
}

/// Tail the backend peer's inbound access log ("who reached into my share").
/// `after` is the last cursor seen (0 for a fresh poll); pass back the returned
/// `cursor` each time. In-memory only server-side.
pub async fn backend_access_log_tail(after: u64) -> Result<BackendAccessTail, String> {
    let args = js_sys::Object::new();
    js_sys::Reflect::set(
        &args,
        &JsValue::from_str("after"),
        &JsValue::from_f64(after as f64),
    )
    .map_err(|_| "failed to set after arg")?;
    let result = invoke("backend_access_log_tail", &args.into()).await?;
    let cursor = js_sys::Reflect::get(&result, &JsValue::from_str("cursor"))
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(after as f64) as u64;
    let mut records = Vec::new();
    if let Some(arr) = js_sys::Reflect::get(&result, &JsValue::from_str("records"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Array>().ok())
    {
        for entry in arr.iter() {
            let caller = get_string(&entry, "caller").unwrap_or_default();
            let target_uri = get_string(&entry, "target_uri").unwrap_or_default();
            let operation = get_string(&entry, "operation").unwrap_or_default();
            let status = js_sys::Reflect::get(&entry, &JsValue::from_str("status"))
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as u32;
            records.push(InboundAccess { caller, target_uri, operation, status });
        }
    }
    Ok(BackendAccessTail { records, cursor })
}

/// List all managed backend peers (running + stopped).
pub async fn list_backend_peers() -> Result<Vec<BackendPeerInfo>, String> {
    let result = invoke("list_backend_peers", &JsValue::undefined()).await?;
    let array = result.dyn_ref::<js_sys::Array>()
        .ok_or("list response is not an array")?;
    let mut peers = Vec::new();
    for i in 0..array.length() {
        if let Some(info) = BackendPeerInfo::from_js(&array.get(i)) {
            peers.push(info);
        }
    }
    Ok(peers)
}
