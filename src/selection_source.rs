//! Panel selection-source — the consumer side of the Stage B
//! selection slots.
//!
//! A consumer panel declares which selector types it can act on
//! (`consumes`); a producer declares which it emits (`produces`). The
//! intersection drives both the build-time dropdown filter and the
//! run-time consumption filter ("the Lego slot"). A panel's chosen
//! source persists in its window state as the wire string below.
//!
//! v1 ships `None` + `App aggregate`. `Panel(WindowId)` is in the
//! enum + wire form for forward-compatibility but the dropdown does
//! not list per-panel sources yet — that needs a cheap panel
//! registry (design §4.1). Don't wire per-panel by faking it with a
//! per-render get-storm; that regresses Stage A's zero-get render.

#![allow(dead_code)]

use crate::window::WindowId;

/// Where a consumer panel pulls selections from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionSource {
    /// Manual — the panel ignores other panels' selections. Default;
    /// the documented safe default (no surprise co-orientation).
    #[default]
    None,
    /// Follow the app-aggregate slot (`workspace/selection`).
    AppAggregate,
    /// Follow one panel's slot (`workspace/panels/{id}/selection`).
    /// Forward-compatible; not offered by the v1 dropdown (§4.1).
    Panel(WindowId),
}

impl SelectionSource {
    /// Parse the persisted / `<select>`-value wire form. Unknown
    /// strings fall back to `None` (manual) — a corrupt setting must
    /// never silently bind a panel to someone else's cursor.
    pub fn parse(wire: &str) -> Self {
        match wire {
            "app" => SelectionSource::AppAggregate,
            other => {
                if let Some(rest) = other.strip_prefix("panel:") {
                    if let Ok(id) = rest.parse::<WindowId>() {
                        return SelectionSource::Panel(id);
                    }
                }
                SelectionSource::None
            }
        }
    }

    /// Wire form. Persisted in window state; also the `<option>`
    /// value. `None` => `"none"` (callers omit it from CBOR when
    /// they prefer absence = default).
    pub fn to_wire(self) -> String {
        match self {
            SelectionSource::None => "none".to_string(),
            SelectionSource::AppAggregate => "app".to_string(),
            SelectionSource::Panel(id) => format!("panel:{id}"),
        }
    }

    /// True when the panel should not co-orient to anything.
    pub fn is_manual(self) -> bool {
        matches!(self, SelectionSource::None)
    }
}

/// The "Lego slot" type contract for a window type. Empty slices =
/// "this panel neither emits nor accepts cross-panel selections."
#[derive(Debug, Clone, Copy)]
pub struct SelectionContract {
    /// `Selection.type_` values this panel publishes.
    pub produces: &'static [&'static str],
    /// `Selection.type_` values this panel can co-orient to.
    pub consumes: &'static [&'static str],
}

impl SelectionContract {
    const EMPTY: SelectionContract = SelectionContract {
        produces: &[],
        consumes: &[],
    };

    /// True if `other` can feed this slot — produces ∩ consumes ≠ ∅.
    /// Used both to filter dropdown options (build-time) and to
    /// decide whether a delivered `Selection.type_` is acceptable
    /// (run-time, via [`accepts`]).
    pub fn can_be_fed_by(&self, other: &SelectionContract) -> bool {
        other
            .produces
            .iter()
            .any(|p| self.consumes.contains(p))
    }

    /// Run-time filter: is this selector type one this slot accepts?
    pub fn accepts(&self, selector_type: &str) -> bool {
        self.consumes.contains(&selector_type)
    }
}

/// Single source of truth for the produce/consume contract, keyed by
/// `WindowView::type_name()`. One table instead of two fields on 12
/// `WindowType` literals — and the table that gets handed to
/// workbench-go for cross-impl parity (design §7).
pub fn selection_contract(type_name: &str) -> SelectionContract {
    match type_name {
        // Entity Tree publishes `type:"entity"` Selections on
        // Navigate (Stage B) and can co-orient to an entity path —
        // both producer and the first proof consumer.
        "Entity Tree" => SelectionContract {
            produces: &["entity"],
            consumes: &["entity"],
        },
        // Shell publishes `type:"entity"` Selections on `cd` (Phase 3).
        // Doesn't consume yet — co-orienting the shell's wd to other
        // panels' selections is a Stage 5 question (auto-follow).
        "Shell" => SelectionContract {
            produces: &["entity"],
            consumes: &[],
        },
        _ => SelectionContract::EMPTY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_round_trip() {
        for s in [
            SelectionSource::None,
            SelectionSource::AppAggregate,
            SelectionSource::Panel(7),
        ] {
            assert_eq!(SelectionSource::parse(&s.to_wire()), s);
        }
    }

    #[test]
    fn unknown_wire_falls_back_to_manual() {
        assert_eq!(SelectionSource::parse(""), SelectionSource::None);
        assert_eq!(SelectionSource::parse("garbage"), SelectionSource::None);
        // Malformed panel ref must not bind — falls back to manual.
        assert_eq!(SelectionSource::parse("panel:notanum"), SelectionSource::None);
    }

    #[test]
    fn entity_tree_is_producer_and_consumer() {
        let et = selection_contract("Entity Tree");
        assert_eq!(et.produces, &["entity"]);
        assert_eq!(et.consumes, &["entity"]);
        // Entity Tree can follow another Entity Tree.
        assert!(et.can_be_fed_by(&et));
        assert!(et.accepts("entity"));
        assert!(!et.accepts("query-result"));
    }

    #[test]
    fn non_participating_windows_have_empty_contract() {
        for name in ["Event Log", "Settings", "Peer Connections", "Knowledge Base"] {
            let c = selection_contract(name);
            assert!(c.produces.is_empty(), "{name} should not produce");
            assert!(c.consumes.is_empty(), "{name} should not consume");
            // Empty consumer can't be fed by Entity Tree.
            assert!(!c.can_be_fed_by(&selection_contract("Entity Tree")));
        }
    }
}
