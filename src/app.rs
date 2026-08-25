//! EntityApp — application state and action dispatch.
//!
//! Active deployments: WASM browser (DOM via rAF) and Tauri desktop
//! (DOM in WebView + native backend peer in `src-tauri/`). `EntityApp`
//! is wasm-only; the native binary is a deprecation stub (the legacy
//! native renderer was removed).

#[cfg(target_arch = "wasm32")]
use std::sync::Arc;

use crate::action::Action;
use crate::connections::ConnectionsWriter;
use crate::event_log_writer::EventLogWriter;
#[cfg(feature = "native-ws")]
use crate::listener_state::ListenerStateWriter;
use crate::peer_registry::PeerRegistry;
use crate::peers::Peers;
// The window-type roster (the `…Window` factories) is now owned by
// `crate::window_registry` — the single source both the registrar and the
// settings startup-surface control read.
use crate::window::WindowManager;

#[cfg(feature = "native-ws")]
use entity_peer::transport::Listener;

/// True when the page URL carries `?remote_fixture` — the e2e/showcase
/// switch that wires a same-origin static fixture site (see the boot hook
/// in [`EntityApp::boot_load`]). Never set in production.
///
/// The fixture's foreign peer-id. A **real 46-char Base58 peer-id** (not a
/// readable label): tree paths validate the peer-segment, so the write-through
/// site cache can durably land this foreign site at `/{REMOTE_FIXTURE_PEER}/
/// sites/labs/...`. A readable id renders fine over the in-memory fetch cache
/// (reads don't validate) but `tree.put` rejects it — which silently defeated
/// durable caching. The boot-hook origin/overlay + the fixture emit
/// ([`crate::content_site::publish_fixture`]) all key off this one constant.
pub const REMOTE_FIXTURE_PEER: &str = "2KFAQwKL6XzdwLkoHkxZ9WE7kvBtS59piFA2AkdBBiQUt5";

#[cfg(target_arch = "wasm32")]
fn remote_fixture_requested() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.contains("remote_fixture"))
        .unwrap_or(false)
}

/// `?boot_window=[{peer}:]{type}` — the e2e/showcase switch that boots straight
/// into a maximized window of the named type, on an optional target peer (bare
/// `{type}` ⇒ the system peer), exercising the `BootSurface::Window` path
/// directly (no persisted config needed). Mirrors [`remote_fixture_requested`];
/// never set in production. Returns the raw `[{peer}:]{type}` string when
/// present (boot_load splits the peer prefix).
#[cfg(target_arch = "wasm32")]
fn boot_window_override() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    search
        .trim_start_matches('?')
        .split('&')
        .find_map(|part| part.strip_prefix("boot_window="))
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
}

/// `?site={peer}/{site}[/{page}]` — the static→live deep link ([F3]). A
/// static page's "open in live peer" banner ([F2]) links here; at boot the
/// live SPA reads it and drops straight into the site overlay at that page.
/// `peer` may be the `self` sentinel ([`paths::SELF_PEER`]) → the system
/// peer. Returns `(peer, site, page)`. Ephemeral like
/// [`boot_window_override`] — navigate-only, never persisted.
#[cfg(target_arch = "wasm32")]
fn site_deeplink_override() -> Option<(String, String, String)> {
    let search = web_sys::window()?.location().search().ok()?;
    let value = search
        .trim_start_matches('?')
        .split('&')
        .find_map(|part| part.strip_prefix("site="))?;
    crate::content_site::paths::parse_site_query(value)
}

/// `?chrome=1` — the **operator escape hatch** out of a locked (strict-site)
/// deployment. A kiosk lock (`site_mode.locked`, no toggle) is correct for a
/// shipped content site, but without an escape it's a one-way door: if the
/// home goes unreachable, or the author just wants to reach Settings, there is
/// no discoverable way back to chrome (the BUG-1 strand, inverted). This flag
/// forces the chrome surface + exposes the toggle + bypasses the lock guard for
/// **this session only** — never persisted (mirrors `?boot_window=` / `?site=`,
/// read at boot, never written durably). So a locked deployment is always
/// recoverable: append `?chrome=1` to the URL → land in chrome → fix the config.
#[cfg(target_arch = "wasm32")]
fn chrome_override() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| {
            s.trim_start_matches('?')
                .split('&')
                .any(|part| matches!(part, "chrome=1" | "chrome" | "unlock=1" | "unlock"))
        })
        .unwrap_or(false)
}

/// Auto-connect bookkeeping for the canonical system backend peer (Phase 1c,
/// `DESIGN-SYSTEM-BACKEND-PEER.md` §10). The boot IPC fills `target` with the
/// ensured backend's id + WS address; `drain_system_backend_connect` (each
/// frame) dials it from S with backoff, iff it isn't already connected, and
/// clears `target` once connected or after the attempt cap — so the frontend
/// system peer connects to the backend with zero user action.
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct SystemBackendConnect {
    /// The ensured backend (id + ws addr). `None` until the boot IPC returns;
    /// then it stays armed for the session so `drain_system_backend_connect`
    /// can **re-dial on a dropped link** (self-healing) — the drain idles while
    /// the connection is live (per real health) and re-dials when it goes
    /// Unreachable.
    target: Option<crate::tauri_ipc::BackendPeerInfo>,
    /// Connect attempts issued so far — the cap guards against a backend that
    /// never accepts (D13: give up loudly, don't spin forever).
    attempts: u32,
    /// Frames to wait before the next attempt. Backoff covers a dial that
    /// races the (async) listener bind or a transient connect failure.
    cooldown: u32,
}

/// Main application state. Only instantiated by the WASM frontend
/// (browser / Tauri WebView); the native binary is a deprecation stub.
#[cfg(target_arch = "wasm32")]
pub struct EntityApp {
    peer_manager: Peers,
    window_manager: WindowManager,
    /// Tree-backed event log writer. Cheap to clone into spawned tasks
    /// that produce log lines from background work.
    event_log_writer: EventLogWriter,
    /// Tree-backed registry of connected remote peers. Cheap to clone
    /// into spawned tasks (e.g. the connect handler).
    connections_writer: ConnectionsWriter,
    /// Tree-backed mirror of the backend-auth observability surface — derived
    /// pending/authorized rows per backend, written by the async remote read
    /// (`Action::RefreshBackendAuth`) so the Peer Connections window can render
    /// them synchronously (`DESIGN-AUTHORIZE-GATE-INCREMENT-3 §3 Step 3`).
    backend_auth_writer: crate::backend_auth::BackendAuthWriter,
    /// Local, subscribable connection-health mirror — written from connect
    /// success + the backend-auth probe, read by windows for reactive
    /// connected/stale state (`crate::connection_health`).
    connection_health_writer: crate::connection_health::ConnectionHealthWriter,
    /// Tree-backed publisher for the WS listener's bound address.
    /// Cloned into the listener-bind spawned task; only used on native.
    #[cfg(feature = "native-ws")]
    listener_state_writer: ListenerStateWriter,
    /// Tree-backed hosted-peer roster. Reconciled from the live
    /// `Peers` once per frame and at boot; every peer-aware window
    /// subscribes to its prefix via the in-tree peer registry. This is
    /// the single peer-membership reactivity mechanism — there is no separate
    /// signal or manual dirty-mark.
    peer_registry: PeerRegistry,
    /// User-theme registry sync: app-lifetime watch on the themes prefix;
    /// `sync()` per frame is one atomic check unless the prefix changed
    /// (then it reconciles the runtime theme registry from the tree).
    user_themes: crate::user_themes::UserThemes,
    /// Session-lived inspect sink on the system peer feeding the app-tier
    /// access log (`crate::access_log_store`) with local dispatches. Installed
    /// once at boot — app-global, not per-window — so the Access Log window is a
    /// pure reader and multiple windows can't double-record. `Drop` detaches it
    /// at app teardown. (The outbound-remote half is captured in `ops::execute`.)
    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    access_log_sink: Option<crate::inspect_router::PeersInspectSinkHandle>,
    #[cfg(target_arch = "wasm32")]
    dom: Option<crate::dom::DomRenderer>,
    /// Pending backend peer registrations from async Tauri IPC results.
    /// Drained each frame in WASM mode.
    #[cfg(target_arch = "wasm32")]
    pending_backend_peers: Arc<std::sync::Mutex<Vec<crate::tauri_ipc::BackendPeerInfo>>>,
    /// System backend auto-connect state (Phase 1c). Filled by the boot IPC
    /// (`ensure_system_backend`) from an async context, consumed by
    /// `drain_system_backend_connect` each frame. `Arc<Mutex>` for the
    /// async fill. `DESIGN-SYSTEM-BACKEND-PEER.md` §10.
    #[cfg(target_arch = "wasm32")]
    system_backend_connect: Arc<std::sync::Mutex<SystemBackendConnect>>,
    /// Pending Worker-SDK attachments from async `WorkerProxy::spawn`
    /// completions (Stage 2B). Drained each frame and integrated into
    /// `peer_manager` via `attach_worker_sdk`. Uses `Rc<RefCell<...>>`
    /// because `WorkerProxy` holds non-`Send` `Rc`s.
    #[cfg(target_arch = "wasm32")]
    pending_sdk_attachments: std::rc::Rc<std::cell::RefCell<Vec<PendingSdkAttachment>>>,
    /// Durable `frontend-idb` peers whose async `PeerContext` build has
    /// completed and are waiting to be inserted into the primary Direct SDK.
    /// The build (`Peers::build_idb_ctx`) runs in a `spawn_local` off the frame
    /// loop (can't hold `&mut peer_manager` across `build_async().await`); the
    /// completed ctx lands here and is drained under `&mut self` each frame by
    /// [`drain_pending_idb_peers`]. Same defer idiom as
    /// `pending_sdk_attachments`, main-thread flavour.
    #[cfg(target_arch = "wasm32")]
    pending_idb_peers: std::rc::Rc<std::cell::RefCell<Vec<PendingIdbPeer>>>,
    /// Cross-Worker MessagePort transport broker. Owns the main-side
    /// control ports for every backend Worker; routes inbound
    /// `xworker://<peer-id>` `OpenChannel` requests by transferring a
    /// fresh `MessageChannel` port pair between source and target
    /// Workers. Main is in the control path (one round-trip per
    /// connect) but not in the data path. Single instance per app.
    #[cfg(target_arch = "wasm32")]
    xworker_broker: std::rc::Rc<entity_wasm_worker_proxy::MessagePortBroker>,
    /// Main-side end of the boot worker's cross-Worker control port,
    /// kept so dynamically-created Frontend peers (`+ Frontend` button,
    /// `peer create frontend` verb) can be registered with the broker
    /// when their `Request::CreatePeer` round-trip completes. `Some`
    /// only in Worker mode after `new_wasm_worker` succeeds.
    #[cfg(target_arch = "wasm32")]
    boot_control_port: Option<web_sys::MessagePort>,
    /// Closures for the always-on status-bar Site Mode toggle (light DOM,
    /// outside the shadow root). Kept alive for the page lifetime —
    /// dropping the app drops these (D12). Wired once at boot; never read
    /// again (the field IS the keep-alive).
    #[cfg(target_arch = "wasm32")]
    #[allow(dead_code)]
    status_closures: crate::window::ClosureVec,
    /// Last session config applied to the DOM, so [`apply_site_mode`]
    /// only touches the container class / toggle when it actually
    /// changed (cheap idempotent guard against per-frame churn).
    #[cfg(target_arch = "wasm32")]
    last_site_state: Option<crate::session_config::SessionConfig>,
    /// The Site Mode overlay surface — renders the active content site
    /// into `#site-layer` while the overlay is active. `Some` whenever
    /// the DOM renderer initialized. Distinct from any Content Site
    /// *window*: app-level nav state, reuses the same renderer.
    #[cfg(target_arch = "wasm32")]
    site_overlay: Option<crate::dom::site_overlay::SiteOverlay>,
    /// The currently maximized window, if any (reframe §4-B Surfaces).
    /// Runtime, one-deep: at most one window is maximized at a time; the
    /// renderer promotes its `section.window` to the full-screen surface.
    /// `None` = the base layer shows (chrome windows, or the site overlay
    /// when `session_config.active`). Set at boot from the durable
    /// `BootSurface::Window { peer, type }` (boot_load spawns + maximizes the
    /// window on its target peer); a runtime maximize sets it live. The
    /// *(peer, type)* pair is the durable identifier — the ephemeral window id
    /// isn't persisted (re-spawned each boot).
    maximized_window: Option<crate::window::WindowId>,
    /// Set when a `?site=` deep link booted us straight into the site overlay
    /// ([F3]). Forces the overlay on for THIS session without touching the
    /// durable config (ephemeral, like the `?boot_window=` maximize override) —
    /// `apply_site_mode` ORs this into the per-frame `active`, so a normal
    /// reload (no `?site=`) honors the persisted surface.
    site_deeplink_active: bool,
    /// Set when the URL carries `?chrome=1` ([`chrome_override`]) — the operator
    /// escape out of a locked (strict-site) deployment. Forces the chrome
    /// surface + exposes the toggle + bypasses the lock guard for this session
    /// only (never persisted), so a kiosk lock is never a one-way door. Read
    /// synchronously at construction so it's honored from the very first frame,
    /// even before `boot_load` resolves the durable config.
    chrome_override: bool,
    /// Whether THIS tab's primary tree is durable — `true` only on the two
    /// durable `BootStorageStatus` arms (`DurableDirectIdb` / `DurableWorker`),
    /// `false` on the three ephemeral ones (ephemeral Direct, Worker→Direct
    /// downgrade, multi-tab secondary). Set once in [`EntityApp::boot_load`]
    /// from the `durable_substrate` bit the constructor already computes
    /// (`idb_active` on Direct, `true` on Worker) — no separate plumbing from
    /// `main.rs`. The 1a durability gate (MAP §10): peer creation is refused
    /// when this is `false`, so a create can't write the shared localStorage
    /// vault while its tree evaporates on reload (S-1 / L-2). Defaults to the
    /// **safe** `false` at construction; a path that never reaches `boot_load`
    /// refuses creation rather than silently losing it.
    can_persist: bool,
    /// Last status string written to the status bar (`#mode-display`), so the
    /// per-frame status update only touches the DOM on a real change (cheap
    /// string compare vs a DOM write every frame).
    #[cfg(target_arch = "wasm32")]
    last_status_text: Option<String>,
}

/// Wall-clock milliseconds since navigation start (the page-load
/// origin) via `Window.performance.now()`. Returns `None` when the
/// performance API isn't reachable (vanishingly rare in real
/// browsers; covers headless environments where we'd rather log
/// `None` than panic). Used for ad-hoc spawn-duration instrumentation.
#[cfg(target_arch = "wasm32")]
fn now_ms() -> Option<f64> {
    web_sys::window().and_then(|w| w.performance()).map(|p| p.now())
}

/// Build the worker loader URL with `?log=...` forwarded from the
/// main-thread URL if present. Without this propagation
/// `entity-worker.rs::init_worker_tracing` falls back to its default
/// (DEBUG/INFO by build profile), making worker-side `tracing::*`
/// inherit the user's chosen verbosity transparently.
#[cfg(target_arch = "wasm32")]
fn worker_loader_url() -> String {
    const BASE: &str = "/entity-worker-loader.js";
    let Some(level) = main_thread_log_param() else {
        return BASE.to_string();
    };
    format!("{BASE}?log={level}")
}

#[cfg(target_arch = "wasm32")]
fn main_thread_log_param() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let q = search.trim_start_matches('?');
    for pair in q.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some("log") {
            return parts.next().map(|s| s.to_string());
        }
    }
    None
}

/// Partition persisted-peer entries by mode into the three boot cohorts.
/// Stage 2C boot helper — `Frontend` peers load in-memory into the boot SDK
/// (Direct or Worker); `FrontendIdb` peers are main-thread durable and get
/// replayed into their own IndexedDB store (Direct posture — see
/// `DESIGN-PERSISTENT-THIS-TAB-PEER.md`); `Backend*` peers each get respawned
/// into their own worker. Persisted order is preserved within each cohort so
/// the "primary" (`frontend[0]`) stays stable across reloads.
#[cfg(target_arch = "wasm32")]
fn partition_entries(
    entries: Vec<crate::persistence::PersistedPeerEntry>,
) -> (
    Vec<crate::persistence::PersistedPeerEntry>,
    Vec<crate::persistence::PersistedPeerEntry>,
    Vec<crate::persistence::PersistedPeerEntry>,
) {
    use crate::peer_mode::PeerMode;
    let mut frontend = Vec::new();
    let mut frontend_idb = Vec::new();
    let mut backend = Vec::new();
    for e in entries {
        match e.mode {
            PeerMode::Frontend => frontend.push(e),
            PeerMode::FrontendIdb => frontend_idb.push(e),
            PeerMode::BackendMemory | PeerMode::BackendOpfs => backend.push(e),
        }
    }
    (frontend, frontend_idb, backend)
}

/// Free-function spawn dispatcher used by both fresh-create and reload
/// flows, in either the pre-`EntityApp`-construction context (boot path)
/// or the post-construction context (user clicks `+ Backend ...`).
///
/// `event_log` is optional because during boot the `EventLogWriter`
/// doesn't exist yet — failures during pre-boot respawn just trace and
/// the user sees a missing SDK rather than a tree-logged error.
#[cfg(target_arch = "wasm32")]
fn spawn_worker_sdk_for_peer_into(
    pending: std::rc::Rc<std::cell::RefCell<Vec<PendingSdkAttachment>>>,
    event_log: Option<EventLogWriter>,
    peer_id: String,
    keypair_seed: Vec<u8>,
    label: Option<String>,
    mode: crate::peer_mode::PeerMode,
) {
    use entity_wasm_worker_protocol::{InitParams, PersistedPeer as WirePersistedPeer};

    // PROTOCOL_VERSION=7: per-worker OPFS subdirectory derived from
    // the peer_id of the peer this worker hosts. Backend-OPFS peers
    // need a unique root or `createSyncAccessHandle` collides with
    // the boot worker's handles (U7). Memory mode leaves it None.
    let opfs_root = if mode.wants_opfs() {
        Some(format!("workers/{peer_id}"))
    } else {
        None
    };

    let init = InitParams {
        primary_peer: WirePersistedPeer {
            peer_id: peer_id.clone(),
            keypair_seed,
            label: label.clone(),
        },
        additional_peers: vec![],
        handlers: vec![],
        opfs_root,
    };

    let peer_mirror = vec![crate::peers_worker::PeerInfo {
        peer_id: peer_id.clone(),
        metadata: entity_sdk::PeerMetadata {
            label: label.clone(),
            // `persisted` is the opfs-vs-memory display discriminator
            // (see peer_display::role_name). Memory backends are not
            // persisted; only OPFS is. Was hardcoded `true`, which
            // mislabelled Memory peers as "backend (opfs)".
            persisted: mode.wants_opfs(),
            ..entity_sdk::PeerMetadata::default()
        },
        local: true,
    }];

    let mode_label = mode.label();
    let peer_id_for_log = peer_id.clone();
    let label_for_pending = label.clone();
    let opfs_root_for_log = init.opfs_root.clone();

    wasm_bindgen_futures::spawn_local(async move {
        tracing::info!(
            peer_id = %peer_id_for_log,
            opfs_root = ?opfs_root_for_log,
            mode = %mode_label,
            "spawning worker for new peer"
        );
        let spawn_t0 = now_ms();
        // Build the Worker + cross-Worker control channel manually
        // (rather than `WorkerProxy::spawn`) so we can transfer the
        // control port via the Init postMessage. The broker-side end
        // (`port1`) stays here and gets registered with the
        // EntityApp's `MessagePortBroker` at drain time.
        let spawn_result: Result<
            (
                entity_wasm_worker_proxy::WorkerProxy<entity_wasm_worker_proxy::WebTransport>,
                web_sys::MessagePort,
            ),
            entity_wasm_worker_proxy::ProxyError,
        > = async {
            let worker = web_sys::Worker::new(&worker_loader_url())
                .map_err(|e| entity_wasm_worker_proxy::ProxyError::WorkerSpawn(format!("{e:?}")))?;
            let mc = web_sys::MessageChannel::new()
                .map_err(|e| entity_wasm_worker_proxy::ProxyError::WorkerSpawn(format!(
                    "MessageChannel::new failed: {e:?}"
                )))?;
            let port_main = mc.port1();
            let port_worker = mc.port2();
            let transport = entity_wasm_worker_proxy::WebTransport::with_control_port(
                worker, port_worker,
            );
            let proxy = entity_wasm_worker_proxy::WorkerProxy::new(transport, init).await?;
            Ok((proxy, port_main))
        }.await;

        match spawn_result {
            Ok((proxy, control_port_main_side)) => {
                let spawn_ms = spawn_t0.map(|t| now_ms().unwrap_or(t) - t);
                tracing::info!(
                    peer_id = %peer_id_for_log,
                    spawn_ms = ?spawn_ms,
                    "worker spawn ok — queuing attachment"
                );
                pending.borrow_mut().push(PendingSdkAttachment {
                    proxy,
                    primary_in_sdk: peer_id_for_log,
                    peer_mirror,
                    label: label_for_pending,
                    mode,
                    control_port_main_side: Some(control_port_main_side),
                });
            }
            Err(err) => {
                tracing::warn!(error = ?err, "worker spawn failed");
                // Detect the WebKitGTK OPFS gap and surface a clean
                // explanation instead of the raw `Reflect.get` stack
                // trace. Tauri Linux runs WebKitGTK ≤ 2.52 which
                // doesn't expose `navigator.storage.getDirectory` to
                // workers — see memory `project_tauri_webview_strategy`.
                // The persisted keypair is orphaned (the worker can't
                // boot it on reload either), so clean it up.
                let err_str = format!("{:?}", err);
                let is_opfs_gap = err_str.contains("OPFS unavailable") // i18n-ignore — Debug-format match key
                    || err_str.contains("storage.getDirectory");
                if is_opfs_gap {
                    crate::persistence::delete_peer(&peer_id_for_log);
                    if let Some(log) = event_log {
                        log.log(format!(
                            "Cannot create backend (opfs) peer: this runtime \
                             (Tauri/WebKitGTK) doesn't support OPFS in workers. \
                             Try backend (memory) or frontend instead."
                        ));
                    }
                } else if let Some(log) = event_log {
                    log.log(format!("Spawn failed for {}: {:?}", mode_label, err));
                }
            }
        }
    });
}

/// Boot-path helper: queue a worker respawn for a persisted Backend*
/// entry. Called from `new_wasm` / `new_wasm_worker` BEFORE the boot
/// worker spawn so backend workers start in parallel with boot.
#[cfg(target_arch = "wasm32")]
fn respawn_persisted_backend_peer_into(
    pending: std::rc::Rc<std::cell::RefCell<Vec<PendingSdkAttachment>>>,
    entry: crate::persistence::PersistedPeerEntry,
) {
    let peer_id = entry.persisted.keypair.peer_id().to_string();
    let seed = entry.persisted.keypair.secret_key_bytes().to_vec();
    tracing::info!(
        peer_id = %peer_id,
        mode = %entry.mode.label(),
        "Stage 2C: respawning persisted backend peer"
    );
    spawn_worker_sdk_for_peer_into(
        pending,
        None,
        peer_id,
        seed,
        entry.persisted.label,
        entry.mode,
    );
}

/// Async-spawn handoff for a new Worker SDK. Action handlers
/// (`spawn_local`) push into the queue when `WorkerProxy::spawn` ok's;
/// `EntityApp::frame()` drains each tick.
#[cfg(target_arch = "wasm32")]
struct PendingSdkAttachment {
    proxy: entity_wasm_worker_proxy::WorkerProxy<entity_wasm_worker_proxy::WebTransport>,
    primary_in_sdk: String,
    peer_mirror: Vec<crate::peers_worker::PeerInfo>,
    label: Option<String>,
    mode: crate::peer_mode::PeerMode,
    /// Main-thread end of the cross-Worker control channel. `Some` for
    /// Workers spawned via `with_control_port` — the drain registers
    /// this with `EntityApp::xworker_broker` once the Worker SDK is
    /// attached, enabling `xworker://<peer-id>` connects from sibling
    /// Workers to this peer.
    control_port_main_side: Option<web_sys::MessagePort>,
}

/// A durable `frontend-idb` peer whose async `PeerContext` build finished and is
/// queued for synchronous insertion into the primary Direct SDK (drained each
/// frame). `seed` + `label` are carried so the drain can persist the peer
/// app-side (vault + roster) exactly as the in-memory create path does.
#[cfg(target_arch = "wasm32")]
struct PendingIdbPeer {
    ctx: entity_sdk::PeerContext,
    seed: [u8; 32],
    label: Option<String>,
}

/// `setTimeout`-backed async sleep for the inbound access-log poll loop (no
/// timer-crate dep). Mirrors the System Backend window's poll cadence helper.
#[cfg(target_arch = "wasm32")]
async fn access_log_poll_sleep_ms(ms: i32) {
    use wasm_bindgen::JsCast;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(win) = web_sys::window() {
            let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                resolve.unchecked_ref(),
                ms,
            );
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

#[cfg(target_arch = "wasm32")]
impl EntityApp {

    /// WASM constructor — Direct-mode path (peers run in this wasm
    /// context). DOM-only. Worker mode uses
    /// [`new_wasm_worker`]. Stage 1A made the body unconditional on
    /// wasm32 (was previously gated by `feature = "worker"`); kept
    /// `async` for boot-path symmetry with `new_wasm_worker`.
    ///
    /// Stage 2C: Frontend-mode persisted peers load into
    /// the Direct SDK; each Backend* peer is respawned in its own
    /// worker SDK with a stable `opfs_root` so OPFS trees survive
    /// reload.
    /// Returns `(app, idb_active)`. When `use_idb` is set, the primary peer
    /// is backed by a durable **main-thread IndexedDB** store keyed on the
    /// stable system seed (`persistence::system_seed`), so the primary tree
    /// — settings, window state, content — survives reload independent of
    /// any worker. `idb_active` reports whether that succeeded (it falls
    /// back to the ephemeral in-memory primary if IDB is unavailable); the
    /// caller uses it to pick the honest durability banner. `use_idb` is
    /// `false` on the multi-tab-secondary / worker-downgrade paths, which
    /// must not open the shared IDB database (see design §9 multi-tab).
    #[cfg(target_arch = "wasm32")]
    pub async fn new_wasm(use_idb: bool) -> (Self, bool) {
        // Drain any pending OPFS tombstones before any worker spawn —
        // post-spawn the sync access handles would block removeEntry.
        crate::opfs_cleanup::run_at_boot().await;
        // Drop any deleted frontend-idb peer's IndexedDB database now, before it
        // could be reopened (it won't be — it's off the roster), so the delete
        // is race-free.
        crate::idb_cleanup::run_at_boot().await;

        // Set A (`entity_peers`) is now the **key VAULT** (id → keypair) +
        // the cold-boot / pre-migration spawn fallback. The authoritative
        // spawn set is the ROSTER on the always-available main-thread system
        // peer (read after construction below).
        let a_entries = crate::persistence::load_all_peer_entries();
        // BootClass stays keyed on the A/seed signal — NEVER the roster (an
        // empty-but-warm roster would misclassify as cold → clobber). On the
        // IDB arm `had_warm_identity` (system seed already present) decides;
        // on the ephemeral fallback, "did set A hold a Frontend".
        let a_has_frontend = a_entries
            .iter()
            .any(|e| matches!(e.mode, crate::peer_mode::PeerMode::Frontend));

        // Build the primary = the persistent **system peer**. With `use_idb`,
        // it is IDB-durable on the stable system seed (main thread, always
        // available — this is what makes the roster readable BEFORE any data
        // peer spawns); otherwise (or on IDB failure) it is the ephemeral
        // in-memory primary, preserving today's Direct behavior.
        let (mut peer_manager, idb_active, had_warm_identity) = if use_idb {
            let (seed, was_persisted) = crate::persistence::system_seed();
            let keypair = entity_crypto::Keypair::from_seed(seed);
            // MIGRATION INVARIANT (MAP §8, danger site #1 — the scariest one):
            // the seed→peer-id derivation names the IndexedDB database below.
            // Changing how `Keypair::from_seed(..).peer_id()` derives the id
            // renames `entity-peer-{id}` and ORPHANS every user's durable tree
            // (the BUG-A class). Never change identity derivation without a
            // data migration that re-keys the old database.
            let db_name = format!("entity-peer-{}", keypair.peer_id());
            match Peers::new_direct_idb(keypair, &db_name).await {
                Ok(pm) => (pm, true, was_persisted),
                Err(e) => {
                    tracing::warn!(
                        error = ?e,
                        "IDB-backed primary unavailable; falling back to ephemeral \
                         in-memory primary"
                    );
                    (Peers::new_direct(), false, false)
                }
            }
        } else {
            (Peers::new_direct(), false, false)
        };

        let boot_class = crate::boot::BootClass::classify(
            if idb_active { had_warm_identity } else { a_has_frontend },
            idb_active,
        );

        // THE FLIP (Brick 4): spawn the data peers from the system peer's
        // ROSTER (the authority for *which* peers exist), joining each id to
        // its keypair in the vault (set A). On a durable IDB arm the roster
        // tree is already replayed + synchronously readable here. Fall back to
        // set A directly when the roster isn't usable (ephemeral arm, or a
        // cold / not-yet-migrated profile whose roster is still empty — the
        // backfill in `boot_load` populates it for next boot). A roster entry
        // with no vault key is a failed/orphan identity: skipped + logged loud
        // (surfaced as a "stopped" row by the Peers window), never spawned.
        let spawn_entries: Vec<crate::persistence::PersistedPeerEntry> = {
            let roster = if idb_active {
                crate::roster::read_roster(&peer_manager)
            } else {
                Vec::new()
            };
            if roster.is_empty() {
                a_entries
            } else {
                let sys = peer_manager.system_peer_id().to_string();
                let mut vault: std::collections::HashMap<String, crate::persistence::PersistedPeerEntry> =
                    a_entries
                        .into_iter()
                        .map(|e| (e.persisted.keypair.peer_id().to_string(), e))
                        .collect();
                let mut out = Vec::new();
                for r in &roster {
                    if r.peer_id == sys {
                        continue; // the system peer is already the hosted primary
                    }
                    match vault.remove(&r.peer_id) {
                        Some(mut entry) => {
                            entry.mode = r.mode; // roster mode is authoritative
                            out.push(entry);
                        }
                        None => tracing::warn!(
                            peer_id = %r.peer_id,
                            "roster entry has no vault key — failed/orphan identity, not spawned"
                        ),
                    }
                }
                // SELF-HEAL (audit HIGH-1/2): vault keys with NO roster entry are
                // NOT deletions. Set A is mutated SYNCHRONOUSLY (localStorage) while
                // the roster rides ASYNC IDB write-behind — so a create-then-fast-
                // reload (roster put not yet flushed) or any out-of-band set-A write
                // leaves a live vault key absent from the roster. The roster is
                // authority for *removal intent*, but it must NEVER silently subtract
                // a peer the synchronous vault still holds (that would ghost a freshly
                // created peer). Spawn every leftover vault peer AND re-shadow it into
                // the roster so the two converge. A genuine delete removed the vault
                // line synchronously, so it is correctly absent here.
                if !vault.is_empty() {
                    let handle = peer_manager.writer_handle();
                    for (_id, entry) in vault {
                        let pid = entry.persisted.keypair.peer_id().to_string();
                        if let Some(h) = handle.as_ref() {
                            crate::roster::put_entry(
                                h,
                                &sys,
                                &crate::roster::RosterEntry {
                                    peer_id: pid.clone(),
                                    mode: entry.mode,
                                    label: entry.persisted.label.clone(),
                                },
                            );
                        }
                        tracing::warn!(
                            peer_id = %pid,
                            "set-A peer missing from roster — spawned + re-shadowed \
                             (create/delete async-window or out-of-band write); roster converges"
                        );
                        out.push(entry);
                    }
                }
                out
            }
        };
        let (frontend, frontend_idb, backend) = partition_entries(spawn_entries);

        // Queue backend-peer worker spawns (each gets its own OPFS worker —
        // heavy data stays on OPFS by design; only the system peer is IDB).
        let pending: std::rc::Rc<std::cell::RefCell<Vec<PendingSdkAttachment>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        for entry in backend {
            respawn_persisted_backend_peer_into(pending.clone(), entry);
        }

        peer_manager.load_persisted_primary(
            frontend.into_iter().map(|e| e.persisted).collect()
        );

        // Durable this-tab (frontend-idb) peers: reopen each one's own
        // IndexedDB store from its saved seed, so its tree rehydrates instead
        // of coming back empty. Only meaningful when the primary is IDB-durable
        // (`idb_active`); on the ephemeral fallback there is no durable arm to
        // host them, so we skip loudly rather than silently masquerade as saved.
        if idb_active {
            for entry in frontend_idb {
                let label = entry.persisted.label.clone();
                let keypair = entry.persisted.keypair;
                let pid = keypair.peer_id().to_string();
                if let Err(e) = peer_manager
                    .replay_persisted_idb_peer(keypair, label)
                    .await
                {
                    tracing::warn!(
                        peer_id = %pid,
                        error = %e,
                        "failed to rehydrate persistent this-tab (idb) peer; its \
                         tree is unavailable this session"
                    );
                }
            }
        } else if !frontend_idb.is_empty() {
            tracing::warn!(
                count = frontend_idb.len(),
                "persistent this-tab (idb) peers present but the primary is on the \
                 ephemeral in-memory fallback (no durable arm) — not rehydrated \
                 this session"
            );
        }
        let mut app = Self::build_wasm_app(peer_manager, pending);
        // Direct arm: the system-peer tree is durable iff the IDB store came up.
        app.boot_load(boot_class, idb_active).await;
        (app, idb_active)
    }

    /// Worker-mode WASM constructor. Spawns the worker, awaits Ready,
    /// wraps as `Peers::Worker`, hands off to the common builder.
    /// Skips Direct-only init (ingest, event bridges); writers/signal
    /// become no-op stubs in Worker mode.
    #[cfg(target_arch = "wasm32")]
    pub async fn new_wasm_worker() -> Result<Self, wasm_bindgen::JsValue> {
        use entity_wasm_worker_protocol::{InitParams, PersistedPeer as WirePersistedPeer};
        use entity_wasm_worker_proxy::WorkerProxy;

        // Drain any pending OPFS tombstones before any worker spawn —
        // post-spawn the sync access handles would block removeEntry.
        crate::opfs_cleanup::run_at_boot().await;
        // Drop any deleted frontend-idb peer's IndexedDB database now, before it
        // could be reopened (it won't be — it's off the roster), so the delete
        // is race-free.
        crate::idb_cleanup::run_at_boot().await;

        // Stage 2C: load all entries, partition by mode. Frontend-mode
        // peers go into the boot worker's SDK (primary + additional).
        // Backend* peers each get their own worker, spawned post-build.
        let entries = crate::persistence::load_all_peer_entries();
        let (mut frontend, frontend_idb, backend) = partition_entries(entries);
        // NAMED DEFERRED HOLE (design arm-matrix): a durable this-tab (idb) peer
        // is a main-thread Direct/IDB construct; the Worker-primary posture has
        // no main-thread SDK to host it. Rather than drop the peer, fold it into
        // the worker cohort so its IDENTITY survives — but its IndexedDB tree is
        // inaccessible this session. Worker-primary is opt-in + de-emphasized;
        // building the cross-arm hosting is deferred until it returns (if ever).
        if !frontend_idb.is_empty() {
            tracing::warn!(
                count = frontend_idb.len(),
                "worker-primary boot: durable this-tab (idb) peers cannot be \
                 main-thread-durable under the Worker arm — loading identity-only, \
                 their IndexedDB trees are inaccessible this session (deferred hole)"
            );
            frontend.extend(frontend_idb);
        }
        // BootClass computed ONCE, BEFORE the cold-boot keypair generate
        // below (reframe §2.2). Worker arm = durable tree (OPFS journal
        // replayed before Ready), so a returning identity is warm-durable
        // and its persisted state must never be clobbered.
        let boot_class = crate::boot::BootClass::classify(!frontend.is_empty(), true);
        if frontend.is_empty() {
            // First launch on a fresh profile (no localStorage). Direct
            // mode's `PeerManager::new()` auto-generates a primary
            // keypair; mirror that here so worker mode boots cleanly on
            // first run instead of failing.
            let keypair = entity_crypto::Keypair::generate();
            let peer_id = keypair.peer_id().to_string();
            crate::persistence::save_peer_with_mode(
                &peer_id,
                &keypair,
                None,
                crate::peer_mode::PeerMode::Frontend,
            );
            tracing::info!(peer_id = %peer_id, "worker bootstrap: generated and persisted fresh primary peer");
            frontend.push(crate::persistence::PersistedPeerEntry {
                persisted: entity_sdk::PersistedPeer {
                    keypair,
                    label: None,
                    sqlite_path: None,
                },
                mode: crate::peer_mode::PeerMode::Frontend,
            });
        }
        let mut persisted: Vec<entity_sdk::PersistedPeer> =
            frontend.into_iter().map(|e| e.persisted).collect();
        let primary = persisted.remove(0);
        let primary_peer_id = primary.keypair.peer_id().to_string();

        // Stage 2C performance: fire backend-peer worker
        // spawns IN PARALLEL with the boot worker's `WorkerProxy::spawn`
        // below. Each is `wasm_bindgen_futures::spawn_local`, so they
        // start now and progress while the main task awaits the boot
        // worker's Ready handshake. The pending queue lives across
        // `build_wasm_app` and gets drained on each frame after the app
        // is constructed.
        let pending: std::rc::Rc<std::cell::RefCell<Vec<PendingSdkAttachment>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        for entry in backend {
            respawn_persisted_backend_peer_into(pending.clone(), entry);
        }

        // Build the peer-info mirror used by main-thread palette /
        // peer-selector reads. Mirrors what the worker's SDK will hold
        // after Init.
        let mut peer_mirror = vec![crate::peers_worker::PeerInfo {
            peer_id: primary.keypair.peer_id().to_string(),
            metadata: entity_sdk::PeerMetadata {
                label: primary.label.clone(),
                persisted: true,
                ..entity_sdk::PeerMetadata::default()
            },
            local: true,
        }];
        for p in &persisted {
            peer_mirror.push(crate::peers_worker::PeerInfo {
                peer_id: p.keypair.peer_id().to_string(),
                metadata: entity_sdk::PeerMetadata {
                    label: p.label.clone(),
                    persisted: true,
                    ..entity_sdk::PeerMetadata::default()
                },
                local: true,
            });
        }

        let to_wire = |p: entity_sdk::PersistedPeer| WirePersistedPeer {
            peer_id: p.keypair.peer_id().to_string(),
            keypair_seed: p.keypair.secret_key_bytes().to_vec(),
            label: p.label,
        };

        // PROTOCOL_VERSION=7: `opfs_root: Option<String>`
        // replaced `enable_opfs: bool`. The boot worker hosts the
        // primary session's persisted peers; we pick a per-session root
        // derived from the primary peer's id so multiple OPFS-backed
        // workers in the same origin don't collide on
        // `createSyncAccessHandle` (upstream U7 fix). Subdirectory is
        // chosen on a per-worker basis, not per-peer, because all peers
        // hosted by one worker share that worker's SDK + store.
        let init = InitParams {
            primary_peer: to_wire(primary),
            additional_peers: persisted.into_iter().map(to_wire).collect(),
            handlers: vec![],
            opfs_root: Some(format!("workers/{}", primary_peer_id)),
        };

        tracing::info!("worker bootstrap: spawning entity-worker-loader.js");
        // Loader file imports entity-worker.js and calls wasm_bindgen()
        // with the explicit wasm URL (worker context has no
        // `document.currentScript`, so the auto-discovery in the
        // wasm-bindgen output can't find the .wasm).
        let spawn_t0 = now_ms();
        // Manual Worker + MessageChannel construction so we can
        // transfer a cross-Worker control port to the boot worker via
        // the Init postMessage. The broker-side end (`port_main`) goes
        // to `build_wasm_app`, which registers every boot peer
        // (primary + persisted Frontends) against it once the broker
        // exists. Without this wiring, the boot worker's peers stay
        // invisible as `xworker://` targets from sibling Workers
        // (the upstream multi-peer reachability fix is the
        // kernel-side prereq).
        let worker = web_sys::Worker::new(&worker_loader_url()).map_err(|e| {
            wasm_bindgen::JsValue::from_str(&format!(
                "boot worker spawn (Worker::new) failed: {e:?}"
            ))
        })?;
        let mc = web_sys::MessageChannel::new().map_err(|e| {
            wasm_bindgen::JsValue::from_str(&format!(
                "boot worker spawn (MessageChannel::new) failed: {e:?}"
            ))
        })?;
        let port_main = mc.port1();
        let port_worker = mc.port2();
        let transport =
            entity_wasm_worker_proxy::WebTransport::with_control_port(worker, port_worker);
        let proxy = WorkerProxy::new(transport, init)
            .await
            .map_err(|e| wasm_bindgen::JsValue::from_str(&format!("worker spawn: {e:?}")))?;
        let spawn_ms = spawn_t0.map(|t| now_ms().unwrap_or(t) - t);
        tracing::info!(
            spawn_ms = ?spawn_ms,
            "worker bootstrap: Ready handshake complete"
        );

        let store = crate::peers_worker::WorkerPeerStore::new(proxy, primary_peer_id, peer_mirror);
        let peers = Peers::new_worker(store);
        let mut app = Self::build_wasm_app_with_boot_control(peers, pending, Some(port_main));
        // Worker arm: the OPFS journal is flush-on-write durable, always.
        app.boot_load(boot_class, true).await;
        Ok(app)
    }

    /// Common WASM bootstrap — runs in both Direct and Worker modes.
    /// Direct-only initialization (embedded-doc ingest, event bridges)
    /// is gated on `peer_manager.primary_as_direct().is_some()`.
    ///
    /// `pending_sdk_attachments` is passed in (not constructed here) so
    /// the boot path can fire backend-peer respawns *before* awaiting
    /// the boot worker; those respawns push into the same queue and
    /// drain on the next frame.
    #[cfg(target_arch = "wasm32")]
    fn build_wasm_app(
        peer_manager: Peers,
        pending_sdk_attachments: std::rc::Rc<std::cell::RefCell<Vec<PendingSdkAttachment>>>,
    ) -> Self {
        Self::build_wasm_app_with_boot_control(peer_manager, pending_sdk_attachments, None)
    }

    /// Same as `build_wasm_app` but with an optional boot-worker
    /// control port. When `Some`, every peer hosted by the primary
    /// SDK (the boot worker in Worker mode) is registered against
    /// the broker so they become reachable as `xworker://` targets
    /// from sibling Workers. `None` is the legacy / Direct path.
    #[cfg(target_arch = "wasm32")]
    fn build_wasm_app_with_boot_control(
        peer_manager: Peers,
        pending_sdk_attachments: std::rc::Rc<std::cell::RefCell<Vec<PendingSdkAttachment>>>,
        boot_control_port: Option<web_sys::MessagePort>,
    ) -> Self {
        // Seed the system peer's knowledge base with the embedded docs.
        // Mode-aware: Direct does sync in-process L0 puts; Worker does
        // fingerprint-gated fire-and-forget dispatch_write. Runs in
        // both modes so the browser/mobile (Worker) build shows docs,
        // not just Tauri (Direct).
        crate::views::knowledge_base::ingest::ingest_embedded_docs(&peer_manager);

        let mut window_manager = WindowManager::new();
        // Single source of the roster (drift-guarded) — the settings
        // startup-surface control reads the same list via
        // `window_registry::standard_window_type_meta`.
        for window_type in crate::window_registry::standard_window_types() {
            window_manager.register_type(window_type);
        }

        // No default window spawn. A Chrome/Full boot opens ZERO windows so
        // the first thing a new user sees is the empty-state tutorial
        // (`dom::build_empty_state`), not a bare Entity Tree. Other postures
        // open their own surface from `boot_load`: `BootSurface::Window`
        // spawns + maximizes its named window, `BootSurface::Site` shows the
        // site overlay. (Previously Direct mode unconditionally spawned Entity
        // Tree here — which also double-spawned alongside a `Window` boot.)

        // Repaint is a no-op — the rAF loop runs every frame.
        // DOM event handlers still call this to signal "something changed"
        // which could be used for smart skipping in the future.
        let repaint_fn: crate::window::RepaintFn = std::rc::Rc::new(|| {});

        let dom = crate::dom::DomRenderer::new(repaint_fn);
        if dom.is_none() {
            tracing::error!("DomRenderer failed to initialize — DOM panels will be empty");
        }

        // Direct-mode only: spawn event bridges for local PeerContexts.
        // Worker mode receives Change events directly via the proxy's
        // subscription pipe — no main-thread bridges needed.
        if let Some(direct) = peer_manager.primary_as_direct() {
            let all_pids: Vec<String> =
                direct.sdk().peer_ids().iter().map(|s| s.to_string()).collect();
            tracing::info!(peer_count = all_pids.len(), "spawning event bridges");
            for pid in &all_pids {
                if let Some(ctx) = direct.peer_context(pid) {
                    let bridge = ctx.event_bridge();
                    wasm_bindgen_futures::spawn_local(bridge);
                }
            }
        } else {
            tracing::info!("worker mode: no main-thread event bridges (worker handles delivery)");
        }

        tracing::info!("EntityApp initialized (WASM, DOM-only)");

        let pending_backend_peers = Arc::new(std::sync::Mutex::new(Vec::new()));
        let system_backend_connect =
            Arc::new(std::sync::Mutex::new(SystemBackendConnect::default()));
        let event_log_writer = EventLogWriter::new(&peer_manager);
        // Sprint #4: route uncaught browser-level errors / unhandled rejections
        // into the in-app Event Log so production failures are visible, not
        // lost to the console. Modest scope — main-thread error/rejection only.
        crate::diagnostics::install_main_thread(event_log_writer.clone());
        let connections_writer = ConnectionsWriter::new(&peer_manager);
        let backend_auth_writer = crate::backend_auth::BackendAuthWriter::new(&peer_manager);
        let connection_health_writer =
            crate::connection_health::ConnectionHealthWriter::new(&peer_manager);
        let mut peer_registry = PeerRegistry::new(&peer_manager);
        // Seed the roster from boot peers (primary + any persisted)
        // so the registry is populated before the first frame.
        peer_registry.sync(&peer_manager);
        // User-theme registry: subscribe the themes prefix now (Worker arm:
        // the observe is what feeds the cache mirror) — the watch starts
        // dirty, so the first frame's sync performs the boot load.
        let user_themes = crate::user_themes::UserThemes::new(&peer_manager);

        // If running in Tauri, fetch persisted backend peers so they
        // appear in the Peers window on startup (as stopped).
        if crate::tauri_ipc::is_tauri() {
            let pending = pending_backend_peers.clone();
            let log = event_log_writer.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::tauri_ipc::list_backend_peers().await {
                    Ok(peers) => {
                        tracing::info!(count = peers.len(), "loaded persisted backend peers");
                        if let Ok(mut q) = pending.lock() {
                            q.extend(peers);
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "failed to list backend peers");
                        log.log(format!("Failed to list backend peers: {}", e));
                    }
                }
            });

            // Auto-provision the canonical system backend peer (B) and arm the
            // S→B auto-connect (Phase 1c). `ensure_system_backend` is idempotent
            // server-side (stable `system-backend` identity), so a reload just
            // re-dials the persisted backend. S is passed as the manager so B
            // self-seeds S's manager grant. This replaces the manual
            // create → start → connect dance. DESIGN-SYSTEM-BACKEND-PEER §10.
            let pending = pending_backend_peers.clone();
            let connect = system_backend_connect.clone();
            let log = event_log_writer.clone();
            let manager_pid = peer_manager.system_peer_id().to_string();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::tauri_ipc::ensure_system_backend(&manager_pid).await {
                    Ok(info) => {
                        tracing::info!(
                            peer_id = %info.peer_id,
                            ws_addr = ?info.ws_addr,
                            "system backend ensured"
                        );
                        log.log(format!(
                            "System backend up: {}",
                            info.ws_addr.as_deref().unwrap_or("?")
                        ));
                        // Register its (running) metadata so it shows in Peers,
                        // then arm the auto-connect (dialed from the frame loop).
                        if let Ok(mut q) = pending.lock() {
                            q.push(info.clone());
                        }
                        if let Ok(mut c) = connect.lock() {
                            c.target = Some(info);
                        }
                    }
                    Err(e) => {
                        // D13 — surface loudly; never a silent "no target".
                        tracing::error!(error = %e, "system backend failed to start");
                        log.log(format!("System backend didn't start: {}", e));
                    }
                }
            });
        }

        let xworker_broker = std::rc::Rc::new(
            entity_wasm_worker_proxy::MessagePortBroker::new(),
        );

        // Boot-worker cross-Worker wiring: when `new_wasm_worker`
        // spawned the boot worker with a control port, register every
        // peer the boot worker hosts (primary + persisted Frontends)
        // against the broker, all pointing at the same control port.
        // Sibling Workers can then resolve `xworker://<any-boot-peer-id>`
        // and the boot worker's `ControlPortClient` dispatches by
        // `to_peer` to the matching local listener.
        //
        // Stash the port (clone is cheap — JsValue handle to the same
        // JS MessagePort) so `create_frontend_peer` can register
        // dynamically-created Frontends against the same control port
        // when their `Request::CreatePeer` round-trip completes. Without
        // this, runtime-added Frontends are silently unreachable via
        // `xworker://` (the boot worker's host binds a listener for
        // them per the multi-peer reachability fix, but the broker has no route).
        let stashed_boot_port = boot_control_port.clone();
        if let Some(port) = boot_control_port {
            let boot_pids = peer_manager.peer_ids();
            for pid in &boot_pids {
                xworker_broker.register_peer(pid.clone(), port.clone());
            }
            tracing::info!(
                peer_count = boot_pids.len(),
                "registered boot-worker peers with xworker broker"
            );
        }

        // Site Mode: wire the always-on status-bar toggle into the
        // renderer's shared action sink and construct the overlay. The
        // overlay's constructor registers the Worker-arm cache
        // subscriptions its render reads (site-mode state, site content,
        // site-origins) — that registration must happen here, synchronously,
        // so the awaited boot-load step's durable get yields into already-
        // queued `observe` tasks (reframe §2.4 ordering note).
        //
        // The mode entity and any remote-fixture origin are NOT seeded here.
        // Those fire-and-forget Stage-4 writes were the Phase-21 ordering
        // race; they now live in the owned, awaited `boot_load` step that
        // runs before the rAF loop arms.
        // The overlay subscribes the session-config prefix, which is
        // system-owned (F-SYS-1) — bind it to the system peer so its
        // subscription covers boot_load's config write even after the
        // future system/primary split.
        let system_pid = peer_manager.system_peer_id().to_string();
        let status_closures = crate::window::new_closure_vec();
        let mut site_overlay = None;
        if let Some(ref dom) = dom {
            crate::dom::util::wire_site_toggle(
                dom.action_sink(),
                dom.repaint_handle(),
                &status_closures,
            );
            site_overlay = Some(crate::dom::site_overlay::SiteOverlay::new(
                &system_pid,
                &peer_manager,
            ));
        }

        // App-tier access log: install ONE inspect sink on the system peer,
        // session-lived, so local dispatches (queries/counts/local executes)
        // land in the shared `access_log_store` with `actor` = system peer.
        // App-global (not per-window) so the Access Log window is a pure reader
        // and multiple windows can't double-record. The outbound-remote half
        // (file transfers etc.) is captured separately in `ops::execute`.
        let access_log_sink = {
            let actor = peer_manager.system_peer_id().to_string();
            let actor_for_cb = actor.clone();
            match peer_manager.install_inspect_sink(&actor, move |fact| {
                if let Some(entry) = crate::access_log_store::dispatch_to_entry(&actor_for_cb, fact) {
                    crate::access_log_store::global().record(entry);
                }
            }) {
                Ok(handle) => Some(handle),
                Err(e) => {
                    tracing::warn!(error = %e, "access log: system-peer inspect sink failed to install");
                    None
                }
            }
        };

        // Inbound half of the access log: under Tauri, poll the native backend's
        // `backend_access_log_tail` (who reached into this device's share) and
        // fold each row into the same app-global store, tagged Inbound. Session-
        // lived, cursor-persistent (no dupes across window opens); store.record
        // wakes the Access Log window. No-op in the browser (no native backend).
        if crate::tauri_ipc::is_tauri() {
            wasm_bindgen_futures::spawn_local(async move {
                let mut cursor = 0u64;
                loop {
                    if let Ok(tail) = crate::tauri_ipc::backend_access_log_tail(cursor).await {
                        cursor = tail.cursor;
                        for r in tail.records {
                            let (target_peer, handler) =
                                crate::access_log_store::parse_target(&r.target_uri);
                            crate::access_log_store::global().record(
                                crate::access_log_store::AccessEntry {
                                    direction: crate::access_log_store::AccessDirection::Inbound,
                                    actor: r.caller,
                                    target_peer,
                                    handler,
                                    operation: r.operation,
                                    resource: None,
                                    outcome: crate::access_log_store::classify(r.status),
                                    detail: format!("status {}", r.status),
                                },
                            );
                        }
                    }
                    access_log_poll_sleep_ms(1000).await;
                }
            });
        }

        Self {
            peer_manager,
            window_manager,
            event_log_writer,
            connections_writer,
            backend_auth_writer,
            connection_health_writer,
            peer_registry,
            user_themes,
            access_log_sink,
            dom,
            pending_backend_peers,
            system_backend_connect,
            pending_sdk_attachments,
            pending_idb_peers: std::rc::Rc::new(std::cell::RefCell::new(Vec::new())),
            xworker_broker,
            boot_control_port: stashed_boot_port,
            status_closures,
            last_site_state: None,
            site_overlay,
            maximized_window: None,
            site_deeplink_active: false,
            // Read the escape hatch synchronously at boot so it's honored from
            // the first frame (a locked deployment must be recoverable even if
            // boot_load is slow / the home is unreachable).
            chrome_override: chrome_override(),
            // Safe default: refuse creation until boot_load proves durability.
            // The window between construction and boot_load (no rAF frame runs
            // yet, so no create action can fire) makes this purely defensive.
            can_persist: false,
            last_status_text: None,
        }
    }

    /// The owned, awaited boot-load step (reframe §2.4). Runs after the
    /// synchronous builder constructs `Self` and **before** `main.rs` arms
    /// the rAF loop, so every boot seed is durable and sequenced rather than
    /// a fire-and-forget write racing the first frame — the root cause of
    /// the Phase-21 race and the clobber bug (they are the same disease:
    /// boot was unowned).
    ///
    /// Sequence:
    ///   1. Resolve the [`SessionConfig`](crate::session_config) spine against
    ///      the **durable** tree: preserve the persisted config (`profile` /
    ///      `boot_surface` / `home_site` / posture — the clobber-safe part) and
    ///      derive the runtime surface (`active`) from `boot_surface`, then
    ///      write it awaited + cache-reflected. Boot lands per config,
    ///      deterministically.
    ///   2. If a remote-fixture origin is requested (e2e / showcase only),
    ///      seed it durably via `put_if_absent` and **await** it before
    ///      navigating the overlay, so the overlay's first resolve reads a
    ///      populated origin instead of racing the write.
    ///
    /// The ordering is load-bearing on the Worker arm: step 1's durable `get`
    /// runs first, and its `.await` is the executor yield that lets the
    /// overlay's already-queued `observe` tasks register their subscriptions —
    /// so the subsequent `put_and_wait`s' cache reflection is honored
    /// (`covered`) rather than silently skipped.
    #[cfg(target_arch = "wasm32")]
    /// `durable_substrate`: whether the system peer's tree persists across a
    /// reload THIS session (Worker+OPFS = always true; Direct = `idb_active`;
    /// ephemeral / multi-tab-secondary = false). This is NOT
    /// `boot_class.tree_is_durable()` — that is true only for `WarmDurable`
    /// (a *returning* durable boot), whereas a fresh durable boot is `Cold`.
    /// The roster backfill/reconcile need "is the substrate durable", which
    /// `BootClass` loses on a cold boot (durable + ephemeral first-boots both
    /// classify `Cold`), so the caller threads the real bit here.
    async fn boot_load(&mut self, boot_class: crate::boot::BootClass, durable_substrate: bool) {
        // 1a durability gate (MAP §10): record whether this tab can durably
        // save, so `CreatePeerWithMode` can refuse on an ephemeral/secondary
        // primary instead of writing a peer whose tree evaporates on reload.
        // This is the same bit that gates the roster backfill below — one
        // authoritative durability signal, set before the rAF loop arms.
        self.can_persist = durable_substrate;

        // Generous: a Worker-arm seed is two round-trips (durable get, then
        // put + cache poll). Boot is one-shot, off the per-frame path.
        const SEED_TIMEOUT_MS: u32 = 5_000;
        let primary_pid = self.peer_manager.primary_peer_id().to_string();
        // The session/startup config is GLOBAL → it lives on the **system
        // peer** (handoff §4.2), reached through the single accessor so the
        // future system/primary split is a pure change. Today system == primary.
        let system_pid = self.peer_manager.system_peer_id().to_string();
        tracing::info!(
            boot_class = boot_class.label(),
            cold = boot_class.is_cold(),
            durable_tree = boot_class.tree_is_durable(),
            primary = %primary_pid,
            system = %system_pid,
            "boot_load: begin"
        );

        // (0) One-time roster backfill (Brick 3): shadow the durable spawn-list
        // (set A, localStorage `entity_peers`) into the authoritative roster on
        // the system peer, keyed on each peer's seed-DERIVED id (BUG-A
        // invariant). The ONE sanctioned bulk write of the roster (model
        // invariant 2); steady-state is per-op dual-write only.
        //
        // Gated by (a) `tree_is_durable` — shadowing an ephemeral/secondary-tab
        // roster is pointless and must not set the done-flag (so a later durable
        // boot still backfills), and (b) the localStorage `entity_roster_migrated`
        // flag, NOT a roster tree read (the Worker sync mirror would read the
        // unwatched roster prefix as silently empty → re-backfill every boot).
        // Runs after primary construction, so it captures the cold-boot fresh
        // primary already written to set A during construction (app.rs ~516).
        let roster_migrated_before = crate::persistence::roster_migration_done(&system_pid);
        if durable_substrate && !roster_migrated_before {
            let entries = crate::persistence::load_all_peer_entries();
            if let Some(handle) = self.peer_manager.writer_handle() {
                for e in &entries {
                    let peer_id = e.persisted.keypair.peer_id().to_string();
                    crate::roster::put_entry(
                        &handle,
                        &system_pid,
                        &crate::roster::RosterEntry {
                            peer_id,
                            mode: e.mode,
                            label: e.persisted.label.clone(),
                        },
                    );
                }
                // Flush before marking done: on the IDB arm the puts are
                // write-behind, so a crash before the debounce would lose the
                // backfill while the flag wrongly suppresses a retry. Worker/OPFS
                // is flush-on-write (checkpoint None → treat as flushed).
                let flushed = match self.peer_manager.idb_checkpoint() {
                    Some(cp) => cp.checkpoint().await.is_ok(),
                    None => true,
                };
                if flushed {
                    crate::persistence::mark_roster_migration_done(&system_pid);
                    tracing::info!(count = entries.len(), "roster backfill: shadowed set A into the roster");
                } else {
                    tracing::warn!(count = entries.len(), "roster backfill: checkpoint failed; will retry next boot");
                }
            }
        }

        // (0b) Reconcile honesty (Brick 3 gate + D13 surface): prove the roster
        // shadow-matches the durable spawn-list (set A). A drift is logged LOUD
        // (silence-is-the-enemy), and the e2e greps this after create/delete.
        // Read via the async L1 path — the sync mirror is Worker-blind for the
        // unwatched roster prefix.
        //
        // Only on a boot where the roster was DURABLY REPLAYED, never the boot
        // that just backfilled it (`roster_migrated_before`): the backfill puts
        // are fire-and-forget on the Worker arm, so an immediate same-boot L1
        // read could race them and report a false DRIFT. After a reload the
        // OPFS/IDB roster is replayed before this runs — no race.
        if durable_substrate && roster_migrated_before {
            let roster = crate::roster::read_roster_async(&self.peer_manager).await;
            let spawn_list = crate::roster::spawn_list_derived();
            let report = crate::roster::reconcile_report(&roster, &spawn_list);
            if report.is_clean() {
                tracing::info!(
                    roster = roster.len(),
                    spawn_list = spawn_list.len(),
                    "roster reconcile: CLEAN (roster shadow-matches set A)"
                );
            } else {
                tracing::warn!(
                    missing = ?report.missing_from_roster,
                    extra = ?report.extra_in_roster,
                    mode_mismatch = ?report.mode_mismatch,
                    "roster reconcile: DRIFT (roster disagrees with set A) — repairing"
                );
                // REPAIR (audit HIGH-2 + review MEDIUM-1): set A is the
                // synchronous truth; the roster lags it across the async write path.
                // Converge the roster toward set A in BOTH directions — symmetric so
                // it works on the Worker arm too (`new_wasm_worker` has no spawn-time
                // self-heal; only the Direct `new_wasm` does):
                //   - `missing_from_roster` (in set A, absent from the roster — a
                //     dropped fire-and-forget Worker roster put, or a cross-session
                //     gap) → RE-SHADOW from set A (labels pulled from the vault).
                //     Safe because set A IS the live-peer floor: anything in it
                //     SHOULD be in the roster. (On the Direct arm these are usually
                //     already healed at spawn time; re-putting is idempotent.)
                //   - `extra_in_roster` (in the roster, gone from set A — a delete
                //     whose async roster-removal lagged) → PRUNE. This is the only
                //     destructive roster op; it is provably safe against live peers
                //     ONLY because both sides key on the seed-DERIVED id
                //     (`keypair.peer_id()`, the BUG-A invariant) — never the stored
                //     `peer_id` field. Any future write path that stores the field id
                //     instead would make this prune drop a live peer; keep them derived.
                // Idempotent; stops a transient drift from festering into permanent
                // DRIFT. Checkpoint-flush on the IDB arm (Worker = flush-on-write).
                if let Some(handle) = self.peer_manager.writer_handle() {
                    if !report.missing_from_roster.is_empty() {
                        let entries = crate::persistence::load_all_peer_entries();
                        let by_id: std::collections::HashMap<String, &crate::persistence::PersistedPeerEntry> =
                            entries
                                .iter()
                                .map(|e| (e.persisted.keypair.peer_id().to_string(), e))
                                .collect();
                        for id in &report.missing_from_roster {
                            if let Some(e) = by_id.get(id) {
                                crate::roster::put_entry(
                                    &handle,
                                    &system_pid,
                                    &crate::roster::RosterEntry {
                                        peer_id: id.clone(),
                                        mode: e.mode,
                                        label: e.persisted.label.clone(),
                                    },
                                );
                            }
                        }
                    }
                    for id in &report.extra_in_roster {
                        crate::roster::remove_entry(&handle, &system_pid, id);
                    }
                    if let Some(cp) = self.peer_manager.idb_checkpoint() {
                        let _ = cp.checkpoint().await;
                    }
                    tracing::info!(
                        shadowed = report.missing_from_roster.len(),
                        pruned = report.extra_in_roster.len(),
                        "roster reconcile: repaired toward set A (the live-peer floor)"
                    );
                }
            }
        }

        // (1) The session config spine (§4-A). Two distinct concerns, and
        // getting them apart is the clobber fix:
        //   * CONFIG (`profile` / `boot_surface` / `home_site` / posture) is
        //     durable and must be PRESERVED across a warm boot — re-seeding
        //     the default over a persisted config was the original clobber
        //     bug. We read it from the **durable** tree (L1 `get_entity_async`,
        //     not the cold cache mirror), defaulting only when truly absent.
        //   * the runtime surface (`active`, "is the overlay showing now") is
        //     DERIVED at boot from `boot_surface` — boot lands where the config
        //     says, NOT wherever a previous session's toggle last left it. A
        //     persisted runtime flag driving the next boot is exactly the kind
        //     of incidental, non-deterministic boot the reframe removes. (The
        //     *location* a user left off at persists separately; this is only
        //     the surface.)
        // The write is AWAITED + cache-reflected (`put_and_wait`, covered by
        // the overlay's session-config subscription) so the first
        // `apply_site_mode` reads the correct surface — no boot-write race.
        {
            let cfg_path = crate::session_config::state_path(&system_pid);
            let durable = self
                .peer_manager
                .get_entity_async(&system_pid, &cfg_path)
                .await
                .ok()
                .flatten();
            // Whether a durable session config PRE-EXISTED this boot — the
            // honest "is there persisted browse state to preserve?" signal. Used
            // below to decide whether to re-point the overlay at the configured
            // home. Captured BEFORE `durable` is moved into `cfg`. NOTE: this is
            // deliberately NOT `boot_class.tree_is_durable()` — on the IDB arm
            // `boot_class` is unreliable (the multi-tab election's
            // `system_seed_id()` persists the system seed before `new_wasm`
            // measures `was_persisted`, so a fresh boot misclassifies as
            // WarmDurable). The config-presence read is the direct, arm-proof
            // signal — same lesson as the roster backfill's `durable_substrate`.
            let config_was_absent = durable.is_none();
            // Cut 2b: when there's NO durable config, fetch the per-domain
            // deployment config (`/entity-deployment.json`) and apply it over
            // the build default — precedence: persisted > fetched > build-time.
            // A returning user's persisted config always wins, so we only fetch
            // on a fresh deployment. Best-effort: a 404 / unparseable doc
            // returns `None` and the build default stands (D16). `deployment`
            // is also threaded into the origin-registration below.
            let mut deployment = if durable.is_none() {
                crate::deployment_config::fetch().await
            } else {
                None
            };
            // Absent durable config → the per-domain deployment config applied
            // over the build-time default surface (§5, `ENTITY_STARTUP_SURFACE`
            // + `ENTITY_HOME_*`), NOT a hard `Chrome` default — so a `site`
            // deployment (baked OR fetched) cold-boots into its surface. A
            // persisted config (the `Some` arm) always wins; this only shapes a
            // fresh or wiped deployment.
            let mut cfg = match durable {
                Some(e) => crate::session_config::SessionConfig::from_entity(&e),
                None => {
                    let base = crate::session_config::boot_default();
                    match &deployment {
                        Some(dc) => dc.apply_to(base),
                        None => base,
                    }
                }
            };
            // DERIVE the runtime surface from the durable `boot_surface` — boot
            // lands where config says, not wherever a previous session's toggle
            // last left it. Everything else on the entity (boot_surface,
            // home_site, posture) is PRESERVED — re-seeding a default over a
            // persisted config was the original clobber bug.
            cfg.active = cfg.active_from_boot_surface();
            match self
                .peer_manager
                .put_and_wait(&system_pid, &cfg_path, cfg.to_entity(), SEED_TIMEOUT_MS)
                .await
            {
                Ok(()) => tracing::info!(
                    boot_surface = %cfg.boot_surface.describe(),
                    active = cfg.active,
                    home_site = %cfg.home_site.id,
                    show_toggle = cfg.site_mode.show_toggle,
                    "boot_load: session config resolved (config preserved, surface derived)"
                ),
                Err(e) => tracing::error!(error = %e, "boot_load: session config write failed"),
            }

            // Keep the pre-peer fast-paint kill switch (localStorage mirror, cut
            // 2c) in sync with the durable config. The mirror is what `start()`
            // reads BEFORE the peer exists; refreshing it here self-heals it for
            // the next boot from whatever the persisted config now says (the
            // settings toggle also writes it immediately for this-reload effect).
            crate::boot_fast_paint::write_enabled_mirror(cfg.fast_paint);

            // (1.2.5) Warm-boot origin RECONCILE (P1, symptom 2 — "site source
            // unreachable"). On a warm boot we deliberately don't re-fetch the
            // deployment config: POSTURE (profile / home / toggle) is a user
            // preference, preserved from the durable config. But the site-origin
            // REGISTRY is a routing FACT, not a preference — and it rides the
            // same cold-only fetch gate above, so a warm boot leans entirely on
            // the origin that the FIRST cold boot seeded durably. If that entry
            // is ever missing or stale (a failed first seed, a wiped origins
            // entry, a served-port change), the home is stranded "unreachable"
            // with no way to self-heal — and in a locked deployment the user
            // can't reach Settings to fix it. So: when the home is a remote
            // thin-lens peer AND its origin is NOT durably registered, re-fetch
            // the served config and let the (1.3) loop below re-apply ONLY the
            // origins map (put_if_absent — a user override still wins; posture
            // stays the persisted value). Routing self-heals; posture doesn't move.
            if deployment.is_none() {
                let home_peer = cfg.home_site.peer_id.clone();
                let home_is_local = home_peer.is_empty() || home_peer == system_pid;
                if !home_is_local {
                    let origin_registered = self
                        .peer_manager
                        .get_entity_async(
                            &system_pid,
                            &crate::content_site::origins::origin_path(&system_pid, &home_peer),
                        )
                        .await
                        .ok()
                        .flatten()
                        .is_some();
                    if !origin_registered {
                        tracing::warn!(
                            home_peer = %home_peer,
                            "boot_load: warm boot — home origin missing from the durable \
                             registry; re-fetching the deployment config to reconcile routing"
                        );
                        deployment = crate::deployment_config::fetch().await;
                    }
                }
            }

            // (1.3 cut 2b) Register every HTTP origin the per-domain deployment
            // config declares, durably + `put_if_absent` (a returning user's
            // override wins), under the system peer. This is how a generic
            // bundle on a CDN learns where each hosting peer's published
            // artifacts live — the resolver HTTP-polls these on first browse —
            // without a per-domain WASM rebuild. Covers the home peer's origin
            // too; the `ENTITY_HOME_ORIGIN` env fallback in the home-provision
            // branch below only fires when no deployment config supplied it.
            if let Some(dc) = &deployment {
                // Collect the resolved (peer, origin) pairs as we register them,
                // to hand to the site-discovery warm-up below — sourced here
                // rather than read back via `list_origins` because the Worker-arm
                // cache mirror isn't seeded for the freshly-written registry
                // prefix yet at boot (`feedback_worker_cache_get_needs_subscription`).
                let mut warm_targets: Vec<(String, String)> = Vec::new();
                for (target_peer, origin) in &dc.origins {
                    // Expand `""`/`self` to the SPA's own origin (the portable
                    // same-origin CDN case — the registry treats an empty origin
                    // as unregistered, so we store the concrete URL).
                    let Some(resolved) = crate::deployment_config::expand_origin(origin) else {
                        tracing::warn!(
                            target_peer = %target_peer,
                            "boot_load: deployment-config origin is same-origin but \
                             window.location.origin is unavailable — skipping"
                        );
                        continue;
                    };
                    match self
                        .peer_manager
                        .put_if_absent(
                            &system_pid,
                            crate::content_site::origins::origin_path(&system_pid, target_peer),
                            crate::content_site::origins::origin_entity(&resolved),
                            SEED_TIMEOUT_MS,
                        )
                        .await
                    {
                        Ok(seeded) => tracing::info!(
                            target_peer = %target_peer,
                            origin = %resolved,
                            seeded,
                            "boot_load: registered deployment-config origin"
                        ),
                        Err(e) => tracing::error!(
                            error = %e,
                            target_peer = %target_peer,
                            "boot_load: deployment-config origin seed failed"
                        ),
                    }
                    warm_targets.push((target_peer.clone(), resolved));
                }
                // Roster summary — the multi-tenant signal (design §8): how many
                // hosted-peer origins this domain declares. >1 means a
                // multi-tenant umbrella (the read-side `origins::list_origins`
                // is the canonical reachable roster the browse-all front-door
                // reads; this is the boot-time, race-free declared count).
                tracing::info!(
                    hosted_peer_origins = dc.origins.len(),
                    "boot_load: domain deployment hosts {} peer origin(s)",
                    dc.origins.len()
                );
                // Warm each registered peer's site index NOW — fetch its
                // `sites.list` + manifests and write them through into my store —
                // so a foreign published peer's sites appear in the directory rail
                // on FIRST paint instead of only after a manual navigate. The
                // origin registration above records *where* a peer is; this
                // enumerates *what* it hosts (fire-and-forget; manifest-pinned).
                crate::content_site::discovery::warm_peer_sites(
                    &self.peer_manager,
                    &system_pid,
                    warm_targets,
                );
            }

            // (1.4) Provision the home site — thin-lens, not eager warehouse
            // (boot-closure reframe). The bundled demo is seeded
            // ONLY when `home_site` is the LOCAL demo (empty/system peer +
            // demo id) — the offline floor / dev default. A `home_site`
            // pointing at another peer (a real content-site deployment) seeds
            // NOTHING here; that content materializes lazily from its origin
            // via the resolver on first browse (`origins.rs` + `MultiResolver`,
            // already built + e2e-proven). This replaces the OLD unconditional
            // `ensure_demo_site` in the synchronous `SiteOverlay::new` builder,
            // which seeded the demo on EVERY boot regardless of what the
            // deployment's home site actually is (D5 — boot deps are owned and
            // sequenced, not a construction side-effect; D9 — we seed only what
            // we chose to). Idempotent (gated on the manifest), arm-aware.
            let home_peer = cfg.home_site.peer_id.clone();
            let home_id = cfg.home_site.id.clone();
            let home_loc = cfg.home_site.loc.clone();
            let home_is_local = home_peer.is_empty() || home_peer == system_pid;
            if home_is_local && home_id == crate::views::content_site::DEMO_SITE_ID {
                crate::views::content_site::ensure_demo_site(&self.peer_manager, &system_pid);
                tracing::info!(
                    home_site = %home_id,
                    "boot_load: provisioned bundled demo home site (local offline floor)"
                );
            } else if !home_is_local {
                // Remote thin-lens home (cut 2a): a real content-site
                // deployment whose home lives on ANOTHER peer. We seed NO
                // content — it materializes lazily from the peer's origin via
                // the resolver's HTTP-poll arm on first browse. Two things make
                // that work:
                //   (a) the site-origin registry must know WHERE that peer's
                //       artifacts are. The per-domain deployment config (cut 2b)
                //       is the production source — if it declared the home
                //       peer's origin, it was already registered in the (1.3)
                //       loop above, so here we only need the build-time
                //       `ENTITY_HOME_ORIGIN` env FALLBACK (cut 2a), seeded via
                //       `put_if_absent` (a returning user's override wins), under
                //       the SYSTEM peer (the overlay's peer), AWAITED so the
                //       first resolve reads it.
                //   (b) the overlay was constructed + seeded to the pre-config
                //       demo default BEFORE this config resolved. When NO durable
                //       session config pre-existed (`config_was_absent` — a fresh
                //       or wiped deployment) that seed is wrong, so re-point the
                //       overlay at the configured home. A returning boot that
                //       already has a persisted config + browse location is left
                //       alone (don't clobber where the user left off). We gate on
                //       config-presence, NOT `boot_class.tree_is_durable()`: on
                //       the IDB arm boot_class misclassifies a fresh boot as
                //       WarmDurable (the seed is persisted by the multi-tab
                //       election before new_wasm reads it), which silently SKIPPED
                //       this re-point and stranded a content-site deployment on
                //       the bundled demo (live trace, IDB-default arm).
                let home_origin_from_deployment = deployment
                    .as_ref()
                    .map(|d| d.origins.contains_key(&home_peer))
                    .unwrap_or(false);
                if home_origin_from_deployment {
                    tracing::debug!(
                        home_peer = %home_peer,
                        "boot_load: remote home origin came from the deployment config"
                    );
                } else if let Some(origin) = crate::session_config::home_origin_default() {
                    match self
                        .peer_manager
                        .put_if_absent(
                            &system_pid,
                            crate::content_site::origins::origin_path(&system_pid, &home_peer),
                            crate::content_site::origins::origin_entity(&origin),
                            SEED_TIMEOUT_MS,
                        )
                        .await
                    {
                        Ok(seeded) => tracing::info!(
                            home_peer = %home_peer,
                            origin = %origin,
                            seeded,
                            "boot_load: registered remote home origin (env fallback)"
                        ),
                        Err(e) => tracing::error!(
                            error = %e,
                            "boot_load: remote home origin seed failed"
                        ),
                    }
                } else {
                    tracing::warn!(
                        home_peer = %home_peer,
                        "boot_load: remote home has no registered origin (no deployment config \
                         entry, ENTITY_HOME_ORIGIN unset) — it will only resolve if the origin \
                         is persisted/registered elsewhere"
                    );
                }
                if config_was_absent {
                    if let Some(overlay) = self.site_overlay.as_ref() {
                        let uri = if home_loc.is_empty() {
                            format!("entity://{home_peer}/sites/{home_id}")
                        } else {
                            format!("entity://{home_peer}/sites/{home_id}/pages/{home_loc}")
                        };
                        overlay.navigate(&uri, &self.peer_manager);
                        tracing::info!(
                            target = %uri,
                            "boot_load: pointed overlay at remote home (no durable config)"
                        );
                    }
                }
            } else {
                // Local, non-demo home: a deployment that names a local site
                // other than the bundled demo. We don't seed it (the deployment
                // owns its own content); just record it.
                tracing::info!(
                    home_site = %home_id,
                    "boot_load: home site is local non-demo — not seeding (deployment owns it)"
                );
            }

            // §4-B Surfaces seam: boot straight into a maximized window on a
            // chosen peer. The EFFECTIVE surface this session is the persisted
            // `boot_surface`, except a `?boot_window=[{peer}:]{type}` URL
            // override (e2e/showcase, never production) wins — for the *spawn
            // only*. The override is deliberately NOT folded into `cfg` above,
            // so it can't clobber the durable config (a normal reload still
            // honors the real persisted surface). In production
            // `effective_surface` IS `cfg.boot_surface`, so a persisted `Window`
            // boots identically.
            let effective_surface = boot_window_override()
                .map(|raw| {
                    // `{peer}:{type}` or bare `{type}` (peer empty → system).
                    let (peer_id, window_type) = match raw.split_once(':') {
                        Some((p, t)) => (p.to_string(), t.to_string()),
                        None => (String::new(), raw),
                    };
                    crate::session_config::BootSurface::Window { peer_id, window_type }
                })
                .unwrap_or_else(|| cfg.boot_surface.clone());
            if let crate::session_config::BootSurface::Window { peer_id, window_type } =
                &effective_surface
            {
                // Resolve the target peer: empty `peer_id` = the system peer
                // (presets/overrides can't bake a runtime id). Then VALIDATE it
                // exists — a config can reference a peer that was since deleted
                // (the delete path self-heals the config, but this is the boot-
                // time backstop, handoff §4 reactive self-heal). A gone peer →
                // fall back to chrome, loudly, rather than spawn into the void.
                let target_peer = if peer_id.is_empty() {
                    system_pid.clone()
                } else {
                    peer_id.clone()
                };
                if !self.peer_manager.peer_ids().iter().any(|p| p == &target_peer) {
                    tracing::warn!(
                        window_type = %window_type,
                        target_peer = %target_peer,
                        "boot_load: BootSurface::Window targets a peer that no longer \
                         exists; falling back to chrome"
                    );
                } else {
                    // Spawn the window on its peer and promote it to the full-
                    // viewport surface via the SAME maximize path used at runtime
                    // (step 5) — no parallel host, no new render path. `active`
                    // stayed false (a `Window` surface is non-overlay via
                    // `active_from_boot_surface`), so the site overlay is off.
                    // Window ids are ephemeral → the durable `(peer, type)` is
                    // the stable identifier, re-spawned each boot; no extra
                    // persistence needed.
                    match self
                        .window_manager
                        .spawn(window_type, &target_peer, &self.peer_manager)
                    {
                        Some(id) => {
                            self.maximized_window = Some(id);
                            tracing::info!(
                                window_type = %window_type,
                                target_peer = %target_peer,
                                window_id = id,
                                "boot_load: booted into maximized window surface"
                            );
                        }
                        None => tracing::warn!(
                            window_type = %window_type,
                            "boot_load: BootSurface::Window names an unknown window type; \
                             staying in chrome"
                        ),
                    }
                }
            }
        }

        // (1.5) Deep-link override: `?site={peer}/{site}/{page}` boots straight
        // into the site overlay at that page — the static→live round-trip ([F3]).
        // Ephemeral like `?boot_window=`: we navigate the overlay + force it on
        // for this session (`site_deeplink_active`), but never write the durable
        // config, so a normal reload honors the persisted surface. `self` ⇒ the
        // system peer (the overlay's own peer, where the demo site is seeded), so
        // a same-origin deep link resolves locally regardless of the publish
        // peer's id; a real peer-id routes through the resolver (origins/HTTP-poll).
        if let Some((peer, site, page)) = site_deeplink_override() {
            let is_self =
                peer == crate::content_site::paths::SELF_PEER || peer.is_empty();
            let target_peer = if is_self { system_pid.clone() } else { peer };
            // A real (non-`self`) peer-id deep link addresses content published
            // same-origin at `/{peer}/sites/…` (the static→live round-trip from a
            // real publish peer — e.g. an ingested corpus's "Open in live peer"
            // banner). The content is NOT on the system peer, so the resolver must
            // HTTP-poll `{peer}`, which needs a same-origin origin entry. Seed one
            // `put_if_absent` (a deployment-config / home origin for this peer
            // already wins) BEFORE navigating, so the first resolve hits HTTP-poll
            // rather than a local read that 404s as "No site manifest". Mirrors the
            // §1.3 deployment-config origin registration above.
            if !is_self {
                if let Some(resolved) = crate::deployment_config::expand_origin("") {
                    match self
                        .peer_manager
                        .put_if_absent(
                            &system_pid,
                            crate::content_site::origins::origin_path(&system_pid, &target_peer),
                            crate::content_site::origins::origin_entity(&resolved),
                            SEED_TIMEOUT_MS,
                        )
                        .await
                    {
                        Ok(seeded) => tracing::info!(
                            target_peer = %target_peer,
                            origin = %resolved,
                            seeded,
                            "boot_load: ?site deep-link — same-origin origin for foreign peer"
                        ),
                        Err(e) => tracing::error!(
                            error = %e,
                            target_peer = %target_peer,
                            "boot_load: ?site deep-link origin seed failed"
                        ),
                    }
                }
            }
            let uri = if page.is_empty() {
                format!("entity://{target_peer}/sites/{site}")
            } else {
                format!("entity://{target_peer}/sites/{site}/pages/{page}")
            };
            if let Some(overlay) = self.site_overlay.as_ref() {
                overlay.navigate(&uri, &self.peer_manager);
                tracing::info!(target = %uri, "boot_load: ?site deep-link → site overlay");
            }
            // Force the overlay on for this session even if the durable surface
            // is Chrome (the deep link is an explicit "show me this site" intent).
            self.site_deeplink_active = self.site_overlay.is_some();
        }

        // (2) Remote-fixture hook (e2e / showcase; never production). Seed
        // the origin durably and AWAIT before navigating — the Phase-21 fix.
        if remote_fixture_requested() {
            tracing::debug!(primary = %primary_pid, "boot_load: remote_fixture — seeding origin then navigating");
            match self
                .peer_manager
                .put_if_absent(
                    &primary_pid,
                    crate::content_site::origins::origin_path(&primary_pid, REMOTE_FIXTURE_PEER),
                    crate::content_site::origins::origin_entity("/remote-fixture"),
                    SEED_TIMEOUT_MS,
                )
                .await
            {
                Ok(seeded) => tracing::info!(seeded, "boot_load: remote-fixture origin"),
                Err(e) => {
                    tracing::error!(error = %e, "boot_load: remote-fixture origin seed failed")
                }
            }
            if let Some(overlay) = self.site_overlay.as_ref() {
                overlay.navigate(
                    &format!("entity://{REMOTE_FIXTURE_PEER}/sites/labs"),
                    &self.peer_manager,
                );
            }
        }

        // (3) Cache-awareness: pre-cache the MANIFESTS of every site each
        // registered foreign origin hosts, so the directory rail lists all of a
        // peer's sites BEFORE any is visited (page bodies still fetch lazily on
        // click). Fire-and-forget — never blocks boot; the rail re-renders
        // reactively as manifests land. (The "browse a bunch of sites" path.)
        self.precache_origin_sites(&system_pid);

        tracing::info!("boot_load: complete");
    }

    /// Fire-and-forget cache-awareness: for each registered foreign origin,
    /// fetch its `sites.list` and pre-cache the MANIFEST of every site it lists
    /// that we don't already hold — written into MY store at the natural
    /// `/{foreign}/sites/{site}/manifest` cached-foreign address + a provenance
    /// record, exactly like the resolver's on-navigate write-through
    /// ([`crate::content_site::resolver`] `persist_to_cache`), but eagerly and
    /// for ALL sites. This is what lets [`crate::content_site::discovery::scan_local_sites`]
    /// enumerate a freshly-published peer's whole site set so the directory rail
    /// shows every site up front. Manifest-only (lightweight); page bodies fetch
    /// lazily on first visit. Idempotent: a sync snapshot of already-cached sites
    /// is taken up front, so a warm boot re-fetches only the per-peer `sites.list`
    /// (cheap) and skips manifests it already holds. Borrows nothing across the
    /// await — grabs a `'static` writer handle + the origin roster first (the
    /// `refresh_site_index` pattern).
    #[cfg(target_arch = "wasm32")]
    fn precache_origin_sites(&self, me: &str) {
        use crate::content_site::{cache, discovery, http_poll, origins, paths};

        let origins_list = origins::list_origins(&self.peer_manager, me);
        // Foreign sites already physically in my store — skip re-fetching these.
        let already: std::collections::BTreeSet<(String, String)> =
            discovery::scan_local_sites(&self.peer_manager, me)
                .into_iter()
                .filter(|r| !r.owned)
                .map(|r| (r.peer, r.site))
                .collect();
        let Some(writer) = self.peer_manager.writer_handle_for(me) else {
            return;
        };
        let me = me.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            let src = http_poll::FetchBinSource;
            for (foreign, origin) in origins_list {
                if foreign == me {
                    continue; // owned sites are already local
                }
                let Ok(site_ids) = http_poll::fetch_sites_list(&src, &origin, &foreign).await
                else {
                    tracing::debug!(peer = %foreign, "precache: peer exposes no sites.list — skipping enumeration");
                    continue;
                };
                let mut cached = 0usize;
                for site in &site_ids {
                    if already.contains(&(foreign.clone(), site.clone())) {
                        continue;
                    }
                    match http_poll::fetch_manifest(&src, &origin, &foreign, site).await {
                        Ok(manifest_ent) => {
                            writer.put(paths::manifest_path(&foreign, site), manifest_ent.clone());
                            let prov = cache::CacheProvenance {
                                last_reconciled: now_ms().unwrap_or(0.0) as u64,
                                pinned_root_hash: cache::manifest_hash_hex(&manifest_ent),
                                source_transport: origin.clone(),
                            };
                            writer.put(cache::provenance_path(&me, &foreign, site), prov.to_entity());
                            cached += 1;
                        }
                        Err(e) => tracing::debug!(
                            peer = %foreign, site = %site, error = ?e,
                            "precache: manifest fetch failed — site stays lazy"
                        ),
                    }
                }
                if cached > 0 {
                    tracing::info!(
                        peer = %foreign, cached, listed = site_ids.len(),
                        "precache: cached foreign site manifests for directory awareness"
                    );
                }
            }
        });
    }

    /// Run one frame — drain actions, render DOM. Called from rAF loop.
    #[cfg(target_arch = "wasm32")]
    pub fn frame(&mut self) {
        // Register any backend peers that were created via async Tauri IPC.
        self.drain_pending_backend_peers();
        // Dial the auto-provisioned system backend from S (Phase 1c) — once B
        // is registered above, connect to it with backoff, iff not already
        // connected. Zero user action; replaces the manual Connect click.
        self.drain_system_backend_connect();
        // Attach any new Worker SDKs spawned for backend-mode peer creation.
        self.drain_pending_sdk_attachments();
        // Insert any durable this-tab (frontend-idb) peers whose async build
        // finished since last frame.
        self.drain_pending_idb_peers();

        let mut actions = Vec::new();
        if let Some(ref mut dom) = self.dom {
            dom.render(&self.peer_manager, &self.window_manager, &mut actions, self.maximized_window);
        }
        if !actions.is_empty() {
            self.process_actions(actions);
        }

        // Reconcile the tree-backed peer roster against the live
        // `Peers`. Derived + idempotent: only rewrites a record when
        // its content changed, so this is a cheap read+compare when
        // nothing moved and self-heals after any lifecycle path
        // (sync create, async worker create/delete landing on a later
        // frame, backend registration). This is the *only*
        // peer-membership reactivity mechanism.
        self.peer_registry.sync(&self.peer_manager);

        // Reconcile the user-theme registry from the tree — one atomic
        // check when the themes prefix hasn't changed (frame-path rule:
        // cheap probe in front of any storage-derived work).
        self.user_themes.sync(&self.peer_manager);

        // Reflect Site Mode (overlay vs chrome) into the DOM. The mode
        // class / toggle apply only on change; the overlay content
        // re-renders every active frame (its own guard skips no-op
        // rebuilds). So a `ToggleSiteMode` processed above (or a
        // `boot_surface: site` config on the first frame) flips the container
        // class and paints the overlay this same frame.
        self.apply_site_mode();
        if self.last_site_state.as_ref().is_some_and(|s| s.active) {
            self.render_site_overlay();
        }

        self.update_status_bar();
    }

    /// Refresh the always-on status bar with a small live summary —
    /// open windows · peers · durability — so the bar carries useful
    /// at-a-glance state instead of a never-changing label. Cheap: only
    /// writes to the DOM when the computed string actually changed.
    #[cfg(target_arch = "wasm32")]
    fn update_status_bar(&mut self) {
        let windows = self.window_manager.open_count();
        let peers = self.peer_manager.peer_ids().len();
        let status = status_summary(windows, peers, self.can_persist);
        if self.last_status_text.as_deref() != Some(status.as_str()) {
            crate::dom::util::set_status_text(&status);
            self.last_status_text = Some(status);
        }
    }

    /// Apply the site-overlay surface to the DOM: container mode class +
    /// status-bar toggle (visibility + symbol). Cheap idempotent guard:
    /// no-ops when the config is unchanged since last frame. The overlay
    /// *content* is rendered separately
    /// ([`render_site_overlay`](Self::render_site_overlay)).
    ///
    /// Maximize is **independent** of this: a maximized window is a
    /// `position: fixed` full-viewport surface (the `.maximized` CSS rule)
    /// that covers everything — including the status bar — on top, and you
    /// can only maximize from windowed chrome, so the two surfaces never
    /// fight. This stays site-only.
    #[cfg(target_arch = "wasm32")]
    fn apply_site_mode(&mut self) {
        let pid = self.peer_manager.system_peer_id().to_string();
        let mut cfg = crate::session_config::read(&self.peer_manager, &pid);
        // A `?site=` deep link ([F3]) forces the overlay on for this session,
        // without persisting — OR it into the per-frame `active` read.
        if self.site_deeplink_active {
            cfg.active = true;
        }
        // `?chrome=1` ([`chrome_override`]) is the operator escape from a locked
        // deployment: force the chrome surface AND expose the toggle/site so the
        // operator can reach Settings (and dip back into the site if they want).
        // Ephemeral — only mutates this per-frame read, never the durable config.
        // Wins over the deep-link force above (an explicit "get me out").
        if self.chrome_override {
            cfg.active = false;
            cfg.site_mode.enabled = true;
            cfg.site_mode.show_toggle = true;
            // The escape forces the CHROME surface for this per-frame read, so
            // the toggle-visibility gate below re-exposes the toggle even for a
            // Window/Site deployment (the operator explicitly asked for chrome).
            cfg.boot_surface = crate::session_config::BootSurface::Chrome;
        }
        if self.last_site_state.as_ref() == Some(&cfg) {
            return;
        }
        crate::dom::util::set_container_mode(if cfg.active {
            "mode-site"
        } else {
            "mode-dom"
        });
        // In Site Mode the overlay fills the whole page — hide the status
        // bar so it reads as a real site; the "Exit Site" control lives in
        // the rendered site's nav bar (see dom::content_site, Overlay host).
        // The status-bar toggle is the chrome-side site entry point — shown
        // only when the toggle is exposed AND a site is actually available
        // (no site configured ⇒ inert, so it can't drop you into an empty
        // surface).
        crate::dom::util::set_status_bar_visible(!cfg.active);
        crate::dom::util::set_site_toggle(cfg.status_toggle_visible(), cfg.active);
        self.last_site_state = Some(cfg);
    }

    /// Render the active content site into `#site-layer` (the overlay's
    /// own equality guard skips no-op rebuilds). Called each frame while
    /// the overlay is active.
    #[cfg(target_arch = "wasm32")]
    fn render_site_overlay(&mut self) {
        let Some(dom) = self.dom.as_ref() else {
            return;
        };
        let sink = dom.action_sink();
        let repaint = dom.repaint_handle();
        if let Some(overlay) = self.site_overlay.as_mut() {
            overlay.render(&self.peer_manager, sink, repaint);
        }
    }


    /// Read the single-instance-windows preference from the global settings
    /// entity (system peer, `settings/ui`). Read live on each spawn so a
    /// toggle takes effect immediately. Defaults to `false` (multi-instance)
    /// when the entity is absent.
    fn singleton_windows_enabled(&self) -> bool {
        let sys = self.peer_manager.system_peer_id();
        let path = crate::app_paths::settings_path(
            crate::app_paths::APP_ID,
            sys,
            crate::views::settings::model::SETTINGS_PATH,
        );
        self.peer_manager
            .get_entity(sys, &path)
            .map(|e| crate::views::settings::model::SettingsState::from_entity(&e).singleton_windows)
            .unwrap_or(false)
    }

    fn process_actions(&mut self, actions: Vec<Action>) {
        for action in &actions {
            match action {
                Action::SpawnWindow { type_name, peer_id } => {
                    let open = self.window_manager.open_count();
                    let pid = peer_id.as_deref()
                        .unwrap_or(self.peer_manager.primary_peer_id());
                    // Single-instance ("immutable") windows: if enabled and a
                    // window of this type is already open for this peer, focus
                    // it instead of spawning a duplicate.
                    if self.singleton_windows_enabled() {
                        if let Some(existing) = self.window_manager.find_open(type_name, pid) {
                            tracing::info!(type_name = %type_name, peer_id = %pid, existing, "SpawnWindow: focusing existing (singleton)");
                            if let Some(dom) = self.dom.as_ref() {
                                dom.focus_window(existing);
                            }
                            continue;
                        }
                    }
                    tracing::info!(type_name = %type_name, peer_id = %pid, open_windows = open, "SpawnWindow");
                    self.window_manager.spawn(type_name, pid, &self.peer_manager);
                }
                Action::CloseWindow(id) => {
                    tracing::info!(window_id = id, "CloseWindow");
                    // Clean up per-window state from the tree. Routes
                    // through Peers::dispatch_remove so it works in
                    // both Direct and Worker modes.
                    if let Some(win) = self.window_manager.get(*id) {
                        let pid = win.view.peer_id().to_string();
                        self.peer_manager.dispatch_remove(
                            &pid,
                            crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, *id),
                        );
                        self.peer_manager.dispatch_remove(
                            &pid,
                            crate::app_paths::window_results_path(crate::app_paths::APP_ID, &pid, *id),
                        );
                        // D9 §2.B: per-panel selection slot is window-scoped;
                        // remove it on close. The app-aggregate slot stays
                        // (other open panels may still hold it).
                        crate::selection::clear_panel_selection_on_close(
                            &self.peer_manager,
                            &pid,
                            *id,
                        );
                    }
                    // Closing the maximized window pops the surface back to
                    // the base (chrome / site).
                    if self.maximized_window == Some(*id) {
                        self.maximized_window = None;
                    }
                    self.window_manager.close(*id);
                }
                Action::ToggleMaximizeWindow(id) => {
                    // One-deep (reframe §4-B decision #3): maximizing replaces
                    // any prior maximized window; toggling the current one
                    // restores it. The `.maximized` class AND the header glyph
                    // are reconciled per-frame from `maximized_window` in the
                    // DOM renderer — we deliberately do NOT mark the affected
                    // windows dirty here. A section rebuild tears down live
                    // DOM-side state the tree can't reconstruct (the Games/Apps
                    // sandboxed iframe resets its running app), so the surface
                    // toggle must be a pure CSS/glyph reconcile, not a rebuild.
                    let prev = self.maximized_window;
                    self.maximized_window =
                        if prev == Some(*id) { None } else { Some(*id) };
                    tracing::info!(
                        window_id = id,
                        maximized = ?self.maximized_window,
                        "Action::ToggleMaximizeWindow"
                    );
                }
                Action::Navigate(id, _)
                | Action::NavigateUp(id)
                | Action::EntityTreeToggleExpand(id, _)
                | Action::EntityTreeSetSearch(id, _)
                | Action::SetSelectionSource(id, _)
                | Action::ShellClear(id) => {
                    if let Some(win) = self.window_manager.get_mut(*id) {
                        win.view.handle_action(action, &self.peer_manager);
                    }
                }
                Action::ShellSubmit { window_id, .. }
                | Action::ShellTabComplete { window_id, .. }
                | Action::ShellHistoryPrev { window_id, .. }
                | Action::ShellHistoryNext { window_id, .. }
                | Action::ShellTail { window_id, .. } => {
                    if let Some(win) = self.window_manager.get_mut(*window_id) {
                        win.view.handle_action(action, &self.peer_manager);
                    }
                }
                Action::WindowEvent { window_id, .. }
                | Action::SiteNavigate { window_id, .. }
                | Action::SiteBack { window_id }
                | Action::SiteOpen { window_id, .. }
                | Action::SiteBookmarkToggle { window_id, .. }
                | Action::SiteKeepToggle { window_id, .. }
                | Action::SiteRailFilter { window_id, .. } => {
                    if let Some(win) = self.window_manager.get_mut(*window_id) {
                        win.view.handle_action(action, &self.peer_manager);
                    }
                }
                Action::ToggleSiteMode => {
                    let pid = self.peer_manager.system_peer_id().to_string();
                    // Honor lockdown: a `locked` deployment (strict-site) has no
                    // chrome↔site toggle, so refuse the exit — the user can't be
                    // stranded in chrome with no way back (BUG-1). The overlay
                    // already hides the "Exit Site" control when locked; this
                    // guards every OTHER dispatch path (status-bar toggle, a
                    // future palette command, a stale `?site=` boot). This is
                    // what makes `locked` actually gate behavior, closing the
                    // §4-C "held seam, no behavior gates on it yet" gap.
                    // `?chrome=1` (the operator escape) unlocks the toggle for
                    // this session, so an escaping operator can also dip back
                    // INTO the site if they want — without it, the lock would
                    // refuse the toggle in both directions.
                    if !self.chrome_override
                        && crate::session_config::read(&self.peer_manager, &pid)
                            .site_mode
                            .locked
                    {
                        tracing::warn!(
                            "Action::ToggleSiteMode ignored — site mode is locked (strict-site)"
                        );
                        continue;
                    }
                    // An explicit toggle (the "Exit Site" button or the
                    // status-bar toggle) RELEASES any ephemeral `?site=`
                    // deep-link override: the override seeds only the INITIAL
                    // surface, and the user's action must win afterward — else
                    // `apply_site_mode` keeps ORing it back on and Exit Site
                    // can't exit (the bug after a `?site=` boot).
                    self.site_deeplink_active = false;
                    // Set `active` to the negation of what's actually VISIBLE
                    // now (the last applied surface), NOT a blind flip of the
                    // persisted flag: the deep-link override (or a just-changed
                    // boot surface) can desync the persisted value from the
                    // visible surface, so a blind toggle would no-op or invert
                    // wrong. `apply_site_mode` at frame end reflects it.
                    let visible = self.last_site_state.as_ref().is_some_and(|s| s.active);
                    let active =
                        crate::session_config::set_active(&self.peer_manager, &pid, !visible);
                    tracing::info!(active, "Action::ToggleSiteMode");
                }
                Action::SiteOverlayNavigate { target } => {
                    // The overlay is a distinct surface (no window_id) —
                    // route to the overlay's model. WASM-only (the overlay
                    // is DOM-side); a no-op elsewhere.
                    #[cfg(target_arch = "wasm32")]
                    if let Some(overlay) = self.site_overlay.as_ref() {
                        overlay.navigate(target, &self.peer_manager);
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    let _ = target;
                }
                Action::SiteOverlayBack => {
                    #[cfg(target_arch = "wasm32")]
                    if let Some(overlay) = self.site_overlay.as_ref() {
                        overlay.back(&self.peer_manager);
                    }
                }
                Action::ConnectPeer { peer_id, addr } => {
                    tracing::info!(peer_id = %peer_id, addr = %addr, "Action::ConnectPeer received");
                    self.handle_connect_peer(peer_id.clone(), addr.clone());
                }
                Action::ForgetConnection { remote_pid } => {
                    tracing::info!(remote_pid = %remote_pid, "Action::ForgetConnection");
                    // Drop the remembered row + its health mirror so a dead
                    // backend stops cluttering the known-devices list.
                    self.connections_writer.remove(remote_pid);
                    self.connection_health_writer.record(
                        remote_pid,
                        crate::connection_health::Liveness::Unreachable,
                        None,
                    );
                }
                Action::StartListener(addr) => {
                    tracing::info!(addr = %addr, "Action::StartListener received");
                    self.handle_start_listener(addr.clone());
                }
                Action::CreatePeerWithMode { label, mode } => {
                    // Entry marker — the downstream "Created frontend
                    // peer …" / "spawning worker for new peer …" lines
                    // already carry mode + peer_id, so keep this at
                    // debug to cut info-level noise.
                    tracing::debug!(label = ?label, mode = %mode.label(), "Action::CreatePeerWithMode");
                    // 1a+1b gate (MAP §10): refuse on a non-durable / secondary
                    // tab (S-1 vault multi-writer + L-2 silent-loss) or a
                    // capability-disabled deployment (L-3). The hard backstop —
                    // it runs BEFORE any vault write, regardless of which surface
                    // (Peers window, shell verb) issued the action. Silence is
                    // the enemy (D13): a refused create is logged loud, written
                    // to the Event Log, and surfaced as a transient banner.
                    if let Some(reason_key) = self.peer_create_refusal() {
                        // `reason_key` is an i18n key — the log keeps the stable
                        // identifier, the user-facing banner gets the resolved text.
                        tracing::warn!(
                            reason = reason_key,
                            mode = %mode.label(),
                            "CreatePeerWithMode REFUSED — no peer created, no vault write"
                        );
                        let reason = crate::i18n::t(reason_key, &[]);
                        self.event_log_writer
                            .log(crate::i18n::t("peercreate.log", &[("reason", &reason)]));
                        #[cfg(target_arch = "wasm32")]
                        crate::storage_durability::show_action_refused_banner(&reason);
                    } else {
                        match mode {
                            crate::peer_mode::PeerMode::Frontend => {
                                // Hosted on the primary SDK. Direct primary:
                                // synchronous. Worker primary: async round-trip.
                                #[cfg(target_arch = "wasm32")]
                                self.create_frontend_peer(label.clone());
                            }
                            crate::peer_mode::PeerMode::FrontendIdb => {
                                // Durable this-tab peer: async IDB build off the
                                // frame loop, then inserted on a later frame.
                                #[cfg(target_arch = "wasm32")]
                                self.create_frontend_idb_peer(label.clone());
                            }
                            crate::peer_mode::PeerMode::BackendMemory
                            | crate::peer_mode::PeerMode::BackendOpfs => {
                                #[cfg(target_arch = "wasm32")]
                                {
                                    self.handle_create_worker_peer_in_new_sdk(
                                        label.clone(),
                                        *mode,
                                    );
                                }
                            }
                        }
                    }
                }
                Action::CreateBackendPeer { label } => {
                    tracing::info!(label = ?label, "Action::CreateBackendPeer");
                    self.handle_create_backend_peer(label.clone());
                }
                Action::StartBackendPeer(peer_id) => {
                    tracing::info!(peer_id = %peer_id, "Action::StartBackendPeer");
                    self.handle_start_backend_peer(peer_id.clone());
                }
                Action::StopBackendPeer(peer_id) => {
                    tracing::info!(peer_id = %peer_id, "Action::StopBackendPeer");
                    self.handle_stop_backend_peer(peer_id.clone());
                }
                Action::RenamePeer { peer_id, label } => {
                    tracing::info!(peer_id = %peer_id, label = ?label, "Action::RenamePeer");
                    match self.peer_manager.set_peer_label(peer_id, label.clone()) {
                        Ok(()) => {
                            let disp = label.as_deref().unwrap_or("(unlabeled)");
                            self.event_log_writer.log(format!(
                                "Renamed peer {} → \"{}\"",
                                &peer_id[..12.min(peer_id.len())],
                                disp,
                            ));
                            // The label-bearing palette signature includes
                            // metadata; rebuilds will pick up the new
                            // label on the next render (palette rebuilds
                            // when its signature changes).
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "RenamePeer failed");
                            self.event_log_writer
                                .log(format!("RenamePeer failed: {}", e));
                        }
                    }
                }
                Action::DeletePeer(peer_id) => {
                    // Close windows bound to this peer.
                    let to_close: Vec<_> = self.window_manager.windows.iter()
                        .filter(|w| w.open && w.view.peer_id() == peer_id)
                        .map(|w| w.id)
                        .collect();
                    for id in &to_close {
                        // F-BOOT-3: closing the maximized window via the delete
                        // path must also pop the surface back to base — mirror
                        // the CloseWindow guard, else maximized_window dangles to
                        // a torn-down id.
                        if self.maximized_window == Some(*id) {
                            self.maximized_window = None;
                        }
                        self.window_manager.close(*id);
                    }
                    // Check if this is a backend peer — needs Tauri IPC to stop it.
                    let is_backend = crate::peer_display::PeerDisplay::classify(
                        &self.peer_manager, peer_id,
                    ) == crate::peer_display::PeerDisplay::Remote;
                    if is_backend {
                        self.handle_delete_backend_peer(peer_id.clone());
                    }

                    // Persistence cleanup is **synchronous on delete-
                    // initiation** (restores pre-§4.5 behaviour):
                    //  - OPFS-dir mark: a deferred-to-next-boot cleanup
                    //    queue marker (the dedicated worker still holds
                    //    OPFS sync handles); not transactional state.
                    //  - keypair (`entity_peers`) removal: must be
                    //    synchronous so a reload during the async
                    //    worker-delete window cannot resurrect the peer
                    //    from a stale localStorage entry.
                    // §4.5's "defer keypair to post-confirm" was
                    // premised on a *panic* mid-delete tearing state —
                    // §4.1a removed that panic (uniform delete, no
                    // twin), so deferral is moot AND introduces a
                    // reload-resurrection race. Sub-item withdrawn;
                    // see review §8.6.
                    // Which peers need LOCAL persistence cleanup here?
                    // Every locally-persisted peer: browser frontend
                    // AND browser worker-hosted backend (Memory/OPFS).
                    // Only Tauri-IPC native backend peers skip it —
                    // Tauri owns their on-disk persistence.
                    //
                    // This used to be gated on `!is_backend`, which
                    // *relied on* backend peers being misclassified as
                    // frontend (so they fell here). With classification
                    // fixed, browser backend peers correctly read as
                    // backend and would otherwise route only to the
                    // Tauri-only `handle_delete_backend_peer` (a no-op
                    // in the browser) — losing both the OPFS tombstone
                    // and the keypair removal (a reload-resurrection
                    // bug). Gate on the Tauri-IPC case instead.
                    #[cfg(target_arch = "wasm32")]
                    {
                        let tauri_ipc_backend =
                            is_backend && crate::tauri_ipc::is_tauri();
                        if !tauri_ipc_backend {
                            let deleted_mode = crate::persistence::load_all_peer_entries()
                                .iter()
                                .find(|e| e.persisted.keypair.peer_id().to_string() == *peer_id)
                                .map(|e| e.mode);
                            if deleted_mode == Some(crate::peer_mode::PeerMode::BackendOpfs) {
                                crate::persistence::mark_opfs_for_cleanup(peer_id);
                            }
                            // A durable this-tab (frontend-idb) peer owns its own
                            // entity-peer-{id} IndexedDB database — tombstone it for
                            // boot-time destruction so a deleted peer doesn't orphan
                            // its store (D9: add → paired remove).
                            if deleted_mode == Some(crate::peer_mode::PeerMode::FrontendIdb) {
                                crate::persistence::mark_idb_for_cleanup(peer_id);
                            }
                            crate::persistence::delete_peer(peer_id);
                            // Roster dual-write (Brick 3): remove from the
                            // authoritative roster on the SYSTEM peer,
                            // synchronously + unconditionally ahead of the async
                            // SDK teardown (model invariant 3), keyed on the same
                            // derived id `delete_peer` matches. Checkpoint-flush
                            // on the IDB arm so the removal survives an immediate
                            // reload (the BUG-A durability contract). Tauri-IPC
                            // backends are never in the roster (not set-A
                            // writers), so this stays inside `!tauri_ipc_backend`.
                            if let Some(h) = self.peer_manager.writer_handle() {
                                crate::roster::remove_entry(
                                    &h,
                                    self.peer_manager.system_peer_id(),
                                    peer_id,
                                );
                                if let Some(cp) = self.peer_manager.idb_checkpoint() {
                                    wasm_bindgen_futures::spawn_local(async move {
                                        if let Err(e) = cp.checkpoint().await {
                                            tracing::warn!(error = %e, "roster delete checkpoint failed");
                                        }
                                    });
                                }
                            }
                        }
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if !is_backend {
                            crate::persistence::delete_peer(peer_id);
                        }
                    }

                    // Reactive self-heal: the startup config can't reference a
                    // peer that no longer exists. If this peer was the boot
                    // target (a `Window` on it, or `home_site` on it), reset
                    // that reference so the next boot is predictable — the
                    // Settings config subscription re-renders the control too.
                    // The boot-time validation in `boot_load` is the backstop;
                    // this keeps the durable state honest (handoff §4).
                    {
                        let system_pid = self.peer_manager.system_peer_id().to_string();
                        if crate::session_config::repair_for_deleted_peer(
                            &self.peer_manager,
                            &system_pid,
                            peer_id,
                        ) {
                            tracing::info!(
                                peer_id = %peer_id,
                                "delete: repaired startup config referencing the deleted peer"
                            );
                        }
                    }

                    // §4.1a: ONE uniform call. No `peer_host_is_worker`
                    // band-aid, no caller-side Direct/Worker arm choice
                    // — `delete_peer` routes per target peer and
                    // returns a detached future (Direct already-ready,
                    // Worker awaits the proxy). Closes the delete-bug
                    // arc; the band-aid is gone.
                    let delete_future = self.peer_manager.delete_peer(peer_id);
                    let event_log = self.event_log_writer.clone();
                    #[cfg(target_arch = "wasm32")]
                    let xworker_broker = self.xworker_broker.clone();
                    let pid = peer_id.clone();
                    let task = async move {
                        match delete_future.await {
                            Ok(true) => {
                                tracing::info!(peer_id = %pid, "Deleted peer");
                                event_log.log(format!(
                                    "Deleted peer {}",
                                    &pid[..12.min(pid.len())]
                                ));
                                // Drop the broker route for this peer so
                                // `xworker://<pid>` from sibling Workers
                                // resolves to ChannelDenied rather than
                                // dispatching to a dead Worker. If the
                                // Worker hosted only this peer, its
                                // control port is now fully unrooted on
                                // the broker side.
                                #[cfg(target_arch = "wasm32")]
                                {
                                    let _ = xworker_broker.unregister_peer(&pid);
                                }
                            }
                            Ok(false) => {
                                tracing::warn!(peer_id = %pid, "Cannot delete peer (primary or not found)");
                                event_log.log(format!(
                                    "Cannot delete peer {} (primary or unknown)",
                                    &pid[..12.min(pid.len())]
                                ));
                            }
                            Err(e) => {
                                tracing::warn!(peer_id = %pid, error = %e, "delete peer failed");
                                event_log.log(format!("Delete peer failed: {e}"));
                            }
                        }
                    };
                    wasm_bindgen_futures::spawn_local(task);
                }
                Action::ClearEventLog => {
                    self.event_log_writer.clear();
                }
                Action::Execute { peer_id, handler_uri, operation, resource, params } => {
                    tracing::info!(peer = %peer_id, uri = %handler_uri, op = %operation, resource = ?resource, "Action::Execute");
                    self.handle_execute(peer_id.clone(), handler_uri.clone(), operation.clone(), resource.clone(), params.clone());
                }
                Action::DownloadFile { peer_id, handler_uri, path, filename } => {
                    tracing::info!(peer = %peer_id, uri = %handler_uri, path = %path, "Action::DownloadFile");
                    self.handle_download_file(peer_id.clone(), handler_uri.clone(), path.clone(), filename.clone());
                }
                Action::UploadFile { peer_id, handler_uri, path, bytes, window_id } => {
                    tracing::info!(peer = %peer_id, uri = %handler_uri, path = %path, len = bytes.len(), "Action::UploadFile");
                    self.handle_upload_file(peer_id.clone(), handler_uri.clone(), path.clone(), bytes.clone(), *window_id);
                }
                Action::Query { peer_id, expression } => {
                    tracing::info!(peer = %peer_id, expr_type = %expression.entity_type, "Action::Query");
                    self.handle_query(peer_id.clone(), expression.clone());
                }
                Action::Count { peer_id, expression } => {
                    tracing::info!(peer = %peer_id, expr_type = %expression.entity_type, "Action::Count");
                    self.handle_count(peer_id.clone(), expression.clone());
                }
                Action::RefreshBackendAuth { local_peer_id, backend_pid } => {
                    tracing::info!(local = %local_peer_id, backend = %backend_pid, "Action::RefreshBackendAuth");
                    self.handle_refresh_backend_auth(local_peer_id.clone(), backend_pid.clone());
                }
                Action::AuthorizePeer { local_peer_id, backend_pid, target_pid, profile } => {
                    tracing::info!(local = %local_peer_id, backend = %backend_pid, target = %target_pid, profile = %profile, "Action::AuthorizePeer");
                    self.handle_authorize_peer(local_peer_id.clone(), backend_pid.clone(), target_pid.clone(), profile.clone());
                }
            }
        }
        self.window_manager.gc_closed();
    }

    /// Connect to a remote peer (async, spawned on runtime).
    ///
    /// Direct: replicates peer.connect_to() inline — connect transport,
    /// handshake, pool the connection for subsequent execute() calls.
    /// Worker (Parity-D-narrow, PROTOCOL_VERSION=5): routes through
    /// `Peers::connect_peer` → `proxy.connect_peer` → host
    /// `Peer::connect_to`. Pooling happens inside the worker; subsequent
    /// `entity://{remote_pid}/...` execute calls flow through Parity-A's
    /// proxy.execute arm and find the pooled connection there. §4.1b
    /// unified `Peers::connect_peer` so the connect step has no
    /// caller-facing arm choice; post-connect type-fetch is still
    /// arm-specific because the worker uses the wire-shape proxy
    /// directly (avoids re-encoding through `entity_handler`).
    fn handle_connect_peer(&self, peer_id: String, addr: String) {
        // C7/AP2: connect FROM the window's bound peer, not the primary.
        // A Peer-scoped Peer Connections window can be bound to a
        // non-primary peer; routing through the primary was a reachable
        // peer-scoping bug (D15).
        let pid = peer_id;
        let log = self.event_log_writer.clone();
        let connections = self.connections_writer.clone();
        let health = self.connection_health_writer.clone();

        log.log(format!("Connecting to {}...", addr));

        // Snapshot other remembered identities already recorded at THIS address.
        // A network address hosts one peer at a time, so once the (possibly
        // re-provisioned) backend answers with its current id, any *other* id at
        // the same address is definitively stale — prune it so reconnecting
        // REPLACES the dead row instead of piling up a new one (the "four dead
        // peers" bug).
        #[cfg(target_arch = "wasm32")]
        let same_addr_pids: Vec<String> = crate::connections::read_connections(&self.peer_manager)
            .into_iter()
            .filter(|c| c.addr == addr)
            .map(|c| c.remote_pid)
            .collect();

        // Evict any pooled connection to a peer remembered at THIS address
        // BEFORE reconnecting. `connect_peer` ends in an insert-if-absent pool
        // insert, so without this a repeat Connect silently keeps the existing
        // (often dead) connection — the "shows connected, but I must reconnect,
        // and even that's inconsistent" bug (`BUGLOG-2026-07-14` B3/B4/B5). A
        // fresh connect (no remembered pid here) evicts nothing and is unchanged.
        #[cfg(target_arch = "wasm32")]
        let evictions: Vec<_> = same_addr_pids
            .iter()
            .map(|stale| self.peer_manager.disconnect_peer(&pid, stale))
            .collect();

        let connect_future = self.peer_manager.connect_peer(&pid, addr.clone());

        #[cfg(target_arch = "wasm32")]
        let proxy = self.peer_manager.worker_proxy_handle_for(&pid);
        #[cfg(target_arch = "wasm32")]
        let shared = self.peer_manager.direct_peer_shared(&pid);
        #[cfg(target_arch = "wasm32")]
        let pid_for_fetch = pid.clone();

        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move {
            // Drop stale pooled connections first (idempotent; absent = no-op),
            // so the connect below actually re-handshakes instead of no-opping
            // on insert-if-absent. Must complete before the dial.
            for ev in evictions {
                let _ = ev.await;
            }
            let remote_pid = match connect_future.await {
                Ok(p) => p,
                Err(msg) => {
                    tracing::error!("{}", msg);
                    log.log(msg);
                    return;
                }
            };
            // Record the address that just worked so this backend is
            // remembered + one-tap reconnectable (§13.2). `addr` was moved
            // in for exactly this.
            connections.add(&remote_pid, &addr);
            // Prune stale identities that were remembered at this same address
            // (a re-provisioned / wiped backend). The address just answered as
            // `remote_pid`, so any other id here is dead — drop it + its health
            // mirror so the known-devices list collapses to the one live row.
            for old in &same_addr_pids {
                if old != &remote_pid {
                    connections.remove(old);
                    health.record(old, crate::connection_health::Liveness::Unreachable, None);
                }
            }
            // Reactive liveness: a fresh handshake ⇒ Connected. Windows watching
            // the conn-health mirror repaint without a manual refresh.
            health.record(&remote_pid, crate::connection_health::Liveness::Connected, None);
            log.log(format!("Connected to {}", remote_pid));

            let uri = format!("entity://{}/system/tree", remote_pid);
            log.log(format!("Fetching types from {}...", remote_pid));
            let params = entity_entity::Entity::new(
                "system/empty",
                entity_ecf::to_ecf(&entity_ecf::Value::Null),
            )
            .unwrap();

            // Worker primary: route the type fetch through the proxy's
            // wire surface (avoids re-encoding through entity_handler
            // back into wire). Direct primary: use the existing
            // `make_execute_fn` flow with cloned shared state.
            if let Some(proxy) = proxy {
                let wire_params = match entity_wasm_worker_protocol::WireEntity::try_from(params) {
                    Ok(w) => w,
                    Err(e) => {
                        log.log(format!("Type fetch param conversion failed: {}", e));
                        return;
                    }
                };
                let opts = entity_wasm_worker_protocol::WireExecuteOptions {
                    resource_targets: vec!["system/type/".into()],
                    resource_exclude: vec![],
                };
                match proxy.execute(pid_for_fetch, uri, "get".into(), wire_params, opts).await {
                    Ok(result) => log.log(format!("Remote types: status {}", result.status)),
                    Err(e) => log.log(format!("Type fetch failed: {:?}", e)),
                }
                return;
            }
            if let Some(shared) = shared {
                let opts = entity_handler::ExecuteOptions {
                    resource: Some(entity_capability::ResourceTarget {
                        targets: vec!["system/type/".into()],
                        exclude: vec![],
                    }),
                    ..Default::default()
                };
                let execute_fn = entity_peer::connection::make_execute_fn(
                    shared,
                    None,
                    std::collections::HashMap::new(),
                    None,
                    None,
                );
                match execute_fn(uri, "get".into(), params, opts).await {
                    Ok(result) => log.log(format!(
                        "Remote types: {}",
                        crate::format::format_handler_result(&result)
                    )),
                    Err(e) => log.log(format!("Type fetch failed: {}", e)),
                }
            }
        });
    }

    /// Start a WS listener on native (no-op on WASM).
    fn handle_start_listener(&self, addr: String) {
        #[cfg(feature = "native-ws")]
        {
            // The app's own listener belongs to the system (host) peer —
            // it feeds the system-owned listener_state (F-SYS-1).
            let pid = self.peer_manager.system_peer_id().to_string();
            let Some(peer) = self.peer_manager.peer(&pid) else { return };
            let Some(shared) = self.peer_manager.direct_peer_shared(&pid) else { return };
            peer.start_engines(&shared); // start engines on our shared state
            let log = self.event_log_writer.clone();
            let listener_writer = self.listener_state_writer.clone();

            // native-ws is a default feature but `EntityApp` is
            // wasm-only after the native-renderer removal, so this body has no
            // host in any current build. Spawn via the ambient runtime
            // (same idiom as `execute_console`/`knowledge_base`) rather
            // than a stored handle, so it stays correct if a native
            // `EntityApp` host is ever reintroduced or a test drives it.
            let listener_task = async move {
                tracing::info!(addr = %addr, "starting WebSocket listener...");
                match entity_peer::transport::WebSocketListener::bind(&addr).await {
                    Ok(listener) => {
                        let raw_addr = listener.local_addr();
                        // Resolve the actual LAN IP for the QR code.
                        let listen_addr = match local_ip_address::local_ip() {
                            Ok(ip) => {
                                // Replace 0.0.0.0 or 127.x with real LAN IP, keep port.
                                let port = listener.socket_addr().port();
                                format!("ws://{}:{}", ip, port)
                            }
                            Err(_) => raw_addr.clone(),
                        };
                        tracing::info!(addr = %listen_addr, raw = %raw_addr, "WebSocket listener started");
                        log.log(format!("Listening on {}", listen_addr));
                        listener_writer.set_address(&listen_addr);
                        if let Err(e) = entity_peer::server::run(listener, shared).await {
                            tracing::error!(error = %e, "listener stopped");
                            log.log(format!("Listener error: {}", e));
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "failed to bind listener");
                        log.log(format!("Bind failed: {}", e));
                    }
                }
            };
            if let Ok(rt) = tokio::runtime::Handle::try_current() {
                rt.spawn(listener_task);
            } else {
                tracing::warn!("WS listener: no tokio runtime available");
            }
        }

        #[cfg(not(feature = "native-ws"))]
        {
            let _ = addr;
            tracing::warn!("WS listener not available (native-ws feature disabled)");
        }
    }

    /// Execute a handler operation via the local primary peer.
    /// handler_uri encodes the target: "system/tree" (local) or
    /// "entity://{remote_pid}/system/tree" (remote, routed via connection pool).
    /// Run an L1 query via the typed SDK helper and log the result.
    /// Replaces the generic `Action::Execute("system/query", "find", ...)`
    /// path for `find` operations so we get structured matches in the log
    /// instead of opaque envelope text.
    fn handle_query(&self, pid: String, expression: entity_entity::Entity) {
        let log = self.event_log_writer.clone();

        log.log("→ system/query find".to_string());

        let fut = self.peer_manager.query(&pid, expression);
        let task = async move {
            match fut.await {
                Ok(results) => {
                    let cursor_note = if let Some(c) = &results.cursor {
                        format!(" cursor={}", c)
                    } else {
                        String::new()
                    };
                    log.log(format!(
                        "← system/query find → {} match(es), total={}, has_more={}{}", // i18n-ignore — console diagnostic (dev log surface)
                        results.matches.len(),
                        results.total,
                        results.has_more,
                        cursor_note,
                    ));
                    for m in results.matches.iter().take(20) {
                        let included_note = if m.entity.is_some() { " [+entity]" } else { "" };
                        log.log(format!("    {} ({}){}", m.path, m.entity_type, included_note));
                    }
                    if results.matches.len() > 20 {
                        log.log(format!("    ... ({} more)", results.matches.len() - 20));
                    }
                }
                Err(e) => {
                    let msg = format!("✗ system/query find → {}", e); // i18n-ignore — console diagnostic (dev log surface)
                    tracing::error!("{}", msg);
                    log.log(msg);
                }
            }
        };

        wasm_bindgen_futures::spawn_local(task);
    }

    /// Run an L1 count via the arm-aware `Peers` method and log it.
    fn handle_count(&self, pid: String, expression: entity_entity::Entity) {
        let log = self.event_log_writer.clone();

        log.log("→ system/query count".to_string());

        let fut = self.peer_manager.count(&pid, expression);
        let task = async move {
            match fut.await {
                Ok(n) => log.log(format!("← system/query count → {}", n)), // i18n-ignore — console diagnostic (dev log surface)
                Err(e) => {
                    let msg = format!("✗ system/query count → {}", e); // i18n-ignore — console diagnostic (dev log surface)
                    tracing::error!("{}", msg);
                    log.log(msg);
                }
            }
        };

        wasm_bindgen_futures::spawn_local(task);
    }

    fn handle_execute(&self, pid: String, handler_uri: String, operation: String, resource: Option<String>, custom_params: Option<entity_entity::Entity>) {
        let log = self.event_log_writer.clone();

        log.log(format!(
            "→ {} {} {}",
            handler_uri, operation, resource.as_deref().unwrap_or("")
        ));

        let uri_for_log = handler_uri.clone();
        let op_for_log = operation.clone();
        // `ops::execute` self-heals a remote 403 / dead pooled connection
        // (evict + reconnect + retry once) — every dispatch chokepoints there.
        let fut = crate::ops::execute(&self.peer_manager, crate::ops::ExecuteRequest {
            peer_id: pid,
            handler_uri,
            operation,
            params: custom_params,
            resource,
        });
        wasm_bindgen_futures::spawn_local(async move {
            match fut.await {
                Ok(resp) => {
                    let msg = format!("← {} {} → {}", uri_for_log, op_for_log, resp.summary);
                    tracing::info!("{}", msg);
                    log.log(msg);
                }
                Err(e) => {
                    let msg = format!("✗ {} {} → {}", uri_for_log, op_for_log, e);
                    tracing::error!("{}", msg);
                    log.log(msg);
                }
            }
        });
    }

    /// Pull a file from `handler_uri` (`local/files:read` on `path`) and
    /// materialize its bytes onto this device via a browser download.
    /// Reuses the same execute path as `handle_execute` but captures the
    /// structured result (with its `included` blob/chunks) rather than
    /// logging only a summary. WASM-only (browser download API).
    #[cfg(target_arch = "wasm32")]
    fn handle_download_file(&self, pid: String, handler_uri: String, path: String, filename: String) {
        let log = self.event_log_writer.clone();
        log.log(format!("↓ pulling {}...", path));

        // Feed the dispatch outcome into the connection-health mirror: an answer
        // (any status) ⇒ reachable; a transport error ⇒ Unreachable.
        let health = self.connection_health_writer.clone();
        let target = entity_uri_authority(&handler_uri).map(str::to_string);
        let fut = crate::ops::execute(&self.peer_manager, crate::ops::ExecuteRequest {
            peer_id: pid,
            handler_uri,
            operation: "read".into(),
            params: None,
            resource: Some(path.clone()),
        });
        wasm_bindgen_futures::spawn_local(async move {
            match fut.await {
                Ok(resp) => {
                    mark_health(&health, &target, Ok(()));
                    // A handler-level error (e.g. 404) comes back as Ok with a
                    // non-OK status; surface it loudly rather than trying to
                    // reassemble an error entity.
                    if resp.result.status != entity_handler::STATUS_OK {
                        log.log(format!("✗ pull {} → {}", path, resp.summary));
                        return;
                    }
                    match crate::ops::download::materialize_and_download(&resp.result, &filename) {
                        Ok(n) => log.log(format!("✓ saved {} ({} bytes)", filename, n)),
                        Err(e) => log.log(format!("✗ save {} → {}", filename, e)),
                    }
                }
                Err(e) => {
                    mark_health(&health, &target, Err(e.clone()));
                    log.log(format!("✗ pull {} → {}", path, e));
                }
            }
        });
    }

    /// Push `bytes` up to `handler_uri` as `local/files:write` on `path`.
    /// The handler chunks the bytes and writes the file to the peer's
    /// disk (DOMAIN-LOCAL-FILES §4.3) — the phone→desktop upload. WASM-only.
    #[cfg(target_arch = "wasm32")]
    fn handle_upload_file(&self, pid: String, handler_uri: String, path: String, bytes: Vec<u8>, window_id: crate::window::WindowId) {
        let log = self.event_log_writer.clone();
        let len = bytes.len();
        log.log(format!("↑ uploading {} ({} bytes)...", path, len));

        // S5 write-refresh: on a confirmed upload, re-list the initiating
        // window's share so the new file appears without a manual Refresh. We
        // enqueue a `ft_refresh` WindowEvent into the shared action sink (drained
        // next frame) rather than touching the view here — same mechanism the
        // window's own Refresh button uses.
        let refresh_handle = self
            .dom
            .as_ref()
            .map(|d| (d.action_sink(), d.repaint_handle()));
        // Dispatch outcome → connection-health (same as pull).
        let health = self.connection_health_writer.clone();
        let target = entity_uri_authority(&handler_uri).map(str::to_string);

        // Write params: a single `bytes` field the handler decodes via
        // WriteRequestData::from_params. The params entity type is not
        // inspected by the handler; only the fields are read.
        let params_data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "bytes" => entity_ecf::Value::Bytes(bytes)
        });
        let params = match entity_entity::Entity::new("app/entity-browser/upload", params_data) {
            Ok(e) => Some(e),
            Err(e) => {
                log.log(format!("✗ upload {} → build params: {}", path, e));
                return;
            }
        };

        let write_fut = crate::ops::execute(
            &self.peer_manager,
            crate::ops::ExecuteRequest {
                peer_id: pid.clone(),
                handler_uri: handler_uri.clone(),
                operation: "write".into(),
                params,
                resource: Some(path.clone()),
            },
        );
        // Read-back verification: a `read` of the same path, dispatched only
        // after the write reports OK. If the write truly landed on the peer
        // this returns the bytes; if the "confirmed" 200 was synthetic (write
        // not actually forwarded/persisted) this 404s — turning an ambiguous
        // "says OK but no file" into a definitive signal. Built eagerly (the
        // future is `'static` and dispatches at await time), awaited after.
        let verify_fut = crate::ops::execute(
            &self.peer_manager,
            crate::ops::ExecuteRequest {
                peer_id: pid,
                handler_uri,
                operation: "read".into(),
                params: None,
                resource: Some(path.clone()),
            },
        );
        wasm_bindgen_futures::spawn_local(async move {
            let write_result = write_fut.await;
            mark_health(
                &health,
                &target,
                write_result.as_ref().map(|_| ()).map_err(|e| e.clone()),
            );
            match write_result {
                Ok(resp) if resp.result.status == entity_handler::STATUS_OK => {
                    let msg = format!("✓ FILE-XFER uploaded {} ({} bytes) — backend confirmed", path, len);
                    tracing::info!("{}", msg);
                    log.log(msg);
                    // S5 write-refresh: the file just landed — re-list the share
                    // so it shows up without the user hitting Refresh.
                    if let Some((sink, repaint)) = &refresh_handle {
                        sink.borrow_mut().push(crate::action::Action::WindowEvent {
                            window_id,
                            event: "ft_refresh".into(),
                            value: String::new(),
                        });
                        repaint();
                    }
                    // Now prove it's really there.
                    match verify_fut.await {
                        Ok(v) if v.result.status == entity_handler::STATUS_OK => {
                            let vmsg = format!("✓ FILE-XFER verified {} readable back ({})", path, v.summary.lines().next().unwrap_or("")); // i18n-ignore — console diagnostic (greppable "FILE-XFER", dev log surface)
                            tracing::info!("{}", vmsg);
                            log.log(vmsg);
                        }
                        Ok(v) => {
                            let vmsg = format!("✗ FILE-XFER VERIFY FAILED {} → write reported OK but read-back is {} — the write did NOT land on the peer", path, v.summary.lines().next().unwrap_or("")); // i18n-ignore — console diagnostic (greppable "FILE-XFER", dev log surface)
                            tracing::error!("{}", vmsg);
                            log.log(vmsg);
                        }
                        Err(e) => {
                            let vmsg = format!("✗ FILE-XFER VERIFY FAILED {} → read-back errored: {}", path, e); // i18n-ignore — console diagnostic (greppable "FILE-XFER", dev log surface)
                            tracing::error!("{}", vmsg);
                            log.log(vmsg);
                        }
                    }
                }
                // A handler-level rejection (403 read_only_root, 400 missing
                // bytes, …) comes back as Ok with a non-OK status. Surface it
                // at error level so it stands out RED in the tauri-run terminal
                // (greppable: "FILE-XFER") rather than getting lost in the log.
                Ok(resp) => {
                    let msg = format!("✗ FILE-XFER upload REJECTED {} → {}", path, resp.summary); // i18n-ignore — console diagnostic (greppable "FILE-XFER", dev log surface)
                    tracing::error!("{}", msg);
                    log.log(msg);
                }
                Err(e) => {
                    let msg = format!("✗ FILE-XFER upload FAILED {} → {}", path, e); // i18n-ignore — console diagnostic (greppable "FILE-XFER", dev log surface)
                    tracing::error!("{}", msg);
                    log.log(msg);
                }
            }
        });
    }

    /// Refresh the backend-auth observability surface for one backend B
    /// (`DESIGN-AUTHORIZE-GATE-INCREMENT-3 §3 Step 3`). Builds the two one-shot
    /// remote reads over S's manager grant and spawns the shared derive-record
    /// task ([`derive_and_record_backend_auth`]).
    #[cfg(target_arch = "wasm32")]
    fn handle_refresh_backend_auth(&self, local_pid: String, backend_pid: String) {
        let (session_fut, policy_fut) = self.backend_auth_read_futures(&local_pid, &backend_pid);
        let prev = self.backend_auth_prev(&backend_pid);
        wasm_bindgen_futures::spawn_local(derive_and_record_backend_auth(
            session_fut,
            policy_fut,
            self.backend_auth_writer.clone(),
            self.event_log_writer.clone(),
            self.peer_manager.system_peer_id().to_string(),
            backend_pid,
            prev,
            self.connection_health_writer.clone(),
        ));
    }

    /// The mirror's current observation for a backend (for dedupe at record
    /// time). Reads the local backend-auth mirror on the system peer.
    #[cfg(target_arch = "wasm32")]
    fn backend_auth_prev(
        &self,
        backend_pid: &str,
    ) -> Option<crate::backend_auth::BackendAuthObservation> {
        let sys = self.peer_manager.system_peer_id().to_string();
        let path =
            crate::app_paths::backend_auth_entry_path(crate::app_paths::APP_ID, &sys, backend_pid);
        self.peer_manager
            .get_entity(&sys, &path)
            .map(|e| crate::backend_auth::BackendAuthObservation::from_entity(&e))
    }

    /// Build the two `'static` remote-read futures for a backend-auth refresh:
    /// B's `system/peer/session/*` (connected) and `system/capability/policy/*`
    /// (authorized). A trailing `/` makes `system/tree:get` return a
    /// single-level listing whose child keys are the peer-id keys (hex). The
    /// futures dispatch at await time, so callers may build them ahead and
    /// await them later (e.g. after an authorize).
    #[cfg(target_arch = "wasm32")]
    fn backend_auth_read_futures(
        &self,
        local_pid: &str,
        backend_pid: &str,
    ) -> (
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::ops::ExecuteResponse, String>>>>,
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<crate::ops::ExecuteResponse, String>>>>,
    ) {
        let session_fut = crate::ops::execute(
            &self.peer_manager,
            crate::ops::ExecuteRequest {
                peer_id: local_pid.to_string(),
                handler_uri: format!("entity://{}/system/tree", backend_pid),
                operation: "get".into(),
                params: None,
                resource: Some("system/peer/session/".into()),
            },
        );
        let policy_fut = crate::ops::execute(
            &self.peer_manager,
            crate::ops::ExecuteRequest {
                peer_id: local_pid.to_string(),
                handler_uri: format!("entity://{}/system/tree", backend_pid),
                operation: "get".into(),
                params: None,
                resource: Some("system/capability/policy/".into()),
            },
        );
        (session_fut, policy_fut)
    }

    /// Authorize `target_pid` on backend B under a grant `profile`
    /// (`§2.2`, `§3 Step 3.3`). Authors `policy/{target}` on B via the
    /// capability `configure` op (over S's manager cap), mirrors the profile
    /// to the S-side `authz` entity, and re-reads the surface so the peer moves
    /// pending → authorized. The grant applies on the target's **next
    /// handshake** (the design's fresh-reconnect constraint, §3 Step 4) — this
    /// authors it; making the peer re-handshake is the reconnect affordance.
    #[cfg(target_arch = "wasm32")]
    fn handle_authorize_peer(
        &self,
        local_pid: String,
        backend_pid: String,
        target_pid: String,
        profile: String,
    ) {
        let log = self.event_log_writer.clone();
        let connections = self.connections_writer.clone();
        let writer = self.backend_auth_writer.clone();
        let system_pid = self.peer_manager.system_peer_id().to_string();

        let Some(prof) = crate::backend_auth::GrantProfile::from_token(&profile) else {
            log.log(format!("✗ authorize {} → unknown grant profile '{}'", target_pid, profile));
            return;
        };
        let params = match crate::backend_auth::build_authorize_params(&backend_pid, &target_pid, prof) {
            Ok(p) => p,
            Err(e) => {
                log.log(format!("✗ authorize {} → {}", target_pid, e));
                return;
            }
        };

        log.log(format!(
            "authorizing {} on {} as '{}'...",
            &target_pid[..12.min(target_pid.len())],
            &backend_pid[..12.min(backend_pid.len())],
            profile
        ));

        let configure_fut = crate::ops::execute(
            &self.peer_manager,
            crate::ops::ExecuteRequest {
                peer_id: local_pid.clone(),
                handler_uri: format!("entity://{}/system/capability", backend_pid),
                operation: "configure".into(),
                params: Some(params),
                resource: None,
            },
        );
        // Built ahead (dispatch at await time) so the post-authorize refresh
        // runs in the same task once the grant has landed — no self needed.
        let (session_fut, policy_fut) = self.backend_auth_read_futures(&local_pid, &backend_pid);
        let refresh_log = log.clone();
        let prev = self.backend_auth_prev(&backend_pid);
        let health = self.connection_health_writer.clone();

        wasm_bindgen_futures::spawn_local(async move {
            match configure_fut.await {
                Ok(resp) if resp.result.status == entity_handler::STATUS_OK => {
                    log.log(format!("✓ authorized {} on backend as '{}'", target_pid, profile));
                    // Mirror the decision locally (§4). Keyed by the target as
                    // B reports it (hex); the base58↔hex reconciliation for the
                    // app-side connection join is a step-5 item.
                    connections.set_authorized(&target_pid, &profile);
                    // Re-read so the row reclassifies pending → authorized.
                    derive_and_record_backend_auth(
                        session_fut,
                        policy_fut,
                        writer,
                        refresh_log,
                        system_pid,
                        backend_pid,
                        prev,
                        health,
                    )
                    .await;
                }
                Ok(resp) => {
                    log.log(format!("✗ authorize {} REJECTED → {}", target_pid, resp.summary));
                }
                Err(e) => {
                    log.log(format!("✗ authorize {} FAILED → {}", target_pid, e));
                }
            }
        });
    }

    /// Create a backend peer via Tauri IPC (WASM only — Tauri IPC is
    /// JS-based). `EntityApp` is wasm-only; there is no native build.
    fn handle_create_backend_peer(&mut self, label: Option<String>) {
        #[cfg(target_arch = "wasm32")]
        {
            if !crate::tauri_ipc::is_tauri() {
                tracing::warn!("CreateBackendPeer: not running in Tauri");
                self.event_log_writer
                    .log("Backend peers require Tauri desktop app");
                return;
            }

            let log = self.event_log_writer.clone();
            log.log("Creating backend peer...");

            // We can't mutate peer_manager from inside the async block,
            // so the IPC result is queued and picked up on the next frame
            // via a pending registration queue.
            let pending = self.pending_backend_peers.clone();

            wasm_bindgen_futures::spawn_local(async move {
                match crate::tauri_ipc::create_backend_peer(label).await {
                    Ok(info) => {
                        tracing::info!(
                            peer_id = %info.peer_id,
                            ws_addr = ?info.ws_addr,
                            "Backend peer created"
                        );
                        log.log(format!(
                            "Backend peer created: {}",
                            &info.peer_id[..12.min(info.peer_id.len())]
                        ));
                        if let Ok(mut q) = pending.lock() {
                            q.push(info);
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Failed to create backend peer");
                        log.log(format!("Backend peer creation failed: {}", e));
                    }
                }
            });
        }

    }

    /// Start a backend peer via Tauri IPC.
    fn handle_start_backend_peer(&self, peer_id: String) {
        #[cfg(target_arch = "wasm32")]
        {
            if !crate::tauri_ipc::is_tauri() { return; }
            let log = self.event_log_writer.clone();
            let pending = self.pending_backend_peers.clone();
            // Designate this system peer (S) as the backend's manager so B can
            // self-seed S's manager grant (increment 3, step 1).
            let manager_pid = self.peer_manager.system_peer_id().to_string();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::tauri_ipc::start_backend_peer(&peer_id, &manager_pid).await {
                    Ok(info) => {
                        log.log(format!(
                            "Backend peer started: {}",
                            info.ws_addr.as_deref().unwrap_or("?")
                        ));
                        if let Ok(mut q) = pending.lock() { q.push(info); }
                    }
                    Err(e) => {
                        log.log(format!("Start failed: {}", e));
                    }
                }
            });
        }
    }

    /// Stop a backend peer via Tauri IPC.
    fn handle_stop_backend_peer(&self, peer_id: String) {
        #[cfg(target_arch = "wasm32")]
        {
            if !crate::tauri_ipc::is_tauri() { return; }
            let log = self.event_log_writer.clone();
            let pending = self.pending_backend_peers.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::tauri_ipc::stop_backend_peer(&peer_id).await {
                    Ok(info) => {
                        log.log("Backend peer stopped");
                        if let Ok(mut q) = pending.lock() { q.push(info); }
                    }
                    Err(e) => {
                        log.log(format!("Stop failed: {}", e));
                    }
                }
            });
        }
    }

    /// Delete a backend peer via Tauri IPC.
    fn handle_delete_backend_peer(&self, peer_id: String) {
        #[cfg(target_arch = "wasm32")]
        {
            if !crate::tauri_ipc::is_tauri() { return; }
            let log = self.event_log_writer.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = crate::tauri_ipc::delete_backend_peer(&peer_id).await {
                    log.log(format!("Delete failed: {}", e));
                }
            });
        }
    }

    /// Drain pending backend peer updates from async IPC results.
    /// Handles both new registrations and updates (start/stop changing addresses).
    #[cfg(target_arch = "wasm32")]
    /// Integrate Worker SDKs that finished spawning since last frame.
    /// Each attachment becomes a new `Sdk::Worker` in `Peers.sdks`;
    /// the peer it was spawned for is routed via `peer_routes`. Logs
    /// + bumps the peer-registry signal so peer-list views refresh.
    #[cfg(target_arch = "wasm32")]
    fn drain_pending_sdk_attachments(&mut self) {
        let pending: Vec<PendingSdkAttachment> =
            self.pending_sdk_attachments.borrow_mut().drain(..).collect();
        for p in pending {
            let pid = p.primary_in_sdk.clone();
            let control_port = p.control_port_main_side;
            // Snapshot the full peer-id list BEFORE `peer_mirror`
            // moves into `WorkerPeerStore::new`. Each peer hosted in
            // this Worker becomes independently reachable via
            // `xworker://` (per the upstream multi-peer
            // reachability fix).
            let all_pids_in_worker: Vec<String> = p
                .peer_mirror
                .iter()
                .map(|pi| pi.peer_id.clone())
                .collect();
            let store = crate::peers_worker::WorkerPeerStore::new(
                p.proxy,
                p.primary_in_sdk,
                p.peer_mirror,
            );
            let idx = self.peer_manager.attach_worker_sdk(store);
            // Register EVERY peer hosted in this Worker against the
            // cross-Worker broker, all pointing at the same control
            // port (clone is cheap — JsValue handle to the same JS
            // MessagePort). Backends spawned via the manual
            // `Worker::new` + `with_control_port` path supply the
            // port; legacy paths leave it None (those Workers stay
            // reachable only via WS).
            //
            // The JS `MessagePort.onmessage` slot is single-valued, so
            // the broker's last `register_peer` call overwrites the
            // prior onmessage handler — harmless because every
            // closure routes via the same `workers` map lookup. See
            // `MessagePortBroker::register_peer` docstring.
            if let Some(port) = control_port {
                for reg_pid in &all_pids_in_worker {
                    self.xworker_broker.register_peer(reg_pid.clone(), port.clone());
                }
                tracing::info!(
                    primary_pid = %pid,
                    peer_count = all_pids_in_worker.len(),
                    "registered worker peers with xworker broker"
                );
            }
            tracing::info!(
                sdk_idx = idx,
                peer_id = %pid,
                mode = %p.mode.label(),
                "attached worker SDK"
            );
            // No manual per-window dirty-mark: the end-of-frame
            // `peer_registry.sync()` writes the newly-attached peer's
            // registry entity, and every peer-aware window subscribes
            // to that prefix — so they rebuild through the ordinary
            // subscription path, same one-frame latency as the old
            // signal bump.
            self.event_log_writer.log(format!(
                "Attached {} peer {} (SDK #{})",
                p.mode.label(),
                &pid[..12.min(pid.len())],
                idx,
            ));
            let _ = p.label;
        }
    }

    /// The 1a+1b peer-creation gate (MAP §10): why a `CreatePeerWithMode`
    /// must be refused right now, or `None` when it's allowed. Composes this
    /// tab's runtime durability ([`can_persist`](Self::can_persist) — 1a) with
    /// the deployment capability flag read from the session config
    /// (`peer_creation_enabled` — 1b). Both the action handler (hard backstop,
    /// prevents the vault write) and any caller use this single decision so the
    /// guard and the hidden UI never disagree (CR-5).
    fn peer_create_refusal(&self) -> Option<&'static str> {
        let enabled = crate::session_config::read(
            &self.peer_manager,
            self.peer_manager.system_peer_id(),
        )
        .peer_creation_enabled;
        crate::session_config::peer_create_refusal_reason(self.can_persist, enabled)
    }

    /// Create a "Frontend" peer — one hosted on the primary SDK.
    ///
    /// Direct primary: create synchronously, persist the seed, set
    /// metadata, spawn the event bridge while we still hold `&mut self`.
    /// Worker primary: spawn the async round-trip; persist when it
    /// lands (the worker host installs its own event bridge).
    ///
    /// The roster is reconciled into the tree registry by the
    /// end-of-frame `peer_registry.sync()` — no signal bump here.
    /// (Backend* modes go through `handle_create_worker_peer_in_new_sdk`.)
    #[cfg(target_arch = "wasm32")]
    fn create_frontend_peer(&mut self, label: Option<String>) {
        // §4.1b: unified `Peers::create_new_peer` — Direct's future
        // resolves synchronously (already-ready) with metadata
        // installed and the event-bridge already spawned; Worker
        // awaits the host round-trip. The caller is uniform across
        // arms.
        let create_future = self.peer_manager.create_new_peer(label.clone());
        let event_log = self.event_log_writer.clone();
        let label_for_persist = label.clone();
        // Worker mode only: hand the spawned future a clone of the
        // boot port + broker Rc so the new Frontend peer can be
        // registered with the xworker broker on success. Without
        // this, runtime-added Frontends are silently unreachable via
        // `xworker://` even though the boot worker's host binds a
        // listener for them (the multi-peer reachability fix).
        // Direct mode leaves both None — nothing to register.
        let xworker_broker = self.xworker_broker.clone();
        let boot_port = self.boot_control_port.clone();
        // Roster dual-write (Brick 3): capture the system-peer writer handle +
        // id + IDB checkpoint BEFORE the spawn (the future can't borrow self).
        // The roster lives on the SYSTEM peer, so writer_handle() (system-bound)
        // is correct here — NOT the per-peer writer_handle_for. The checkpoint
        // is Some only on the Direct/IDB arm; awaiting it makes the create
        // durable across an immediate reload.
        let roster_handle = self.peer_manager.writer_handle();
        let roster_sys = self.peer_manager.system_peer_id().to_string();
        let roster_checkpoint = self.peer_manager.idb_checkpoint();
        wasm_bindgen_futures::spawn_local(async move {
            match create_future.await {
                Ok((new_pid, seed, _metadata)) => {
                    tracing::info!(peer_id = %new_pid, "Created frontend peer");
                    let keypair = entity_crypto::Keypair::from_seed(seed);
                    crate::persistence::save_peer(
                        &new_pid,
                        &keypair,
                        label_for_persist.as_deref(),
                    );
                    // Shadow the spawn-list (set A) into the authoritative
                    // roster, keyed on the seed-derived `new_pid` (BUG-A
                    // invariant: identity is always re-derived).
                    if let Some(h) = roster_handle.as_ref() {
                        crate::roster::put_entry(
                            h,
                            &roster_sys,
                            &crate::roster::RosterEntry {
                                peer_id: new_pid.clone(),
                                mode: crate::peer_mode::PeerMode::Frontend,
                                label: label_for_persist.clone(),
                            },
                        );
                        if let Some(cp) = roster_checkpoint.as_ref() {
                            if let Err(e) = cp.checkpoint().await {
                                tracing::warn!(error = %e, "roster create checkpoint failed (frontend)");
                            }
                        }
                    }
                    if let Some(port) = boot_port {
                        xworker_broker.register_peer(new_pid.clone(), port);
                        tracing::info!(
                            peer_id = %new_pid,
                            "registered runtime-added Frontend with xworker broker"
                        );
                    }
                    event_log.log(format!(
                        "Created peer {}",
                        &new_pid[..12.min(new_pid.len())]
                    ));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "frontend create failed");
                    event_log.log(format!("Create peer failed: {e}"));
                }
            }
        });
    }

    /// Create a durable **this-tab (`frontend-idb`)** peer. The IDB
    /// `PeerContext` build is async (`Peers::build_idb_ctx`), so it runs in a
    /// `spawn_local` off the frame loop; the built ctx lands in
    /// `pending_idb_peers` and is inserted + persisted on a later frame by
    /// [`drain_pending_idb_peers`]. Direct posture only — refused loudly on a
    /// Worker primary (the UI also gates the option to Direct).
    #[cfg(target_arch = "wasm32")]
    fn create_frontend_idb_peer(&self, label: Option<String>) {
        if self.peer_manager.primary_as_direct().is_none() {
            let reason = "persistent this-tab peers require the Direct arm"; // i18n-ignore — internal arm invariant, dev log detail
            tracing::warn!(reason, "frontend-idb create refused");
            self.event_log_writer
                .log(format!("Cannot create peer: {reason}"));
            return;
        }
        let keypair = entity_crypto::Keypair::generate();
        let seed = keypair.secret_key_bytes();
        let queue = self.pending_idb_peers.clone();
        let event_log = self.event_log_writer.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match crate::peers::Peers::build_idb_ctx(keypair).await {
                Ok(ctx) => {
                    queue.borrow_mut().push(PendingIdbPeer { ctx, seed, label });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "durable this-tab peer build failed");
                    event_log.log(format!("Create peer failed: {e}"));
                }
            }
        });
    }

    /// Drain durable `frontend-idb` peers whose async build finished: insert
    /// each into the primary Direct SDK, persist it app-side (vault + roster,
    /// mode `FrontendIdb`), and checkpoint both the roster write and the new
    /// peer's own store so the create survives an immediate reload.
    #[cfg(target_arch = "wasm32")]
    fn drain_pending_idb_peers(&mut self) {
        let drained: Vec<PendingIdbPeer> =
            self.pending_idb_peers.borrow_mut().drain(..).collect();
        for PendingIdbPeer { ctx, seed, label } in drained {
            let new_pid = match self.peer_manager.insert_built_idb_peer(ctx, label.clone())
            {
                Ok(pid) => pid,
                Err(e) => {
                    tracing::warn!(error = %e, "durable this-tab peer insert failed");
                    self.event_log_writer.log(format!("Create peer failed: {e}"));
                    continue;
                }
            };
            tracing::info!(peer_id = %new_pid, "Created durable this-tab (idb) peer");
            let keypair = entity_crypto::Keypair::from_seed(seed);
            crate::persistence::save_peer_with_mode(
                &new_pid,
                &keypair,
                label.as_deref(),
                crate::peer_mode::PeerMode::FrontendIdb,
            );
            // Roster dual-write (Brick 3) on the SYSTEM peer, keyed on the
            // seed-derived id, mode = FrontendIdb. Checkpoint makes it durable.
            if let Some(h) = self.peer_manager.writer_handle() {
                crate::roster::put_entry(
                    &h,
                    self.peer_manager.system_peer_id(),
                    &crate::roster::RosterEntry {
                        peer_id: new_pid.clone(),
                        mode: crate::peer_mode::PeerMode::FrontendIdb,
                        label: label.clone(),
                    },
                );
                if let Some(cp) = self.peer_manager.idb_checkpoint() {
                    wasm_bindgen_futures::spawn_local(async move {
                        if let Err(e) = cp.checkpoint().await {
                            tracing::warn!(error = %e, "roster create checkpoint failed (frontend-idb)");
                        }
                    });
                }
            }
            // Checkpoint the NEW peer's own store so its initial state is durable
            // across an immediate reload (the create-path flush, step 2 intent).
            if let Some(cp) = self.peer_manager.idb_checkpoint_for(&new_pid) {
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(e) = cp.checkpoint().await {
                        tracing::warn!(error = %e, "durable this-tab peer: create checkpoint failed");
                    }
                });
            }
            self.event_log_writer.log(format!(
                "Created peer {}",
                &new_pid[..12.min(new_pid.len())]
            ));
        }
    }

    /// Spawn a new Worker SDK that hosts a fresh peer in the requested
    /// backend mode. Generates a keypair on the main thread, persists
    /// it immediately (orphans on spawn failure are benign — user can
    /// delete), then dispatches the worker spawn through
    /// [`spawn_worker_sdk_for_peer_into`].
    #[cfg(target_arch = "wasm32")]
    fn handle_create_worker_peer_in_new_sdk(
        &self,
        label: Option<String>,
        mode: crate::peer_mode::PeerMode,
    ) {
        let keypair = entity_crypto::Keypair::generate();
        let peer_id = keypair.peer_id().to_string();
        crate::persistence::save_peer_with_mode(
            &peer_id,
            &keypair,
            label.as_deref(),
            mode,
        );
        // Roster dual-write (Brick 3): shadow set A into the authoritative
        // roster on the SYSTEM peer. `peer_id` is already the seed-derived id
        // (keypair.peer_id()). Sync fn → spawn the checkpoint await.
        if let Some(h) = self.peer_manager.writer_handle() {
            crate::roster::put_entry(
                &h,
                self.peer_manager.system_peer_id(),
                &crate::roster::RosterEntry {
                    peer_id: peer_id.clone(),
                    mode,
                    label: label.clone(),
                },
            );
            if let Some(cp) = self.peer_manager.idb_checkpoint() {
                wasm_bindgen_futures::spawn_local(async move {
                    if let Err(e) = cp.checkpoint().await {
                        tracing::warn!(error = %e, "roster create checkpoint failed (backend)");
                    }
                });
            }
        }
        spawn_worker_sdk_for_peer_into(
            self.pending_sdk_attachments.clone(),
            Some(self.event_log_writer.clone()),
            peer_id,
            keypair.secret_key_bytes().to_vec(),
            label,
            mode,
        );
    }

    /// Dial the auto-provisioned system backend from S (Phase 1c,
    /// `DESIGN-SYSTEM-BACKEND-PEER.md` §10.2/§10.3). Fires at most once per
    /// cooldown window, only while `target` is armed and B isn't already in the
    /// connections registry, and stops after a successful connect (clears
    /// `target`) or the attempt cap (gives up loudly, D13). Backoff covers a
    /// dial that races the (async) listener bind. Guard reads the connections
    /// registry directly — reliable on the Tauri IDB arm this path runs under.
    #[cfg(target_arch = "wasm32")]
    fn drain_system_backend_connect(&mut self) {
        // ~1s between tries at 60fps; a burst of attempts, then a slow retry.
        const MAX_ATTEMPTS: u32 = 8;
        const BACKOFF_FRAMES: u32 = 60;
        const RETRY_COOLDOWN: u32 = 600; // ~10s slow-retry after a failed burst

        // Snapshot the armed target without holding the lock across the dial.
        let target = {
            let Ok(mut c) = self.system_backend_connect.lock() else {
                return;
            };
            let Some(info) = c.target.clone() else {
                return;
            };
            if c.cooldown > 0 {
                c.cooldown -= 1;
                return;
            }
            if c.attempts >= MAX_ATTEMPTS {
                // Don't give up permanently — back off and keep retrying so a
                // backend that comes back later self-heals without a reload.
                c.attempts = 0;
                c.cooldown = RETRY_COOLDOWN;
                drop(c);
                // Burst exhausted without a handshake — genuinely unreachable
                // for now, so the link chip drops from Connecting to Offline
                // (honest: we're backing off, not actively dialing). A new burst
                // after the cooldown re-arms Connecting.
                self.connection_health_writer.record(
                    &info.peer_id,
                    crate::connection_health::Liveness::Unreachable,
                    Some("backend unreachable — backing off, will retry".into()), // i18n-ignore — connection-health diagnostic detail (dev log surface)
                );
                tracing::warn!("system backend unreachable after {MAX_ATTEMPTS} tries — backing off, will retry");
                return;
            }
            info
        };

        let Some(ws_addr) = target.ws_addr.clone() else {
            // No address to dial — clear so we don't spin.
            if let Ok(mut c) = self.system_backend_connect.lock() {
                c.target = None;
            }
            return;
        };

        // Live connection? Trust the real connection-health signal, not the
        // remembered registry — the registry stays "connected" forever after
        // the first dial, so guarding on it means we never re-dial a *dropped*
        // link (the stale-connection bug). Registered + not-Unreachable ⇒
        // treat as up (idle, keep the target armed + reset the budget so a
        // later drop gets a fresh set of attempts). A definitive Unreachable ⇒
        // re-dial even though it's remembered — self-healing.
        let registered = crate::connections::read_connections(&self.peer_manager)
            .iter()
            .any(|p| p.remote_pid == target.peer_id);
        let unreachable = crate::connection_health::read(&self.peer_manager, &target.peer_id)
            == crate::connection_health::Liveness::Unreachable;
        if registered && !unreachable {
            if let Ok(mut c) = self.system_backend_connect.lock() {
                c.attempts = 0;
            }
            return;
        }

        // Armed and actively dialing — surface Connecting (not Offline) while
        // the boot provision→dial→handshake latency plays out, so the operator
        // sees progress rather than a broken-looking Offline chip that snaps to
        // Connected. Honest per D13/S6: the async connect success records
        // Connected; an exhausted burst records Unreachable. Deduped by the
        // health writer, so re-recording each dial frame is a no-op.
        self.connection_health_writer.record(
            &target.peer_id,
            crate::connection_health::Liveness::Connecting,
            None,
        );

        // Dial from S (the frontend system peer) — the same "from" peer as the
        // manual backend-connect affordance.
        let system_pid = self.peer_manager.system_peer_id().to_string();
        self.handle_connect_peer(system_pid, ws_addr);
        if let Ok(mut c) = self.system_backend_connect.lock() {
            c.attempts += 1;
            c.cooldown = BACKOFF_FRAMES;
        }
    }

    fn drain_pending_backend_peers(&mut self) {
        let pending: Vec<crate::tauri_ipc::BackendPeerInfo> = {
            let mut q = self.pending_backend_peers.lock().unwrap();
            q.drain(..).collect()
        };
        for info in pending {
            if self.peer_manager.sdk_primary().peer_metadata(&info.peer_id).is_some() {
                // Existing peer — update metadata (addresses changed on start/stop).
                self.peer_manager.sdk_mut_primary().set_metadata(&info.peer_id, entity_sdk::sdk::PeerMetadata {
                    label: info.label.clone(),
                    persisted: true,
                    listen_addresses: info.listen_addresses(),
                });
            } else {
                // New peer — register it.
                self.peer_manager.register_backend_peer_primary(
                    info.peer_id.clone(),
                    info.label.clone(),
                    info.listen_addresses(),
                );
            }
        }
        // No signal bump: the end-of-frame `peer_registry.sync()`
        // reconciles these registered/updated backend peers into the
        // tree registry, which every peer-aware window subscribes to.
    }
}

/// Shared derive-and-record task for the backend-auth observability surface
/// (`DESIGN-AUTHORIZE-GATE-INCREMENT-3 §3 Step 3`). Awaits the two prebuilt
/// remote reads (B's `session/*` and `policy/*`), derives pending
/// (`session ∧ ¬policy`) via `peer_auth::classify`, and records the result to
/// the local mirror. Any read failure records a `failed` observation + a loud
/// log line (§5) — never a silent blank that reads as "no peers". Shared by the
/// refresh handler and the post-authorize re-read so the two stay in lockstep.
#[cfg(target_arch = "wasm32")]
async fn derive_and_record_backend_auth(
    session_fut: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::ops::ExecuteResponse, String>>>,
    >,
    policy_fut: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<crate::ops::ExecuteResponse, String>>>,
    >,
    writer: crate::backend_auth::BackendAuthWriter,
    log: EventLogWriter,
    system_pid: String,
    backend_pid: String,
    // The mirror's current observation, captured at trigger time. Used to
    // dedupe: an identical result (same rows, or the same failure) is neither
    // re-logged nor re-written — so the reactive periodic refresh is silent when
    // nothing changed, and a persistently-dead link logs its error once.
    prev: Option<crate::backend_auth::BackendAuthObservation>,
    // The auth read doubles as B's liveness probe — feed its outcome into the
    // subscribable connection-health mirror (the writer dedupes internally).
    health: crate::connection_health::ConnectionHealthWriter,
) {
    let session_keys = match session_fut.await {
        Ok(resp) if resp.result.status == entity_handler::STATUS_OK => {
            crate::backend_auth::parse_listing_keys(&resp.result.result)
        }
        Ok(resp) => {
            return record_auth_failure(
                &writer,
                &log,
                &backend_pid,
                format!("session read returned status {}", resp.result.status), // i18n-ignore — auth-failure diagnostic detail (dev log surface)
                &prev,
                &health,
            )
        }
        Err(e) => {
            return record_auth_failure(
                &writer,
                &log,
                &backend_pid,
                format!("the backend link isn't ready ({})", e), // i18n-ignore — auth-failure diagnostic detail (dev log surface)
                &prev,
                &health,
            )
        }
    };
    let policy_keys = match policy_fut.await {
        Ok(resp) if resp.result.status == entity_handler::STATUS_OK => {
            crate::backend_auth::parse_listing_keys(&resp.result.result)
        }
        Ok(resp) => {
            return record_auth_failure(
                &writer,
                &log,
                &backend_pid,
                format!("policy read returned status {}", resp.result.status), // i18n-ignore — auth-failure diagnostic detail (dev log surface)
                &prev,
                &health,
            )
        }
        Err(e) => {
            return record_auth_failure(
                &writer,
                &log,
                &backend_pid,
                format!("policy read failed ({})", e), // i18n-ignore — auth-failure diagnostic detail (dev log surface)
                &prev,
                &health,
            )
        }
    };

    // Exclude S (manager) and B (self) — base58. Sessions are hex-keyed; S's
    // base58 won't string-match its hex session key, but S holds the manager
    // policy grant → once that self-heals to hex at S's handshake, classify
    // marks S Authorized (not Pending), so it never surfaces as an *actionable*
    // row. Cleanly hiding the manager row entirely would need S's identity-hash
    // hex — a documented follow-up (the `peer_auth` keying contract).
    let mut exclude = std::collections::BTreeSet::new();
    exclude.insert(system_pid);
    exclude.insert(backend_pid.clone());
    let authorized: std::collections::BTreeSet<String> = policy_keys.into_iter().collect();
    let rows = crate::peer_auth::classify(&session_keys, &authorized, &exclude);
    let pending = rows
        .iter()
        .filter(|r| r.state == crate::peer_auth::AuthState::Pending)
        .count();
    // The read reached B and came back — B is live. Feed the health mirror
    // (deduped) regardless of whether the auth rows themselves changed.
    health.record(&backend_pid, crate::connection_health::Liveness::Connected, None);
    let obs = crate::backend_auth::BackendAuthObservation::ok(&backend_pid, rows);
    // Dedupe: unchanged since the last read → stay silent (no event-log line, no
    // mirror rewrite → no needless repaint). Keeps the reactive periodic refresh
    // quiet when nothing's new.
    if prev.as_ref() == Some(&obs) {
        return;
    }
    log.log(format!(
        "✓ auth-refresh {} → {} connected, {} authorized, {} pending",
        backend_pid,
        session_keys.len(),
        authorized.len(),
        pending
    ));
    writer.record(&obs);
}

/// The remote-peer authority from an `entity://{pid}/...` handler URI (the
/// dispatch target), or `None` for a non-`entity` / local URI.
#[cfg(target_arch = "wasm32")]
fn entity_uri_authority(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix("entity://")?;
    let authority = rest.split('/').next()?;
    (!authority.is_empty()).then_some(authority)
}

/// Fold a dispatch outcome into the connection-health mirror for a target: any
/// answer (`Ok`) ⇒ the peer is reachable (`Connected`); a transport `Err` ⇒
/// `Unreachable`. No-op when the target isn't a resolvable remote. The writer
/// dedupes, so repeated same-state calls are free.
#[cfg(target_arch = "wasm32")]
fn mark_health(
    health: &crate::connection_health::ConnectionHealthWriter,
    target: &Option<String>,
    outcome: Result<(), String>,
) {
    let Some(pid) = target else { return };
    match outcome {
        Ok(()) => health.record(pid, crate::connection_health::Liveness::Connected, None),
        Err(e) => health.record(pid, crate::connection_health::Liveness::Unreachable, Some(e)),
    }
}

/// Record a loud backend-auth read failure (§5): the observation mirror gets a
/// `failed` entry (so the window renders an error banner, not "no peers") and
/// the event log gets a line.
#[cfg(target_arch = "wasm32")]
fn record_auth_failure(
    writer: &crate::backend_auth::BackendAuthWriter,
    log: &EventLogWriter,
    backend_pid: &str,
    detail: String,
    prev: &Option<crate::backend_auth::BackendAuthObservation>,
    health: &crate::connection_health::ConnectionHealthWriter,
) {
    // The probe couldn't reach/read B — mark the link Unreachable (deduped).
    health.record(
        backend_pid,
        crate::connection_health::Liveness::Unreachable,
        Some(detail.clone()),
    );
    let msg = format!("cannot read backend authorizations — {}", detail); // i18n-ignore — auth-failure diagnostic detail (dev log surface)
    let obs = crate::backend_auth::BackendAuthObservation::failed(backend_pid, msg);
    // Dedupe: the same failure is already recorded → don't re-log or re-write
    // (so a persistently-stale link logs its error once, not every refresh tick).
    if prev.as_ref() == Some(&obs) {
        return;
    }
    log.log(format!(
        "✗ auth-refresh {} → {}",
        backend_pid,
        obs.error.as_deref().unwrap_or("")
    ));
    writer.record(&obs);
}

/// Compose the always-on status-bar summary (`N windows · M peers · Saved`).
/// Pure so the wording/pluralization/durability label is unit-testable without
/// a DOM. Called from the wasm-only [`EntityApp::update_status_bar`].
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn status_summary(windows: usize, peers: usize, can_persist: bool) -> String {
    let win = crate::i18n::t_plural("window.count", windows as i64, &[("n", &windows.to_string())]);
    let peer = crate::i18n::t_plural("peer.count", peers as i64, &[("n", &peers.to_string())]);
    let durability = crate::i18n::t(
        if can_persist { "statusbar.saved" } else { "statusbar.not_saved" },
        &[],
    );
    format!("{win} · {peer} · {durability}") // i18n-ignore — slot-only composition; parts localized above
}

#[cfg(test)]
mod status_summary_tests {
    use super::status_summary;

    #[test]
    fn pluralizes_and_labels_durability() {
        // Counts route through t_plural, which bidi-isolates the {n} arg
        // (U+2068 … U+2069) so numbers render correctly in RTL locales too.
        assert_eq!(
            status_summary(1, 1, true),
            "\u{2068}1\u{2069} window · \u{2068}1\u{2069} peer · Saved"
        );
        assert_eq!(
            status_summary(3, 2, true),
            "\u{2068}3\u{2069} windows · \u{2068}2\u{2069} peers · Saved"
        );
        assert_eq!(
            status_summary(0, 0, false),
            "\u{2068}0\u{2069} windows · \u{2068}0\u{2069} peers · Not saved"
        );
    }
}
