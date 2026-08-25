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
    /// Set when this row is a **file the peer offered** rather than a file in
    /// its `local/files` share — the hex of its content blob. It is the one
    /// field that distinguishes the two kinds of serving peer, and it is
    /// consumed exactly once, by [`FsBrowseCache::selected_pull`], to build a
    /// [`PullPlan`]. Nothing in the DOM reads it.
    pub offer_blob: Option<String>,
}

/// Why the **share** half of this pane produced no rows.
///
/// **A peer having no share is a fact, not an error**, and until now the two
/// arrived as the same `Option<String>`. A browser peer has no `local/files`
/// handler and cannot have one (`entity-local-files` is
/// `#![cfg(not(target_arch = "wasm32"))]`), so selecting one auto-listed a
/// share that does not exist, got a protocol error back, and rendered it
/// **loudly beside a transfer that was working perfectly** through the offers
/// half. Reported from a real two-device run as a protocol error next to a
/// file that transferred — which is its own kind of harm, because the next
/// real error is now indistinguishable from the noise.
///
/// The two outcomes get different variants rather than one nullable string:
/// the same move as `SignedFetchError::IncompleteWalk` against `Transport`,
/// and for the same reason — *absent* and *broken* keep arriving as one value.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum ShareState {
    /// Listed, in flight, or not asked yet.
    #[default]
    Fine,
    /// This peer serves no share at all. Expected for every browser peer, and
    /// **not** something to render as a failure.
    Absent,
    /// The peer has a share and the listing genuinely failed — a denial, a
    /// transport fault, a broken directory. Loud (D13).
    Failed(String),
}

/// Classify a non-OK answer to a share `list`.
///
/// **Structural, not a string match.** A missing handler is a `404` carrying a
/// `system/protocol/error` entity whose code is `handler_not_found` — produced
/// identically by local dispatch and by wire dispatch (`core/peer`), so this
/// holds for the remote peers this window actually talks to.
///
/// A pure function over plain arguments rather than over `HandlerResult`,
/// which is neither `Debug` nor `Clone` and so cannot be built in a native
/// test — the same native-shadow split as `AppServerView::grade`. The caller
/// decodes; this decides.
///
/// **Only `handler_not_found` means absent.** A `403` is a peer that has a
/// share and will not show it to you, which is a different sentence and a
/// different remedy; anything unrecognised stays loud, because failing quiet is
/// how a real fault becomes invisible.
pub fn classify_share_failure(status: u32, code: Option<&str>, summary: &str) -> ShareState {
    if status == 404 && code == Some("handler_not_found") {
        ShareState::Absent
    } else {
        ShareState::Failed(summary.to_string())
    }
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
    /// Why the share half produced no rows — absent, failed, or fine.
    share: ShareState,
    /// Whether the one-shot auto-load of the root has been claimed for the
    /// current target. Reset (with the rest of `Inner`) on a target switch, so
    /// each target auto-loads exactly once.
    auto_attempted: bool,
}

/// Drop `key` and everything cached beneath it — nodes, expand/listed/loading
/// marks, and the selection if it pointed inside.
///
/// Used when a re-listing says a directory is gone. Leaving the subtree behind
/// would keep it marked `listed`, so a directory of the same name reappearing
/// later would render from children nobody would ever re-fetch.
fn forget_subtree(inner: &mut Inner, key: &str) {
    let sub = format!("{key}/");
    let doomed: Vec<String> = inner
        .nodes
        .keys()
        .filter(|k| k.as_str() == key || k.starts_with(&sub))
        .cloned()
        .collect();
    for k in doomed {
        inner.nodes.remove(&k);
        inner.listed.remove(&k);
        inner.expanded.remove(&k);
        inner.loading.remove(&k);
        if inner.selected.as_deref() == Some(k.as_str()) {
            inner.selected = None;
        }
    }
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

    /// Claim the one-shot auto-load of the share root (S5 / ROADMAP 3d — no
    /// first "Browse" click). Returns `true` at most once per target (the cache
    /// resets on a target switch via [`sync_target`]), and only when the root
    /// isn't already loaded or loading. A failed load leaves the root un-listed
    /// but keeps `auto_attempted` set, so it does **not** retry every frame —
    /// the user re-triggers with Browse/Refresh.
    pub fn claim_auto_load(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.auto_attempted {
            return false;
        }
        inner.auto_attempted = true;
        !inner.loading.contains("") && !inner.listed.contains("")
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
    ///
    /// **A refresh does NOT un-list the directory**, and that is the whole
    /// point: `listed` is what the renderer gates the tree on, so clearing it
    /// here turned an in-place re-ask into a *teardown* — the rows vanished the
    /// instant Refresh was pressed and came back only if the re-list succeeded.
    /// Against a peer whose share is absent (every browser peer) the re-list
    /// never succeeds, so Refresh permanently replaced a working listing of
    /// offered files with the "Browse shared files" button. Reported from a real
    /// two-device run as *"I hit refresh and it deletes the file"*. The
    /// in-flight state is `loading`, which the renderer shows beside the rows.
    pub fn begin_load(&self, relpath: &str, force: bool) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if inner.loading.contains(relpath) {
            return false;
        }
        if !force && inner.listed.contains(relpath) {
            return false;
        }
        inner.loading.insert(relpath.to_string());
        inner.share = ShareState::Fine;
        true
    }

    /// Record a directory's children (marks it listed, clears its loading flag).
    ///
    /// **This REPLACES `relpath`'s direct children rather than merging into
    /// them.** A file deleted on the far side has to leave the listing, and a
    /// cache that only ever inserts can never show that — Refresh would fetch
    /// the shorter listing, add nothing, and leave the vanished file on screen
    /// under a Pull button that 404s. Same rule and same reason as
    /// [`apply_offers`](Self::apply_offers), which has always replaced; the
    /// share half simply never did.
    ///
    /// A removed *directory* takes its whole cached subtree with it, along with
    /// its expand/listed marks — otherwise a directory that came back later
    /// would render already-listed from stale children nobody re-fetched.
    /// Offered rows (`~`-keyed) are never touched: the two sources meet in this
    /// cache without knowing about each other.
    pub fn apply_listing(&self, relpath: &str, children: Vec<FsChild>) {
        let mut inner = self.inner.lock().unwrap();
        inner.loading.remove(relpath);
        inner.listed.insert(relpath.to_string());

        let prefix = if relpath.is_empty() { String::new() } else { format!("{relpath}/") };
        let incoming: HashSet<&str> = children.iter().map(|c| c.relpath.as_str()).collect();
        let stale: Vec<String> = inner
            .nodes
            .keys()
            .filter(|k| !k.starts_with('~'))
            .filter(|k| {
                k.strip_prefix(prefix.as_str())
                    .map(|rest| !rest.is_empty() && !rest.contains('/'))
                    .unwrap_or(false)
            })
            .filter(|k| !incoming.contains(k.as_str()))
            .cloned()
            .collect();
        for key in stale {
            forget_subtree(&mut inner, &key);
        }

        for child in children {
            inner.nodes.insert(child.relpath.clone(), child);
        }
    }

    /// Fold what the target is **offering** into the same root listing its
    /// share fills. A browser peer has no directory to mount, so its files
    /// arrive this way; a native peer typically offers nothing and this is a
    /// no-op. Both end up as ordinary rows.
    ///
    /// Keyed by `~{offer-id}` rather than by filename: the id is the content
    /// hash, so the key is collision-free against both the share's relpaths
    /// (which never start with `~`) and against two offers that happen to share
    /// a name. The *displayed* name is still the manifest's filename — which is
    /// the entire reason a manifest exists beside hash-addressed content.
    ///
    /// **Replaces the offer set rather than merging into it**, and returns
    /// whether anything changed. An offer can be *withdrawn* — the sender takes
    /// the manifest down — and a listing that only ever inserts can never show
    /// that: Refresh would fetch the shorter list, add nothing, and leave the
    /// vanished file on screen with a Pull button that 404s. The `~` prefix is
    /// what makes the replacement safe to scope: it touches offers only, never a
    /// share row, so the two sources still meet here without knowing about each
    /// other.
    ///
    /// **"Changed" means any state a render can see — including the listed /
    /// loading flags, not only the row set.** The caller repaints on `true` and
    /// on nothing else, so reporting only the rows meant an *unchanged* offer
    /// listing marked the root listed and then told the caller there was nothing
    /// to paint. That is the second half of the Refresh bug: the share half's
    /// failure repainted (correctly, showing no tree), the offers half then
    /// repaired the state silently, and the window sat on the "Browse shared
    /// files" button with a perfectly good listing behind it — for every Refresh
    /// after the first, forever.
    pub fn apply_offers(&self, offers: Vec<crate::file_offer::FileOffer>) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let mut changed = false;
        if !offers.is_empty() {
            // The root counts as listed even if the share half failed: a browser
            // peer answers `local/files` with an error and its offers ARE the
            // listing, and leaving the window on "Browse" would be a lie. Only
            // on a non-empty answer, though — an empty one must not turn a
            // failed share browse into a confident "this share is empty".
            changed |= inner.loading.remove("");
            changed |= inner.listed.insert(String::new());
        }
        let fresh: BTreeMap<String, FsChild> = offers
            .into_iter()
            .map(|offer| {
                let id = offer.id();
                let relpath = format!("~{id}");
                (
                    relpath.clone(),
                    FsChild {
                        name: offer.name,
                        relpath,
                        // No tree path — an offer is fetched by content hash,
                        // not by path. `selected_pull` never reads this for an
                        // offer.
                        full_path: String::new(),
                        is_dir: false,
                        size: Some(offer.size),
                        offer_blob: Some(id),
                    },
                )
            })
            .collect();
        let had: BTreeMap<String, FsChild> = inner
            .nodes
            .iter()
            .filter(|(k, _)| k.starts_with('~'))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if had == fresh {
            return changed;
        }
        for key in had.keys() {
            inner.nodes.remove(key);
        }
        inner.nodes.extend(fresh);
        true
    }

    /// Record a load failure, already classified by
    /// [`classify_share_failure`]. Clears the loading flag either way — an
    /// absent share is still an answer, and leaving the row spinning would
    /// make "this peer has no share" look like "still asking".
    pub fn fail_load(&self, relpath: &str, state: ShareState) {
        let mut inner = self.inner.lock().unwrap();
        inner.loading.remove(relpath);
        inner.share = state;
    }

    /// Select a file by relpath.
    pub fn select(&self, relpath: &str) {
        self.inner.lock().unwrap().selected = Some(relpath.to_string());
    }

    /// The selected file's full tree path (for display), if a file is selected.
    pub fn selected_full_path(&self) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        let sel = inner.selected.as_ref()?;
        inner.nodes.get(sel).filter(|n| !n.is_dir).map(|n| n.full_path.clone())
    }

    /// **How to fetch the selected file** — the model boundary the whole
    /// transparency argument rests on. One `match` here, and every surface
    /// above it (the row, the button, the DOM, the action) is peer-kind-blind.
    pub fn selected_pull(&self) -> Option<crate::action::PullPlan> {
        use crate::action::PullPlan;
        let inner = self.inner.lock().unwrap();
        let sel = inner.selected.as_ref()?;
        let node = inner.nodes.get(sel).filter(|n| !n.is_dir)?;
        Some(match &node.offer_blob {
            Some(blob_hex) => PullPlan::Offer {
                blob_hex: blob_hex.clone(),
                filename: node.name.clone(),
            },
            None => PullPlan::Share {
                path: node.full_path.clone(),
                // The share's own leaf name, not the tree key — they agree
                // today, and taking it from the node keeps them agreeing if a
                // listing ever renames on the way through.
                filename: node.name.clone(),
            },
        })
    }

    /// Has the root been listed yet? (Drives "Browse" button vs. the tree.)
    pub fn root_listed(&self) -> bool {
        self.inner.lock().unwrap().listed.contains("")
    }

    /// Is the root currently loading?
    pub fn root_loading(&self) -> bool {
        self.inner.lock().unwrap().loading.contains("")
    }

    /// A **real** share failure, for loud rendering (D13). An absent share is
    /// deliberately not one — see [`share_absent`](Self::share_absent).
    pub fn error(&self) -> Option<String> {
        match &self.inner.lock().unwrap().share {
            ShareState::Failed(msg) => Some(msg.clone()),
            ShareState::Fine | ShareState::Absent => None,
        }
    }

    /// Does this peer serve no share at all?
    ///
    /// Two consumers, deliberately reading it differently: the note explaining
    /// an empty pane is suppressed once there is anything else on screen, while
    /// the Send card is withheld outright — an upload is `local/files:write`,
    /// so against a peer with no such handler it can only reproduce the error
    /// the browse half stopped showing.
    pub fn share_absent(&self) -> bool {
        matches!(self.inner.lock().unwrap().share, ShareState::Absent)
    }

    /// Does the current listing hold any rows at all (share or offered)?
    pub fn is_empty(&self) -> bool {
        self.inner.lock().unwrap().nodes.is_empty()
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
                    // The entry's own name when we have one — an offer's key is
                    // its content hash, and rendering that instead of the
                    // filename would defeat the manifest.
                    name: child.map(|c| c.name.clone()).unwrap_or(r.segment),
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
            offer_blob: None,
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
                offer_blob: None,
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
                offer_blob: None,
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

    /// **A re-ask is not a teardown.** Refresh used to clear `listed`, which is
    /// what the renderer gates the whole tree on — so the rows vanished the
    /// instant the button was pressed and returned only if the re-list came
    /// back. The in-flight state is `loading`, which renders *beside* the rows.
    #[test]
    fn a_refresh_keeps_the_listing_on_screen_while_it_re_asks() {
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("a.txt", false)]);
        assert!(c.begin_load("", true), "Refresh forces a re-list");
        assert!(c.root_listed(), "the rows stay on screen while we re-ask");
        assert!(c.root_loading(), "…and the re-ask is visible as in-flight");
        assert_eq!(c.rows().len(), 1);
    }

    /// **The reported bug, end to end.** A peer that serves no share (every
    /// browser peer) lists its *offers*; pressing Refresh re-asks both halves,
    /// the share half answers `handler_not_found`, and the offers half answers
    /// with the same offer as before. Before the fix that sequence left a
    /// working listing showing the "Browse shared files" button — and pressing
    /// that button repeated it, forever.
    #[test]
    fn refreshing_an_offers_only_peer_does_not_empty_the_pane() {
        let c = FsBrowseCache::new();
        c.sync_target("PEER_SERVING");
        let file = offer("report.bin", b"the file they could download");

        // First browse: no share, one offered file.
        assert!(c.begin_load("", false));
        c.fail_load("", classify_share_failure(404, Some("handler_not_found"), "protocol error"));
        assert!(c.apply_offers(vec![file.clone()]));
        assert!(c.root_listed() && c.rows().len() == 1);

        // Refresh. The share half fails again (it always will), and the offers
        // half returns exactly what it returned last time.
        assert!(c.begin_load("", true));
        c.fail_load("", classify_share_failure(404, Some("handler_not_found"), "protocol error"));
        let unchanged_needs_paint = c.apply_offers(vec![file]);

        assert!(c.root_listed(), "the listing survives its own Refresh");
        assert_eq!(c.rows().len(), 1, "the offered file is still there: {:?}", c.rows().len());
        assert!(!c.root_loading(), "both halves answered — nothing is still in flight");
        assert!(
            !unchanged_needs_paint,
            "with the listing never torn down there is genuinely nothing new to \
             paint — the repaint this used to owe was for state IT had repaired"
        );
    }

    /// The other half of that fix, in isolation: when `apply_offers` *does*
    /// repair render-visible state (the root going from un-listed to listed),
    /// it must say so — the caller repaints on `true` and on nothing else.
    #[test]
    fn an_unchanged_offer_listing_still_reports_state_it_repaired() {
        let c = FsBrowseCache::new();
        let file = offer("report.bin", b"payload");
        assert!(c.apply_offers(vec![file.clone()]), "first listing: new rows");

        // Force the root back to un-listed *without* touching the rows — the
        // shape a target switch mid-flight leaves behind.
        c.begin_load("", true);
        assert!(c.root_loading());
        assert!(
            c.apply_offers(vec![file]),
            "the row set is identical, but the root just became listed and the \
             spinner just stopped: that is a repaint the caller owes"
        );
        assert!(c.root_listed() && !c.root_loading());
    }

    /// A share file deleted on the far side must LEAVE the listing. The offers
    /// half has always replaced; the share half only ever inserted, so a
    /// deletion was invisible and its Pull button stayed on screen.
    #[test]
    fn a_file_deleted_on_the_far_side_leaves_the_listing() {
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("keep.txt", false), child("gone.txt", false)]);
        c.select("gone.txt");
        assert_eq!(c.rows().len(), 2);

        c.begin_load("", true);
        c.apply_listing("", vec![child("keep.txt", false)]);

        let names: Vec<String> = c.rows().into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["keep.txt".to_string()], "the deleted file is gone");
        assert!(
            c.selected_pull().is_none(),
            "…and so is the Pull button that would have 404'd"
        );
    }

    /// A removed directory takes its cached subtree and its `listed` mark with
    /// it — otherwise a directory of the same name reappearing later renders
    /// from children nobody would re-fetch.
    #[test]
    fn a_removed_directory_takes_its_subtree_with_it() {
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
                offer_blob: None,
            }],
        );
        assert_eq!(c.rows().len(), 2);

        // The directory is gone on the far side.
        c.begin_load("", true);
        c.apply_listing("", vec![child("other.txt", false)]);
        assert_eq!(c.rows().len(), 1);

        // It comes back — and must be re-fetched, not served from the stale
        // children still sitting in the cache.
        c.begin_load("", true);
        c.apply_listing("", vec![child("sub", true)]);
        assert!(c.begin_load("sub", false), "the subtree's listing was forgotten too");
    }

    /// The replacement is scoped to the directory that was listed: a listing of
    /// the root must not evict a *deeper* cached directory's children, and must
    /// never touch an offered row.
    #[test]
    fn re_listing_a_directory_leaves_its_siblings_and_offers_alone() {
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("sub", true), child("a.txt", false)]);
        c.set_expanded("sub");
        c.apply_listing(
            "sub",
            vec![FsChild {
                name: "deep.txt".into(),
                relpath: "sub/deep.txt".into(),
                full_path: format!("{SHARE_PREFIX}sub/deep.txt"),
                is_dir: false,
                size: Some(9),
                offer_blob: None,
            }],
        );
        c.apply_offers(vec![offer("offered.bin", b"from the other source")]);
        assert_eq!(c.rows().len(), 4);

        // Re-list the root with the same entries: nothing may be lost.
        c.begin_load("", true);
        c.apply_listing("", vec![child("sub", true), child("a.txt", false)]);
        let names: Vec<String> = c.rows().into_iter().map(|r| r.name).collect();
        assert_eq!(names.len(), 4, "{names:?}");
        assert!(names.iter().any(|n| n == "deep.txt"), "the subtree survives: {names:?}");
        assert!(names.iter().any(|n| n == "offered.bin"), "the offer survives: {names:?}");
    }

    pub(super) fn offer(name: &str, bytes: &[u8]) -> crate::file_offer::FileOffer {
        let (blob, _) = crate::file_offer::chunk_bytes(bytes).unwrap();
        crate::file_offer::FileOffer {
            name: name.to_string(),
            size: bytes.len() as u64,
            blob: blob.content_hash,
            from: "PEER_SERVING".into(),
        }
    }

    /// **THE TRANSPARENCY TEST.** A share file and an offered file land in one
    /// listing, render as the same kind of row, and differ only in the plan the
    /// model hands out for pulling them. If this ever needs a "which kind of
    /// peer is this?" branch above the cache, that is the regression.
    #[test]
    fn offered_files_and_share_files_are_the_same_rows() {
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("from-share.txt", false)]);
        c.apply_offers(vec![offer("from-offer.bin", b"hello, offered world")]);

        let rows = c.rows();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"from-share.txt"));
        assert!(
            names.contains(&"from-offer.bin"),
            "an offer must render under its FILENAME, not its content hash — \
             giving a hash a name is the manifest's whole job; got {names:?}"
        );
        assert!(rows.iter().all(|r| !r.is_dir), "both are plain files");
        let offered = rows.iter().find(|r| r.name == "from-offer.bin").unwrap();
        assert_eq!(offered.depth, 0, "offers sit at the root, beside the share");
        assert_eq!(offered.size, Some(20));
    }

    #[test]
    fn the_pull_plan_is_where_the_two_kinds_of_peer_diverge() {
        use crate::action::PullPlan;
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("doc.txt", false)]);
        let off = offer("photo.jpg", b"not really a jpeg");
        let id = off.id();
        c.apply_offers(vec![off]);

        c.select("doc.txt");
        assert_eq!(
            c.selected_pull(),
            Some(PullPlan::Share {
                path: format!("{SHARE_PREFIX}doc.txt"),
                filename: "doc.txt".into()
            }),
            "a share file pulls by tree path (local/files:read)"
        );

        c.select(&format!("~{id}"));
        assert_eq!(
            c.selected_pull(),
            Some(PullPlan::Offer { blob_hex: id.clone(), filename: "photo.jpg".into() }),
            "an offered file pulls by content hash (the system/content walk)"
        );

        // A directory is not pullable, whichever source it came from.
        c.apply_listing("", vec![child("sub", true)]);
        c.select("sub");
        assert!(c.selected_pull().is_none());
    }

    /// A withdrawn offer must **leave** the listing. The sender can take a
    /// manifest down; a cache that only ever inserted would keep the row (and
    /// its Pull button) on screen through every Refresh, and the user's only
    /// escape was switching targets. Share rows are untouched by the same call —
    /// the two sources still meet in this cache without knowing about each
    /// other.
    #[test]
    fn a_withdrawn_offer_disappears_on_the_next_listing() {
        let c = FsBrowseCache::new();
        c.apply_listing("", vec![child("from-share.txt", false)]);
        let a = offer("a.bin", b"first offered file");
        let b = offer("b.bin", b"second offered file");
        assert!(c.apply_offers(vec![a.clone(), b.clone()]), "first listing is a change");
        assert!(!c.apply_offers(vec![a.clone(), b.clone()]), "an identical listing is not");
        assert_eq!(c.rows().len(), 3);

        // The sender withdrew b.bin.
        assert!(c.apply_offers(vec![a.clone()]), "a shorter listing is a change");
        let names: Vec<String> = c.rows().into_iter().map(|r| r.name).collect();
        assert!(!names.iter().any(|n| n == "b.bin"), "the withdrawn offer is gone: {names:?}");
        assert!(
            names.iter().any(|n| n == "a.bin") && names.iter().any(|n| n == "from-share.txt"),
            "{names:?}"
        );

        // ...and all of them: an empty answer clears the offers and still
        // leaves the share alone.
        assert!(c.apply_offers(vec![]));
        assert_eq!(
            c.rows().iter().map(|r| r.name.clone()).collect::<Vec<_>>(),
            vec!["from-share.txt".to_string()]
        );
        // A selection pointing at a vanished offer yields no plan rather than a
        // Pull that would 404.
        c.select(&format!("~{}", a.id()));
        assert!(c.selected_pull().is_none());
    }

    #[test]
    fn an_offer_id_round_trips_to_its_blob_hash() {
        // `selected_pull` hands the id across as text; the app turns it back
        // into a hash to fetch. If those two ever disagree the Pull button
        // silently stops working, so pin the pair here.
        let off = offer("x.bin", b"round trip");
        assert_eq!(crate::file_offer::hash_from_id(&off.id()).unwrap(), off.blob);
        assert!(crate::file_offer::hash_from_id("not-hex").is_err());
        assert!(crate::file_offer::hash_from_id("abc").is_err(), "odd length is not hex");
    }
}

#[cfg(test)]
mod share_state_tests {
    use super::tests::offer;
    use super::*;

    /// **The bug, in one assertion.** A browser peer has no `local/files`
    /// handler and cannot have one, so selecting one auto-lists a share that
    /// does not exist and gets `404 handler_not_found` back. That is an ANSWER.
    /// Rendering it as a failure put a red protocol error beside a file that
    /// was transferring perfectly through the offers half — observed on the
    /// first real two-device run.
    #[test]
    fn a_peer_with_no_share_is_absent_and_never_an_error() {
        let state = classify_share_failure(404, Some("handler_not_found"), "protocol error");
        assert_eq!(state, ShareState::Absent);

        let cache = FsBrowseCache::new();
        assert!(cache.begin_load("", false));
        cache.fail_load("", state);
        assert_eq!(cache.error(), None, "an absent share is not a loud error");
        assert!(cache.share_absent());
        assert!(!cache.root_loading(), "an answer clears the spinner");
    }

    /// The other direction, and it is the one that matters for not failing
    /// quiet: **only `handler_not_found` means absent.** A 403 is a peer that
    /// HAS a share and will not show it to you — a different sentence with a
    /// different remedy — and anything unrecognised stays loud, because a fault
    /// classified as "no share" is a fault nobody ever sees.
    #[test]
    fn every_other_refusal_stays_loud() {
        for (status, code) in [
            (403u32, Some("capability_denied")),
            (500, Some("internal")),
            (404, Some("not_found")),
            (404, None),
            (503, None),
        ] {
            let state = classify_share_failure(status, code, "boom");
            assert_eq!(
                state,
                ShareState::Failed("boom".into()),
                "status {status} code {code:?} must not be silently absent",
            );
            let cache = FsBrowseCache::new();
            assert!(cache.begin_load("", false));
            cache.fail_load("", state);
            assert_eq!(cache.error().as_deref(), Some("boom"));
            assert!(!cache.share_absent());
        }
    }

    /// A retry clears the verdict, so a peer that grows a share (or a transient
    /// fault that passes) is not stuck reporting the old answer.
    #[test]
    fn re_listing_clears_a_previous_verdict() {
        let cache = FsBrowseCache::new();
        assert!(cache.begin_load("", false));
        cache.fail_load("", ShareState::Absent);
        assert!(cache.share_absent());

        assert!(cache.begin_load("", true), "Refresh forces a re-list");
        assert!(!cache.share_absent(), "the verdict is not sticky across a retry");
        assert_eq!(cache.error(), None);
    }

    /// **The note is suppressed by content; the verdict is not.** Offered files
    /// arriving must not retract "this peer serves no share" — the Send card
    /// stays withheld either way, because an upload is `local/files:write` and
    /// the handler still is not there. What changes is only whether the
    /// sentence is worth printing, and that is the renderer's `&&`.
    ///
    /// Both halves asserted, because collapsing them is the tempting bug: a
    /// verdict that cleared itself when offers landed would put a Send button
    /// back on a peer that cannot receive.
    #[test]
    fn offers_suppress_the_note_without_retracting_the_verdict() {
        let cache = FsBrowseCache::new();
        assert!(cache.begin_load("", false));
        cache.fail_load("", ShareState::Absent);
        assert!(cache.share_absent() && cache.is_empty(), "nothing else to show: say so");

        cache.apply_offers(vec![offer("shared.bin", b"payload")]);
        assert!(cache.share_absent(), "the verdict itself is unchanged");
        assert!(!cache.is_empty(), "…but there is content now, so the note is suppressed");
    }
}
