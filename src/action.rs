//! Actions — the Controller in MVC.
//!
//! The DOM renderer produces `Action` values. The app processes
//! them to mutate state.

use crate::window::WindowId;

/// Actions produced by renderers.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Action {
    // -- Window management --
    /// Spawn a new window of the given type name, optionally bound to a specific peer.
    SpawnWindow { type_name: &'static str, peer_id: Option<String> },
    /// Close a specific window instance.
    CloseWindow(WindowId),
    /// Maximize a window into the full-screen surface, or restore it if it is
    /// already the maximized one (reframe §4-B Surfaces, one-deep).
    ToggleMaximizeWindow(WindowId),

    // -- Per-window actions (targeted by window ID) --
    /// Navigate to a tree path in a specific window.
    Navigate(WindowId, String),
    /// Go up one level in a specific window.
    NavigateUp(WindowId),
    /// Toggle the expanded state of a tree group at `path` in the
    /// Entity Tree window. Panel-local action (Stage A).
    EntityTreeToggleExpand(WindowId, String),
    /// Set the Entity Tree search filter. Panel-local action (Stage A).
    EntityTreeSetSearch(WindowId, String),
    /// Set a consumer panel's selection source (wire form: `none` /
    /// `app` / `panel:{id}`). Panel-local; persists in window state.
    SetSelectionSource(WindowId, String),

    // -- Peer operations (async, handled by app with runtime) --
    /// Connect to a remote peer at a WebSocket address. `peer_id` is the
    /// originating window's bound local peer — the outbound connection is
    /// established from *that* peer's SDK (and the post-connect type fetch
    /// routes through it), not the primary's. A Peer-scoped window can be
    /// bound to a non-primary peer, so defaulting to primary here was a
    /// reachable peer-scoping bug (AP2 / D15).
    ConnectPeer { peer_id: String, addr: String },
    /// Start listening for inbound connections on the given address.
    StartListener(String),
    /// Execute a handler operation. `peer_id` is the originating
    /// window's bound local peer — the op routes against *that* peer's
    /// SDK, not the primary's (a console can be bound to a non-primary
    /// backend peer via the command palette). For a genuinely-remote
    /// target the producer rewrites `handler_uri` to
    /// `entity://{remote}/...` and it resolves through `peer_id`'s
    /// connection pool; a bare URI (e.g. "system/tree") runs against
    /// `peer_id`'s own tree.
    /// params: optional entity to send as handler params (defaults to system/empty).
    Execute {
        peer_id: String,
        handler_uri: String,
        operation: String,
        resource: Option<String>,
        params: Option<entity_entity::Entity>,
    },
    /// Run an L1 query via the typed `ctx.query()` helper
    /// (SDK-OPERATIONS §5.1). Carries the prebuilt
    /// `system/query/expression` entity from the calling window.
    /// `peer_id` is the originating window's bound peer (see
    /// [`Action::Execute`] re: non-primary binding).
    Query {
        peer_id: String,
        expression: entity_entity::Entity,
    },
    /// Run an L1 count via the typed `ctx.count()` helper
    /// (SDK-EXTENSION-OPERATIONS §6, `count` op). Same expression shape
    /// as [`Action::Query`]; `peer_id` likewise the bound window peer.
    Count {
        peer_id: String,
        expression: entity_entity::Entity,
    },

    // -- Generic window event (system routes by ID, window interprets) --
    /// Window-specific event. The system routes by window_id, the window's
    /// handle_action interprets the event name and value to update its state.
    WindowEvent {
        window_id: WindowId,
        event: String,
        value: String,
    },

    // -- Content Site (Site Mode) --
    /// Navigate a Content Site window to a link target. `target` is a
    /// raw entity-native link string as written in markdown / a nav
    /// entry (`./about`, `site:id/page`, `entity://peer/...`); the
    /// window's model classifies + resolves it. External links never
    /// reach here — the renderer leaves those as real anchors.
    SiteNavigate { window_id: WindowId, target: String },

    /// Navigate the Site Mode **overlay** (the app-level surface, not a
    /// window) to a link target. Same target grammar as [`SiteNavigate`];
    /// dispatched by the overlay's nav/menu and in-page links. External
    /// links never reach here.
    SiteOverlayNavigate { target: String },

    /// Go back to the previous location in a Content Site **window**'s
    /// in-session navigation history (the back affordance). No-op at the
    /// start of history.
    SiteBack { window_id: WindowId },

    /// Open a site from a site-aware window's directory rail at its root
    /// page. `peer` empty = an owned site on the window's bound peer; a
    /// non-empty `peer` = a cached foreign site (resolved from my store).
    SiteOpen { window_id: WindowId, peer: String, site: String },

    /// Toggle the bookmark flag for a site in a site-aware window's
    /// directory (owned or cached). `peer` empty = owned/bound peer.
    SiteBookmarkToggle { window_id: WindowId, peer: String, site: String },

    /// Toggle "keep offline" (full page caching, O3) for a cached site in a
    /// site-aware window's directory. `peer` empty = owned/bound peer.
    SiteKeepToggle { window_id: WindowId, peer: String, site: String },

    /// Set the directory rail's view filter (My / All / External) in a
    /// site-aware window. `filter` is a `RailFilter` wire token. Session-only.
    SiteRailFilter { window_id: WindowId, filter: String },

    /// Go back in the Site Mode **overlay**'s navigation history.
    SiteOverlayBack,

    /// Toggle the Site Mode overlay (`#site-layer`) on/off — flips the
    /// container between the content-site overlay and the entity-browser
    /// chrome. Dispatched by the always-on status-bar toggle button.
    /// Pure app-surface switch; no peer/window scope.
    ToggleSiteMode,

    // -- Peer lifecycle --
    /// Create a new local peer in a specific host/persistence mode —
    /// the single create path (the older mode-less `CreatePeer` was
    /// removed; it was an unproduced dead twin of `Frontend`).
    /// `Frontend` = hosted on the primary SDK; `BackendMemory` /
    /// `BackendOpfs` lazy-spawn a dedicated Worker SDK.
    CreatePeerWithMode {
        label: Option<String>,
        mode: crate::peer_mode::PeerMode,
    },
    /// Create a backend peer via Tauri IPC (native peer in Tauri process).
    CreateBackendPeer { label: Option<String> },
    /// Start a stopped backend peer (boot Peer + WS listener).
    StartBackendPeer(String),
    /// Stop a running backend peer (keep persisted identity).
    StopBackendPeer(String),
    /// Delete a local peer by ID (cannot delete the default peer).
    /// Works for both WASM and backend peers.
    DeletePeer(String),
    /// Update a peer's metadata label. `peer_id` is resolved via
    /// the multi-SDK router, so backend peers rename through their
    /// own worker SDK. Empty `label` clears it.
    RenamePeer { peer_id: String, label: Option<String> },

    /// Clear the shared event log.
    ClearEventLog,

    // -- Shell actions (entity-shell window) --
    /// User pressed Enter in the shell prompt. `line` is the raw input.
    ShellSubmit { window_id: WindowId, line: String },
    /// Up arrow — recall the previous history entry into the input.
    /// `current` carries the live input value so we can save the
    /// in-progress draft before starting a history walk (and restore
    /// it on ArrowDown past the bottom). The shell window is the only
    /// consumer; other windows ignore.
    ShellHistoryPrev { window_id: WindowId, current: String },
    /// Down arrow — recall the next history entry (or restore draft).
    ShellHistoryNext { window_id: WindowId, current: String },
    /// Tab key — request completion against the current input.
    ShellTabComplete { window_id: WindowId, partial: String },
    /// Ctrl-L (or `clear` verb) — wipe scrollback for this shell.
    ShellClear(WindowId),
    /// Install a `tail <prefix>` subscription on a shell window. The
    /// model can't reach the WindowWatch directly so it routes the
    /// install request through this action — the controller installs
    /// the subscription on `self.watch` with a callback that writes
    /// scrollback rows on each change event.
    ShellTail { window_id: WindowId, prefix: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_variants_constructible() {
        let _ = Action::SpawnWindow { type_name: "Entity Tree", peer_id: None };
        let _ = Action::CloseWindow(1);
        let _ = Action::Navigate(1, "docs/test".into());
        let _ = Action::NavigateUp(1);
    }
}
