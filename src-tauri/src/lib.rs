use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use entity_crypto::Keypair;
use entity_peer::{Peer, PeerBuilder, PeerConfig, PeerShared};
use entity_peer::transport::{Connector, WebSocketConnector, WebSocketListener, Listener};
use serde::Serialize;
use tauri::Manager;

mod persistence;

/// Convert a listener-bound address (which may use the wildcard
/// `0.0.0.0` or `[::]` for "all interfaces") into an address that's
/// actually connectable from a client. The wildcards are valid for
/// listening but cannot be used as a connect target — browsers in
/// particular reject them.
///
/// We substitute the loopback address so the WebView can connect to
/// the backend in the same Tauri process. The listener still binds
/// to the wildcard, so external clients on the LAN can also reach
/// the backend via its real LAN IP — that's just not the address
/// reported back to the local frontend.
fn connectable_addr(listen_addr: &str) -> String {
    listen_addr
        .replace("ws://0.0.0.0:", "ws://127.0.0.1:")
        .replace("ws://[::]:", "ws://[::1]:")
        .replace("tcp://0.0.0.0:", "tcp://127.0.0.1:")
        .replace("tcp://[::]:", "tcp://[::1]:")
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
        .connector(Arc::new(WebSocketConnector) as Arc<dyn Connector>);

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

    // C3 (security): bind to LOOPBACK by default. The first backend peer
    // takes the well-known port 4041, subsequent peers get dynamic ports.
    // A `0.0.0.0` bind exposes the peer to the entire LAN — opt into that
    // explicitly with ENTITY_BROWSER_LAN_BIND=1 (e.g. to pair a phone over
    // the local network). See STANDARDS-RELEASE-ACCEPTANCE §6.D.
    let has_running_peer = {
        let peers = state.peers.lock().unwrap();
        peers.values().any(|bp| bp.peer_id != peer_id && bp.is_running())
    };
    let lan_bind = std::env::var("ENTITY_BROWSER_LAN_BIND")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let bind_addr = match (has_running_peer, lan_bind) {
        (false, false) => "127.0.0.1:4041",
        (false, true) => "0.0.0.0:4041",
        (true, false) => "127.0.0.1:0",
        (true, true) => "0.0.0.0:0",
    };
    let listener = match WebSocketListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(_) if bind_addr.ends_with(":4041") => {
            // Well-known port in use — fall back to a dynamic port on the
            // same host (loopback unless LAN bind was opted into).
            log::info!("Port 4041 in use, falling back to dynamic port");
            let dynamic = if lan_bind { "0.0.0.0:0" } else { "127.0.0.1:0" };
            WebSocketListener::bind(dynamic)
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
    if let Some(mut bp) = peers.remove(&peer_id) {
        bp.stop();
        persistence::delete_peer(&peer_id);
        Ok(())
    } else {
        Err(format!("Backend peer {} not found", peer_id))
    }
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

/// Test-only autostart entry point. Drives the same listener-start logic
/// the WebView would invoke when the user clicks Start, just without a
/// user click. Reuses (or creates) a persisted peer labelled "autostart"
/// so repeated test runs hit the same identity and don't accumulate
/// junk peers in `~/.entity/peers`.
async fn autostart_listener(
    app_handle: &tauri::AppHandle,
) -> Result<(String, String), String> {
    let state = app_handle.state::<BackendPeers>();

    let peer_id = {
        let mut peers = state.peers.lock().unwrap();
        if let Some(existing) =
            peers.values().find(|bp| bp.label.as_deref() == Some("autostart"))
        {
            existing.peer_id.clone()
        } else {
            let kp = Keypair::generate();
            let seed = kp.secret_key_bytes();
            let pid = kp.peer_id().to_string();
            let label = Some("autostart".to_string());
            let sqlite_path = persistence::save_peer(&kp, label.as_deref());
            peers.insert(
                pid.clone(),
                BackendPeer {
                    peer_id: pid.clone(),
                    seed,
                    label,
                    sqlite_path,
                    runtime: None,
                },
            );
            pid
        }
    };

    // Reuse the production command implementation so the test exercises
    // the exact same listener-build path as a real user click.
    let response = start_backend_peer(state, peer_id).await?;
    let ws_addr = response
        .ws_addr
        .ok_or_else(|| "start_backend_peer returned no ws_addr".to_string())?;
    Ok((response.peer_id, ws_addr))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
