//! Render-ready output for the System Backend window. Pure data — the DOM
//! renderer (`crate::dom::system_backend`) consumes this without touching the
//! model or `Peers`.

/// Backend identity + lifecycle for the status panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendStatusView {
    /// Full Base58 peer id.
    pub peer_id: String,
    /// First 12 chars, for a compact display.
    pub short_id: String,
    /// Lifecycle string from the backend (`running` / `stopped` / …).
    pub status: String,
    /// WS listen / connect address, when running.
    pub ws_addr: Option<String>,
}

/// Everything the System Backend window renders.
#[derive(Clone, Debug)]
pub struct SystemBackendOutput {
    /// The `system-backend` peer, once polled; `None` if not fetched yet or
    /// (unexpectedly) not provisioned.
    pub backend: Option<BackendStatusView>,
    /// Whether the backend list has been polled at least once — lets the view
    /// distinguish "loading…" from "not provisioned".
    pub fetched: bool,
    /// Whether S is connected to B (from the connections registry).
    pub connected: bool,
    /// Tree prefix B shares its files at.
    pub share_prefix: String,
    /// The backend's shared-files directory on disk (behind `share_prefix`);
    /// `None` until fetched. Answers "where do shared files actually live".
    pub share_path: Option<String>,
    /// Current backend log level (`off`/…/`trace`) — the level control's
    /// selected value. Empty until fetched.
    pub log_level: String,
    /// Tailed lines of B's native `tracing` output, oldest first.
    pub log_lines: Vec<String>,
    /// Whether we're in the desktop app (backend visibility requires Tauri).
    /// `false` in a plain browser → the view shows a desktop-only note.
    pub tauri: bool,
    /// Inbound devices connected to B and their authorization status — the
    /// authority surface (moved here from Peer Connections, Direction A). `None`
    /// until a backend is known.
    pub authorizations: Option<AuthorizationsView>,
}

/// The canonical backend's inbound-device authorization surface — a render-ready
/// projection of a `backend_auth::BackendAuthObservation`. Managing *who can do
/// what on the backend* lives here (the backend owns the grant), not in the
/// file-transfer app.
#[derive(Clone, Debug)]
pub struct AuthorizationsView {
    /// The backend these devices are connected to (dispatch target for
    /// refresh/authorize).
    pub backend_pid: String,
    /// The manager peer that dispatches the read/grant (the system peer, which
    /// holds B's manager capability).
    pub manager_pid: String,
    /// Whether a mirror observation exists yet (a check/refresh has run). `false`
    /// renders a "not checked" prompt rather than an empty list.
    pub checked: bool,
    /// A read failure, cleaned for display (`§5` — shown, never swallowed).
    pub error: Option<String>,
    /// Connected devices awaiting authorization — each an actionable row.
    pub pending: Vec<AuthRow>,
    /// Devices already authorized on the backend.
    pub authorized: Vec<AuthRow>,
}

/// One inbound device in an [`AuthorizationsView`].
#[derive(Clone, Debug)]
pub struct AuthRow {
    /// Device id as the backend reports it (identity-hash hex) — the authorize
    /// target key.
    pub peer_id: String,
    /// Short display form (truncated id).
    pub display: String,
}
