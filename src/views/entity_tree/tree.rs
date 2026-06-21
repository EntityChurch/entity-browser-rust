//! Tree-node helpers — port of `workbench/ui_tree.go`.
//!
//! Maintains a hierarchical tree of path segments. Children at every
//! level are kept sorted by `segment`, so insert / find / remove are
//! O(log n) per level via binary search. Expand state and "has_entry"
//! (this path actually binds an entity vs. is only an intermediate
//! folder) live on each node.
//!
//! Pure data structure: no `Peers` access, no subscription, no I/O.
//! `EntityTreeModel` (`model.rs`) owns one root and drives mutations
//! from the subscription callback (Direct arm) or a cache-diff
//! (Worker arm).

use std::collections::HashSet;

/// One node in the path tree. Root is constructed via [`TreeNode::new_root`]
/// with `depth: -1`; first-level children have `depth: 0`, and so on.
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub segment: String,
    pub full_path: String,
    pub children: Vec<TreeNode>,
    /// True when this path itself binds an entity (vs. an intermediate
    /// folder that exists only because deeper paths do).
    pub has_entry: bool,
    pub expanded: bool,
    pub depth: i32,
}

impl TreeNode {
    /// Sentinel root. Always `expanded = true`, never has children of
    /// its own with `has_entry = true` at this depth.
    pub fn new_root() -> Self {
        Self {
            segment: String::new(),
            full_path: String::new(),
            children: Vec::new(),
            has_entry: false,
            expanded: true,
            depth: -1,
        }
    }
}

/// A flattened visible row in the tree. Owned values — no borrow
/// tracking — so the caller can drop the tree root if needed without
/// invalidating rows. Matches workbench-go's `TreeBrowserRow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRow {
    pub path: String,
    pub segment: String,
    pub depth: usize,
    pub has_children: bool,
    pub expanded: bool,
    pub has_entry: bool,
    /// `Some(n)` on collapsed groups for the "(N)" hint; `None`
    /// otherwise.
    pub leaf_count: Option<usize>,
}

/// Split a path into non-empty segments. Tolerates leading slash.
/// `/a/b/c` and `a/b/c` both yield `["a", "b", "c"]`.
fn segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// Insert (or update) a binding at `path`. Intermediate nodes are
/// created as needed. Newly-created nodes with `depth <
/// auto_expand_below` are created with `expanded = true`; deeper new
/// nodes default to collapsed. **Existing nodes' expand state is
/// never overridden** — user toggles survive subsequent inserts.
///
/// `auto_expand_below = 0` disables auto-expand entirely (all new
/// nodes collapsed). Larger values make the tree progressively
/// disclose deeper paths as they arrive. Picking the right value is
/// an app-policy concern, not a tree-shape concern.
///
/// Children at each level are kept sorted by segment, so insertion is
/// O(log n) per level via binary search and the tree stays ordered
/// without a separate `sort_tree` pass.
pub fn insert_or_update(root: &mut TreeNode, path: &str, auto_expand_below: i32) {
    if let Some(node) = walk_or_create(root, path, auto_expand_below) {
        node.has_entry = true;
    }
}

/// Ensure the folder node at `path` exists (creating intermediate nodes as
/// needed) **without** marking a binding — a folder that exists as an
/// organizational/navigation affordance only, with no entity bound at it. Same
/// node-creation + auto-expand rules as [`insert_or_update`]; a node that
/// already binds an entity keeps its binding. Used by consumers (e.g. the Site
/// Editor) that want an empty, not-yet-populated folder to be visible.
pub fn insert_folder(root: &mut TreeNode, path: &str, auto_expand_below: i32) {
    walk_or_create(root, path, auto_expand_below);
}

/// Walk to the node at `path`, creating any missing nodes along the way, and
/// return a mutable reference to it. Children at each level stay sorted by
/// segment (binary-search insert), so the tree is ordered without a separate
/// sort pass. Returns `None` for an empty path. Does not touch `has_entry` —
/// the caller decides whether the terminal binds an entity.
fn walk_or_create<'a>(
    root: &'a mut TreeNode,
    path: &str,
    auto_expand_below: i32,
) -> Option<&'a mut TreeNode> {
    let parts = segments(path);
    if parts.is_empty() {
        return None;
    }

    // Build full paths for each level. Preserve the input's leading
    // slash so the stored `full_path` round-trips with the source key.
    let leading = if path.starts_with('/') { "/" } else { "" };
    let mut full_paths: Vec<String> = Vec::with_capacity(parts.len());
    let mut acc = String::new();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            acc = format!("{}{}", leading, part);
        } else {
            acc.push('/');
            acc.push_str(part);
        }
        full_paths.push(acc.clone());
    }

    let mut node = root;
    for (depth_idx, part) in parts.iter().enumerate() {
        let pos = match node
            .children
            .binary_search_by(|c| c.segment.as_str().cmp(part))
        {
            Ok(p) => p,
            Err(p) => {
                let depth = depth_idx as i32;
                node.children.insert(
                    p,
                    TreeNode {
                        segment: (*part).to_string(),
                        full_path: full_paths[depth_idx].clone(),
                        children: Vec::new(),
                        has_entry: false,
                        expanded: depth < auto_expand_below,
                        depth,
                    },
                );
                p
            }
        };
        node = &mut node.children[pos];
    }
    Some(node)
}

/// Remove the binding at `path`. If the leaf has no children, the leaf
/// and any empty ancestor folders are pruned. Returns `true` when the
/// binding existed, `false` when the path was absent or had no entry.
pub fn remove(root: &mut TreeNode, path: &str) -> bool {
    let parts = segments(path);
    if parts.is_empty() {
        return false;
    }
    remove_recursive(root, &parts)
}

fn remove_recursive(node: &mut TreeNode, parts: &[&str]) -> bool {
    let part = parts[0];
    let pos = match node
        .children
        .binary_search_by(|c| c.segment.as_str().cmp(part))
    {
        Ok(p) => p,
        Err(_) => return false,
    };

    let found = if parts.len() == 1 {
        let child = &mut node.children[pos];
        if child.has_entry {
            child.has_entry = false;
            true
        } else {
            false
        }
    } else {
        remove_recursive(&mut node.children[pos], &parts[1..])
    };

    if found {
        let child = &node.children[pos];
        if !child.has_entry && child.children.is_empty() {
            node.children.remove(pos);
        }
    }
    found
}

/// Flatten the tree to a list of currently-visible rows, in
/// depth-first order. Collapsed nodes contribute one row each; their
/// descendants are skipped.
pub fn flatten_visible(root: &TreeNode) -> Vec<VisibleRow> {
    let mut rows = Vec::new();
    for child in &root.children {
        flatten_node(child, &mut rows);
    }
    rows
}

fn flatten_node(node: &TreeNode, rows: &mut Vec<VisibleRow>) {
    let has_children = !node.children.is_empty();
    let leaf_count = if has_children && !node.expanded {
        Some(count_leaves(node))
    } else {
        None
    };
    rows.push(VisibleRow {
        path: node.full_path.clone(),
        segment: node.segment.clone(),
        depth: node.depth.max(0) as usize,
        has_children,
        expanded: node.expanded,
        has_entry: node.has_entry,
        leaf_count,
    });
    if node.expanded {
        for child in &node.children {
            flatten_node(child, rows);
        }
    }
}

/// Count the leaf entries under `node` (including `node` itself if it
/// has an entry). Recursive; O(subtree).
pub fn count_leaves(node: &TreeNode) -> usize {
    let mut count = if node.has_entry { 1 } else { 0 };
    for child in &node.children {
        count += count_leaves(child);
    }
    count
}

/// Expand every ancestor of `path` so the row at `path` becomes
/// visible. No-op for paths that don't exist in the tree.
pub fn expand_ancestors(root: &mut TreeNode, path: &str) {
    let parts = segments(path);
    if parts.is_empty() {
        return;
    }
    expand_ancestors_recursive(root, &parts);
}

fn expand_ancestors_recursive(node: &mut TreeNode, parts: &[&str]) {
    let part = parts[0];
    let pos = match node
        .children
        .binary_search_by(|c| c.segment.as_str().cmp(part))
    {
        Ok(p) => p,
        Err(_) => return,
    };
    let child = &mut node.children[pos];
    if parts.len() > 1 {
        // Walking towards target — expand intermediate folder.
        child.expanded = true;
        expand_ancestors_recursive(child, &parts[1..]);
    }
    // The terminal node itself is NOT auto-expanded — selection
    // doesn't imply "open the group beneath it." Go's reference
    // behaves the same way.
}

/// Toggle the `expanded` flag of the node at `path`. Returns `true`
/// when a togglable (has-children) node was found and flipped; leaf
/// nodes can't toggle and return `false`. No-op for absent paths.
///
/// (`entity_tree::model` has an older private equivalent; this is the
/// shared one for new consumers. Unifying the two is a deferred
/// follow-up to the tree-component extraction.)
pub fn toggle_expanded(root: &mut TreeNode, path: &str) -> bool {
    let parts = segments(path);
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

/// Collect the full paths of every currently-expanded node into a
/// set. Used to persist expand state across sessions.
pub fn collect_expanded(root: &TreeNode) -> HashSet<String> {
    let mut out = HashSet::new();
    collect_expanded_recursive(root, &mut out);
    out
}

fn collect_expanded_recursive(node: &TreeNode, out: &mut HashSet<String>) {
    if node.expanded && !node.full_path.is_empty() {
        out.insert(node.full_path.clone());
    }
    for child in &node.children {
        collect_expanded_recursive(child, out);
    }
}

/// Re-expand any node whose `full_path` appears in `expanded`. Used
/// to restore expand state after the tree has been rebuilt from a
/// fresh seed.
pub fn restore_expanded(node: &mut TreeNode, expanded: &HashSet<String>) {
    if expanded.contains(&node.full_path) {
        node.expanded = true;
    }
    for child in &mut node.children {
        restore_expanded(child, expanded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(rows: &[VisibleRow]) -> Vec<&str> {
        rows.iter().map(|r| r.path.as_str()).collect()
    }

    // Bulk-expand setup helper for the flatten / leaf-count /
    // expanded-round-trip tests below. Production code expands via
    // per-event `insert_or_update` + the seed path in
    // `entity_tree/model.rs`, not a depth sweep — so this lives here,
    // test-only, rather than on the production surface.
    fn expand_to_depth(node: &mut TreeNode, max_depth: i32) {
        if node.depth < max_depth {
            node.expanded = true;
        }
        for child in &mut node.children {
            expand_to_depth(child, max_depth);
        }
    }

    #[test]
    fn insert_creates_intermediate_nodes() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a/b/c", 0);

        // Walk down and check shape.
        assert_eq!(root.children.len(), 1);
        let p = &root.children[0];
        assert_eq!(p.segment, "p");
        assert_eq!(p.full_path, "/p");
        assert_eq!(p.depth, 0);
        assert!(!p.has_entry);

        let a = &p.children[0];
        assert_eq!(a.segment, "a");
        assert_eq!(a.full_path, "/p/a");
        assert!(!a.has_entry);

        let b = &a.children[0];
        assert_eq!(b.segment, "b");
        assert_eq!(b.full_path, "/p/a/b");
        assert!(!b.has_entry);

        let c = &b.children[0];
        assert_eq!(c.segment, "c");
        assert_eq!(c.full_path, "/p/a/b/c");
        assert!(c.has_entry);
        assert_eq!(c.depth, 3);
    }

    #[test]
    fn insert_is_order_independent() {
        let mut a = TreeNode::new_root();
        for p in ["/p/x/1", "/p/y/2", "/p/x/2", "/p/y/1"] {
            insert_or_update(&mut a, p, 0);
        }

        let mut b = TreeNode::new_root();
        for p in ["/p/y/1", "/p/x/2", "/p/y/2", "/p/x/1"] {
            insert_or_update(&mut b, p, 0);
        }

        // Same structure regardless of insertion order — children
        // sorted at every level by segment.
        fn shape(n: &TreeNode) -> String {
            let mut s = format!(
                "{}[{},{}]",
                n.segment,
                n.has_entry as u8,
                n.children.len()
            );
            for c in &n.children {
                s.push('(');
                s.push_str(&shape(c));
                s.push(')');
            }
            s
        }
        assert_eq!(shape(&a), shape(&b));
    }

    #[test]
    fn insert_update_existing_path_idempotent() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a", 0);
        insert_or_update(&mut root, "/p/a", 0);
        // No duplicate child.
        assert_eq!(root.children[0].children.len(), 1);
        assert!(root.children[0].children[0].has_entry);
    }

    #[test]
    fn remove_clears_leaf_and_prunes_empty_ancestors() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a/b/c", 0);
        insert_or_update(&mut root, "/p/a/b/d", 0);

        assert!(remove(&mut root, "/p/a/b/c"));
        // /p/a/b/d still exists, so the chain stays.
        let p = &root.children[0];
        let a = &p.children[0];
        let b = &a.children[0];
        assert_eq!(b.children.len(), 1);
        assert_eq!(b.children[0].segment, "d");

        assert!(remove(&mut root, "/p/a/b/d"));
        // Now /p/a/b is empty and pruned all the way to root.
        assert!(root.children.is_empty());
    }

    #[test]
    fn remove_keeps_intermediate_that_also_binds() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a", 0);
        insert_or_update(&mut root, "/p/a/b", 0);

        assert!(remove(&mut root, "/p/a/b"));
        // /p/a still has its own binding; not pruned.
        let p = &root.children[0];
        let a = &p.children[0];
        assert!(a.has_entry);
        assert!(a.children.is_empty());
    }

    #[test]
    fn remove_absent_returns_false() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a", 0);
        assert!(!remove(&mut root, "/p/missing"));
        assert!(!remove(&mut root, "/missing"));
    }

    #[test]
    fn flatten_respects_expand_state() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a/x", 0);
        insert_or_update(&mut root, "/p/a/y", 0);
        insert_or_update(&mut root, "/p/b", 0);

        // Collapsed by default: only top-level children visible.
        let rows = flatten_visible(&root);
        assert_eq!(paths(&rows), vec!["/p"]);

        // Expand /p (depth=0) — its children become visible.
        expand_to_depth(&mut root, 1);
        let rows = flatten_visible(&root);
        assert_eq!(paths(&rows), vec!["/p", "/p/a", "/p/b"]);

        // Expand /p/a (depth=1) — its leaves show up.
        expand_to_depth(&mut root, 2);
        let rows = flatten_visible(&root);
        assert_eq!(
            paths(&rows),
            vec!["/p", "/p/a", "/p/a/x", "/p/a/y", "/p/b"]
        );
    }

    #[test]
    fn flatten_emits_leaf_count_on_collapsed_groups_only() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a/x", 0);
        insert_or_update(&mut root, "/p/a/y", 0);

        // Default: /p collapsed → leaf_count = Some(2).
        let rows = flatten_visible(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/p");
        assert_eq!(rows[0].leaf_count, Some(2));

        // Expand /p (depth=0) → leaf_count on /p clears.
        expand_to_depth(&mut root, 1);
        let rows = flatten_visible(&root);
        assert_eq!(rows[0].leaf_count, None);
        // /p/a is now visible and still collapsed.
        let a = rows.iter().find(|r| r.path == "/p/a").unwrap();
        assert_eq!(a.leaf_count, Some(2));
    }

    #[test]
    fn count_leaves_sums_descendants_with_entries() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a/x", 0);
        insert_or_update(&mut root, "/p/a/y", 0);
        insert_or_update(&mut root, "/p/a", 0); // intermediate also binds
        insert_or_update(&mut root, "/p/b/1/2", 0);

        let p = &root.children[0];
        // /p itself has no binding; /p/a has 1 (self) + 2 (x,y) = 3;
        // /p/b/1/2 contributes 1 → total 4.
        assert_eq!(count_leaves(p), 4);

        let a = &p.children[0];
        assert_eq!(count_leaves(a), 3);
    }

    #[test]
    fn expand_ancestors_walks_deeply_nested_path() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a/b/c/d/e", 0);

        expand_ancestors(&mut root, "/p/a/b/c/d/e");

        // Every ancestor expanded; the terminal node itself is NOT
        // auto-expanded (selection ≠ expansion).
        let p = &root.children[0];
        assert!(p.expanded);
        let a = &p.children[0];
        assert!(a.expanded);
        let b = &a.children[0];
        assert!(b.expanded);
        let c = &b.children[0];
        assert!(c.expanded);
        let d = &c.children[0];
        assert!(d.expanded);
        let e = &d.children[0];
        assert!(!e.expanded);
    }

    #[test]
    fn expand_ancestors_on_missing_path_is_noop() {
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/p/a", 0);

        expand_ancestors(&mut root, "/p/missing/deep");
        // /p got expanded along the matching prefix; the missing
        // segment terminates the walk.
        let p = &root.children[0];
        assert!(p.expanded);
    }

    #[test]
    fn collect_and_restore_expanded_round_trip() {
        let mut a = TreeNode::new_root();
        insert_or_update(&mut a, "/p/x/1", 0);
        insert_or_update(&mut a, "/p/y/2", 0);
        // Expand /p (depth=0) and /p/x, /p/y (depth=1) — leaves stay
        // collapsed.
        expand_to_depth(&mut a, 2);

        let expanded = collect_expanded(&a);
        // /p, /p/x, /p/y should be in the set.
        assert!(expanded.contains("/p"));
        assert!(expanded.contains("/p/x"));
        assert!(expanded.contains("/p/y"));
        // Leaves are not expanded.
        assert!(!expanded.contains("/p/x/1"));

        // Fresh tree, then restore.
        let mut b = TreeNode::new_root();
        insert_or_update(&mut b, "/p/x/1", 0);
        insert_or_update(&mut b, "/p/y/2", 0);
        // All children start collapsed.
        assert!(!b.children[0].expanded);

        restore_expanded(&mut b, &expanded);
        let p = &b.children[0];
        assert!(p.expanded);
        assert!(p.children[0].expanded); // /p/x
        assert!(p.children[1].expanded); // /p/y
    }

    #[test]
    fn segments_tolerates_leading_and_trailing_slashes() {
        assert_eq!(segments("/a/b/c"), vec!["a", "b", "c"]);
        assert_eq!(segments("a/b/c"), vec!["a", "b", "c"]);
        assert_eq!(segments("//a//b"), vec!["a", "b"]);
        assert_eq!(segments(""), Vec::<&str>::new());
        assert_eq!(segments("/"), Vec::<&str>::new());
    }

    // -- Default-collapse policy (AUTO_EXPAND_BELOW == 1) --
    //
    // These pin the production default chosen in `model.rs`: a fresh
    // tree opens with each peer-root node (depth 0) expanded and its
    // top-level groups (depth 1) visible-but-collapsed, nothing deeper.
    // `model::AUTO_EXPAND_BELOW` is a private const, so we mirror its
    // value here; if it changes, these scenarios are the canary.
    const DEFAULT_AUTO_EXPAND_BELOW: i32 = 1;

    #[test]
    fn fresh_tree_opens_collapsed_at_top_level() {
        // Realistic single-peer layout: app bindings sit 5–6 deep,
        // exactly the case that depth-8 auto-expand used to blow open.
        let mut root = TreeNode::new_root();
        for p in [
            "/peerA/app/entity-browser/workspace/windows/w1/state",
            "/peerA/app/entity-browser/settings/ui",
            "/peerA/system/roster/peerA",
            "/peerA/content/sites/demo/index",
        ] {
            insert_or_update(&mut root, p, DEFAULT_AUTO_EXPAND_BELOW);
        }

        // Only the peer node + its top-level groups are visible; the
        // deep app/system/content state stays hidden behind collapsed
        // groups.
        assert_eq!(
            paths(&flatten_visible(&root)),
            vec!["/peerA", "/peerA/app", "/peerA/content", "/peerA/system"],
        );

        // The top-level groups are collapsed and advertise a leaf-count
        // hint, proving they have hidden children the user can open.
        let rows = flatten_visible(&root);
        let app = rows.iter().find(|r| r.path == "/peerA/app").unwrap();
        assert!(!app.expanded, "top-level group should open collapsed");
        assert!(app.has_children);
        assert!(app.leaf_count.is_some());
    }

    #[test]
    fn fresh_tree_collapsed_across_multiple_top_level_peers() {
        // The local store can hold many top-level peer-id routes at
        // once: the bound peer plus cached foreign sites
        // (`/{foreign}/sites/...`) and other hosted peers. Each must
        // open as its own collapsed root — no peer's subtree leaks open.
        let mut root = TreeNode::new_root();
        for p in [
            "/peerA/app/entity-browser/settings/ui",
            "/peerA/system/roster/peerA",
            "/foreignB/sites/blog/posts/hello",
            "/foreignB/system/cache/manifest",
            "/peerC/content/notes/todo",
        ] {
            insert_or_update(&mut root, p, DEFAULT_AUTO_EXPAND_BELOW);
        }

        // Each peer root (depth 0) is expanded; every depth-1 group is
        // collapsed. Order is segment-sorted: foreignB < peerA < peerC.
        assert_eq!(
            paths(&flatten_visible(&root)),
            vec![
                "/foreignB",
                "/foreignB/sites",
                "/foreignB/system",
                "/peerA",
                "/peerA/app",
                "/peerA/system",
                "/peerC",
                "/peerC/content",
            ],
        );
        for r in flatten_visible(&root) {
            // Every visible non-root row sits at depth 0 or 1; nothing
            // from depth 2+ leaked into the initial view.
            assert!(r.depth <= 1, "depth {} leaked: {}", r.depth, r.path);
        }
    }

    #[test]
    fn incremental_writes_do_not_reexpand_collapsed_peers() {
        // The regression that made it "come back": every entity write
        // runs insert_or_update, and the old depth-8 default re-popped
        // the tree open. With the collapsed default, a write under an
        // already-collapsed group adds the node but never flips an
        // existing group back to expanded.
        let mut root = TreeNode::new_root();
        insert_or_update(&mut root, "/peerA/app/entity-browser/settings/ui", DEFAULT_AUTO_EXPAND_BELOW);

        // User leaves it at the default (app collapsed). A background
        // write lands a brand-new deep path under app.
        insert_or_update(
            &mut root,
            "/peerA/app/entity-browser/workspace/windows/w9/state",
            DEFAULT_AUTO_EXPAND_BELOW,
        );

        // app is still collapsed — the view didn't blow open.
        let rows = flatten_visible(&root);
        let app = rows.iter().find(|r| r.path == "/peerA/app").unwrap();
        assert!(!app.expanded);
        // And the deep write is not visible (hidden behind collapsed app).
        assert!(!rows.iter().any(|r| r.path.contains("/windows/")));
    }
}
