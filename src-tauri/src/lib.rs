use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use entity_crypto::Keypair;
use entity_peer::{Peer, PeerBuilder, PeerConfig, PeerShared};
use entity_peer::transport::{Connector, WebSocketConnector, WebSocketListener, Listener};
use serde::Serialize;
use tauri::Manager;

mod access_log;
mod backend_log;
mod manager_grant;
mod persistence;

/// Best-effort detection of this host's primary LAN IP — the address
/// another device on the same network (e.g. a phone being paired) uses
/// to reach this machine.
///
/// Uses the standard "connect a UDP socket toward a public address and
/// read back the local interface the OS picked" trick: UDP `connect`
/// only sets the socket's default route, so **nothing is transmitted**,
/// and it needs no external crate (supply-chain conscious). Loopback /
/// unspecified results are rejected so we never advertise an
/// unreachable address. Returns `None` when the host is offline.
fn local_lan_ip() -> Option<std::net::IpAddr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // 8.8.8.8 is only a routing hint; no packet leaves the machine.
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_loopback() || ip.is_unspecified() {
        None
    } else {
        Some(ip)
    }
}

/// Convert a listener-bound address (which may use the wildcard
/// `0.0.0.0` / `[::]` for "all interfaces") into one that's actually
/// connectable from a client — wildcards are valid to listen on but a
/// browser rejects them as a connect target.
///
/// We substitute the host's **LAN IP** so the reported address is
/// reachable both from the local WebView and from another device on the
/// network (phone pairing). Only when LAN detection fails (host offline)
/// do we fall back to loopback, which still works for the same-machine
/// WebView.
fn connectable_addr(listen_addr: &str) -> String {
    let sub = local_lan_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    listen_addr
        .replace("ws://0.0.0.0:", &format!("ws://{sub}:"))
        .replace("ws://[::]:", &format!("ws://{sub}:"))
        .replace("tcp://0.0.0.0:", &format!("tcp://{sub}:"))
        .replace("tcp://[::]:", &format!("tcp://{sub}:"))
}

/// Tree prefix the backend peer exposes its shared filesystem root at.
/// A connected peer (e.g. a paired phone) lists/reads under
/// `entity://{backend}/local/files/shared/…`. See
/// `docs/architecture/reviews/DESIGN-CROSS-DEVICE-FILE-TRANSFER.md` §7.
const SHARE_PREFIX: &str = "local/files/shared/";

/// Label of the canonical, auto-provisioned system backend peer (B). Exactly
/// one exists per install; it is created idempotently at boot, is un-deletable
/// (mirrors the frontend system peer — S can minimize away but B stays), and is
/// the deterministic File Transfer / pairing target. Additional backend peers
/// created via `create_backend_peer` are advanced and carry other labels.
/// See `docs/architecture/reviews/DESIGN-SYSTEM-BACKEND-PEER.md` §10.
const SYSTEM_BACKEND_LABEL: &str = "system-backend";

/// Ensure the demo share directory exists and always has at least one
/// file to pull, then return its filesystem path.
///
/// **Slice 0 (design §7):** a single hardcoded, writable root over
/// `~/.entity/tori-share`, seeded with `welcome.txt` so a freshly paired
/// phone always sees something to pull. Writable so the phone can also
/// push (upload). Folder-picking + per-peer grants are Phase 1 — this is
/// the narrowest "prove the pipe" surface.
fn ensure_share_root() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".entity").join("tori-share");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("share root: create {:?} failed: {}", dir, e);
        return None;
    }
    let welcome = dir.join("welcome.txt");
    if !welcome.exists() {
        let body = "Hello from Tori-native! This file crossed the network as \
                    entities — a local/files entity + a content blob/chunks — \
                    with no USB and no cloud.\n";
        if let Err(e) = std::fs::write(&welcome, body) {
            log::warn!("share root: seed welcome.txt failed: {}", e);
        }
    }
    Some(dir)
}

// ---------------------------------------------------------------------------
// Backend peer state
//
// Persistence I/O lives in `persistence.rs` (spec layout per
// GUIDE-PERSISTENCE.md §1: `~/.entity/peers/{name}/{keypair,
// config.toml, store.db}`). The legacy `~/.entity/backend-peers/{peer_id}`
// layout is migrated on startup.
// ---------------------------------------------------------------------------

/// Runtime state for a running backend peer.
#[allow(dead_code)]
struct BackendPeerRuntime {
    peer: Peer,
    shared: Arc<PeerShared>,
    ws_addr: String,
    listener_handle: tokio::task::JoinHandle<()>,
}

/// A backend peer managed by the Tauri process.
/// May be stopped (persisted identity only) or running (full peer + listener).
struct BackendPeer {
    peer_id: String,
    /// Seed bytes for reconstructing the keypair (Keypair is not Clone).
    seed: [u8; 32],
    label: Option<String>,
    /// SQLite database file backing this peer's tree, if any. Set when
    /// the peer was loaded from disk with storage_backend = "sqlite".
    /// `None` means in-memory tree (legacy fallback).
    sqlite_path: Option<PathBuf>,
    runtime: Option<BackendPeerRuntime>,
}

impl BackendPeer {
    fn is_running(&self) -> bool {
        self.runtime.is_some()
    }

    fn status(&self) -> &'static str {
        if self.is_running() { "running" } else { "stopped" }
    }

    fn ws_addr(&self) -> Option<&str> {
        self.runtime.as_ref().map(|r| r.ws_addr.as_str())
    }

    fn stop(&mut self) {
        if let Some(rt) = self.runtime.take() {
            rt.listener_handle.abort();
            log::info!("Stopped backend peer {}", &self.peer_id[..12.min(self.peer_id.len())]);
        }
    }
}

/// Shared state holding all managed backend peers.
struct BackendPeers {
    peers: Mutex<HashMap<String, BackendPeer>>,
}

/// Response sent back to the WASM frontend.
#[derive(Serialize, Clone)]
struct BackendPeerResponse {
    peer_id: String,
    label: Option<String>,
    status: String,
    ws_addr: Option<String>,
}

/// Native-store stats for the system backend, surfaced to the Storage window
/// (`system_backend_store_stats`). `entity_count`/`path_count` are `None` when
/// the peer is stopped (no live store to read); `sqlite_bytes` is the on-disk
/// file size, available regardless.
#[derive(Serialize, Clone)]
struct BackendStoreStats {
    peer_id: String,
    running: bool,
    sqlite_bytes: Option<u64>,
    entity_count: Option<usize>,
    path_count: Option<usize>,
}

// ---------------------------------------------------------------------------
// Tauri IPC commands
// ---------------------------------------------------------------------------

/// Tauri command: receive log messages from the WebView and print to stdout.
#[tauri::command]
fn webview_log(level: String, message: String) {
    match level.as_str() {
        "error" => log::error!("[webview] {}", message),
        "warn" => log::warn!("[webview] {}", message),
        "debug" => log::debug!("[webview] {}", message),
        "trace" => log::trace!("[webview] {}", message),
        _ => log::info!("[webview] {}", message),
    }
}

/// Create a new backend peer. Persists keypair to disk.
/// Does NOT start the peer — call start_backend_peer to boot it.
#[tauri::command]
fn create_backend_peer(
    state: tauri::State<'_, BackendPeers>,
    label: Option<String>,
) -> Result<BackendPeerResponse, String> {
    let keypair = Keypair::generate();
    let seed = keypair.secret_key_bytes();
    let peer_id = keypair.peer_id().to_string();
    log::info!("Creating backend peer: {}", &peer_id[..12.min(peer_id.len())]);

    let sqlite_path = persistence::save_peer(&keypair, label.as_deref());

    let response = BackendPeerResponse {
        peer_id: peer_id.clone(),
        label: label.clone(),
        status: "stopped".into(),
        ws_addr: None,
    };

    state.peers.lock().unwrap().insert(peer_id.clone(), BackendPeer {
        peer_id,
        seed,
        label,
        sqlite_path,
        runtime: None,
    });

    Ok(response)
}

/// Start a stopped backend peer — build Peer, start WS listener.
#[tauri::command]
async fn start_backend_peer(
    state: tauri::State<'_, BackendPeers>,
    peer_id: String,
    // The system peer (S) that manages this backend — designated at start so B
    // can self-seed S's manager grant (DESIGN-AUTHORIZE-GATE-INCREMENT-3 §3
    // Step 1). Empty when no frontend designates a manager (headless spawn).
    manager_peer_id: String,
) -> Result<BackendPeerResponse, String> {
    // Extract what we need under the lock, then release it for async work.
    let (seed, label, sqlite_path) = {
        let peers = state.peers.lock().unwrap();
        let bp = peers.get(&peer_id)
            .ok_or_else(|| format!("Backend peer {} not found", peer_id))?;
        if bp.is_running() {
            return Ok(BackendPeerResponse {
                peer_id: bp.peer_id.clone(),
                label: bp.label.clone(),
                status: "running".into(),
                ws_addr: bp.ws_addr().map(String::from),
            });
        }
        (bp.seed, bp.label.clone(), bp.sqlite_path.clone())
    };

    log::info!("Starting backend peer: {}", &peer_id[..12.min(peer_id.len())]);

    let keypair = Keypair::from_seed(seed);
    let config = PeerConfig {
        debug_open_grants: true,
        ..PeerConfig::default()
    };

    let mut builder = PeerBuilder::new()
        .keypair(keypair)
        .config(config)
        .connector(Arc::new(WebSocketConnector) as Arc<dyn Connector>)
        // Inbound access log: correlate the caller (wire hook) with the target +
        // outcome (dispatch hook) so S can see "who reached into my share, and
        // did I allow it" — streamed to the WebView via `backend_access_log_tail`.
        .with_wire_hook("entity-browser/access-log", access_log::on_wire)
        .with_dispatch_hook("entity-browser/access-log", access_log::on_dispatch);

    // Wire SQLite-backed tree storage when a path is configured for
    // this peer. Without this, the tree is in-memory only and resets
    // on every restart (the earlier behavior).
    if let Some(ref db_path) = sqlite_path {
        log::info!("Backend peer {} using SQLite store at {:?}",
            &peer_id[..12.min(peer_id.len())], db_path);
        builder = builder
            .sqlite(db_path)
            .map_err(|e| format!("Failed to open SQLite store at {:?}: {}", db_path, e))?;
    } else {
        log::warn!("Backend peer {} has no SQLite path; tree state will not persist",
            &peer_id[..12.min(peer_id.len())]);
    }

    let peer = builder
        .build()
        .map_err(|e| format!("Failed to build peer: {}", e))?;

    let shared = peer.shared();
    peer.start_engines(&shared);

    // Seed the manager grant for S on B's own tree, before the listener opens.
    // The only pre-authored grant in the locked-down posture; inert while
    // debug_open_grants is on, load-bearing once it's retired (step 7).
    manager_grant::seed_manager_grant(&shared, &peer_id, &manager_peer_id);

    // Mount the demo share root so a paired phone can list/read files over
    // the connection (DESIGN-CROSS-DEVICE-FILE-TRANSFER §7, Slice 0). The
    // handler is built into every Peer; we only add a root mapping. Failure
    // here is non-fatal — the peer still runs, just with nothing to share.
    if let Some(share_dir) = ensure_share_root() {
        let cfg = entity_peer::local_files::RootConfigData {
            prefix: SHARE_PREFIX.to_string(),
            filesystem_root: share_dir.to_string_lossy().into_owned(),
            // Writable so a paired peer can BOTH pull (read) and push
            // (write, Phase 2 upload) files in the shared folder. With
            // debug_open_grants any connected peer may write here — that's
            // the intended demo posture; the real per-peer put-grant is the
            // Phase-1 hardening (DESIGN §2).
            read_only: false,
            ..Default::default()
        };
        match peer.local_files_handler().add_root("shared", cfg) {
            Ok(()) => log::info!(
                "Backend peer {} sharing {:?} at {}",
                &peer_id[..12.min(peer_id.len())],
                share_dir,
                SHARE_PREFIX
            ),
            Err(e) => log::warn!("Failed to mount share root: {}", e),
        }
    }

    // Bind ALL interfaces (`0.0.0.0`) by default so the listener is
    // reachable from other devices on the LAN — the reported address is
    // the host's real LAN IP (connectable_addr), which is what QR pairing
    // a phone needs. The first backend peer takes the well-known port
    // 4041, subsequent peers get dynamic ports.
    //
    // Security (C3): a `0.0.0.0` bind exposes the peer to the local
    // network. That is the intended desktop posture here (pairing). Lock
    // it back down to loopback-only with ENTITY_BROWSER_LOOPBACK_ONLY=1
    // (the e2e sets this to keep its same-host expectations). See
    // STANDARDS-RELEASE-ACCEPTANCE §6.D.
    let has_running_peer = {
        let peers = state.peers.lock().unwrap();
        peers.values().any(|bp| bp.peer_id != peer_id && bp.is_running())
    };
    let loopback_only = std::env::var("ENTITY_BROWSER_LOOPBACK_ONLY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let host = if loopback_only { "127.0.0.1" } else { "0.0.0.0" };
    let port = if has_running_peer { "0" } else { "4041" };
    let bind_addr = format!("{host}:{port}");
    let listener = match WebSocketListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(_) if bind_addr.ends_with(":4041") => {
            // Well-known port in use — fall back to a dynamic port on the
            // same host (same interface scope as the primary bind).
            log::info!("Port 4041 in use, falling back to dynamic port");
            let dynamic = format!("{host}:0");
            WebSocketListener::bind(&dynamic)
                .await
                .map_err(|e| format!("Failed to bind WS listener: {}", e))?
        }
        Err(e) => return Err(format!("Failed to bind WS listener: {}", e)),
    };
    let bound_addr = listener.local_addr();
    // Reported / connect address: listener.local_addr() returns the
    // bound address, which uses 0.0.0.0 when binding to all
    // interfaces. Browsers can't connect to 0.0.0.0, so we
    // substitute the loopback address for the reported value.
    let ws_addr = connectable_addr(&bound_addr);
    log::info!(
        "Backend peer {} listening on {} (reported as {})",
        &peer_id[..12.min(peer_id.len())],
        bound_addr,
        ws_addr
    );

    let shared_for_run = shared.clone();
    let pid_for_task = peer_id.clone();
    let listener_handle = tokio::spawn(async move {
        if let Err(e) = entity_peer::server::run(listener, shared_for_run).await {
            log::error!("Backend peer {} listener error: {}", &pid_for_task[..12.min(pid_for_task.len())], e);
        }
    });

    let response = BackendPeerResponse {
        peer_id: peer_id.clone(),
        label: label.clone(),
        status: "running".into(),
        ws_addr: Some(ws_addr.clone()),
    };

    // Update the peer with runtime state.
    let mut peers = state.peers.lock().unwrap();
    if let Some(bp) = peers.get_mut(&peer_id) {
        bp.runtime = Some(BackendPeerRuntime {
            peer,
            shared,
            ws_addr,
            listener_handle,
        });
    }

    Ok(response)
}

/// Stop a running backend peer. Keeps the persisted identity.
#[tauri::command]
fn stop_backend_peer(
    state: tauri::State<'_, BackendPeers>,
    peer_id: String,
) -> Result<BackendPeerResponse, String> {
    let mut peers = state.peers.lock().unwrap();
    let bp = peers.get_mut(&peer_id)
        .ok_or_else(|| format!("Backend peer {} not found", peer_id))?;
    bp.stop();
    Ok(BackendPeerResponse {
        peer_id: bp.peer_id.clone(),
        label: bp.label.clone(),
        status: "stopped".into(),
        ws_addr: None,
    })
}

/// Delete a backend peer entirely — stop if running, remove from disk.
#[tauri::command]
fn delete_backend_peer(
    state: tauri::State<'_, BackendPeers>,
    peer_id: String,
) -> Result<(), String> {
    let mut peers = state.peers.lock().unwrap();
    // The canonical system backend is un-deletable — it is auto-provisioned at
    // every boot (mirrors the frontend system peer). Refuse loudly rather than
    // let a delete succeed only to have B reappear on the next launch, which
    // would read as a broken control. Additional backends delete normally.
    if peers.get(&peer_id).and_then(|bp| bp.label.as_deref()) == Some(SYSTEM_BACKEND_LABEL) {
        return Err("The system backend peer cannot be deleted".to_string());
    }
    if let Some(mut bp) = peers.remove(&peer_id) {
        bp.stop();
        persistence::delete_peer(&peer_id);
        Ok(())
    } else {
        Err(format!("Backend peer {} not found", peer_id))
    }
}

/// Set the backend's `tracing` level at runtime (`off`/`error`/…/`trace`), from
/// the System Backend window's level control. Swaps the reload filter.
#[tauri::command]
fn set_backend_log_level(level: String) -> Result<(), String> {
    backend_log::set_level(&level)
}

/// The backend's current `tracing` level, for the window to pre-select its
/// level control on open.
#[tauri::command]
fn get_backend_log_level() -> String {
    backend_log::current_level()
}

/// The backend's shared-files directory on disk — the real filesystem location
/// behind the `local/files/shared/` tree prefix a paired device browses. Shown
/// in the System Backend window so the operator knows *where* shared files land
/// (answers "what is local/files/shared"). System-level, not per-peer.
#[tauri::command]
fn system_backend_share_path() -> Option<String> {
    ensure_share_root().map(|p| p.to_string_lossy().into_owned())
}

/// Tail the backend peer's captured `tracing` output for the System Backend
/// window's live log stream (DESIGN §4). Returns every buffered line after the
/// caller's `after` cursor plus the new cursor to poll with next. In-memory ring
/// (`backend_log`), so this is process-lifetime only — no durable history.
#[tauri::command]
fn backend_log_tail(after: u64) -> backend_log::LogTail {
    backend_log::tail(after)
}

/// Tail the backend peer's **inbound access log** — who reached into this
/// device's share, what they hit, and whether it was allowed or denied. Wire +
/// dispatch hooks are stitched by `request_id` in `access_log`; the WebView
/// folds these into the app-tier Access Log (direction = Inbound). Same in-memory
/// ring + cursor contract as `backend_log_tail`.
#[tauri::command]
fn backend_access_log_tail(after: u64) -> access_log::AccessTail {
    access_log::tail(after)
}

/// Storage stats for the canonical system backend's **native** store, so the
/// Storage window can surface it (it otherwise sees only the frontend arms —
/// B is a remote peer over the connection pool, its SQLite store lives in this
/// process and is reachable only over IPC). On-disk size comes from the SQLite
/// file (present even when B is stopped); live entity/path counts come from the
/// running peer's store (`None` when stopped). System-level, not per-peer.
#[tauri::command]
fn system_backend_store_stats(state: tauri::State<'_, BackendPeers>) -> Option<BackendStoreStats> {
    let peers = state.peers.lock().unwrap();
    let bp = peers
        .values()
        .find(|bp| bp.label.as_deref() == Some(SYSTEM_BACKEND_LABEL))?;
    let sqlite_bytes = bp
        .sqlite_path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len());
    let (entity_count, path_count) = match &bp.runtime {
        Some(rt) => (
            Some(rt.peer.content_store().len()),
            Some(rt.peer.location_index().len_prefix("")),
        ),
        None => (None, None),
    };
    Some(BackendStoreStats {
        peer_id: bp.peer_id.clone(),
        running: bp.is_running(),
        sqlite_bytes,
        entity_count,
        path_count,
    })
}

/// List all managed backend peers (running and stopped).
#[tauri::command]
fn list_backend_peers(state: tauri::State<'_, BackendPeers>) -> Vec<BackendPeerResponse> {
    let peers = state.peers.lock().unwrap();
    peers.values().map(|bp| BackendPeerResponse {
        peer_id: bp.peer_id.clone(),
        label: bp.label.clone(),
        status: bp.status().into(),
        ws_addr: bp.ws_addr().map(String::from),
    }).collect()
}

/// JavaScript that intercepts console.log/warn/error and forwards
/// to the Tauri backend via invoke("webview_log").
/// Also installs a global error handler to catch WASM crashes.
const CONSOLE_BRIDGE_JS: &str = r#"
(function() {
    if (window.__ENTITY_CONSOLE_BRIDGE__) return;
    window.__ENTITY_CONSOLE_BRIDGE__ = true;
    const T = window.__TAURI__;
    if (!T || !T.core || !T.core.invoke) return;
    const orig = { log: console.log.bind(console), warn: console.warn.bind(console), error: console.error.bind(console), debug: console.debug.bind(console) };
    function forward(level, args) {
        try {
            const msg = Array.from(args).map(a => typeof a === 'string' ? a : JSON.stringify(a)).join(' ');
            T.core.invoke('webview_log', { level: level, message: msg });
        } catch(e) {}
    }
    console.log = function() { orig.log.apply(null, arguments); forward('info', arguments); };
    console.warn = function() { orig.warn.apply(null, arguments); forward('warn', arguments); };
    console.error = function() { orig.error.apply(null, arguments); forward('error', arguments); };
    console.debug = function() { orig.debug.apply(null, arguments); forward('debug', arguments); };

    // Catch uncaught errors (including WASM RuntimeError).
    window.addEventListener('error', function(e) {
        forward('error', ['[UNCAUGHT] ' + e.message + ' at ' + (e.filename || '?') + ':' + (e.lineno || '?')]);
        if (e.error && e.error.stack) {
            forward('error', ['[STACK] ' + e.error.stack]);
        }
    });
    window.addEventListener('unhandledrejection', function(e) {
        forward('error', ['[UNHANDLED PROMISE] ' + (e.reason || e)]);
    });

    // Monitor WASM memory usage — log periodically and on errors.
    function logWasmMemory(label) {
        try {
            // wasm-bindgen exposes the memory object on the WASM instance
            var mem = null;
            if (typeof wasm_bindgen !== 'undefined' && wasm_bindgen.memory) {
                mem = wasm_bindgen.memory();
            }
            if (mem && mem.buffer) {
                var mb = (mem.buffer.byteLength / 1048576).toFixed(1);
                forward('info', ['[WASM memory] ' + label + ': ' + mb + 'MB']);
            }
        } catch(e) {}
    }
    // Log memory after init settles.
    setTimeout(function() { logWasmMemory('after-init'); }, 2000);
    setTimeout(function() { logWasmMemory('steady-state'); }, 10000);

    orig.log('[console bridge] attached');
    forward('info', ['[console bridge] attached and forwarding to stdout']);
})();
"#;

/// Reuse-or-create the persisted backend peer labelled `label`, then start it
/// (build → seed the manager grant for `manager_peer_id` → mount the share →
/// bind the listener). Idempotent: a second call with the same label returns
/// the already-running peer (via `start_backend_peer`'s running short-circuit).
/// Reusing by a stable label keeps one identity across restarts and avoids
/// accumulating junk peers in `~/.entity/peers`.
///
/// The shared core behind `ensure_system_backend` (production auto-provision,
/// label `system-backend`, manager = S) and `autostart_listener` (the E2E hook,
/// label `autostart`, empty manager).
async fn ensure_backend_peer(
    state: tauri::State<'_, BackendPeers>,
    label: &str,
    manager_peer_id: String,
) -> Result<BackendPeerResponse, String> {
    let peer_id = {
        let mut peers = state.peers.lock().unwrap();
        if let Some(existing) =
            peers.values().find(|bp| bp.label.as_deref() == Some(label))
        {
            existing.peer_id.clone()
        } else {
            let kp = Keypair::generate();
            let seed = kp.secret_key_bytes();
            let pid = kp.peer_id().to_string();
            let lbl = Some(label.to_string());
            let sqlite_path = persistence::save_peer(&kp, lbl.as_deref());
            peers.insert(
                pid.clone(),
                BackendPeer {
                    peer_id: pid.clone(),
                    seed,
                    label: lbl,
                    sqlite_path,
                    runtime: None,
                },
            );
            pid
        }
    };

    // Reuse the production command implementation so every entry point exercises
    // the exact same listener-build path as a real user click.
    start_backend_peer(state, peer_id, manager_peer_id).await
}

/// Auto-provision the canonical **system backend peer** (B) and return its id +
/// WS address. Idempotent across restarts and reloads (stable `system-backend`
/// identity + `start_backend_peer`'s running short-circuit). `manager_peer_id`
/// is the frontend system peer (S), which B self-seeds as its manager grant so
/// the authorize gate has an authority once `debug_open_grants` is retired.
///
/// Driven by the WebView (S) at boot — `src-tauri` setup does not know S's
/// peer-id, and the manager grant is load-bearing, so S makes the call. This is
/// the production replacement for the manual create → start dance.
/// See `DESIGN-SYSTEM-BACKEND-PEER.md` §10.2.
#[tauri::command]
async fn ensure_system_backend(
    state: tauri::State<'_, BackendPeers>,
    manager_peer_id: String,
) -> Result<BackendPeerResponse, String> {
    ensure_backend_peer(state, SYSTEM_BACKEND_LABEL, manager_peer_id).await
}

/// Test-only autostart entry point. Drives the same production listener-start
/// path as a real user click, without a click. No manager peer in this headless
/// context (empty manager → grant seed skipped); its own `autostart` label
/// keeps a stable per-test identity distinct from the `system-backend` peer.
async fn autostart_listener(
    app_handle: &tauri::AppHandle,
) -> Result<(String, String), String> {
    let state = app_handle.state::<BackendPeers>();
    let response = ensure_backend_peer(state, "autostart", String::new()).await?;
    let ws_addr = response
        .ws_addr
        .ok_or_else(|| "start_backend_peer returned no ws_addr".to_string())?;
    Ok((response.peer_id, ws_addr))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Capture the backend peer's `tracing` output → stdout. Without this the
    // entity-peer runtime (handshake, dispatch, write handling, errors) is
    // dropped — the backend runs dark (DESIGN-SYSTEM-BACKEND-PEER §2). Default
    // `info`; override with RUST_LOG (e.g. `RUST_LOG=info,entity_peer=debug`).
    // Independent of tauri-plugin-log, which handles the `log` crate (the
    // webview console bridge). `try_init` so a double-init never panics.
    // Install the backend's `tracing` subscriber: a reload-able level filter
    // (the System Backend window changes it live) feeding an ANSI-colored fmt
    // layer that tees into the ring buffer (window's log stream, DESIGN §4) and
    // stdout. ANSI is kept on — the window converts the escapes to colored HTML
    // (`crate::ansi`), the terminal shows them natively.
    backend_log::init_tracing();

    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                ])
                .level(log::LevelFilter::Info)
                .build(),
        )
        .manage({
            // Load persisted backend peers from disk (all start as stopped).
            // First-run after this migration: legacy `~/.entity/backend-peers/`
            // is migrated into the spec layout `~/.entity/peers/{name}/`.
            let persisted = persistence::load_all_peers();
            let mut peers = HashMap::new();
            for entry in persisted {
                let seed = entry.keypair.secret_key_bytes();
                peers.insert(entry.peer_id.clone(), BackendPeer {
                    peer_id: entry.peer_id,
                    seed,
                    label: entry.label,
                    sqlite_path: entry.sqlite_path,
                    runtime: None,
                });
            }
            BackendPeers { peers: Mutex::new(peers) }
        })
        .invoke_handler(tauri::generate_handler![
            webview_log,
            create_backend_peer,
            start_backend_peer,
            stop_backend_peer,
            delete_backend_peer,
            list_backend_peers,
            ensure_system_backend,
            backend_log_tail,
            backend_access_log_tail,
            system_backend_share_path,
            system_backend_store_stats,
            set_backend_log_level,
            get_backend_log_level,
        ])
        .setup(|app| {
            log::info!("Tauri backend starting");
            // Inject console bridge as early as possible, retrying until the
            // Tauri JS API is available. The bridge also installs global error
            // handlers to catch WASM crashes.
            if let Some(window) = app.webview_windows().values().next().cloned() {
                std::thread::spawn(move || {
                    // Try multiple times with short delays — the WebView needs
                    // a moment to load the page and Tauri JS API.
                    for attempt in 1..=20 {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        match window.eval(CONSOLE_BRIDGE_JS) {
                            Ok(_) => {
                                log::info!("Console bridge injected (attempt {})", attempt);
                                break;
                            }
                            Err(e) => {
                                if attempt == 20 {
                                    log::error!("Console bridge injection failed after 20 attempts: {}", e);
                                }
                            }
                        }
                    }
                });
            }

            // E2E hook: when ENTITY_BROWSER_AUTOSTART_LISTENER=1 is set,
            // immediately bring up a backend peer + WS listener and print
            // a single parseable line to stdout. The Phase 14 E2E test
            // (tests/e2e_worker.rs) spawns this binary, scrapes the
            // line, and uses ws_addr as the ConnectPeer target.
            // Outside of tests this env var is never set, so production
            // behavior is unchanged.
            if std::env::var("ENTITY_BROWSER_AUTOSTART_LISTENER").is_ok() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    match autostart_listener(&handle).await {
                        Ok((pid, ws_addr)) => {
                            // Newline-terminated, stable prefix so the
                            // test can grep for it deterministically.
                            println!(
                                "ENTITY_BACKEND_LISTENER_READY peer_id={} ws_addr={}",
                                pid, ws_addr
                            );
                        }
                        Err(e) => {
                            eprintln!("ENTITY_BACKEND_LISTENER_FAILED {}", e);
                        }
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
