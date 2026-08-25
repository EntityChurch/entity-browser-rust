//! Lazy, remote-backed filesystem browse cache for the File Transfer window.
//!
//! The share exposed at `entity://{target}/local/files/shared/` is a real
//! filesystem, browsed one directory at a time via remote `list` ops. This
//! cache holds the fetched listings + the ephemeral tree state (which folders
//! are open, which file is selected) so the window can render a hierarchical
//! tree — reusing the shared [`crate::views::entity_tree::tree`] machinery
//! (the same `TreeNode`/`VisibleRow` the Entity Tree and Site Creator use),
//! just with a remote FS data source instead of the local entity tree.
//!
//! Cloneable (`Arc<Mutex<_>>`) so an async `list` task can hold a clone and
//! write results back after it resolves, then flip the window's [`DirtyFlag`]
//! (mirrors the [`crate::event_log_cache`] pattern). State is deliberately
//! **ephemeral** — a transient view of remote FS state, not persisted (no tree
//! pollution); it rebuilds on demand and resets when the target changes.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::views::entity_tree::tree::{
    flatten_visible, insert_folder, insert_or_update, restore_expanded, TreeNode,
};

use super::model::SHARE_PREFIX;
use super::output::FileRow;

/// One entry in a directory listing (a file or a sub-directory).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsChild {
    /// Display name (the leaf segment).
    pub name: String,
    /// Path relative to the share root — the tree key (`""` = root, `dir/leaf`
    /// deeper). What `expanded`/`selected` are keyed by and the tree is built
    /// from, so the tree roots at the share, not at `local/files/shared`.
    pub relpath: String,
    /// Full tree path (`local/files/shared/…`) — the `resource` for a `read`
    /// (pull) or a deeper `list`.
    pub full_path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Default, Debug)]
struct Inner {
    /// The target these listings belong to; a change resets the cache.
    target: String,
    /// Open directory relpaths (`""` = root).
    expanded: HashSet<String>,
    /// Directory relpaths with an in-flight `list`.
    loading: HashSet<String>,
    /// Directory relpaths whose `list` has returned (may be empty dirs).
    listed: HashSet<String>,
    /// Every known entry, keyed by relpath.
    nodes: BTreeMap<String, FsChild>,
    /// Selected file relpath (for the Pull button).
    selected: Option<String>,
    /// Last browse error (surfaced loudly; D13).
    error: Option<String>,
}

/// Cheap-to-clone handle to the browse state.
#[derive(Clone, Default, Debug)]
pub struct FsBrowseCache {
    inner: Arc<Mutex<Inner>>,
}

impl FsBrowseCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop all cached state if `target` differs from what the cache holds
    /// (a target switch invalidates every listing). Returns `true` if it reset.
    pub fn sync_target(&self, target: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.target == target {
            return false;
        }
        *inner = Inner { target: target.to_string(), ..Inner::default() };
        true
    }

    /// Toggle a directory open/closed. Returns the new expanded state.
    pub fn toggle_expand(&self, relpath: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.expanded.contains(relpath) {
            inner.expanded.remove(relpath);
            false
        } else {
            inner.expanded.insert(relpath.to_string());
            true
        }
    }

    /// Force `relpath` open (used when (re)loading the root).
    pub fn set_expanded(&self, relpath: &str) {
        self.inner.lock().unwrap().expanded.insert(relpath.to_string());
    }

    /// Reserve a load for `relpath`. Returns `true` if the caller should fetch
    /// (not already listed, not already in flight); marks it loading. `force`
    /// re-fetches even a previously-listed directory (the Refresh path).
    pub fn begin_load(&self, relpath: &str, force: bool) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.loading.contains(relpath) {
            return false;
        }
        if !force && inner.listed.contains(relpath) {
            return false;
        }
        if force {
            inner.listed.remove(relpath);
        }
        inner.loading.insert(relpath.to_string());
        inner.error = None;
        true
    }

    /// Record a directory's children (marks it listed, clears its loading flag).
    pub fn apply_listing(&self, relpath: &str, children: Vec<FsChild>) {
        let mut inner = self.inner.lock().unwrap();
        inner.loading.remove(relpath);
        inner.listed.insert(relpath.to_string());
        for child in children {
            inner.nodes.insert(child.relpath.clone(), child);
        }
    }

    /// Record a load failure (clears loading, surfaces the error).
    pub fn fail_load(&self, relpath: &str, err: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.loading.remove(relpath);
        inner.error = Some(err.to_string());
    }

    /// Select a file by relpath.
    pub fn select(&self, relpath: &str) {
        self.inner.lock().unwrap().selected = Some(relpath.to_string());
    }

    /// The selected file's full tree path (for a Pull), if a file is selected.
    pub fn selected_full_path(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        let sel = inner.selected.as_ref()?;
        inner.nodes.get(sel).filter(|n| !n.is_dir).map(|n| n.full_path.clone())
    }

    /// Has the root been listed yet? (Drives "Browse" button vs. the tree.)
    pub fn root_listed(&self) -> bool {
        self.inner.lock().unwrap().listed.contains("")
    }

    /// Is the root currently loading?
    pub fn root_loading(&self) -> bool {
        self.inner.lock().unwrap().loading.contains("")
    }

    pub fn error(&self) -> Option<String> {
        self.inner.lock().unwrap().error.clone()
    }

    /// Build the flattened tree rows for rendering. Reuses the shared
    /// `TreeNode`/`flatten_visible` machinery: folders via `insert_folder`,
    /// files via `insert_or_update`, expand state re-applied from `expanded`.
    /// A directory that is expanded but whose listing hasn't arrived is marked
    /// `loading`.
    pub fn rows(&self) -> Vec<FileRow> {
        let inner = self.inner.lock().unwrap();
        let mut root = TreeNode::new_root();
        for (relpath, child) in &inner.nodes {
            if child.is_dir {
                insert_folder(&mut root, relpath, 0);
            } else {
                insert_or_update(&mut root, relpath, 0);
            }
        }
        restore_expanded(&mut root, &inner.expanded);

        flatten_visible(&root)
            .into_iter()
            .map(|r| {
                let child = inner.nodes.get(&r.path);
                // A row that binds no entry is a directory (folders come from
                // `insert_folder`, which leaves `has_entry = false`).
                let is_dir = child.map(|c| c.is_dir).unwrap_or(!r.has_entry);
                let expanded = inner.expanded.contains(&r.path);
                FileRow {
                    name: r.segment,
                    path: r.path.clone(),
                    depth: r.depth,
                    is_dir,
                    expanded,
                    // An expanded dir whose listing hasn't come back yet.
                    loading: is_dir && expanded && inner.loading.contains(&r.path),
                    selected: inner.selected.as_deref() == Some(r.path.as_str()),
                    size: child.and_then(|c| c.size),
                    full_path: child
                        .map(|c| c.full_path.clone())
                        .unwrap_or_else(|| format!("{SHARE_PREFIX}{}", r.path)),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(name: &str, is_dir: bool) -> FsChild {
        FsChild {
            name: name.to_string(),
            relpath: name.to_string(),
            full_path: format!("{SHARE_PREFIX}{name}"),
            is_dir,
            size: if is_dir { None } else { Some(4) },
        }
    }

    #[test]
    fn root_listing_yields_rows() {
        let c = FsBrowseCache::new();
        assert!(c.begin_load("", false), "root should fetch first time");
        assert!(!c.begin_load("", false), "second concurrent load is suppressed");
        c.apply_listing("", vec![child("a.txt", false), child("sub", true)]);
        let rows = c.rows();
        assert_eq!(rows.len(), 2);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"a.txt") && names.contains(&"sub"));
        assert!(rows.iter().find(|r| r.name == "sub").unwrap().is_dir);
        assert!(!rows.iter().find(|r| r.name == "a.txt").unwrap().is_dir);
    }

    #[test]
    fn expanded_dir_shows_children_nested() {
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("sub", true)]);
        c.set_expanded("sub");
        c.apply_listing(
            "sub",
            vec![FsChild {
                name: "deep.txt".into(),
                relpath: "sub/deep.txt".into(),
                full_path: format!("{SHARE_PREFIX}sub/deep.txt"),
                is_dir: false,
                size: Some(9),
            }],
        );
        let rows = c.rows();
        // sub (depth 0, dir, expanded) + deep.txt (depth 1, file).
        assert_eq!(rows.len(), 2);
        let deep = rows.iter().find(|r| r.name == "deep.txt").unwrap();
        assert_eq!(deep.depth, 1);
        assert!(!deep.is_dir);
    }

    #[test]
    fn collapsed_dir_hides_children() {
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("sub", true)]);
        c.apply_listing(
            "sub",
            vec![FsChild {
                name: "deep.txt".into(),
                relpath: "sub/deep.txt".into(),
                full_path: format!("{SHARE_PREFIX}sub/deep.txt"),
                is_dir: false,
                size: None,
            }],
        );
        // Not expanded → only the folder row shows.
        let rows = c.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "sub");
    }

    #[test]
    fn target_switch_resets() {
        let c = FsBrowseCache::new();
        c.sync_target("PEER_A");
        c.apply_listing("", vec![child("a.txt", false)]);
        assert_eq!(c.rows().len(), 1);
        assert!(c.sync_target("PEER_B"), "different target resets");
        assert_eq!(c.rows().len(), 0, "cache cleared on target switch");
        assert!(!c.sync_target("PEER_B"), "same target is a no-op");
    }

    #[test]
    fn selected_file_full_path() {
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("a.txt", false), child("sub", true)]);
        c.select("a.txt");
        assert_eq!(c.selected_full_path().as_deref(), Some("local/files/shared/a.txt"));
        // A directory selection yields no pull target.
        c.select("sub");
        assert_eq!(c.selected_full_path(), None);
    }

    #[test]
    fn force_reload_refetches_listed_dir() {
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("a.txt", false)]);
        assert!(!c.begin_load("", false), "already listed → no refetch");
        assert!(c.begin_load("", true), "force → refetch");
    }
}
