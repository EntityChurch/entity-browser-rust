//! Knowledge Base model — long-lived data layer for the wiki window.
//!
//! Architecture:
//!
//! - The model is **long-lived** — owned by the window directly (no
//!   `RefCell` needed because all model methods take `&self` and the
//!   internal state is in `Arc<Mutex<Inner>>`).
//! - The model **subscribes** to relevant tree paths during
//!   `initialize`. The subscription is a real SDK subscription
//!   (`PeerContext::subscribe`), and the callback updates the model's
//!   cache when external changes arrive. From the consumer
//!   perspective there's no difference between this and a per-path
//!   subscription routed through the subscription extension.
//! - Action methods take `&Peers` when they need to do tree
//!   I/O. The model is the only place that knows how to do this for
//!   the wiki window's data.
//! - Pure read (`render_output`) takes no peers parameter. It's what
//!   the renderer calls. Just a quick lock on the inner mutex to
//!   build the output snapshot.
//!
//! No web-sys, no DOM imports. The model is renderer-
//! independent.
//!
//! **D9 lifecycle (per the memory-accounting audit, §2.D):**
//! KB articles at `app/entity-browser/kb/{slug}` are
//! **user-content surfaces**, not app-tier infrastructure state. The
//! user is the GC — articles are created, edited, and deleted via UI
//! actions. No app-side eviction policy exists or is intended; that is
//! correct semantics for a wiki-shaped surface, not a missing
//! requirement. The seed-side `ingest::ingest_embedded_docs` path is
//! separately bounded (fingerprint-gated re-seed; tracked prev-keys
//! removed before re-seed) and is gated on the opt-in `KB_DOCS_ROOT`
//! build env (default 0 embedded docs).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use entity_entity::Entity;

use super::output::{
    ArticleDetail, ArticleListItem, DraftInitial, KbTreeRow, KnowledgeBaseOutput, ViewMode,
};
use crate::peers::{ChangeOp, Peers};
use crate::views::entity_tree::tree::{
    collect_expanded, flatten_visible, insert_or_update, remove as tree_remove,
    restore_expanded, toggle_expanded, TreeNode,
};
use crate::window::WindowId;

/// New tree nodes are created collapsed (depth < 0 is never true), so
/// the docs browser opens showing just the top-level repo folders —
/// the only sane entry point for a corpus this large. The user drills
/// in; opened folders are persisted in `expanded_paths`.
const AUTO_EXPAND_BELOW: i32 = 0;

/// Entity type name for knowledge base content. Matches the Go team.
pub const ARTICLE_TYPE: &str = "knowledge/article";

/// Tree path prefix where articles live, relative to a peer's namespace.
const ARTICLES_SUBPATH: &str = "knowledge/articles";

/// Persisted window state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnowledgeBaseState {
    pub view_mode: ViewMode,
    pub current_slug: Option<String>,
    /// Full paths of expanded directory nodes in the docs tree.
    /// Persisted so opened folders survive reloads. Mirrors the
    /// `entity_tree` window-state idiom.
    pub expanded_paths: Vec<String>,
}

impl KnowledgeBaseState {
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };

        let mut state = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("view_mode") => {
                    if let Some(s) = v.as_text() {
                        state.view_mode = parse_view_mode(s);
                    }
                }
                Some("current_slug") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            state.current_slug = Some(s.to_string());
                        }
                    }
                }
                Some("expanded_paths") => {
                    if let Some(arr) = v.as_array() {
                        state.expanded_paths = arr
                            .iter()
                            .filter_map(|el| el.as_text().map(String::from))
                            .collect();
                    }
                }
                _ => {}
            }
        }
        state
    }

    pub fn to_entity(&self) -> Entity {
        let mut pairs: Vec<(entity_ecf::Value, entity_ecf::Value)> = vec![
            (
                entity_ecf::Value::Text("view_mode".into()),
                entity_ecf::text(view_mode_str(self.view_mode)),
            ),
            (
                entity_ecf::Value::Text("current_slug".into()),
                entity_ecf::text(self.current_slug.clone().unwrap_or_default()),
            ),
        ];
        if !self.expanded_paths.is_empty() {
            let arr: Vec<entity_ecf::Value> =
                self.expanded_paths.iter().map(entity_ecf::text).collect();
            pairs.push((
                entity_ecf::Value::Text("expanded_paths".into()),
                entity_ecf::Value::Array(arr),
            ));
        }
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        Entity::new("app/state/knowledge_base", data).unwrap()
    }
}

fn view_mode_str(mode: ViewMode) -> &'static str {
    match mode {
        ViewMode::List => "list",
        ViewMode::Reader => "reader",
        ViewMode::Editor => "editor",
        ViewMode::New => "new",
    }
}

fn parse_view_mode(s: &str) -> ViewMode {
    match s {
        "reader" => ViewMode::Reader,
        "editor" => ViewMode::Editor,
        "new" => ViewMode::New,
        _ => ViewMode::List,
    }
}

/// Generate a URL slug from a free-form title.
pub fn slug_from_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut last_was_hyphen = true;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            out.push('-');
            last_was_hyphen = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

pub fn decode_article(entity: &Entity) -> (String, String) {
    let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
        Ok(v) => v,
        Err(_) => return (String::new(), String::new()),
    };
    let map = match value.as_map() {
        Some(m) => m,
        None => return (String::new(), String::new()),
    };
    let mut title = String::new();
    let mut content = String::new();
    for (k, v) in map {
        match k.as_text() {
            Some("title") => {
                if let Some(s) = v.as_text() {
                    title = s.to_string();
                }
            }
            Some("content") => {
                if let Some(s) = v.as_text() {
                    content = s.to_string();
                }
            }
            _ => {}
        }
    }
    (title, content)
}

pub fn encode_article(title: &str, content: &str) -> Entity {
    let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
        "title" => entity_ecf::text(title),
        "content" => entity_ecf::text(content)
    });
    Entity::new(ARTICLE_TYPE, data).unwrap()
}

pub fn article_path(peer_id: &str, slug: &str) -> String {
    format!("/{}/{}/{}", peer_id, ARTICLES_SUBPATH, slug)
}

pub fn articles_prefix(peer_id: &str) -> String {
    format!("/{}/{}/", peer_id, ARTICLES_SUBPATH)
}

pub fn slug_from_path(peer_id: &str, full_path: &str) -> Option<String> {
    let prefix = articles_prefix(peer_id);
    full_path.strip_prefix(&prefix).map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// The model
// ---------------------------------------------------------------------------

/// Internal mutable state. Holds the persisted window state plus a
/// local mirror of article entities maintained by the per-event
/// subscription installed in `apply_change`.
///
/// Stage C: the prior implementation read the full
/// article list + each article's title via `tree_listing` + per-slug
/// `get_entity` on every render. The local mirror replaces that —
/// `known_slugs` tracks which articles exist, `cached` holds decoded
/// entities. The subscription invalidates `cached[slug]` on Put so
/// the next render refetches just that one slug.
#[derive(Debug)]
pub struct ModelInner {
    state: KnowledgeBaseState,
    /// All known article slugs from the subscription stream.
    known_slugs: HashSet<String>,
    /// Decoded article entities, keyed by slug. Missing entries are
    /// lazily filled in `render_output` via `get_entity`.
    cached: HashMap<String, Entity>,
    /// Collapsible directory tree built from `known_slugs` (article
    /// keys are relative paths). Reuses the `entity_tree::tree`
    /// primitive. Expand state lives on the nodes and is mirrored to
    /// `state.expanded_paths` on toggle for persistence.
    root: TreeNode,
    /// Resync requested after a `ChangeOp::Resync` (Worker overflow).
    /// `render_output` clears the mirror and rebuilds from
    /// `tree_listing` when set.
    needs_resync: bool,
}

impl Default for ModelInner {
    fn default() -> Self {
        Self {
            state: KnowledgeBaseState::default(),
            known_slugs: HashSet::new(),
            cached: HashMap::new(),
            root: TreeNode::new_root(),
            needs_resync: false,
        }
    }
}

/// Long-lived data model for one Knowledge Base window instance.
///
/// Holds only the window's persisted state (view mode, current slug).
/// Article list and content come from the peer tree on demand via
/// `render_output(peers)`. The watch on `articles_prefix` (installed
/// by the factory) marks the window dirty when changes arrive,
/// triggering a re-render that reads fresh from the tree.
pub struct KnowledgeBaseModel {
    window_id: WindowId,
    peer_id: String,
    inner: Arc<Mutex<ModelInner>>,
}

impl std::fmt::Debug for KnowledgeBaseModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KnowledgeBaseModel")
            .field("window_id", &self.window_id)
            .field("peer_id", &self.peer_id)
            .field("inner", &self.inner)
            .finish()
    }
}

impl KnowledgeBaseModel {
    /// Create an empty model. Call `initialize(peers)` once after
    /// construction to populate the cache and start the subscription.
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(ModelInner::default())),
        }
    }

    /// Initialize the window's persisted state. Writes a default state
    /// entity to the tree on first open; otherwise hydrates the
    /// in-memory state from the persisted entity so view mode +
    /// current slug survive page reload. The article-cache mirror
    /// fills lazily — first via the subscription's seed Created events,
    /// and on render via on-demand `get_entity` for slugs not yet
    /// cached.
    pub fn initialize(&mut self, peers: &Peers) {
        self.ensure_state_in_tree(peers);
        let state = self.read_window_state(peers);
        self.inner.lock().unwrap().state = state;
    }

    /// Borrow the inner state behind the model's `Arc<Mutex>`. The
    /// window factory hands this to the `observe_with_events`
    /// callback so per-event apply_change calls land here.
    pub fn inner_arc(&self) -> Arc<Mutex<ModelInner>> {
        self.inner.clone()
    }

    fn ensure_state_in_tree(&self, peers: &Peers) {
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &self.peer_id, self.window_id);
        if peers.get_entity(&self.peer_id, &path).is_none() {
            peers.dispatch_write(&self.peer_id, path, KnowledgeBaseState::default().to_entity());
        }
    }

    fn read_window_state(&self, peers: &Peers) -> KnowledgeBaseState {
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &self.peer_id, self.window_id);
        peers
            .get_entity(&self.peer_id, &path)
            .map(|e| KnowledgeBaseState::from_entity(&e))
            .unwrap_or_default()
    }

    /// Refill any cache entries that are missing for known slugs.
    /// Costs O(missing) `get_entity` calls per render — typically 0
    /// after the subscription seed populates the cache; non-zero only
    /// when a recent `Put` event invalidated a slug or when a new
    /// article appeared without us having decoded it yet.
    fn refill_cache(&self, peers: &Peers) {
        let to_fill: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            inner
                .known_slugs
                .iter()
                .filter(|s| !inner.cached.contains_key(s.as_str()))
                .cloned()
                .collect()
        };
        for slug in to_fill {
            let path = article_path(&self.peer_id, &slug);
            if let Some(entity) = peers.get_entity(&self.peer_id, &path) {
                self.inner.lock().unwrap().cached.insert(slug, entity);
            }
        }
    }

    /// Resync path: wipe local mirror and rebuild from `tree_listing`
    /// plus per-slug `get_entity`. Costs O(N) on a peer with N
    /// articles — same as the pre-Stage-C behavior, but happens only
    /// on `ChangeOp::Resync` (worker overflow recovery), not per
    /// render.
    fn resync_from_tree(&self, peers: &Peers) {
        let prefix = articles_prefix(&self.peer_id);
        let entries = peers.tree_listing(&self.peer_id, &prefix);

        // Worker arm only: an overflow-Resync at reload can fire
        // before the worker cache mirror is primed → `tree_listing`
        // returns [] transiently. Wiping + clearing the flag then
        // would render the KB empty *forever* (the flag never
        // re-arms) — the post-reload KB-empty bug. So on the Worker
        // arm an empty read means "not primed yet": keep the existing
        // mirror and leave `needs_resync` set to retry next render.
        // The Direct arm's tree_listing is authoritative (empty ==
        // empty, e.g. everything was deleted) so it always rebuilds —
        // and this guard is compiled out on native entirely, so the
        // delete-to-empty unit tests keep their behaviour.
        #[cfg(target_arch = "wasm32")]
        if entries.is_empty() && peers.primary_as_direct().is_none() {
            return;
        }

        let mut inner = self.inner.lock().unwrap();
        inner.known_slugs.clear();
        inner.cached.clear();
        inner.root = TreeNode::new_root();
        for entry in entries {
            if let Some(slug) = slug_from_path(&self.peer_id, &entry.path) {
                insert_or_update(&mut inner.root, &slug, AUTO_EXPAND_BELOW);
                inner.known_slugs.insert(slug);
            }
        }
        let expanded: HashSet<String> = inner.state.expanded_paths.iter().cloned().collect();
        restore_expanded(&mut inner.root, &expanded);
        inner.needs_resync = false;
    }

    fn read_article_list_from_cache(&self) -> Vec<ArticleListItem> {
        let inner = self.inner.lock().unwrap();
        let mut items: Vec<ArticleListItem> = inner
            .known_slugs
            .iter()
            .map(|slug| {
                let display_title = inner
                    .cached
                    .get(slug)
                    .map(|e| {
                        let (title, _) = decode_article(e);
                        if title.is_empty() {
                            slug.clone()
                        } else {
                            title
                        }
                    })
                    .unwrap_or_else(|| slug.clone());
                ArticleListItem {
                    slug: slug.clone(),
                    display_title,
                }
            })
            .collect();
        items.sort_by(|a, b| a.slug.cmp(&b.slug));
        items
    }

    fn read_article_from_cache(&self, slug: &str) -> Option<ArticleDetail> {
        let inner = self.inner.lock().unwrap();
        let entity = inner.cached.get(slug)?;
        let (title, content) = decode_article(entity);
        Some(ArticleDetail {
            slug: slug.to_string(),
            title: if title.is_empty() {
                slug.to_string()
            } else {
                title
            },
            content,
        })
    }

    fn persist_state(&self, peers: &Peers) {
        let inner = self.inner.lock().unwrap();
        let entity = inner.state.to_entity();
        drop(inner);
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &self.peer_id, self.window_id);
        peers.dispatch_write(&self.peer_id, path, entity);
    }

    // -- Action methods (&self — interior mutability via the Mutex) --

    /// Show the article list (clearing current selection).
    pub fn show_list(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.state.view_mode = ViewMode::List;
        inner.state.current_slug = None;
    }

    /// Open an existing article in reader mode. The render reads its
    /// content fresh from the tree.
    pub fn select_article(&self, slug: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.state.view_mode = ViewMode::Reader;
        inner.state.current_slug = Some(slug);
    }

    /// Enter editor mode for the currently selected article.
    pub fn enter_editor(&self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.state.current_slug.is_some() {
            inner.state.view_mode = ViewMode::Editor;
        }
    }

    /// Enter new-article mode (blank form).
    pub fn enter_new(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.state.view_mode = ViewMode::New;
        inner.state.current_slug = None;
    }

    /// Discard any in-progress edit and return to the appropriate view.
    /// Editor → Reader, New → List. Pure transition — drafts live in
    /// the DOM, not the model.
    pub fn cancel(&self) {
        let mut inner = self.inner.lock().unwrap();
        match inner.state.view_mode {
            ViewMode::Editor => {
                inner.state.view_mode = ViewMode::Reader;
            }
            ViewMode::New => {
                inner.state.view_mode = ViewMode::List;
                inner.state.current_slug = None;
            }
            _ => {}
        }
    }

    /// Save an article via the legacy fire-and-forget path. Used by
    /// tests. The runtime controller uses `prepare_save` +
    /// `commit_view_after_save` + `peers.put_and_wait` instead to
    /// avoid the §3.2 read-your-own-write race; this method exists so
    /// the test surface that pre-dates that refactor keeps working.
    #[cfg(test)]
    pub fn save_article(
        &self,
        title: String,
        content: String,
        peers: &Peers,
    ) -> Result<String, String> {
        let (target_slug, path, entity) = self.prepare_save(title, content)?;
        peers.dispatch_write(&self.peer_id, path, entity);
        self.commit_view_after_save(target_slug.clone());
        Ok(target_slug)
    }

    /// Sync prep step for a save. Validates the title, computes the
    /// target slug from current in-memory state, and encodes the
    /// article entity. Returns `(target_slug, article_path,
    /// article_entity)` ready for the controller to dispatch via
    /// `peers.put_and_wait`. Does **not** mutate state — call
    /// `commit_view_after_save` once the write is in flight.
    pub fn prepare_save(
        &self,
        title: String,
        content: String,
    ) -> Result<(String, String, Entity), String> {
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err("Title cannot be empty".into());
        }

        let target_slug = {
            let inner = self.inner.lock().unwrap();
            match inner.state.view_mode {
                ViewMode::New => {
                    let s = slug_from_title(&title);
                    if s.is_empty() {
                        return Err(
                            "Title must contain at least one alphanumeric character".into(),
                        );
                    }
                    s
                }
                ViewMode::Editor => match &inner.state.current_slug {
                    Some(s) => s.clone(),
                    None => return Err("No article selected to edit".into()),
                },
                _ => return Err("Not in editable mode".into()),
            }
        };

        let path = article_path(&self.peer_id, &target_slug);
        let entity = encode_article(&title, &content);
        Ok((target_slug, path, entity))
    }

    /// Mutate in-memory state to reflect a completed save: switch to
    /// Reader and bind `current_slug`. Call after `prepare_save`
    /// returns Ok and the article write has been dispatched (or is
    /// in flight via `put_and_wait`).
    pub fn commit_view_after_save(&self, target_slug: String) {
        let mut inner = self.inner.lock().unwrap();
        inner.state.view_mode = ViewMode::Reader;
        inner.state.current_slug = Some(target_slug);
    }

    /// Encode the current in-memory window state for persistence.
    /// Caller is responsible for actually writing it (typically via
    /// `peers.put_and_wait` or `peers.dispatch_write` against
    /// `window_state_path(...)`). Used by the KB save flow to chain
    /// the state write after the article-cache reflects, avoiding the
    /// §3.2 read-your-own-write race.
    pub fn window_state_entity(&self) -> Entity {
        self.inner.lock().unwrap().state.to_entity()
    }

    /// Delete the currently selected article. Returns true if a slug
    /// was selected and the delete was dispatched. The actual removal
    /// is fire-and-forget; the watch will fire and re-render once the
    /// Change event lands.
    pub fn delete_current(&self, peers: &Peers) -> bool {
        let slug = match self.inner.lock().unwrap().state.current_slug.clone() {
            Some(s) => s,
            None => return false,
        };
        let path = article_path(&self.peer_id, &slug);
        peers.dispatch_remove(&self.peer_id, path);

        let mut inner = self.inner.lock().unwrap();
        inner.state.view_mode = ViewMode::List;
        inner.state.current_slug = None;

        true
    }

    /// Persist the current state to the tree. Called by the controller
    /// after action methods that mutate state.
    pub fn save_state(&self, peers: &Peers) {
        self.persist_state(peers);
    }

    // -- Pure read API (called by renderer) --

    /// Build the renderer-neutral output. Reads from the local
    /// article-entity cache populated by the per-event subscription.
    ///
    /// Stage C: get_entity calls per render dropped from
    /// O(article-count) (one per slug for the list view + one for
    /// the current article) to O(missing-from-cache) — typically 0
    /// after the subscription seed completes. Resync fallback path
    /// runs only after `ChangeOp::Resync` (Worker overflow).
    #[allow(dead_code)] // called from WASM render path and tests
    pub fn render_output(&self, peers: &Peers) -> KnowledgeBaseOutput {
        // Resync fallback. Rare — fires after a Worker EventChannel
        // overflow forced ChangeOp::Resync.
        let needs_resync = self.inner.lock().unwrap().needs_resync;
        if needs_resync {
            self.resync_from_tree(peers);
        }

        // Lazy fill any slugs whose cache entry is missing or stale.
        self.refill_cache(peers);

        let (view_mode, current_slug) = {
            let inner = self.inner.lock().unwrap();
            (inner.state.view_mode, inner.state.current_slug.clone())
        };

        let articles = self.read_article_list_from_cache();
        let tree_rows = self.read_tree_rows();
        let current = current_slug
            .as_ref()
            .and_then(|slug| self.read_article_from_cache(slug));
        let draft_initial = compute_draft_initial(view_mode, &current);

        let peer_label = crate::views::display_name(peers, &self.peer_id);
        KnowledgeBaseOutput {
            view_mode,
            articles,
            tree_rows,
            current,
            draft_initial,
            peer_label,
        }
    }

    /// Flatten the docs tree into renderer rows (List view).
    fn read_tree_rows(&self) -> Vec<KbTreeRow> {
        let inner = self.inner.lock().unwrap();
        flatten_visible(&inner.root)
            .into_iter()
            .map(|r| KbTreeRow {
                path: r.path,
                segment: r.segment,
                depth: r.depth,
                has_children: r.has_children,
                expanded: r.expanded,
                has_entry: r.has_entry,
                leaf_count: r.leaf_count,
            })
            .collect()
    }

    /// Toggle a directory node's expand state and mirror the new
    /// expanded set into persisted window state. The controller then
    /// calls `save_state`, whose tree write fires the window-state
    /// watch and re-renders.
    pub fn toggle_expand(&self, path: &str) {
        let mut inner = self.inner.lock().unwrap();
        if toggle_expanded(&mut inner.root, path) {
            inner.state.expanded_paths =
                collect_expanded(&inner.root).into_iter().collect();
        }
    }

    // -- Test helpers --

    #[cfg(test)]
    pub fn state_snapshot(&self) -> KnowledgeBaseState {
        self.inner.lock().unwrap().state.clone()
    }

    #[cfg(test)]
    pub fn articles_snapshot(&self, peers: &Peers) -> Vec<ArticleListItem> {
        // Most KB unit tests don't install the per-event subscription
        // (factory wiring is integration-level). Pretend the
        // subscription just resynced: pull the current set of slugs
        // from `tree_listing` and refill the cache.
        self.resync_from_tree(peers);
        self.refill_cache(peers);
        self.read_article_list_from_cache()
    }

    #[cfg(test)]
    pub fn current_article_snapshot(&self, peers: &Peers) -> Option<ArticleDetail> {
        let slug = self.inner.lock().unwrap().state.current_slug.clone()?;
        self.resync_from_tree(peers);
        self.refill_cache(peers);
        self.read_article_from_cache(&slug)
    }
}

/// Apply one [`ChangeOp`] to the inner state. Called from the
/// per-event subscription callback set up by the window factory.
///
/// On `Put`: register the slug as known and invalidate the cached
/// entity (next render refetches). We don't decode here — the
/// subscription callback is `Send + Sync` and runs without `&Peers`
/// in scope, so the get_entity refresh has to wait until render.
///
/// On `Remove`: drop the slug from both the known set and the cache.
///
/// On `Resync`: flag for full rebuild via `tree_listing` at the next
/// render. Used when the Worker arm's event channel overflows.
pub fn apply_change(
    inner: &Mutex<ModelInner>,
    peer_id: &str,
    op: ChangeOp,
) {
    let mut guard = inner.lock().unwrap();
    match op {
        ChangeOp::Put { path } => {
            if let Some(slug) = slug_from_path(peer_id, &path) {
                insert_or_update(&mut guard.root, &slug, AUTO_EXPAND_BELOW);
                // Keep folders the user previously opened expanded even
                // when a later Put recreates intermediate nodes.
                let expanded: HashSet<String> =
                    guard.state.expanded_paths.iter().cloned().collect();
                restore_expanded(&mut guard.root, &expanded);
                guard.known_slugs.insert(slug.clone());
                guard.cached.remove(&slug);
            }
        }
        ChangeOp::Remove { path } => {
            if let Some(slug) = slug_from_path(peer_id, &path) {
                tree_remove(&mut guard.root, &slug);
                guard.known_slugs.remove(&slug);
                guard.cached.remove(&slug);
            }
        }
        ChangeOp::Resync => {
            guard.needs_resync = true;
        }
    }
}

fn compute_draft_initial(
    view_mode: ViewMode,
    current: &Option<ArticleDetail>,
) -> Option<DraftInitial> {
    match view_mode {
        ViewMode::Editor => {
            let detail = current.as_ref()?;
            Some(DraftInitial {
                is_new: false,
                initial_title: detail.title.clone(),
                initial_content: detail.content.clone(),
                editing_slug: Some(detail.slug.clone()),
            })
        }
        ViewMode::New => Some(DraftInitial {
            is_new: true,
            initial_title: String::new(),
            initial_content: String::new(),
            editing_slug: None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm() -> Peers {
        Peers::new_direct()
    }

    fn fresh_model(peers: &Peers) -> KnowledgeBaseModel {
        let pid = peers.primary_peer_id().to_string();
        let mut model = KnowledgeBaseModel::new(1, pid.clone());
        model.initialize(peers);
        // Stage C: install the per-event subscription
        // that the production factory wires up. Tests that go through
        // `render_output(peers)` directly need this so writes via
        // `save_article` propagate into the local mirror.
        // Watch is leaked since tests don't drop it explicitly — the
        // model + peers outlive it.
        let inner = model.inner_arc();
        let peer_id_for_cb = pid.clone();
        let mut watch = crate::window_watch::WindowWatch::new();
        peers.observe_with_events(
            &mut watch,
            &pid,
            articles_prefix(&pid),
            move |op| apply_change(&inner, &peer_id_for_cb, op),
        );
        std::mem::forget(watch);
        model
    }

    /// Flush pending spawned tasks. `dispatch_write` fires-and-forgets
    /// through the peer's execute pipeline; a brief sleep lets the full
    /// chain complete before the assertion reads the tree. Also gives
    /// the per-event subscription installed by `fresh_model` time to
    /// deliver Created events.
    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // -- Pure helpers --

    #[test]
    fn slug_from_title_basic() {
        assert_eq!(slug_from_title("Hello World"), "hello-world");
        assert_eq!(slug_from_title("My First Article"), "my-first-article");
        assert_eq!(slug_from_title("  Trim  Spaces  "), "trim-spaces");
        assert_eq!(slug_from_title("UPPER CASE"), "upper-case");
    }

    #[test]
    fn slug_from_title_punctuation() {
        assert_eq!(slug_from_title("Hello, World!"), "hello-world");
        assert_eq!(slug_from_title("Type/Subtype"), "type-subtype");
        assert_eq!(slug_from_title("foo--bar___baz"), "foo-bar-baz");
    }

    #[test]
    fn slug_from_title_empty_or_punct_only() {
        assert_eq!(slug_from_title(""), "");
        assert_eq!(slug_from_title("   "), "");
        assert_eq!(slug_from_title("!!!"), "");
    }

    #[test]
    fn article_path_format() {
        assert_eq!(
            article_path("peerXY", "hello-world"),
            "/peerXY/knowledge/articles/hello-world"
        );
    }

    #[test]
    fn articles_prefix_format() {
        assert_eq!(articles_prefix("peerXY"), "/peerXY/knowledge/articles/");
    }

    #[test]
    fn slug_from_path_strips_prefix() {
        assert_eq!(
            slug_from_path("peerXY", "/peerXY/knowledge/articles/foo"),
            Some("foo".into())
        );
    }

    #[test]
    fn article_round_trip() {
        let e = encode_article("Hello", "Body content");
        let (title, content) = decode_article(&e);
        assert_eq!(title, "Hello");
        assert_eq!(content, "Body content");
    }

    // -- State (de)serialization --

    #[test]
    fn state_default_is_list_mode() {
        let s = KnowledgeBaseState::default();
        assert_eq!(s.view_mode, ViewMode::List);
        assert!(s.current_slug.is_none());
    }

    #[test]
    fn state_round_trip_through_entity() {
        let s = KnowledgeBaseState {
            view_mode: ViewMode::Editor,
            current_slug: Some("hello".into()),
            expanded_paths: vec!["entity-core-architecture".into(), "entity-core-architecture/docs".into()],
        };
        let e = s.to_entity();
        let s2 = KnowledgeBaseState::from_entity(&e);
        assert_eq!(s2, s);
    }

    // -- Initialize / refresh / subscription --

    #[test]
    fn initialize_starts_in_list_mode_with_no_articles() {
        let pm = pm();
        let model = fresh_model(&pm);
        assert_eq!(model.state_snapshot().view_mode, ViewMode::List);
        assert!(model.articles_snapshot(&pm).is_empty());
        assert!(model.current_article_snapshot(&pm).is_none());
    }

    #[tokio::test]
    async fn initialize_seeds_state_in_tree_when_absent() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut model = KnowledgeBaseModel::new(42, pid.clone());
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 42);
        assert!(pm.get_entity(&pid, &path).is_none());

        model.initialize(&pm);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert!(pm.get_entity(&pid, &path).is_some());
    }

    #[test]
    fn initialize_does_not_overwrite_existing_state() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();

        let custom = KnowledgeBaseState {
            view_mode: ViewMode::New,
            current_slug: None,
            expanded_paths: Vec::new(),
        };
        let ctx = pm.test_seed_ctx(&pid);
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, ctx.peer_id(), 7);
        ctx.store().put(&path, custom.to_entity()).ok();

        let mut model = KnowledgeBaseModel::new(7, pid);
        model.initialize(&pm);

        assert_eq!(model.state_snapshot().view_mode, ViewMode::New);
    }

    // -- Action methods --

    #[tokio::test]
    async fn save_creates_article_transitions_to_reader_and_updates_cache() {
        let pm = pm();
        let model = fresh_model(&pm);

        model.enter_new();
        let result = model.save_article("First Article".into(), "Body text".into(), &pm);
        assert_eq!(result, Ok("first-article".into()));
        flush_writes().await;

        assert_eq!(model.state_snapshot().view_mode, ViewMode::Reader);
        assert_eq!(model.state_snapshot().current_slug, Some("first-article".into()));

        let detail = model.current_article_snapshot(&pm).expect("current cached");
        assert_eq!(detail.title, "First Article");
        assert_eq!(detail.content, "Body text");

        let articles = model.articles_snapshot(&pm);
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].slug, "first-article");
    }

    #[test]
    fn save_with_empty_title_errors() {
        let pm = pm();
        let model = fresh_model(&pm);
        model.enter_new();
        assert!(model.save_article("   ".into(), "body".into(), &pm).is_err());
        assert_eq!(model.state_snapshot().view_mode, ViewMode::New);
    }

    #[test]
    fn save_with_punctuation_only_title_errors() {
        let pm = pm();
        let model = fresh_model(&pm);
        model.enter_new();
        assert!(model.save_article("!!!".into(), "body".into(), &pm).is_err());
    }

    #[tokio::test]
    async fn edit_existing_article_updates_in_place() {
        let pm = pm();
        let model = fresh_model(&pm);

        model.enter_new();
        let _ = model.save_article("Test".into(), "original".into(), &pm);
        flush_writes().await;

        model.enter_editor();
        assert_eq!(model.state_snapshot().view_mode, ViewMode::Editor);

        let _ = model.save_article("Test".into(), "updated".into(), &pm);
        flush_writes().await;

        assert_eq!(model.state_snapshot().view_mode, ViewMode::Reader);
        let detail = model.current_article_snapshot(&pm).unwrap();
        assert_eq!(detail.content, "updated");
        assert_eq!(model.articles_snapshot(&pm).len(), 1);
    }

    #[test]
    fn select_article_loads_current_into_cache() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();

        let ctx = pm.test_seed_ctx(&pid);
        ctx.store().put(&article_path(&pid, "preexisting"), encode_article("Pre", "existing body"))
            .ok();

        let model = fresh_model(&pm);
        model.select_article("preexisting".into());

        assert_eq!(model.state_snapshot().view_mode, ViewMode::Reader);
        assert_eq!(model.state_snapshot().current_slug, Some("preexisting".into()));
        let detail = model.current_article_snapshot(&pm).unwrap();
        assert_eq!(detail.title, "Pre");
        assert_eq!(detail.content, "existing body");
    }

    #[tokio::test]
    async fn delete_removes_article_and_returns_to_list() {
        let pm = pm();
        let model = fresh_model(&pm);

        model.enter_new();
        let _ = model.save_article("Doomed".into(), "body".into(), &pm);
        flush_writes().await;

        let removed = model.delete_current(&pm);
        assert!(removed);
        assert_eq!(model.state_snapshot().view_mode, ViewMode::List);
        assert!(model.state_snapshot().current_slug.is_none());
        assert!(model.current_article_snapshot(&pm).is_none());
        assert!(model.articles_snapshot(&pm).is_empty());
    }

    #[test]
    fn cancel_editor_returns_to_reader() {
        let pm = pm();
        let model = fresh_model(&pm);

        model.enter_new();
        let _ = model.save_article("Test".into(), "body".into(), &pm);
        model.enter_editor();
        model.cancel();

        assert_eq!(model.state_snapshot().view_mode, ViewMode::Reader);
        assert_eq!(model.state_snapshot().current_slug, Some("test".into()));
    }

    #[test]
    fn cancel_new_returns_to_list_without_creating() {
        let pm = pm();
        let model = fresh_model(&pm);

        model.enter_new();
        model.cancel();

        assert_eq!(model.state_snapshot().view_mode, ViewMode::List);
        assert!(model.state_snapshot().current_slug.is_none());
        assert!(model.articles_snapshot(&pm).is_empty());
    }

    #[tokio::test]
    async fn show_list_clears_selection() {
        let pm = pm();
        let model = fresh_model(&pm);

        model.enter_new();
        let _ = model.save_article("Test".into(), "body".into(), &pm);
        flush_writes().await;
        model.show_list();

        assert_eq!(model.state_snapshot().view_mode, ViewMode::List);
        assert!(model.state_snapshot().current_slug.is_none());
        assert!(model.current_article_snapshot(&pm).is_none());
        assert_eq!(model.articles_snapshot(&pm).len(), 1);
    }

    #[tokio::test]
    async fn save_state_persists_to_tree() {
        let pm = pm();
        let model = fresh_model(&pm);
        model.enter_new();
        model.save_state(&pm);
        flush_writes().await;

        let pid = pm.primary_peer_id();
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, pid, model.window_id);
        let entity = pm.get_entity(pid, &path).unwrap();
        let loaded = KnowledgeBaseState::from_entity(&entity);
        assert_eq!(loaded.view_mode, ViewMode::New);
    }

    // -- Pure render output --

    #[tokio::test]
    async fn render_output_in_list_mode() {
        let pm = pm();
        let model = fresh_model(&pm);
        model.enter_new();
        let _ = model.save_article("First".into(), "body".into(), &pm);
        flush_writes().await;
        model.show_list();

        let out = model.render_output(&pm);
        assert_eq!(out.view_mode, ViewMode::List);
        assert_eq!(out.articles.len(), 1);
        assert!(out.current.is_none());
        assert!(out.draft_initial.is_none());
    }

    #[tokio::test]
    async fn render_output_in_editor_mode_seeds_draft_from_current() {
        let pm = pm();
        let model = fresh_model(&pm);
        model.enter_new();
        let _ = model.save_article("Test".into(), "body".into(), &pm);
        flush_writes().await;
        model.enter_editor();

        let out = model.render_output(&pm);
        assert_eq!(out.view_mode, ViewMode::Editor);
        let draft = out.draft_initial.expect("draft initial in editor mode");
        assert!(!draft.is_new);
        assert_eq!(draft.initial_title, "Test");
        assert_eq!(draft.initial_content, "body");
        assert_eq!(draft.editing_slug, Some("test".into()));
    }

    #[test]
    fn render_output_in_new_mode_has_empty_draft() {
        let pm = pm();
        let model = fresh_model(&pm);
        model.enter_new();

        let out = model.render_output(&pm);
        assert_eq!(out.view_mode, ViewMode::New);
        assert!(out.current.is_none());
        let draft = out.draft_initial.expect("draft initial in new mode");
        assert!(draft.is_new);
        assert_eq!(draft.initial_title, "");
        assert_eq!(draft.initial_content, "");
        assert!(draft.editing_slug.is_none());
    }

    // -- L1 subscription end-to-end --

    /// Writing an article from OUTSIDE the model (bypassing
    /// `save_article`'s local-cache update) should still land in the
    /// articles list via the L1 subscription callback.
    ///
    /// This proves the full L1 pipeline works: `dispatch_write` →
    /// tree change → subscription engine → delivery dispatch via
    /// `system/inbox` → SDK delivery handler → model callback.
    #[tokio::test]
    async fn l1_subscription_delivers_external_write_to_cache() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut model = KnowledgeBaseModel::new(42, pid.clone());
        model.initialize(&pm);
        // Let the subscribe dispatch settle before writing.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert!(model.articles_snapshot(&pm).is_empty(), "starts empty");

        // Write an article directly to the tree (bypassing save_article,
        // which would also update the local cache). Only the
        // subscription callback can populate the cache.
        let ctx = pm.test_seed_ctx(&pid);
        ctx.store()
            .put(
                &article_path(&pid, "external"),
                encode_article("External", "written outside model"),
            )
            .unwrap();

        // Let the subscription chain run.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let articles = model.articles_snapshot(&pm);
        assert_eq!(articles.len(), 1, "subscription should have populated cache");
        assert_eq!(articles[0].slug, "external");
        assert_eq!(articles[0].display_title, "External");
    }
}
