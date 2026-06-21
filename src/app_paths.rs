//! Application-level path conventions for entity-browser.
//!
//! These helpers encode the entity-browser namespace conventions per the
//! SDK domain spec (GUIDE-PEER-CONCERNS-AND-NAMESPACES.md). They are
//! application-specific — the SDK itself is generic and knows nothing
//! about app IDs, workspace layouts, or window IDs.
//!
//! Per PEER-CONCERNS §4.3 helper signatures take `app_id: &str` as the
//! first argument so the same helpers can serve any app's namespace.
//! Callers within entity-browser pass the [`APP_ID`] constant.
//!
//! Other applications using the SDK define their own path conventions
//! (potentially calling these helpers with their own app id).

use crate::window::WindowId;

/// Default application identifier for entity-browser.
/// Convention: `app/{app_id}/workspace/...` for per-window state,
/// `app/{app_id}/settings/...` for global configuration.
pub const APP_ID: &str = "entity-browser";

/// Build a path under the app workspace.
/// e.g., `workspace_path(APP_ID, pid, "windows/3/state")` → `"/{pid}/app/entity-browser/workspace/windows/3/state"`
pub fn workspace_path(app_id: &str, peer_id: &str, suffix: &str) -> String {
    format!("/{}/app/{}/workspace/{}", peer_id, app_id, suffix)
}

/// Build a path under the app settings namespace.
/// e.g., `settings_path(APP_ID, pid, "ui")` → `"/{pid}/app/entity-browser/settings/ui"`
pub fn settings_path(app_id: &str, peer_id: &str, suffix: &str) -> String {
    format!("/{}/app/{}/settings/{}", peer_id, app_id, suffix)
}

/// Build a per-window state path.
/// e.g., `window_state_path(APP_ID, pid, 3)` → `"/{pid}/app/entity-browser/workspace/windows/3/state"`
pub fn window_state_path(app_id: &str, peer_id: &str, window_id: WindowId) -> String {
    workspace_path(app_id, peer_id, &format!("windows/{}/state", window_id))
}

/// Per-game save-state path (app-tier — frontend persistence under OUR peer,
/// keyed by game id). The embedding contract makes save persistence the host's
/// job; we store the app's opaque `serialize()` output here. Keyed by app-set
/// so a game and an app sharing an id don't collide.
/// e.g. `app_save_path(APP_ID, pid, "games", "chess")` →
/// `"/{pid}/app/entity-browser/apps/games/state/chess"`
pub fn app_save_path(app_id: &str, peer_id: &str, set: &str, app_id_in_set: &str) -> String {
    format!(
        "/{}/app/{}/apps/{}/state/{}",
        peer_id, app_id, set, app_id_in_set
    )
}

/// Prefix for the site-origin registry — a bootstrap/override cache of
/// `target_peer_id → static HTTP origin` for fetching another peer's
/// published content sites over HTTP-poll. App-tier (frontend) state,
/// stored under OUR peer. NOT canonical discovery (that is the target
/// peer's advertised transport profile); see `content_site::origins`.
/// e.g. `site_origins_prefix(APP_ID, pid)` →
/// `"/{pid}/app/entity-browser/site-origins/"`
pub fn site_origins_prefix(app_id: &str, peer_id: &str) -> String {
    format!("/{}/app/{}/site-origins/", peer_id, app_id)
}

/// Registry entry for one target peer's HTTP origin.
/// e.g. `site_origin_path(APP_ID, pid, "PEERB")` →
/// `"/{pid}/app/entity-browser/site-origins/PEERB"`
pub fn site_origin_path(app_id: &str, peer_id: &str, target_peer_id: &str) -> String {
    format!("{}{}", site_origins_prefix(app_id, peer_id), target_peer_id)
}

/// Prefix for the **app-tier** site-cache preferences for a cached foreign
/// site — the browser's own bookkeeping (visit count, bookmarked, is-home,
/// last-viewed-page). The FIELD-SPLIT counterpart to the SDK-tier provenance
/// ledger ([`crate::content_site::cache`], which lives under
/// `/{me}/system/cache/...`): provenance is uniform across every L5 app and is
/// SDK-tier; preferences are genuinely Content-Site-specific and stay in the
/// app namespace. Keyed by `(foreign_peer, site)`, mirroring the content tail.
/// e.g. `site_cache_pref_prefix(APP_ID, me, "BOB", "blog")` →
/// `"/{me}/app/entity-browser/site-cache/BOB/sites/blog/"`
pub fn site_cache_pref_prefix(
    app_id: &str,
    peer_id: &str,
    foreign_peer_id: &str,
    site_id: &str,
) -> String {
    format!(
        "/{}/app/{}/site-cache/{}/sites/{}/",
        peer_id, app_id, foreign_peer_id, site_id
    )
}

/// The whole app-tier site-cache preferences ledger prefix — every
/// `(peer, site)` preference record this peer holds. A Worker-arm surface
/// that reads preferences (the site-aware Content Site window) must observe
/// this prefix, exactly like the SDK-tier provenance ledger
/// ([`crate::content_site::cache::provenance_prefix`]) — the cache mirror only
/// feeds subscribed prefixes (`feedback_worker_cache_get_needs_subscription`).
/// e.g. `site_cache_prefix(APP_ID, me)` → `"/{me}/app/entity-browser/site-cache/"`
pub fn site_cache_prefix(app_id: &str, peer_id: &str) -> String {
    format!("/{}/app/{}/site-cache/", peer_id, app_id)
}

/// Leaf path of the preference record for one `(peer, site)` — the `prefs`
/// entity under [`site_cache_pref_prefix`]. `peer` may be MY own id (prefs on
/// an owned site, e.g. is-home) or a foreign id (prefs on a cached site); the
/// keying is uniform either way.
/// e.g. `site_cache_prefs_path(APP_ID, me, "BOB", "blog")` →
/// `"/{me}/app/entity-browser/site-cache/BOB/sites/blog/prefs"`
pub fn site_cache_prefs_path(
    app_id: &str,
    peer_id: &str,
    site_peer_id: &str,
    site_id: &str,
) -> String {
    format!(
        "{}prefs",
        site_cache_pref_prefix(app_id, peer_id, site_peer_id, site_id)
    )
}

/// Path to the **derived site index** — a small app-tier cache of every site
/// my store holds (owned + cached), refreshed by an async
/// [`crate::content_site::discovery::list_all_sites`] type-query and read
/// synchronously by surfaces that can't await (the Settings picker). The query
/// is the source of truth; this is just its materialized, subscribable result —
/// a sync surface reads it, and a normal subscription on this path re-renders
/// the surface when a refresh repopulates it.
/// e.g. `site_index_path(APP_ID, me)` → `"/{me}/app/entity-browser/site-index"`
pub fn site_index_path(app_id: &str, peer_id: &str) -> String {
    format!("/{}/app/{}/site-index", peer_id, app_id)
}

/// Build a per-window results path.
pub fn window_results_path(app_id: &str, peer_id: &str, window_id: WindowId) -> String {
    workspace_path(app_id, peer_id, &format!("windows/{}/results", window_id))
}

/// Per-panel selection slot — a panel's own "what is selected here"
/// state. Panels publish here on navigate/select; other panels that
/// want to track a specific panel's cursor subscribe to this exact
/// path.
///
/// Path namespace uses **panels**, not **windows** — matches
/// workbench-go's rename (`workspace/panels/{id}/selection`).
/// Rust-side type stays `WindowId` for now; the `windows` vs `panels`
/// distinction is purely on-disk / on-wire for cross-impl portability
/// of the selection schema. The full window-id → panel-id rename in
/// Rust code is tracked separately (design doc §11 deferred item).
///
/// e.g., `panel_selection_path(APP_ID, pid, 5)` →
/// `"/{pid}/app/entity-browser/workspace/panels/5/selection"`
pub fn panel_selection_path(app_id: &str, peer_id: &str, window_id: WindowId) -> String {
    workspace_path(app_id, peer_id, &format!("panels/{}/selection", window_id))
}

/// App-aggregate selection slot — the flat-app analog of the Go
/// reference's per-screen aggregate. Panels publish here on
/// navigate/select with `Propagation::App`; consumers (future
/// inspector, KB co-orient, …) subscribe to this exact path.
///
/// e.g., `app_selection_path(APP_ID, pid)` →
/// `"/{pid}/app/entity-browser/workspace/selection"`
pub fn app_selection_path(app_id: &str, peer_id: &str) -> String {
    workspace_path(app_id, peer_id, "selection")
}

/// Prefix path for the app's event log entries.
/// Each entry lives at `{prefix}{seq:020}` where `seq` is a monotonic counter.
pub fn event_log_prefix(app_id: &str, peer_id: &str) -> String {
    format!("/{}/app/{}/event-log/", peer_id, app_id)
}

/// Path for a specific event log entry by sequence number.
pub fn event_log_entry_path(app_id: &str, peer_id: &str, seq: u64) -> String {
    format!("/{}/app/{}/event-log/{:020}", peer_id, app_id, seq)
}

/// Prefix path for the app's connected-peers registry. Each connected
/// remote peer is recorded as one entity under this prefix.
pub fn connections_prefix(app_id: &str, peer_id: &str) -> String {
    format!("/{}/app/{}/connections/", peer_id, app_id)
}

/// Path for a specific connection entry, keyed by remote peer id.
pub fn connection_entry_path(app_id: &str, peer_id: &str, remote_pid: &str) -> String {
    format!("/{}/app/{}/connections/{}", peer_id, app_id, remote_pid)
}

/// Path for the WebSocket listener's published state (current listen
/// address, when bound).
pub fn listener_state_path(app_id: &str, peer_id: &str) -> String {
    format!("/{}/app/{}/listener/state", peer_id, app_id)
}

/// Prefix for the hosted-peer registry. Every peer the app hosts
/// (primary + frontend + backend) is recorded as one entity under this
/// prefix in the system (primary) peer's tree. Peer-aware windows
/// subscribe here and render the roster directly from the tree, like
/// every other entity-backed window — this is the single
/// peer-membership reactivity mechanism (the in-tree peer registry).
pub fn peers_registry_prefix(app_id: &str, peer_id: &str) -> String {
    format!("/{}/app/{}/system/peers/", peer_id, app_id)
}

/// Path for a single hosted-peer registry record, keyed by hosted peer
/// id. Mirrors [`connection_entry_path`]'s shape.
pub fn peer_registry_entry_path(app_id: &str, peer_id: &str, hosted_pid: &str) -> String {
    format!("/{}/app/{}/system/peers/{}", peer_id, app_id, hosted_pid)
}

/// Prefix for the **authoritative peer roster** — the durable spawn list
/// that replaces the localStorage `entity_peers` lines (set A). One PUBLIC
/// entity per peer (`{peer_id, mode, label}`, NO private key — design §13
/// inv. 8) under the system peer's tree, keyed on the **seed-derived id**.
/// Distinct from [`peers_registry_prefix`] (`system/peers/`, the *derived*
/// display set C) so the authoritative source and the display projection
/// stay separable during the migration. Boot reads this to know which peers
/// to spawn; the private key for each is fetched from the localStorage vault
/// by id.
pub fn roster_prefix(app_id: &str, peer_id: &str) -> String {
    format!("/{}/app/{}/system/roster/", peer_id, app_id)
}

/// Path for a single authoritative roster entry, keyed by the seed-derived
/// hosted peer id. See [`roster_prefix`].
pub fn roster_entry_path(app_id: &str, peer_id: &str, hosted_pid: &str) -> String {
    format!("/{}/app/{}/system/roster/{}", peer_id, app_id, hosted_pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_path_builds_correctly() {
        let path = workspace_path(APP_ID, "PEER1", "ui/selection");
        assert_eq!(path, "/PEER1/app/entity-browser/workspace/ui/selection");
    }

    #[test]
    fn settings_path_builds_correctly() {
        let path = settings_path(APP_ID, "PEER1", "ui");
        assert_eq!(path, "/PEER1/app/entity-browser/settings/ui");
    }

    #[test]
    fn window_state_path_builds_correctly() {
        let path = window_state_path(APP_ID, "PEER1", 5);
        assert_eq!(path, "/PEER1/app/entity-browser/workspace/windows/5/state");
    }

    #[test]
    fn window_results_path_builds_correctly() {
        let path = window_results_path(APP_ID, "PEER1", 5);
        assert_eq!(path, "/PEER1/app/entity-browser/workspace/windows/5/results");
    }

    #[test]
    fn panel_selection_path_builds_correctly() {
        let path = panel_selection_path(APP_ID, "PEER1", 5);
        assert_eq!(
            path,
            "/PEER1/app/entity-browser/workspace/panels/5/selection"
        );
    }

    #[test]
    fn app_selection_path_builds_correctly() {
        let path = app_selection_path(APP_ID, "PEER1");
        assert_eq!(path, "/PEER1/app/entity-browser/workspace/selection");
    }

    #[test]
    fn peers_registry_paths_build_correctly() {
        assert_eq!(
            peers_registry_prefix(APP_ID, "SYS1"),
            "/SYS1/app/entity-browser/system/peers/"
        );
        assert_eq!(
            peer_registry_entry_path(APP_ID, "SYS1", "PEER2"),
            "/SYS1/app/entity-browser/system/peers/PEER2"
        );
    }

    #[test]
    fn site_cache_pref_prefix_builds_correctly() {
        assert_eq!(
            site_cache_pref_prefix(APP_ID, "ME", "BOB", "blog"),
            "/ME/app/entity-browser/site-cache/BOB/sites/blog/"
        );
    }

    #[test]
    fn site_cache_prefix_and_prefs_path_build_correctly() {
        // The broad ledger prefix (what a site-aware window subscribes).
        assert_eq!(
            site_cache_prefix(APP_ID, "ME"),
            "/ME/app/entity-browser/site-cache/"
        );
        // A leaf prefs record nests the `prefs` entity under the per-site prefix,
        // so it sits inside the broad ledger prefix (subscription covers it).
        let leaf = site_cache_prefs_path(APP_ID, "ME", "BOB", "blog");
        assert_eq!(leaf, "/ME/app/entity-browser/site-cache/BOB/sites/blog/prefs");
        assert!(leaf.starts_with(&site_cache_prefix(APP_ID, "ME")));
        assert!(leaf.starts_with(&site_cache_pref_prefix(APP_ID, "ME", "BOB", "blog")));
        // Owned-site prefs key by my own id — uniform with the cached case.
        assert_eq!(
            site_cache_prefs_path(APP_ID, "ME", "ME", "demo"),
            "/ME/app/entity-browser/site-cache/ME/sites/demo/prefs"
        );
    }

    #[test]
    fn parameterized_app_id_changes_namespace() {
        // Demonstrates the parameterization: a different app_id yields a
        // different namespace, so the same helpers can serve any app.
        let path = workspace_path("other-app", "PEER1", "x");
        assert_eq!(path, "/PEER1/app/other-app/workspace/x");
    }
}
