//! Renderer-neutral output types for the Entity Tree window.
//!
//! These structs describe what to show, not how to show it. The model
//! (`model.rs`) materializes these from its local mirror; renderers
//! (`crate::dom::entity_tree`) consume them to build DOM (or, in the
//! future, terminal output, etc.).
//!
//! No peer access, no rendering logic, no event handling — just data.
//!
//! Stage A: flat `Vec<TreeRow>` replaces the prior nested
//! `Vec<TreeNode>` to match workbench-go's `TreeBrowserOutput`. Easier
//! for renderers (DOM, immediate-mode, terminal) — indentation comes
//! from `depth` and toggle glyphs come from `expanded`/`has_children`.

#![allow(dead_code)]

/// Top-level output for one render pass of the Entity Tree window.
#[derive(Debug, Clone)]
pub struct EntityTreeOutput {
    /// Friendly identifier of the peer this window is bound to.
    pub peer_label: String,
    /// Currently selected path in the tree, if any.
    pub current_path: Option<String>,
    /// Flat list of visible rows. Collapsed groups contribute one row;
    /// their descendants are absent. Indentation is via `depth`.
    pub rows: Vec<TreeRow>,
    /// Footer counts.
    pub footer: TreeFooter,
    /// Resolved document for the current path.
    pub document: DocumentView,
    /// Resolved inspector data for the current path.
    pub inspector: InspectorView,
    /// Current search filter (empty when not filtering).
    pub search: String,
    /// Number of rows matching the current filter (or total when not
    /// filtering).
    pub match_count: usize,
    /// Selection-source wire form (`none` / `app` / `panel:{id}`) —
    /// drives the "Selection source" `<select>`'s marked option.
    pub selection_source: String,
}

/// One row in the rendered tree. Self-contained: a renderer can draw
/// a row from this struct alone without consulting the tree graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// Full path (qualified, with leading slash) — click target.
    pub path: String,
    /// Display segment (last path component).
    pub segment: String,
    /// Indent level. Multiply by the renderer's indent unit.
    pub depth: usize,
    /// True when this row groups children below it (deeper rows or
    /// hidden when collapsed).
    pub has_children: bool,
    /// True when the group is currently expanded. Drives the
    /// toggle glyph (`▼` vs `▶`).
    pub expanded: bool,
    /// True when this path itself binds an entity (vs intermediate
    /// folder). A row can be `has_children && has_entry` — a group
    /// that's also a binding.
    pub has_entry: bool,
    /// Leaf-count hint for collapsed groups (`Some(n)`); `None` on
    /// expanded groups or leaves.
    pub leaf_count: Option<usize>,
    /// True when this row is the currently-selected path.
    pub is_selected: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeFooter {
    pub entity_count: usize,
    pub path_count: usize,
}

/// Document panel view — what the renderer should show in the main
/// reading area.
#[derive(Debug, Clone)]
pub enum DocumentView {
    /// No path selected.
    Empty,
    /// Path was selected but no entity is bound there.
    NotFound { path: String },
    /// Entity resolved.
    Entity {
        path: String,
        entity_type: String,
        body: DocumentBody,
    },
}

/// Body content for an Entity-shaped document.
#[derive(Debug, Clone)]
pub enum DocumentBody {
    /// Plain text (the entity's data was a CBOR string, or had a
    /// top-level "content" field that decoded to a string).
    Text(String),
    /// Pre-formatted dump (CBOR map or other structured data, already
    /// stringified by `format::format_entity_data`).
    Formatted(String),
}

/// Inspector panel view — entity metadata and raw hash.
#[derive(Debug, Clone)]
pub enum InspectorView {
    Empty,
    NotFound {
        path: String,
    },
    Entity {
        path: String,
        fields: Vec<(String, String)>,
        raw_hash_hex: String,
    },
}
