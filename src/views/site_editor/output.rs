//! Site Editor render output — pure value types the DOM renderer consumes.
//!
//! Built by [`super::model::SiteEditorModel`] from the live tree + the in-memory
//! editor UI state (selection, current directory, collapse/preview toggles). No
//! behaviour, so they unit-test without WASM.

use crate::views::entity_tree::tree::VisibleRow;

/// A one-line status message after an action (create / save / validation error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// True = a problem (validation failure); false = an informational success.
    pub is_error: bool,
}

/// One owned site in the "Your sites" list, with its render health folded in as
/// a small per-row indicator (a ✓ / ⚠ next to the name) rather than a separate
/// top-level line. `reason` is the not-renderable explanation (tooltip).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteListItem {
    pub id: String,
    /// Does the site render in the (frozen) browser right now? The D13 check.
    pub renderable: bool,
    /// Why it won't render (empty when it renders) — surfaced as a tooltip.
    pub reason: String,
}

/// The currently-selected site's editing context.
pub struct SelectedSite {
    pub site_id: String,
    /// Is the tree-navigator region expanded?
    pub pages_open: bool,
    /// The site's page tree, flattened to the currently-visible rows (the same
    /// [`VisibleRow`] the Entity Tree inspector renders). A row with `has_entry`
    /// is an editable page; otherwise it's a folder (the add-target when clicked).
    pub rows: Vec<VisibleRow>,
    /// The single highlighted tree node (`""` = site root) — the cursor. Exactly
    /// one row is highlighted, whether it's a page or a folder.
    pub cursor: String,
    /// The directory new pages/folders are added into (`""` = site root),
    /// derived from the cursor. Shown in the "Adding to: …" hint.
    pub add_target: String,
    /// The page slug open in the editor (full slug from the site root). Its tree
    /// row carries a ✎ marker (and a ● when it has unsaved changes), independent
    /// of the cursor highlight.
    pub selected_page: Option<String>,
    /// The **saved** title of the selected page (the title-field initial value).
    pub page_title: String,
    /// The **saved** body of the selected page (textarea initial / preview source).
    pub page_body: String,
    /// Is the live-preview pane shown? (Off = focus mode, textarea only.)
    pub show_preview: bool,
}

/// The whole window's render input.
pub struct SiteEditorOutput {
    /// Sites I own on this peer (sorted), each with its render-health indicator.
    pub sites: Vec<SiteListItem>,
    /// Is the "Your sites" region expanded?
    pub sites_open: bool,
    /// Is the "New site" create card expanded?
    pub create_open: bool,
    /// The active editing context, if a site is selected.
    pub selected: Option<SelectedSite>,
    /// A transient status line from the last action.
    pub notice: Option<Notice>,
}
