//! The standard window-type roster — **one source of truth**.
//!
//! Before this module the 15 `register_type(…::window_type())` calls lived
//! inline in `EntityApp::build_wasm_app`, and nothing else could enumerate the
//! roster. The startup-surface settings control needs that list (to offer
//! "boot into <window type>"), and a second hand-maintained copy would drift
//! (handoff §6.3 — and CLAUDE.md already drifted to "Ten Windows" while 15 are
//! registered). So the roster lives here, built once, consumed by both the
//! registrar ([`standard_window_types`]) and the UI ([`standard_window_type_meta`]).
//!
//! Non-gated: the `…::window_type()` factories compile natively, so the
//! drift-guard test runs in `cargo test` without WASM.

use crate::views::{
    chain_trace::ChainTraceWindow,
    content_stream::ContentStreamWindow,
    entity_tree::EntityTreeWindow,
    event_log::EventLogWindow,
    execute_console::ExecuteConsoleWindow,
    games::AppWindow,
    key_manager::KeyManagerWindow,
    knowledge_base::KnowledgeBaseWindow,
    path_tap::PathTapWindow,
    peer_connections::PeerConnectionsWindow,
    peer_management::PeerManagementWindow,
    query_console::QueryConsoleWindow,
    settings::SettingsWindow,
    shell::ShellWindow,
    site_editor::SiteEditorWindow,
    storage::StorageWindow,
    wire_recorder::WireRecorderWindow,
};
use crate::window::{WindowCategory, WindowScope, WindowType};

/// The 19 standard window types, in registration order. The single source —
/// `build_wasm_app` registers exactly these, and the settings UI reads their
/// metadata from the same list. Add a window here and it shows up in both.
pub fn standard_window_types() -> Vec<WindowType> {
    vec![
        EntityTreeWindow::window_type(),
        AppWindow::games_window_type(),
        AppWindow::apps_window_type(),
        KnowledgeBaseWindow::window_type(),
        KeyManagerWindow::window_type(),
        PeerConnectionsWindow::window_type(),
        ExecuteConsoleWindow::window_type(),
        QueryConsoleWindow::window_type(),
        SettingsWindow::window_type(),
        EventLogWindow::window_type(),
        PeerManagementWindow::window_type(),
        ShellWindow::window_type(),
        ChainTraceWindow::window_type(),
        PathTapWindow::window_type(),
        WireRecorderWindow::window_type(),
        ContentStreamWindow::window_type(),
        crate::views::content_site::ContentSiteWindow::window_type(),
        StorageWindow::window_type(),
        SiteEditorWindow::window_type(),
    ]
}

/// Lightweight `(name, scope)` view of the roster for UI consumers that only
/// need to label + scope-filter the list (no factory fn pointer). Same source
/// as [`standard_window_types`], so it can't drift.
pub fn standard_window_type_meta() -> Vec<(&'static str, WindowScope)> {
    standard_window_types()
        .iter()
        .map(|t| (t.name, t.scope))
        .collect()
}

/// The user-facing menu grouping: each category and the window names under it,
/// in display order. **This is the one place to re-sort the menu** — move a name
/// between groups, reorder within a group, and both the palette and the
/// drift-guard test follow.
///
/// Membership is by `name` (the `&'static str` from each `window_type()`), which
/// keeps the mapping a flat, readable table without threading a category field
/// through all 19 factories. The drift test below proves every registered window
/// appears here exactly once.
pub fn window_groups() -> Vec<(WindowCategory, Vec<&'static str>)> {
    use WindowCategory::*;
    vec![
        (
            AppsContent,
            vec!["Games", "Apps", "Site Browser", "Site Creator", "Knowledge Base"],
        ),
        (
            System,
            vec!["Settings", "Peers", "Peer Connections", "Key Manager", "Storage"],
        ),
        (
            Developer,
            vec![
                "Entity Tree",
                "Shell",
                "Execute Console",
                "Query Console",
                "Event Log",
                "Chain Trace",
                "Path Tap",
                "Wire Recorder",
                "Content Stream",
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn meta_matches_registered_types_no_drift() {
        let full = standard_window_types();
        let meta = standard_window_type_meta();
        assert_eq!(full.len(), meta.len());
        for (t, (name, scope)) in full.iter().zip(meta.iter()) {
            assert_eq!(t.name, *name);
            assert_eq!(t.scope, *scope);
        }
    }

    #[test]
    fn roster_is_nineteen_and_settings_is_system_scoped() {
        let meta = standard_window_type_meta();
        assert_eq!(meta.len(), 19, "the standard roster is 19 windows");
        // Spot-check the scope partition the settings filter relies on.
        let settings = meta.iter().find(|(n, _)| *n == "Settings").expect("Settings present");
        assert_eq!(settings.1, WindowScope::System);
        let tree = meta.iter().find(|(n, _)| *n == "Entity Tree").expect("Entity Tree present");
        assert_eq!(tree.1, WindowScope::Peer);
        // The Site Creator (formerly "Site Editor") is Peer-scoped (it edits sites on a chosen peer).
        let editor = meta.iter().find(|(n, _)| *n == "Site Creator").expect("Site Creator present");
        assert_eq!(editor.1, WindowScope::Peer);
    }

    /// Every registered window must appear in exactly one menu group, and no
    /// group may name a window that isn't registered. This catches the common
    /// mistake: add a window to `standard_window_types` but forget to file it
    /// under a category, leaving it invisible in the palette.
    #[test]
    fn every_window_is_in_exactly_one_group() {
        let registered: HashSet<&'static str> =
            standard_window_types().iter().map(|t| t.name).collect();

        let mut grouped: Vec<&'static str> = Vec::new();
        for (_cat, names) in window_groups() {
            grouped.extend(names);
        }
        let grouped_set: HashSet<&'static str> = grouped.iter().copied().collect();

        assert_eq!(
            grouped.len(),
            grouped_set.len(),
            "a window is listed under more than one group"
        );
        assert_eq!(
            grouped_set, registered,
            "menu groups must cover exactly the registered roster (missing or stale name)"
        );
    }
}
