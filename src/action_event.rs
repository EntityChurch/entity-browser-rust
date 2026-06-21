//! Canonical action-event vocabulary — Rust mirror of the workbench-go
//! `entitysdk/action_event.go` naming conventions.
//!
//! Action events are the **application-agnostic** pub-sub channel
//! defined in `GUIDE-ENTITY-WORKBENCH-APP.md` §5.3 + §6: `navigate`,
//! `select`, `submit`, … mean the same thing across any application
//! that adopts the convention. They're distinct from app-specific
//! shell verbs.
//!
//! **What this module does today (Stage B):** documents the canonical
//! event names + default propagation so call sites pick consistent
//! string literals when writing selection slots (or any future
//! cross-impl wire-shape persistence). The internal `Action` enum in
//! `crate::action` stays Rust-idiomatic; this module is the schema
//! reference, not a dispatcher.
//!
//! **What it doesn't do:** replace `Action`. Wire ⇄ enum translation
//! happens at the persistence boundary (per the entity-tree refactor
//! design §4.4).
//!
//! Differs from the Go reference in one named-thing: our context
//! propagation level is **App** (no screen layer — we're flat) where
//! the Go reference uses **Context** (per-screen). Same semantic
//! intent: "write the panel's emission to the aggregate slot so other
//! panels in the same scope can co-orient."
//!
//! ⚠ **NOT DEAD CODE — DO NOT DELETE without arch-team sign-off.**
//! This module has zero in-repo call sites *by design*: it is a
//! load-bearing **cross-impl schema anchor** (mirrors workbench-go's
//! `entitysdk/action_event.go`). The `#![allow(dead_code)]` below
//! exists because a string/enum schema reference legitimately has no
//! Rust callers until wire-shape persistence/replay lands; it is the
//! contract, not unused code. Keep/delete is an explicit cross-impl
//! decision (system review §5) — handed to the arch team,
//! tracked in BACKLOG, not a unilateral cleanup target.

#![allow(dead_code)]

/// Canonical event name. `as_str()` returns the wire-form string used
/// in `(window_id, event, value)` triples and in the
/// `EventName -> default propagation` lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionEvent {
    /// "I'm attending to this path." Value: text (path).
    /// Producer: tree-browser cursor move, future shell `cd`.
    /// Default propagation: App.
    Navigate,
    /// "I have chosen this path." Value: text (path).
    /// Producer: tree-browser enter, query-console result click.
    /// Default propagation: App.
    Select,
    /// "Execute the buffered command/query." Value: empty.
    /// Producer: execute-console, query-console.
    /// Default propagation: Panel.
    Submit,
    /// "Clear my output / buffer." Value: empty.
    /// Producer: event-log clear, execute-console clear.
    /// Default propagation: Panel.
    Clear,
    /// "Filter view by this." Value: text (filter expression).
    /// Default propagation: Panel.
    SetFilter,
    /// "Toggle raw/decoded view." Value: empty.
    /// Default propagation: Panel.
    ToggleRaw,
    /// "Toggle expand state of this group." Value: text (path).
    /// Our extension to the guide's vocabulary — added for the Entity
    /// Tree refactor. Flagged in design §4.4 as a coordination item:
    /// asks the arch team to canonicalize a vocabulary-extension
    /// process so each impl ships the same name. Default propagation:
    /// Panel (expand state is per-panel view state).
    ToggleExpand,
}

impl ActionEvent {
    /// Wire-form string. Stable identifier; persisted in action
    /// history (future) and in cross-impl portable state.
    pub fn as_str(self) -> &'static str {
        match self {
            ActionEvent::Navigate => "navigate",
            ActionEvent::Select => "select",
            ActionEvent::Submit => "submit",
            ActionEvent::Clear => "clear",
            ActionEvent::SetFilter => "set_filter",
            ActionEvent::ToggleRaw => "toggle_raw",
            ActionEvent::ToggleExpand => "toggle_expand",
        }
    }

    /// Default propagation per the guide §5.3.
    pub fn default_propagation(self) -> Propagation {
        match self {
            ActionEvent::Navigate | ActionEvent::Select => Propagation::App,
            ActionEvent::Submit
            | ActionEvent::Clear
            | ActionEvent::SetFilter
            | ActionEvent::ToggleRaw
            | ActionEvent::ToggleExpand => Propagation::Panel,
        }
    }
}

/// Whether an action event propagates beyond its emitting panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Propagation {
    /// Stays local to the emitting panel; written only to the
    /// per-panel selection slot.
    Panel,
    /// Written to the app-aggregate selection slot too; other panels
    /// in the app can subscribe and co-orient.
    ///
    /// Differs from the Go reference's `Context` only in scoping
    /// (their per-screen vs our flat-app). Same semantic intent.
    App,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_match_guide() {
        assert_eq!(ActionEvent::Navigate.as_str(), "navigate");
        assert_eq!(ActionEvent::Select.as_str(), "select");
        assert_eq!(ActionEvent::Submit.as_str(), "submit");
        assert_eq!(ActionEvent::Clear.as_str(), "clear");
        assert_eq!(ActionEvent::SetFilter.as_str(), "set_filter");
        assert_eq!(ActionEvent::ToggleRaw.as_str(), "toggle_raw");
        assert_eq!(ActionEvent::ToggleExpand.as_str(), "toggle_expand");
    }

    #[test]
    fn navigate_and_select_propagate_to_app() {
        assert_eq!(
            ActionEvent::Navigate.default_propagation(),
            Propagation::App
        );
        assert_eq!(
            ActionEvent::Select.default_propagation(),
            Propagation::App
        );
    }

    #[test]
    fn panel_local_events_stay_panel() {
        for ev in [
            ActionEvent::Submit,
            ActionEvent::Clear,
            ActionEvent::SetFilter,
            ActionEvent::ToggleRaw,
            ActionEvent::ToggleExpand,
        ] {
            assert_eq!(ev.default_propagation(), Propagation::Panel);
        }
    }
}
