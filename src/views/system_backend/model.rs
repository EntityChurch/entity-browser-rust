//! System Backend window model — the live view of the canonical backend peer
//! (B): its identity/status (polled from `list_backend_peers`), the S↔B
//! connection (read from the connections registry), and a tail of B's native
//! `tracing` output (polled from `backend_log_tail`).
//!
//! Cloneable (`Arc<Mutex<_>>`) so the async poll loop can hold a `Weak` and
//! write results back, then flip the window's [`DirtyFlag`] — the same
//! async-fetch-then-repaint pattern as the File Transfer browse cache. State is
//! in-memory (the log ring is process-lifetime only, DESIGN §7), reset when the
//! window closes.

use std::sync::{Arc, Mutex};

use crate::peers::Peers;

use super::output::{AuthRow, AuthorizationsView, BackendStatusView, SystemBackendOutput};

/// Label of the canonical backend peer — must match `src-tauri`'s
/// `SYSTEM_BACKEND_LABEL`. The poll picks this peer out of `list_backend_peers`.
pub const SYSTEM_BACKEND_LABEL: &str = "system-backend";

/// Tree prefix B shares its filesystem root at (mirrors `src-tauri`'s
/// `SHARE_PREFIX`). Shown in the status panel so the operator knows where shared
/// files live in B's namespace.
pub const SHARE_PREFIX: &str = "local/files/shared/";

/// Log levels the window's control offers, coarsest → finest — must match
/// `src-tauri`'s `backend_log::LEVELS`. `off` silences the backend.
pub const LEVELS: &[&str] = &["off", "error", "warn", "info", "debug", "trace"];

/// Client-side cap on retained log lines. Independent of (and ≤) the server
/// ring; keeps the DOM `<pre>` bounded on a long session.
const MAX_LINES: usize = 2000;

/// Backend identity + lifecycle, as last polled. Plain owned strings so the
/// model compiles + unit-tests natively (the IPC types are wasm-only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackendStatus {
    pub peer_id: String,
    pub status: String,
    pub ws_addr: Option<String>,
}

#[derive(Default)]
struct Inner {
    /// The `system-backend` peer as last seen; `None` until first poll returns
    /// (or if it isn't provisioned — shouldn't happen post-Phase-1).
    backend: Option<BackendStatus>,
    /// Whether the backend list has been polled at least once (distinguishes
    /// "not provisioned" from "not fetched yet" — honest empty state, D13).
    fetched: bool,
    /// Tailed log lines in seq order.
    lines: Vec<String>,
    /// Cursor for the next `backend_log_tail` poll.
    cursor: u64,
    /// The backend's on-disk shared-files directory, fetched once (it doesn't
    /// change). `None` until fetched / if the share root couldn't be resolved.
    share_path: Option<String>,
    /// Current backend log level (`off`/…/`trace`). Fetched once at open, then
    /// updated optimistically when the user changes the level control.
    level: String,
    /// Epoch-ms of the last auto-fired backend-auth refresh — throttles the
    /// reactive re-read so device authorizations stay current without a button
    /// (the auth read is also the real S↔B liveness probe).
    last_auth_refresh_ms: f64,
}

#[derive(Clone, Default)]
pub struct SystemBackendModel {
    inner: Arc<Mutex<Inner>>,
}

impl SystemBackendModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the render output. B's **identity and connection state come from
    /// the entity registries**, not the Tauri IPC poll: the peer registry
    /// (frame-loop populated at boot, window-independent) gives B's id + listen
    /// address, and the connections registry + `connection_health` give the live
    /// S↔B link. So the window shows the real state the instant B registers at
    /// boot — no waiting for this window's own poll to warm up — and the same
    /// reads work unchanged the day B is a remote peer over a connection instead
    /// of a local native process. The IPC poll now only *supplements*:
    /// native-process-local facts not in the tree (lifecycle status string,
    /// logs, share path, log level).
    #[allow(dead_code)] // called from the WASM render path
    pub fn render_output(&self, peers: &Peers) -> SystemBackendOutput {
        let inner = self.inner.lock().unwrap();

        // B = the peer-registry record classified System + Native (same
        // structural test the Peers roster / System Overview cards use — no
        // magic label string, no IPC).
        let modes = crate::persistence::peer_modes();
        let native = crate::peer_registry::read_registry(peers).into_iter().find(|r| {
            let d = crate::peer_display::PeerDescriptor::describe(peers, &r.peer_id, &modes);
            d.role == crate::peer_display::PeerRole::System
                && d.runtime == crate::peer_display::PeerRuntime::Native
        });

        // Live S↔B link: registered in the connections registry AND not a
        // definitive Unreachable — the same "up" test the auto-connect drain
        // uses (app.rs `drain_system_backend_connect`), so the two agree.
        let connected = native
            .as_ref()
            .map(|r| {
                let registered = crate::connections::read_connections(peers)
                    .iter()
                    .any(|p| p.remote_pid == r.peer_id);
                let unreachable = crate::connection_health::read(peers, &r.peer_id)
                    == crate::connection_health::Liveness::Unreachable;
                registered && !unreachable
            })
            .unwrap_or(false);

        // Inbound-device authorizations for the canonical backend: read the
        // local mirror (written by the async RefreshBackendAuth read over the
        // system peer's manager grant). The System Backend window binds to the
        // system peer — the manager — so it is the right home for this surface.
        let authorizations = native.as_ref().map(|r| {
            let sys_pid = peers.system_peer_id();
            let path = crate::app_paths::backend_auth_entry_path(
                crate::app_paths::APP_ID,
                sys_pid,
                &r.peer_id,
            );
            match peers
                .get_entity(sys_pid, &path)
                .map(|e| crate::backend_auth::BackendAuthObservation::from_entity(&e))
            {
                Some(o) => AuthorizationsView {
                    backend_pid: r.peer_id.clone(),
                    manager_pid: sys_pid.to_string(),
                    checked: true,
                    error: o.error.clone(),
                    pending: o.pending().map(to_auth_row).collect(),
                    authorized: o.authorized().map(to_auth_row).collect(),
                },
                None => AuthorizationsView {
                    backend_pid: r.peer_id.clone(),
                    manager_pid: sys_pid.to_string(),
                    checked: false,
                    error: None,
                    pending: Vec::new(),
                    authorized: Vec::new(),
                },
            }
        });

        let backend_view = native.as_ref().map(|r| {
            let short = r.peer_id.chars().take(12).collect::<String>();
            // Listen address from the registry; the IPC-polled one is a fallback.
            let ws_addr = r
                .listen_addresses
                .first()
                .cloned()
                .or_else(|| inner.backend.as_ref().and_then(|b| b.ws_addr.clone()));
            // Lifecycle string ("running"/"stopped") is genuine native-process
            // state not in the tree — take it from the IPC poll when present;
            // else derive from the link so the row is never blank.
            let status = inner
                .backend
                .as_ref()
                .map(|b| b.status.clone())
                .unwrap_or_else(|| if connected { "running".into() } else { "starting…".into() });
            BackendStatusView {
                short_id: short,
                peer_id: r.peer_id.clone(),
                status,
                ws_addr,
            }
        });

        SystemBackendOutput {
            backend: backend_view,
            fetched: inner.fetched,
            connected,
            share_prefix: SHARE_PREFIX.to_string(),
            share_path: inner.share_path.clone(),
            log_level: inner.level.clone(),
            log_lines: inner.lines.clone(),
            tauri: is_tauri_runtime(),
            authorizations,
        }
    }

    /// Change the backend log level (a user action on the level control).
    /// Optimistic: reflect the choice locally at once, then apply it server-side
    /// over IPC. The effect is self-evident in the stream (more / fewer lines).
    #[cfg(target_arch = "wasm32")]
    pub fn set_level(&self, level: &str) {
        self.inner.lock().unwrap().level = level.to_string();
        let level = level.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            let _ = crate::tauri_ipc::set_backend_log_level(&level).await;
        });
    }

    /// Whether it's time to auto-fire the backend-auth re-read (reactive, no
    /// button). Throttled so a render every few hundred ms doesn't spam remote
    /// reads; the read itself dedupes so an unchanged result is silent. The
    /// caller only invokes this when connected + a backend is present.
    #[cfg(target_arch = "wasm32")]
    pub fn due_for_auth_refresh(&self) -> bool {
        const INTERVAL_MS: f64 = 4000.0;
        let now = js_sys::Date::now();
        let mut inner = self.inner.lock().unwrap();
        if now - inner.last_auth_refresh_ms >= INTERVAL_MS {
            inner.last_auth_refresh_ms = now;
            true
        } else {
            false
        }
    }

    /// Clear the displayed log lines (a user "Clear" action). The server ring is
    /// left intact and the poll cursor is unchanged, so we simply stop showing
    /// what we've already seen; new lines still stream in. Reopening the window
    /// re-reads the ring's history.
    pub fn clear_logs(&self) {
        self.inner.lock().unwrap().lines.clear();
    }

    /// Start the background poll loop (WASM + Tauri only). Polls
    /// `list_backend_peers` (for B's status) and `backend_log_tail` (for the log
    /// stream) on a fixed cadence, folds results into the model, and marks the
    /// window dirty only when something changed (preserves scroll + avoids
    /// needless rebuilds). Lifetime-tied to the model via a `Weak`: when the
    /// window closes and drops the model, the next `upgrade` fails and the loop
    /// exits — no leak, no dangling timer.
    #[cfg(target_arch = "wasm32")]
    pub fn start_polling(&self, dirty: crate::window_watch::DirtyFlag) {
        if !crate::tauri_ipc::is_tauri() {
            return; // browser: no backend to poll; the window shows a desktop-only note.
        }
        let weak = Arc::downgrade(&self.inner);
        wasm_bindgen_futures::spawn_local(async move {
            // The share path is fixed for the process — fetch it once up front.
            if let Ok(Some(path)) = crate::tauri_ipc::system_backend_share_path().await {
                if let Some(inner_arc) = weak.upgrade() {
                    inner_arc.lock().unwrap().share_path = Some(path);
                }
            }
            // Seed the level control with the backend's current level.
            if let Ok(level) = crate::tauri_ipc::get_backend_log_level().await {
                if !level.is_empty() {
                    if let Some(inner_arc) = weak.upgrade() {
                        inner_arc.lock().unwrap().level = level;
                    }
                }
            }
            // Counts poll ticks so we can nudge a render on a slower cadence
            // (~every 7 ticks ≈ 4s) even when nothing changed — that render
            // drives the reactive backend-auth re-read (which is throttled +
            // deduped, so this stays cheap).
            let mut tick: u32 = 0;
            loop {
                // Bail if the window closed since the last tick.
                let Some(inner_arc) = weak.upgrade() else {
                    break;
                };

                tick = tick.wrapping_add(1);
                let cursor = { inner_arc.lock().unwrap().cursor };
                let list = crate::tauri_ipc::list_backend_peers().await.ok();
                let tail = crate::tauri_ipc::backend_log_tail(cursor).await.ok();

                let mut changed = false;
                {
                    let mut inner = inner_arc.lock().unwrap();
                    if let Some(list) = list {
                        inner.fetched = true;
                        let found = list
                            .into_iter()
                            .find(|p| p.label.as_deref() == Some(SYSTEM_BACKEND_LABEL))
                            .map(|p| BackendStatus {
                                peer_id: p.peer_id,
                                status: p.status,
                                ws_addr: p.ws_addr,
                            });
                        if found != inner.backend {
                            inner.backend = found;
                            changed = true;
                        }
                    }
                    if let Some(tail) = tail {
                        inner.cursor = tail.cursor;
                        if !tail.lines.is_empty() {
                            inner.lines.extend(tail.lines);
                            let overflow = inner.lines.len().saturating_sub(MAX_LINES);
                            if overflow > 0 {
                                inner.lines.drain(0..overflow);
                            }
                            changed = true;
                        }
                    }
                }
                // Drop the strong ref before sleeping so the window can drop the
                // model (and end this loop) during the wait.
                drop(inner_arc);
                // Nudge a render on a slow cadence to drive the reactive
                // auth re-read even when logs/status are quiet.
                if changed || tick % 7 == 0 {
                    dirty.mark();
                }
                sleep_ms(600).await;
            }
        });
    }
}

/// Project a derived auth row into its render view. The device key is an
/// identity-hash hex string (how the backend reports sessions); keep the full id
/// for the authorize target and show a truncated form.
fn to_auth_row(row: &crate::peer_auth::PeerAuthRow) -> AuthRow {
    let display = if row.peer_id.len() > 14 {
        format!("{}…", &row.peer_id[..14])
    } else {
        row.peer_id.clone()
    };
    AuthRow {
        peer_id: row.peer_id.clone(),
        display,
    }
}

/// Whether we're in a Tauri WebView (backend IPC available). Split out so
/// `render_output` compiles natively (always `false` off-wasm).
#[cfg(target_arch = "wasm32")]
fn is_tauri_runtime() -> bool {
    crate::tauri_ipc::is_tauri()
}
#[cfg(not(target_arch = "wasm32"))]
fn is_tauri_runtime() -> bool {
    false
}

/// `setTimeout`-backed async sleep (no timer-crate dep). Resolves a promise from
/// a `window.setTimeout` callback and awaits it.
#[cfg(target_arch = "wasm32")]
async fn sleep_ms(ms: i32) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Register B as the canonical native system backend in the entity peer
    /// registry (the real path — the frame loop does this at boot), then sync it
    /// to the tree so `read_registry` reflects it. Replaces the old
    /// `set_backend_for_test` inner-injection: identity now comes from the
    /// registry, so the test drives the same source production does.
    fn register_native_backend(peers: &mut Peers, pid: &str) {
        peers.register_backend_peer_primary(
            pid.to_string(),
            Some(SYSTEM_BACKEND_LABEL.to_string()),
            vec!["ws://127.0.0.1:4042".to_string()],
        );
        let mut reg = crate::peer_registry::PeerRegistry::new(peers);
        reg.sync(peers);
    }

    #[test]
    fn empty_model_reports_not_fetched_and_disconnected() {
        let peers = Peers::new_direct();
        let model = SystemBackendModel::new();
        let out = model.render_output(&peers);
        assert!(out.backend.is_none());
        assert!(!out.fetched);
        assert!(!out.connected);
        assert!(out.log_lines.is_empty());
        assert_eq!(out.share_prefix, SHARE_PREFIX);
        assert!(out.authorizations.is_none(), "no backend → no auth surface");
    }

    // --- Device-authorization projection (moved here from peer_connections) ---

    #[test]
    fn backend_without_observation_reads_unchecked() {
        let mut peers = Peers::new_direct();
        register_native_backend(&mut peers, "REMOTE_B");
        let model = SystemBackendModel::new();

        let auth = model.render_output(&peers).authorizations.expect("backend → auth surface");
        assert_eq!(auth.backend_pid, "REMOTE_B");
        assert_eq!(auth.manager_pid, peers.system_peer_id());
        assert!(!auth.checked, "no observation yet → prompts a Check access");
        assert!(auth.pending.is_empty() && auth.authorized.is_empty());
    }

    #[test]
    fn observation_mirror_projects_into_pending_and_authorized_rows() {
        use crate::backend_auth::{BackendAuthObservation, BackendAuthWriter};
        use crate::peer_auth::{AuthState, PeerAuthRow};

        let mut peers = Peers::new_direct();
        register_native_backend(&mut peers, "REMOTE_B");
        // The async remote read would write this mirror; do it directly here.
        BackendAuthWriter::new(&peers).record(&BackendAuthObservation::ok(
            "REMOTE_B",
            vec![
                PeerAuthRow { peer_id: "aaaa1111".into(), state: AuthState::Pending },
                PeerAuthRow { peer_id: "bbbb2222".into(), state: AuthState::Authorized },
            ],
        ));

        let model = SystemBackendModel::new();
        let auth = model.render_output(&peers).authorizations.unwrap();

        assert!(auth.checked, "mirror present → checked");
        assert_eq!(auth.error, None);
        assert_eq!(auth.pending.len(), 1);
        assert_eq!(auth.pending[0].peer_id, "aaaa1111", "authorize target keeps the full id");
        assert_eq!(auth.authorized.len(), 1);
        assert_eq!(auth.authorized[0].peer_id, "bbbb2222");
    }

    #[test]
    fn observation_read_failure_surfaces_error() {
        use crate::backend_auth::{BackendAuthObservation, BackendAuthWriter};

        let mut peers = Peers::new_direct();
        register_native_backend(&mut peers, "REMOTE_B");
        BackendAuthWriter::new(&peers)
            .record(&BackendAuthObservation::failed("REMOTE_B", "no manager capability"));

        let model = SystemBackendModel::new();
        let auth = model.render_output(&peers).authorizations.unwrap();

        assert!(auth.checked);
        assert_eq!(auth.error.as_deref(), Some("no manager capability"));
        assert!(auth.pending.is_empty(), "an error must not read as pending rows");
    }
}
