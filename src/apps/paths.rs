//! Tree-path conventions for the embedded-app (games) convention.
//!
//! Content lives in a free subgraph under the owning peer at
//! `/{peer}/apps/{set}/…` — sibling to the content-site `/{peer}/sites/{site}/…`
//! layout. `{set}` is the app-set (today only [`GAMES_SET`]); the prefix is
//! `apps` so non-game sets can join later without a new reserved word.
//!
//! Save-state is *app-tier* (frontend state under our own peer) and lives under
//! the `entity-browser` namespace — see [`crate::app_paths::app_save_path`].

/// The reserved subpath for embedded apps under a peer.
pub const APPS_SUBPATH: &str = "apps";
/// The app-set id for games (canvas games).
pub const GAMES_SET: &str = "games";
/// The app-set id for non-game apps (tools / utilities — calculator, calendar…).
pub const APPS_SET: &str = "apps";

/// Every app-set the platform knows, in display order. One window per set
/// (Games, Apps); publish emits each; discovery scans each.
pub const APP_SETS: &[&str] = &[GAMES_SET, APPS_SET];

/// Classify an entity-apps `index.json` `type` into the owning app-set.
/// `canvas-game` → [`GAMES_SET`]; everything else (tool, calendar, …) →
/// [`APPS_SET`]. This is the one place the games/apps split is decided, so the
/// ingester, the seed, and any future importer agree.
pub fn set_for_type(app_type: &str) -> &'static str {
    if app_type == "canvas-game" {
        GAMES_SET
    } else {
        APPS_SET
    }
}

/// Prefix for an app set's subgraph: `/{peer}/apps/{set}/`.
pub fn set_prefix(peer_id: &str, set: &str) -> String {
    format!("/{}/{}/{}/", peer_id, APPS_SUBPATH, set)
}

/// The catalog (index) entity path: `/{peer}/apps/{set}/catalog`.
pub fn catalog_path(peer_id: &str, set: &str) -> String {
    format!("/{}/{}/{}/catalog", peer_id, APPS_SUBPATH, set)
}

/// A single bundle blob path: `/{peer}/apps/{set}/bundles/{id}`.
pub fn bundle_path(peer_id: &str, set: &str, id: &str) -> String {
    format!("/{}/{}/{}/bundles/{}", peer_id, APPS_SUBPATH, set, id)
}

/// Convenience: the games catalog path for a peer.
pub fn games_catalog_path(peer_id: &str) -> String {
    catalog_path(peer_id, GAMES_SET)
}

/// Convenience: a games bundle path for a peer.
pub fn games_bundle_path(peer_id: &str, id: &str) -> String {
    bundle_path(peer_id, GAMES_SET, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_well_formed() {
        assert_eq!(set_prefix("P", "games"), "/P/apps/games/");
        assert_eq!(games_catalog_path("P"), "/P/apps/games/catalog");
        assert_eq!(games_bundle_path("P", "chess"), "/P/apps/games/bundles/chess");
    }
}
