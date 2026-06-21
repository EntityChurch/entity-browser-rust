//! Pure click-routing for the Entity Tree (`dom::entity_tree`, wasm-only).
//!
//! Split out of the wasm DOM handler so the navigate-vs-toggle branching is
//! unit-testable on the native target without a DOM. The handler resolves the
//! clicked element's `data-*` attributes and hands them here.

use crate::action::Action;
use crate::window::WindowId;

/// Map a resolved tree-click target (`data-action` / `data-path` /
/// `data-has-children`) to the actions to dispatch.
///
/// - `data-action` wins (the "Up" button / the disclosure glyph): exactly one
///   action, no navigate — so clicking the glyph toggles once, never twice.
/// - Otherwise a row body (`data-path`): always `Navigate` (select). A
///   **directory** row (`data-has-children`) ALSO toggles expansion, so the
///   whole row — not just the tiny glyph — is a toggle target (touch-friendly).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn click_actions(
    wid: WindowId,
    data_action: Option<&str>,
    data_path: Option<&str>,
    has_children: bool,
) -> Vec<Action> {
    if let Some(a) = data_action {
        return match (a, data_path) {
            ("navigate-up", _) => vec![Action::NavigateUp(wid)],
            ("toggle-expand", Some(p)) => {
                vec![Action::EntityTreeToggleExpand(wid, p.to_string())]
            }
            _ => Vec::new(),
        };
    }
    if let Some(p) = data_path {
        let mut out = Vec::new();
        if has_children {
            out.push(Action::EntityTreeToggleExpand(wid, p.to_string()));
        }
        out.push(Action::Navigate(wid, p.to_string()));
        return out;
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::click_actions;
    use crate::action::Action;

    const WID: u64 = 7;

    // A directory row body (data-path + data-has-children, no data-action)
    // toggles expansion AND navigates — the whole row is a toggle target.
    #[test]
    fn directory_row_body_toggles_then_navigates() {
        let acts = click_actions(WID, None, Some("a/b"), true);
        assert!(
            matches!(
                acts.as_slice(),
                [Action::EntityTreeToggleExpand(w, p), Action::Navigate(w2, p2)]
                    if *w == WID && *w2 == WID && p == "a/b" && p2 == "a/b"
            ),
            "directory row body should toggle then navigate, got {acts:?}"
        );
    }

    // A leaf row (no children) only navigates — unchanged behaviour.
    #[test]
    fn leaf_row_body_only_navigates() {
        let acts = click_actions(WID, None, Some("a/b/leaf"), false);
        assert!(
            matches!(acts.as_slice(), [Action::Navigate(w, p)] if *w == WID && p == "a/b/leaf"),
            "leaf row body should only navigate, got {acts:?}"
        );
    }

    // Clicking the disclosure glyph itself toggles exactly once — no navigate,
    // no double-toggle (data-action wins over the row's data-path).
    #[test]
    fn toggle_glyph_toggles_once() {
        let acts = click_actions(WID, Some("toggle-expand"), Some("a/b"), true);
        assert!(
            matches!(acts.as_slice(), [Action::EntityTreeToggleExpand(w, p)] if *w == WID && p == "a/b"),
            "glyph click should toggle exactly once, got {acts:?}"
        );
    }

    #[test]
    fn navigate_up_button() {
        let acts = click_actions(WID, Some("navigate-up"), None, false);
        assert!(matches!(acts.as_slice(), [Action::NavigateUp(w)] if *w == WID));
    }

    #[test]
    fn nothing_matched_is_empty() {
        assert!(click_actions(WID, None, None, false).is_empty());
    }
}
