//! Entity Tree model — local-mirror state for the tree browser window.
//!
//! Shape ported from `entity-workbench-go/workbench/tree_model.go`
//! (Stage A). The model holds a local mirror of the
//! peer's tree + expand state + the cached selected entity. Render
//! reads from this mirror only — zero `get_entity` / `tree_listing`
//! calls in the render path on steady-state.
//!
//! **Update mechanism (Stage A.1, landed):** a per-event subscription
//! (`observe_with_events`) drives [`apply_change`], which mutates the
//! mirror O(depth) per `Put`/`Remove` without losing expand state.
//! The original Stage-A "diff `tree_listing` against `known` on every
//! dirty tick" loop is **superseded** — `refresh_mirror` survives
//! only as the [`resync`] fallback for Worker `Lagged` overflow
//! recovery, not the steady-state path.
//!
//! No web-sys, no DOM imports.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use entity_entity::Entity;
use entity_hash::Hash;

use crate::peers::{ChangeOp, Peers};
use crate::selection::Selection;
use crate::selection_source::{selection_contract, SelectionSource};
use crate::window::WindowId;

use super::output::{
    DocumentBody, DocumentView, EntityTreeOutput, InspectorView, TreeFooter, TreeRow,
};
use super::tree::{
    collect_expanded, expand_ancestors, flatten_visible, insert_or_update,
    remove as tree_remove, restore_expanded, TreeNode, VisibleRow,
};

/// Auto-expand depth applied to newly-inserted intermediate nodes
/// (via [`insert_or_update`]). New nodes with `depth <
/// AUTO_EXPAND_BELOW` are created already-expanded; deeper new nodes
/// default to collapsed.
///
/// `1` means only the peer-root node (depth 0) opens; its top-level
/// groups (`app`, `system`, `content`, …, depth 1) are visible but
/// collapsed, and everything beneath stays closed until the user drills
/// in. This is the conventional file-tree default and the one the user
/// wants — a fresh tree opens collapsed.
///
/// It was `8` (expand everything shallower than the deepest app paths),
/// which made a fresh peer open *fully* expanded and, because the same
/// rule runs on every incremental entity write, re-popped the tree open
/// whenever a new deep path was written. Keeping this low also stops
/// background writes from re-expanding the tree, since the shallow nodes
/// they'd touch already exist after first load.
const AUTO_EXPAND_BELOW: i32 = 1;

/// Persisted window state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EntityTreeState {
    pub current_path: Option<String>,
    pub search: String,
    pub expanded_paths: Vec<String>,
    /// Selection-source wire form (`none` / `app` / `panel:{id}`).
    /// Absent in CBOR = `none` (manual).
    pub selection_source: String,
}

impl EntityTreeState {
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
                Some("current_path") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            state.current_path = Some(s.to_string());
                        }
                    }
                }
                Some("search") => {
                    if let Some(s) = v.as_text() {
                        state.search = s.to_string();
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
                Some("selection_source") => {
                    if let Some(s) = v.as_text() {
                        state.selection_source = s.to_string();
                    }
                }
                _ => {}
            }
        }
        state
    }

    pub fn to_entity(&self) -> Entity {
        let mut pairs: Vec<(entity_ecf::Value, entity_ecf::Value)> = Vec::new();
        if let Some(ref path) = self.current_path {
            pairs.push((
                entity_ecf::Value::Text("current_path".into()),
                entity_ecf::text(path),
            ));
        }
        if !self.search.is_empty() {
            pairs.push((
                entity_ecf::Value::Text("search".into()),
                entity_ecf::text(&self.search),
            ));
        }
        if !self.expanded_paths.is_empty() {
            let arr: Vec<entity_ecf::Value> = self
                .expanded_paths
                .iter()
                .map(entity_ecf::text)
                .collect();
            pairs.push((
                entity_ecf::Value::Text("expanded_paths".into()),
                entity_ecf::Value::Array(arr),
            ));
        }
        // Omit when manual — absence = `none`, the default.
        if !self.selection_source.is_empty() && self.selection_source != "none" {
            pairs.push((
                entity_ecf::Value::Text("selection_source".into()),
                entity_ecf::text(&self.selection_source),
            ));
        }
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        Entity::new("app/state/entity_tree", data).unwrap()
    }
}

/// Local-mirror state. The `Mutex` lets the per-event subscription
/// callback mutate from a `Send + Sync` context. Public only because
/// the window factory needs to name the type when threading the
/// `Arc<Mutex<EntityTreeInner>>` into the `observe_with_events`
/// closure; fields stay private.
pub struct EntityTreeInner {
    /// Currently-selected path. Owned by the model — UI gestures
    /// (Navigate, NavigateUp) update it.
    current_path: Option<String>,

    /// Path → hash mirror. Maintained by `refresh_mirror`.
    known: HashMap<String, Hash>,

    /// Tree graph with expand state. Built incrementally from `known`;
    /// nodes carry `expanded: bool`.
    root: TreeNode,

    /// Currently-flattened visible rows. Rebuilt lazily in
    /// `render_output` when `visible_dirty` is set — so a 281-event
    /// seed phase produces one rebuild at first render, not 281
    /// during the event drain.
    visible_rows: Vec<VisibleRow>,
    visible_dirty: bool,

    /// Set by [`apply_change`] when it receives [`ChangeOp::Resync`]
    /// (Worker arm overflow recovery). On the next render the model
    /// blows away its local mirror, rebuilds from `tree_listing`, and
    /// re-applies persisted expand state.
    needs_resync: bool,

    /// Cached entity for `current_path`. Updated lazily in
    /// `render_output` when `selected_dirty` is true.
    selected_entity: Option<Entity>,
    selected_dirty: bool,

    /// Search filter — empty when not filtering. Stage A2; not yet
    /// applied in `filtered_rows` (TODO when search UI lands).
    search: String,

    /// Persisted expand-path set. Applied to the tree after the
    /// initial seed via `restore_expanded`. Held in case the seed
    /// completes asynchronously (Worker arm) and the user hasn't yet
    /// produced the rows.
    pending_expand_restore: Option<HashSet<String>>,

    /// Which slot this panel co-orients to. Persisted (wire form) in
    /// window state; default `None` (manual).
    selection_source: SelectionSource,

    /// `updated_at` of the last Selection this panel consumed. Makes
    /// `consume_from_source` idempotent across the re-renders the
    /// `/{peer_id}/` subscription already triggers, and stops it from
    /// re-applying a selection it already acted on.
    last_consumed_at: u64,
}

impl EntityTreeInner {
    fn new() -> Self {
        Self {
            current_path: None,
            known: HashMap::new(),
            root: TreeNode::new_root(),
            visible_rows: Vec::new(),
            visible_dirty: true,
            needs_resync: false,
            selected_entity: None,
            selected_dirty: false,
            search: String::new(),
            pending_expand_restore: None,
            selection_source: SelectionSource::None,
            last_consumed_at: 0,
        }
    }
}

/// Long-lived data model for one Entity Tree window instance.
pub struct EntityTreeModel {
    window_id: WindowId,
    peer_id: String,
    inner: Arc<Mutex<EntityTreeInner>>,
}

impl std::fmt::Debug for EntityTreeModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntityTreeModel")
            .field("window_id", &self.window_id)
            .field("peer_id", &self.peer_id)
            .finish()
    }
}

impl EntityTreeModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(EntityTreeInner::new())),
        }
    }

    /// One-time initial load + state seeding. Called by the window
    /// factory after `new`. Reads persisted window state and primes
    /// the pending-expand-restore so [`apply_change`] applies it on
    /// the first delivered subscription event.
    pub fn initialize(&mut self, peers: &Peers) {
        self.ensure_state_in_tree(peers);
        let persisted = self.read_window_state(peers);
        let mut inner = self.inner.lock().unwrap();
        inner.current_path = persisted.current_path;
        inner.search = persisted.search;
        // Empty set means "no overrides; use defaults" — the per-event
        // `insert_or_update` path auto-expands nodes below
        // `AUTO_EXPAND_BELOW` as they arrive.
        inner.pending_expand_restore =
            Some(persisted.expanded_paths.into_iter().collect());
        inner.selection_source = SelectionSource::parse(&persisted.selection_source);
        if inner.current_path.is_some() {
            inner.selected_dirty = true;
        }
    }

    /// Borrow the inner state behind the model's `Arc<Mutex>`. The
    /// window factory hands this to the `observe_with_events` callback
    /// so per-event apply_change calls land here without traversing
    /// the controller.
    pub fn inner_arc(&self) -> Arc<Mutex<EntityTreeInner>> {
        self.inner.clone()
    }

    /// Snapshot the current selected path. Used by the controller's
    /// Stage B selection-slot publishing so it doesn't have to lock
    /// the model itself.
    pub fn current_path(&self) -> Option<String> {
        self.inner.lock().unwrap().current_path.clone()
    }

    fn ensure_state_in_tree(&self, peers: &Peers) {
        let path = crate::app_paths::window_state_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            self.window_id,
        );
        if peers.get_entity(&self.peer_id, &path).is_none() {
            peers.dispatch_write(&self.peer_id, path, EntityTreeState::default().to_entity());
        }
    }

    fn read_window_state(&self, peers: &Peers) -> EntityTreeState {
        let path = crate::app_paths::window_state_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            self.window_id,
        );
        peers
            .get_entity(&self.peer_id, &path)
            .map(|e| EntityTreeState::from_entity(&e))
            .unwrap_or_default()
    }

    fn persist_state(&self, peers: &Peers) {
        let entity = {
            let inner = self.inner.lock().unwrap();
            let expanded_paths: Vec<String> =
                collect_expanded(&inner.root).into_iter().collect();
            EntityTreeState {
                current_path: inner.current_path.clone(),
                search: inner.search.clone(),
                expanded_paths,
                selection_source: inner.selection_source.to_wire(),
            }
            .to_entity()
        };
        let path = crate::app_paths::window_state_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            self.window_id,
        );
        peers.dispatch_write(&self.peer_id, path, entity);
    }

    // -- Action methods --

    pub fn navigate(&self, path: &str) {
        let mut inner = self.inner.lock().unwrap();
        let changed = inner.current_path.as_deref() != Some(path);
        inner.current_path = Some(path.to_string());
        if changed {
            inner.selected_dirty = true;
            // Expand ancestors so the row becomes visible.
            expand_ancestors(&mut inner.root, path);
            rebuild_visible(&mut inner);
        }
    }

    pub fn navigate_up(&self) {
        let mut inner = self.inner.lock().unwrap();
        let new_path = inner.current_path.as_ref().and_then(|p| {
            p.rfind('/').and_then(|pos| {
                let parent = &p[..pos];
                if parent.is_empty() {
                    None
                } else {
                    Some(parent.to_string())
                }
            })
        });
        let changed = inner.current_path != new_path;
        inner.current_path = new_path;
        if changed {
            inner.selected_dirty = true;
        }
    }

    /// Toggle the expand state of the node at `path`. No-op if the
    /// path isn't in the tree or has no children.
    pub fn toggle_expand(&self, path: &str) {
        let mut inner = self.inner.lock().unwrap();
        if toggle_expanded(&mut inner.root, path) {
            rebuild_visible(&mut inner);
        }
    }

    /// Update search filter. (Stage A2: filter rendering itself not
    /// yet implemented — for now the filter is stored but
    /// `visible_rows` ignores it.)
    pub fn set_search(&self, query: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.search = query.to_string();
        rebuild_visible(&mut inner);
    }

    /// Set the selection source from the dropdown's wire value.
    /// Resets the consume guard so the new source's current selection
    /// is picked up on the next render rather than being suppressed
    /// by a stale `last_consumed_at`.
    pub fn set_selection_source(&self, wire: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.selection_source = SelectionSource::parse(wire);
        inner.last_consumed_at = 0;
    }

    /// Co-orient to the configured source slot, if any. Reads the
    /// source's `Selection`, and applies it **only if**: the panel
    /// has a non-manual source, the selector type is one Entity Tree
    /// consumes, the pointee peer is local, and the selection is
    /// newer than the last one consumed. Calls the model's own
    /// `navigate` — which does **not** publish — so a consumer never
    /// re-emits and two mutually-following panels can't loop
    /// (design §5).
    fn consume_from_source(&self, peers: &Peers) {
        let source = {
            let inner = self.inner.lock().unwrap();
            inner.selection_source
        };
        let slot_path = match source {
            SelectionSource::None => return,
            SelectionSource::AppAggregate => {
                crate::app_paths::app_selection_path(crate::app_paths::APP_ID, &self.peer_id)
            }
            // v1 dropdown never sets this (design §4.1); reading the
            // specific panel slot is the same once a registry exists.
            SelectionSource::Panel(id) => crate::app_paths::panel_selection_path(
                crate::app_paths::APP_ID,
                &self.peer_id,
                id,
            ),
        };
        let Some(entity) = peers.get_entity(&self.peer_id, &slot_path) else {
            return;
        };
        let sel = Selection::from_entity(&entity);
        // Run-time selector-type filter ("Lego slot").
        let ty = sel.type_.as_deref().unwrap_or("");
        if !selection_contract("Entity Tree").accepts(ty) {
            return;
        }
        // Peer-scope v1: only co-orient to selections whose pointee
        // is the local peer. `peer_id == None` means host peer.
        if let Some(ref pid) = sel.peer_id {
            if pid != &self.peer_id {
                return;
            }
        }
        if sel.path.is_empty() {
            return;
        }
        {
            let mut inner = self.inner.lock().unwrap();
            if sel.updated_at <= inner.last_consumed_at {
                return;
            }
            inner.last_consumed_at = sel.updated_at;
        }
        // Non-publishing navigate (controller publishes only on
        // user-initiated Action::Navigate; this path is not that).
        self.navigate(&sel.path);
    }

    /// Persist current state to the tree.
    pub fn save_state(&self, peers: &Peers) {
        self.persist_state(peers);
    }

    // -- Resync fallback (Worker arm overflow recovery) --

    /// Wipe local mirror state and rebuild from `peers.tree_listing`.
    /// Called from `render_output` when `needs_resync` was flagged by
    /// [`ChangeOp::Resync`]. Preserves the user's current expand
    /// state via a `collect_expanded` snapshot before the wipe.
    fn resync(&self, peers: &Peers) {
        let snapshot = {
            let inner = self.inner.lock().unwrap();
            collect_expanded(&inner.root)
        };
        {
            let mut inner = self.inner.lock().unwrap();
            inner.known.clear();
            inner.root = TreeNode::new_root();
            inner.pending_expand_restore = Some(snapshot);
            inner.needs_resync = false;
        }
        self.refresh_mirror(peers);
    }

    /// Refresh the local mirror from `peers.tree_listing`. Idempotent
    /// — diff'd against `inner.known` so unchanged paths are no-ops.
    ///
    /// Steady-state mirror maintenance is now per-event (see
    /// [`apply_change`]). This function only runs from [`resync`] —
    /// recovery from Worker `ChangeEvent::Lagged` overflow.
    fn refresh_mirror(&self, peers: &Peers) {
        #[cfg(feature = "measurement")]
        crate::frame_counters::bump_tree_listing();
        let entries = peers.tree_listing(&self.peer_id, "");
        let mut inner = self.inner.lock().unwrap();

        let mut new_paths: HashSet<String> = HashSet::with_capacity(entries.len());
        let mut any_change = false;

        for entry in entries {
            new_paths.insert(entry.path.clone());
            match inner.known.get(&entry.path) {
                Some(existing) if *existing == entry.hash => {
                    // Unchanged — skip.
                }
                _ => {
                    inner.known.insert(entry.path.clone(), entry.hash);
                    insert_or_update(&mut inner.root, &entry.path, AUTO_EXPAND_BELOW);
                    if inner.current_path.as_deref() == Some(entry.path.as_str()) {
                        inner.selected_dirty = true;
                    }
                    any_change = true;
                }
            }
        }

        // Removals — paths in known but not in the fresh listing.
        let removed: Vec<String> = inner
            .known
            .keys()
            .filter(|p| !new_paths.contains(p.as_str()))
            .cloned()
            .collect();
        for path in removed {
            inner.known.remove(&path);
            tree_remove(&mut inner.root, &path);
            if inner.current_path.as_deref() == Some(path.as_str()) {
                inner.selected_dirty = true;
                inner.selected_entity = None;
            }
            any_change = true;
        }

        // Apply any pending expand restore. `insert_or_update` above
        // already auto-expanded depth < AUTO_EXPAND_BELOW; this layer
        // re-applies persisted user expand state on top.
        if let Some(pending) = inner.pending_expand_restore.take() {
            if !pending.is_empty() {
                restore_expanded(&mut inner.root, &pending);
            }
            if let Some(ref p) = inner.current_path.clone() {
                expand_ancestors(&mut inner.root, p);
            }
            any_change = true;
        }

        if any_change {
            rebuild_visible(&mut inner);
        }
    }

    // -- Pure read API (called by renderer) --

    /// Materialize the full output for one render pass.
    ///
    /// Reads from the local mirror only. No `tree_listing` call on
    /// steady state — mirror is maintained by the per-event
    /// subscription via [`apply_change`]. Resync (Worker overflow
    /// recovery) is the one path that re-invokes `refresh_mirror`.
    pub fn render_output(&self, peers: &Peers) -> EntityTreeOutput {
        // Co-orient to the configured selection source first, so the
        // rest of this pass renders the followed path. No-op (and no
        // get_entity) when the source is manual — the common case.
        self.consume_from_source(peers);

        // Resync fallback. Rare — only fires when the Worker arm's
        // event channel overflowed and we got ChangeOp::Resync.
        let needs_resync = {
            let inner = self.inner.lock().unwrap();
            inner.needs_resync
        };
        if needs_resync {
            self.resync(peers);
        }

        // Lazy rebuild of visible_rows. Per-event apply_change marks
        // visible_dirty without flattening; we flatten once here per
        // dirty render.
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.visible_dirty {
                rebuild_visible(&mut inner);
                inner.visible_dirty = false;
            }
        }

        // Selected-entity lazy refresh. Costs 0 or 1 get_entity call
        // per render.
        let (current_path, selected_entity_clone) = {
            let mut inner = self.inner.lock().unwrap();
            if inner.selected_dirty {
                inner.selected_entity = inner
                    .current_path
                    .as_deref()
                    .and_then(|p| peers.get_entity(&self.peer_id, p));
                inner.selected_dirty = false;
            }
            (inner.current_path.clone(), inner.selected_entity.clone())
        };

        let inner = self.inner.lock().unwrap();

        let rows: Vec<TreeRow> = inner
            .visible_rows
            .iter()
            .map(|v| TreeRow {
                path: v.path.clone(),
                segment: v.segment.clone(),
                depth: v.depth,
                has_children: v.has_children,
                expanded: v.expanded,
                has_entry: v.has_entry,
                leaf_count: v.leaf_count,
                is_selected: current_path.as_deref() == Some(v.path.as_str()),
            })
            .collect();

        let match_count = rows.len();

        let footer = TreeFooter {
            entity_count: peers.entity_count(&self.peer_id),
            path_count: peers.path_count(&self.peer_id),
        };

        let document = build_document_view(current_path.as_deref(), selected_entity_clone.as_ref());
        let inspector =
            build_inspector_view(current_path.as_deref(), selected_entity_clone.as_ref());

        EntityTreeOutput {
            peer_label: crate::views::display_name(peers, &self.peer_id),
            current_path,
            rows,
            footer,
            document,
            inspector,
            search: inner.search.clone(),
            match_count,
            selection_source: inner.selection_source.to_wire(),
        }
    }

    // -- Test helpers --

    #[cfg(test)]
    pub fn state_snapshot(&self) -> EntityTreeState {
        let inner = self.inner.lock().unwrap();
        EntityTreeState {
            current_path: inner.current_path.clone(),
            search: inner.search.clone(),
            expanded_paths: collect_expanded(&inner.root).into_iter().collect(),
            selection_source: inner.selection_source.to_wire(),
        }
    }
}

fn rebuild_visible(inner: &mut EntityTreeInner) {
    inner.visible_rows = flatten_visible(&inner.root);
}

/// Apply one [`ChangeOp`] to the inner state. Called from the
/// per-event subscription callback set up by the window factory.
///
/// Mutex-locked because the callback closure is `Send + Sync` and may
/// run on a background task (Direct: SDK subscription executor;
/// Worker: `spawn_local` event-drain task).
///
/// Cost: O(depth) per Put/Remove via [`insert_or_update`] /
/// [`tree_remove`]. Visible-row rebuild is deferred to the next
/// `render_output` via `visible_dirty`, so a 281-event seed phase
/// produces one rebuild at first render rather than 281 during the
/// drain.
pub fn apply_change(inner: &Mutex<EntityTreeInner>, op: ChangeOp) {
    let mut guard = inner.lock().unwrap();
    match op {
        ChangeOp::Put { path } => {
            insert_or_update(&mut guard.root, &path, AUTO_EXPAND_BELOW);
            if guard.current_path.as_deref() == Some(path.as_str()) {
                guard.selected_dirty = true;
            }
        }
        ChangeOp::Remove { path } => {
            tree_remove(&mut guard.root, &path);
            if guard.current_path.as_deref() == Some(path.as_str()) {
                guard.selected_dirty = true;
                guard.selected_entity = None;
            }
        }
        ChangeOp::Resync => {
            // Defer the actual rebuild to render_output, which has
            // `&Peers` in scope. Just flag here.
            guard.needs_resync = true;
            guard.visible_dirty = true;
            return;
        }
    }
    // Apply pending expand state once. After this fires, persisted
    // expand_paths are restored and the current selection's
    // ancestors are revealed.
    //
    // We DON'T bulk-expand by depth here — `insert_or_update` now
    // sets new intermediate nodes' `expanded = true` when their depth
    // is below `AUTO_EXPAND_BELOW`, so the per-event path
    // self-maintains the default expand state without re-walking the
    // whole tree each time.
    if let Some(pending) = guard.pending_expand_restore.take() {
        if !pending.is_empty() {
            restore_expanded(&mut guard.root, &pending);
        }
        if let Some(ref p) = guard.current_path.clone() {
            expand_ancestors(&mut guard.root, p);
        }
    }
    guard.visible_dirty = true;
}

/// Walk the tree to `path` and toggle that node's expand state.
/// Returns `true` when the toggle happened.
fn toggle_expanded(root: &mut TreeNode, path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return false;
    }
    toggle_expanded_recursive(root, &parts)
}

fn toggle_expanded_recursive(node: &mut TreeNode, parts: &[&str]) -> bool {
    let part = parts[0];
    let pos = match node
        .children
        .binary_search_by(|c| c.segment.as_str().cmp(part))
    {
        Ok(p) => p,
        Err(_) => return false,
    };
    let child = &mut node.children[pos];
    if parts.len() == 1 {
        if child.children.is_empty() {
            return false;
        }
        child.expanded = !child.expanded;
        return true;
    }
    toggle_expanded_recursive(child, &parts[1..])
}

fn build_document_view(current_path: Option<&str>, entity: Option<&Entity>) -> DocumentView {
    let Some(path) = current_path else {
        return DocumentView::Empty;
    };
    let Some(entity) = entity else {
        return DocumentView::NotFound { path: path.to_string() };
    };

    let body = match ciborium::from_reader::<ciborium::Value, _>(entity.data.as_slice()) {
        Ok(value) => {
            use entity_ecf::ValueExt;
            let text = value
                .as_str()
                .map(String::from)
                .or_else(|| value.get("content").and_then(|v| v.as_str()).map(String::from));
            match text {
                Some(t) => DocumentBody::Text(t),
                None => DocumentBody::Formatted(crate::format::format_entity_data(&entity.data)),
            }
        }
        Err(_) => DocumentBody::Formatted(crate::format::format_entity_data(&entity.data)),
    };

    DocumentView::Entity {
        path: path.to_string(),
        entity_type: entity.entity_type.clone(),
        body,
    }
}

fn build_inspector_view(current_path: Option<&str>, entity: Option<&Entity>) -> InspectorView {
    let Some(path) = current_path else {
        return InspectorView::Empty;
    };
    let Some(entity) = entity else {
        return InspectorView::NotFound { path: path.to_string() };
    };

    let algorithm_label = match entity.content_hash.algorithm {
        0x00 => "SHA-256".to_string(),
        n => format!("Unknown (0x{:02x})", n),
    };
    let fields = vec![
        ("Path".into(), path.to_string()),
        ("Type".into(), entity.entity_type.clone()),
        ("Hash".into(), entity.content_hash.to_string()),
        ("Data size".into(), format!("{} bytes", entity.data.len())),
        ("Algorithm".into(), algorithm_label),
    ];
    let raw_hash_hex: String = entity
        .content_hash
        .digest()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    InspectorView::Entity {
        path: path.to_string(),
        fields,
        raw_hash_hex,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity_ecf::{text, to_ecf};

    fn pm() -> Peers {
        Peers::new_direct()
    }

    fn pm_with_entity() -> (Peers, String, String) {
        let pm = Peers::new_direct();
        let pid = pm.primary_peer_id().to_string();
        let data = to_ecf(&text("hello"));
        let entity = Entity::new("test/type", data).unwrap();
        let path = format!("/{}/docs/arch/overview", pid);
        pm.put_entity(&pid, &path, entity);
        (pm, pid, path)
    }

    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[test]
    fn state_default_is_empty() {
        let s = EntityTreeState::default();
        assert!(s.current_path.is_none());
        assert!(s.search.is_empty());
        assert!(s.expanded_paths.is_empty());
    }

    #[test]
    fn state_round_trip_full() {
        let s = EntityTreeState {
            current_path: Some("/p/docs/arch/overview".into()),
            search: "type:note".into(),
            expanded_paths: vec!["/p".into(), "/p/docs".into()],
            selection_source: "app".into(),
        };
        let e = s.to_entity();
        let s2 = EntityTreeState::from_entity(&e);
        assert_eq!(s2.current_path, s.current_path);
        assert_eq!(s2.search, s.search);
        assert_eq!(s2.selection_source, "app");
        // expanded_paths set comparison.
        let a: HashSet<&str> = s.expanded_paths.iter().map(String::as_str).collect();
        let b: HashSet<&str> = s2.expanded_paths.iter().map(String::as_str).collect();
        assert_eq!(a, b);
    }

    #[test]
    fn state_round_trip_no_path() {
        let s = EntityTreeState::default();
        let e = s.to_entity();
        let s2 = EntityTreeState::from_entity(&e);
        assert_eq!(s2, s);
    }

    #[test]
    fn navigate_sets_path() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut model = EntityTreeModel::new(1, pid);
        model.initialize(&pm);
        model.navigate("/p/docs/arch/overview");
        assert_eq!(
            model.state_snapshot().current_path.as_deref(),
            Some("/p/docs/arch/overview")
        );
    }

    #[test]
    fn navigate_up_walks_one_level() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut model = EntityTreeModel::new(1, pid);
        model.initialize(&pm);
        model.navigate("/p/docs/arch/overview");
        model.navigate_up();
        assert_eq!(
            model.state_snapshot().current_path.as_deref(),
            Some("/p/docs/arch")
        );
    }

    #[test]
    fn navigate_up_from_top_clears() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut model = EntityTreeModel::new(1, pid);
        model.initialize(&pm);
        model.navigate("/docs");
        model.navigate_up();
        assert!(model.state_snapshot().current_path.is_none());
    }

    #[tokio::test]
    async fn initialize_seeds_state_in_tree_when_absent() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 42);
        assert!(pm.get_entity(&pid, &path).is_none());

        let mut model = EntityTreeModel::new(42, pid.clone());
        model.initialize(&pm);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        assert!(pm.get_entity(&pid, &path).is_some());
    }

    #[tokio::test]
    async fn save_state_persists_current_path_and_expanded() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        // Pre-populate some paths so the tree has structure.
        let entity = Entity::new("test/type", to_ecf(&text("x"))).unwrap();
        pm.put_entity(&pid, &format!("/{}/a/b", pid), entity.clone());
        pm.put_entity(&pid, &format!("/{}/a/c", pid), entity);

        let mut model = EntityTreeModel::new(1, pid.clone());
        model.initialize(&pm);
        subscribe_and_seed(&model, &pm).await;

        model.navigate(&format!("/{}/a/b", pid));
        model.save_state(&pm);
        flush_writes().await;

        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 1);
        let entity = pm.get_entity(&pid, &path).unwrap();
        let loaded = EntityTreeState::from_entity(&entity);
        assert_eq!(loaded.current_path.as_deref(), Some(format!("/{}/a/b", pid).as_str()));
    }

    /// Drive the model the same way the window factory does: spawn
    /// the per-event subscription and wait long enough for the
    /// snapshot Created events to fan out. After `await`-ing this,
    /// the tree mirror is populated and `render_output` reads
    /// consistent state.
    async fn subscribe_and_seed(model: &EntityTreeModel, peers: &Peers) {
        let inner = model.inner_arc();
        let mut watch = crate::window_watch::WindowWatch::new();
        peers.observe_with_events(
            &mut watch,
            &model.peer_id,
            format!("/{}/", model.peer_id),
            move |op| apply_change(&inner, op),
        );
        // Subscription callback runs on a tokio task; give it a tick
        // to drain the seed events.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        // Hold the watch (subscription stays alive while the test
        // borrows the returned watch).
        std::mem::forget(watch);
    }

    #[tokio::test]
    async fn render_output_resolves_current_entity_for_document_and_inspector() {
        let (pm, pid, path) = pm_with_entity();
        let mut model = EntityTreeModel::new(1, pid.clone());
        model.initialize(&pm);
        subscribe_and_seed(&model, &pm).await;
        model.navigate(&path);

        let out = model.render_output(&pm);

        match &out.document {
            DocumentView::Entity { entity_type, body, .. } => {
                assert_eq!(entity_type, "test/type");
                match body {
                    DocumentBody::Text(t) => assert_eq!(t, "hello"),
                    other => panic!("expected Text body, got {:?}", other),
                }
            }
            other => panic!("expected Entity document, got {:?}", other),
        }

        match &out.inspector {
            InspectorView::Entity { fields, .. } => {
                let by_label: std::collections::HashMap<&str, &str> =
                    fields.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                assert_eq!(by_label.get("Type").copied(), Some("test/type"));
            }
            other => panic!("expected Entity inspector, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn render_output_with_no_selection_is_empty_document_and_inspector() {
        let (pm, pid, _) = pm_with_entity();
        let mut model = EntityTreeModel::new(1, pid);
        model.initialize(&pm);
        subscribe_and_seed(&model, &pm).await;

        let out = model.render_output(&pm);
        assert!(matches!(out.document, DocumentView::Empty));
        assert!(matches!(out.inspector, InspectorView::Empty));
        assert!(out.current_path.is_none());
    }

    #[tokio::test]
    async fn render_output_builds_flat_rows() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let entity = Entity::new("test/type", to_ecf(&text("x"))).unwrap();
        pm.put_entity(&pid, &format!("/{}/a/b", pid), entity.clone());
        pm.put_entity(&pid, &format!("/{}/a/c", pid), entity);

        let mut model = EntityTreeModel::new(1, pid.clone());
        model.initialize(&pm);
        subscribe_and_seed(&model, &pm).await;

        let out = model.render_output(&pm);
        // Top-level row: /{pid}. Default-expanded to depth 8, so its
        // immediate children should also be visible.
        let peer_path = format!("/{}", pid);
        let row = out.rows.iter().find(|r| r.path == peer_path).expect("peer row");
        assert_eq!(row.depth, 0);
        assert!(row.has_children);
        assert!(row.expanded);
    }

    #[tokio::test]
    async fn toggle_expand_flips_node_state() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let entity = Entity::new("test/type", to_ecf(&text("x"))).unwrap();
        pm.put_entity(&pid, &format!("/{}/a/b", pid), entity.clone());
        pm.put_entity(&pid, &format!("/{}/a/c", pid), entity);

        let mut model = EntityTreeModel::new(1, pid.clone());
        model.initialize(&pm);
        subscribe_and_seed(&model, &pm).await;

        let peer_path = format!("/{}", pid);
        model.toggle_expand(&peer_path);

        let out = model.render_output(&pm);
        let row = out.rows.iter().find(|r| r.path == peer_path).expect("peer row");
        assert!(!row.expanded);
        // Collapsed group should show a leaf count.
        assert!(row.leaf_count.is_some());
    }

    #[tokio::test]
    async fn navigate_expands_ancestors() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let entity = Entity::new("test/type", to_ecf(&text("x"))).unwrap();
        let deep = format!("/{}/a/b/c/d", pid);
        pm.put_entity(&pid, &deep, entity);

        let mut model = EntityTreeModel::new(1, pid.clone());
        model.initialize(&pm);
        subscribe_and_seed(&model, &pm).await;

        model.navigate(&deep);
        let out = model.render_output(&pm);
        // Every ancestor row should be present (visible) — confirms
        // expand_ancestors fired on navigate.
        for path in [
            format!("/{}", pid),
            format!("/{}/a", pid),
            format!("/{}/a/b", pid),
            format!("/{}/a/b/c", pid),
            format!("/{}/a/b/c/d", pid),
        ] {
            assert!(
                out.rows.iter().any(|r| r.path == path),
                "expected row at {}",
                path
            );
        }
    }

    // -- Panel selection-source (consume side) --

    /// Consuming a selection from the app-aggregate slot co-orients
    /// the panel but must NOT re-publish — otherwise two mutually
    /// following panels loop (design §5). Asserts: B follows the
    /// slot, the slot is byte-for-byte unchanged, and B never wrote
    /// its own per-panel slot.
    #[tokio::test]
    async fn consume_from_app_slot_does_not_republish() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let target = format!("/{}/docs/followed", pid);

        // Producer "A" (window 1) publishes to the app-aggregate slot.
        let app_path = crate::app_paths::app_selection_path(crate::app_paths::APP_ID, &pid);
        let original = Selection::entity(target.clone(), &pid);
        pm.put_entity(&pid, &app_path, original.to_entity());

        // Consumer "B" (window 2) follows the app aggregate.
        let mut b = EntityTreeModel::new(2, pid.clone());
        b.initialize(&pm);
        b.set_selection_source("app");

        let out = b.render_output(&pm);
        assert_eq!(
            out.current_path.as_deref(),
            Some(target.as_str()),
            "consumer should co-orient to the slot's path"
        );

        // Loop guard 1: B did not overwrite the app slot.
        let after = Selection::from_entity(&pm.get_entity(&pid, &app_path).unwrap());
        assert_eq!(after.path, original.path);
        assert_eq!(
            after.updated_at, original.updated_at,
            "consumer must not re-write the app-aggregate slot"
        );
        // Loop guard 2: B never published its own per-panel slot.
        let b_panel =
            crate::app_paths::panel_selection_path(crate::app_paths::APP_ID, &pid, 2);
        assert!(
            pm.get_entity(&pid, &b_panel).is_none(),
            "consumer must not publish a per-panel selection on consume"
        );
    }

    /// The "Lego slot" run-time filter: a selector type Entity Tree
    /// does not consume is ignored; the `updated_at` guard makes
    /// consumption idempotent and only advances on a newer selection.
    #[tokio::test]
    async fn consume_respects_type_filter_and_updated_at_guard() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let app_path = crate::app_paths::app_selection_path(crate::app_paths::APP_ID, &pid);

        let mut b = EntityTreeModel::new(2, pid.clone());
        b.initialize(&pm);
        b.set_selection_source("app");

        // Wrong selector type — must be ignored.
        let wrong = Selection {
            path: format!("/{}/q/result", pid),
            type_: Some("query-result".into()),
            peer_id: None,
            updated_at: 1_000,
        };
        pm.put_entity(&pid, &app_path, wrong.to_entity());
        assert_eq!(
            b.render_output(&pm).current_path,
            None,
            "non-consumed selector type must not co-orient"
        );

        // Correct type at t=2000 — consumed.
        let p1 = format!("/{}/docs/one", pid);
        pm.put_entity(
            &pid,
            &app_path,
            Selection {
                path: p1.clone(),
                type_: Some("entity".into()),
                peer_id: None,
                updated_at: 2_000,
            }
            .to_entity(),
        );
        assert_eq!(b.render_output(&pm).current_path.as_deref(), Some(p1.as_str()));

        // A staler selection (t=1500) must NOT clobber the newer one.
        pm.put_entity(
            &pid,
            &app_path,
            Selection {
                path: format!("/{}/docs/stale", pid),
                type_: Some("entity".into()),
                peer_id: None,
                updated_at: 1_500,
            }
            .to_entity(),
        );
        assert_eq!(
            b.render_output(&pm).current_path.as_deref(),
            Some(p1.as_str()),
            "older selection must be suppressed by the updated_at guard"
        );
    }
}
