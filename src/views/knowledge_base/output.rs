//! Renderer-neutral output types for the Knowledge Base window.
//!
//! These structs describe what to show, not how to show it. The model
//! layer (`model.rs`) produces these from peer state; renderers
//! (`crate::dom::knowledge_base`) consume them to build DOM (or, in
//! the future, terminal output, etc.).
//!
//! No peer access, no rendering logic, no event handling — just data.

// The output types are constructed by the model layer and consumed by
// the WASM DOM renderer. On native builds without the WASM render path,
// the structs and their fields appear unused; this is expected.
#![allow(dead_code)]

/// Top-level output for one render pass of the Knowledge Base window.
#[derive(Debug, Clone)]
pub struct KnowledgeBaseOutput {
    /// Which sub-view is active.
    pub view_mode: ViewMode,
    /// Flat article list (sorted by key). Used for the count line and
    /// the empty-state check. The List view renders `tree_rows`, not
    /// this — but a flat list stays cheap and other modes/tests use it.
    pub articles: Vec<ArticleListItem>,
    /// Collapsible directory tree for the List view, mirroring the
    /// docs' on-disk layout. Built from the article keys (relative
    /// paths) via the shared `entity_tree::tree` primitive. Folder
    /// rows toggle expand; leaf rows (`has_entry`) open the reader.
    pub tree_rows: Vec<KbTreeRow>,
    /// The currently displayed article (Reader and Editor modes).
    /// None in List and New modes.
    pub current: Option<ArticleDetail>,
    /// Initial values to populate input/textarea elements with when
    /// entering Editor or New mode. The DOM elements own the live
    /// draft state after creation; this is just the seed.
    /// None in List and Reader modes.
    pub draft_initial: Option<DraftInitial>,
    /// Friendly identifier of the peer this window is bound to.
    /// Used in the empty-state messaging so users know which peer
    /// they're looking at.
    pub peer_label: String,
}

/// Which sub-view the Knowledge Base window is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ViewMode {
    /// Browsing the article list.
    #[default]
    List,
    /// Reading a specific article.
    Reader,
    /// Editing an existing article.
    Editor,
    /// Composing a new article.
    New,
}

/// One row in the article list view.
#[derive(Debug, Clone)]
pub struct ArticleListItem {
    /// URL slug — unique within this peer's knowledge base.
    pub slug: String,
    /// Display title (falls back to slug if the entity has no title).
    pub display_title: String,
}

/// One row of the collapsible docs tree (List view). Shape mirrors
/// `entity_tree::tree::VisibleRow` — produced by mapping that
/// primitive's flatten output.
#[derive(Debug, Clone)]
pub struct KbTreeRow {
    /// Full article key (== slug) for leaves; folder path for folders.
    pub path: String,
    /// Display label — the path segment (file or directory name).
    pub segment: String,
    /// Indentation level (0 = top-level repo dir).
    pub depth: usize,
    /// True for directory nodes (togglable).
    pub has_children: bool,
    /// Current expand state (directories only).
    pub expanded: bool,
    /// True when this node is an article leaf (clickable → reader).
    pub has_entry: bool,
    /// `Some(n)` on collapsed directories — count of articles beneath.
    pub leaf_count: Option<usize>,
}

/// Detail of one article — used in Reader and Editor modes.
#[derive(Debug, Clone)]
pub struct ArticleDetail {
    pub slug: String,
    pub title: String,
    pub content: String,
}

/// Initial values for an Editor or New form. The renderer puts these
/// into the input/textarea elements when constructing the form.
/// After that, the DOM elements are the source of truth for the live
/// draft — the model never tracks per-keystroke state.
#[derive(Debug, Clone)]
pub struct DraftInitial {
    /// True for New mode, false for Editor mode.
    pub is_new: bool,
    /// Initial title to populate the title input with.
    pub initial_title: String,
    /// Initial content to populate the content textarea with.
    pub initial_content: String,
    /// The slug being edited (for Editor mode). None for New mode.
    pub editing_slug: Option<String>,
}
