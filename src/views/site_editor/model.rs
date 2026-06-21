//! Site Editor model — read + write side.
//!
//! Owns the editor's in-memory **selection** (which site / page is open) behind
//! an `Arc<Mutex<Inner>>` (all methods `&self`, mirroring the Content Site
//! model). The **data** — manifests and pages — lives in the tree, not here;
//! the editor just `put`s the same entities the browser reads, so a created /
//! edited site is picked up by the (frozen) Content Site browser for free.
//!
//! Arm: Direct/IDB, the bound peer (F2-arm).
//! Writes route via `Peers::seed_write` (arm-aware: sync L0 on Direct, routed on Worker) and the window
//! subscribes `sites_prefix(peer)`, so a write reflects in this window's list
//! and in the browser without any shared code — the tree is the only interface.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use entity_hash::Hash;

use crate::content_site::{discovery, humanize, paths, NavItem, SiteManifest, SitePage};
use crate::peers::Peers;
use crate::views::entity_tree::tree::{
    flatten_visible, insert_folder, insert_or_update, restore_expanded, TreeNode,
};

use super::output::{Notice, SelectedSite, SiteEditorOutput};
use super::validate::{self, site_health};

/// The landing page slug a new site is seeded with (the browser's default root).
const INDEX_PAGE: &str = "index";

struct Inner {
    /// The site currently open for editing (an owned site id), if any.
    selected_site: Option<String>,
    /// The page slug open in the editor (full slug from the site root). The tree
    /// shows a ✎ on this row so you can see which page is loaded even when the
    /// cursor has moved to a folder. Clicking a folder does NOT change this.
    selected_page: Option<String>,
    /// The single **tree cursor** — the one highlighted node (`""` = site root).
    /// There is only ever ONE highlight: clicking a page or a folder moves this
    /// cursor to it. `cursor_is_page` records which kind it is (so the add-target
    /// can be derived). The cursor drives the highlight; `selected_page` drives
    /// the editor — they coincide right after a page click and diverge once you
    /// click a folder.
    cursor: String,
    /// Is the cursor pointing at a page (vs. a folder / the root)? Used to derive
    /// the add-target: a folder/root cursor adds *into* itself, a page cursor adds
    /// into the page's parent directory.
    cursor_is_page: bool,
    /// Folder slugs currently expanded in the tree navigator. Session-only,
    /// never persisted — a pure UI affordance (which folders are open). A toggle
    /// changes only this set, so the handler must `mark_dirty` to rebuild.
    expanded: HashSet<String>,
    /// Folders created via "+ Add folder" that don't yet have a page under them.
    /// Directories are implicit in page paths (no entity), so an empty folder
    /// would otherwise be invisible — we seed these into the tree so a folder you
    /// just made shows up immediately. Session-only; once a page lands under one
    /// the page slug carries the folder anyway.
    pending_folders: HashSet<String>,
    /// Live-preview pane shown? Default off (focus mode = just the textarea).
    show_preview: bool,
    /// "Your sites" list region expanded? Default on.
    sites_open: bool,
    /// "New site" create card expanded? Default off — it opens on demand so the
    /// list isn't cluttered with input boxes.
    create_open: bool,
    /// Tree-navigator region expanded? Default on.
    pages_open: bool,
    /// Transient status from the last action (success or validation error).
    notice: Option<Notice>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            selected_site: None,
            selected_page: None,
            cursor: String::new(),
            cursor_is_page: false,
            expanded: HashSet::new(),
            pending_folders: HashSet::new(),
            show_preview: false,
            sites_open: true,
            create_open: false,
            pages_open: true,
            notice: None,
        }
    }
}

/// Join a directory cursor and a leaf name into a full slug (`""` + `about` →
/// `about`; `guide` + `intro` → `guide/intro`).
fn join_cwd(cwd: &str, name: &str) -> String {
    if cwd.is_empty() {
        name.to_string()
    } else {
        format!("{cwd}/{name}")
    }
}

/// The parent directory of a slug: `guide/advanced/intro` → `guide/advanced`;
/// `about` → `""` (root).
fn parent_dir(slug: &str) -> String {
    match slug.rsplit_once('/') {
        Some((parent, _)) => parent.to_string(),
        None => String::new(),
    }
}

/// The directory new pages/folders are added into, derived from the cursor: a
/// page cursor adds into the page's parent directory (a sibling); a folder /
/// root cursor adds into itself.
fn add_target(inner: &Inner) -> String {
    if inner.cursor_is_page {
        parent_dir(&inner.cursor)
    } else {
        inner.cursor.clone()
    }
}

/// The ancestor folder slugs of a page slug, e.g. `guide/advanced/intro` →
/// `["guide", "guide/advanced"]`. Used to auto-expand the path to the
/// selected page so it's always visible in the tree.
fn ancestor_slugs(slug: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc = String::new();
    let segs: Vec<&str> = slug.split('/').collect();
    // All but the last segment are ancestor folders.
    for seg in &segs[..segs.len().saturating_sub(1)] {
        acc = join_cwd(&acc, seg);
        out.push(acc.clone());
    }
    out
}

pub struct SiteEditorModel {
    inner: Arc<Mutex<Inner>>,
}

impl SiteEditorModel {
    pub fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(Inner::default())) }
    }

    // ---- actions (write / select) ----

    /// Create a new owned site: an atomic-as-we-can manifest + `index` page so
    /// the first observable state is **Renderable** (never a bare manifest).
    /// Validates the id and refuses a collision. On success, selects the new
    /// site at its index page. Writes the page first, then the manifest, so the
    /// moment the site becomes enumerable (manifest present) its root page
    /// already exists.
    pub fn create_site(&self, peers: &Peers, peer_id: &str, site_id: &str, title: &str) {
        let site_id = site_id.trim();
        if let Err(reason) = validate::validate_site_id(site_id) {
            self.set_notice(reason, true);
            return;
        }
        if peers.get_entity(peer_id, &paths::manifest_path(peer_id, site_id)).is_some() {
            self.set_notice(format!("A site '{site_id}' already exists."), true);
            return;
        }
        let title = {
            let t = title.trim();
            if t.is_empty() { site_id.to_string() } else { t.to_string() }
        };

        // Page first, then manifest (minimize the manifest-without-root window).
        let body = format!("# {title}\n\nWelcome to **{title}**.\n");
        peers.seed_write(
            peer_id,
            paths::page_path(peer_id, site_id, INDEX_PAGE),
            SitePage::markdown(&title, body).to_entity(),
        );
        // Nav is auto-derived from the site's top-level pages (see
        // `rebuild_nav`); a fresh site has only the root page, which the browser
        // already surfaces as the title/Home link, so the nav starts empty.
        let manifest = SiteManifest::new(site_id, &title, INDEX_PAGE, vec![]);
        peers.seed_write(peer_id, paths::manifest_path(peer_id, site_id), manifest.to_entity());

        {
            let mut inner = self.inner.lock().unwrap();
            inner.selected_site = Some(site_id.to_string());
            inner.selected_page = Some(INDEX_PAGE.to_string());
            inner.cursor = INDEX_PAGE.to_string();
            inner.cursor_is_page = true;
            inner.create_open = false; // collapse the create card; the new site is now open
            inner.expanded.clear();
            inner.pending_folders.clear();
            inner.notice = Some(Notice { text: format!("Created site '{site_id}'."), is_error: false });
        }
    }

    /// Select an existing owned site for editing; opens it at its manifest root.
    pub fn select_site(&self, peers: &Peers, peer_id: &str, site_id: &str) {
        let root = peers
            .get_entity(peer_id, &paths::manifest_path(peer_id, site_id))
            .map(|e| SiteManifest::from_entity(&e).root().to_string())
            .unwrap_or_else(|| INDEX_PAGE.to_string());
        let mut inner = self.inner.lock().unwrap();
        inner.selected_site = Some(site_id.to_string());
        // Fresh expansion / pending-folder state for the newly-opened site, then
        // open the path to its root page (a no-op for a top-level root).
        inner.expanded.clear();
        inner.pending_folders.clear();
        expand_ancestors(&mut inner, &root);
        inner.cursor = root.clone();
        inner.cursor_is_page = true;
        inner.selected_page = Some(root);
        inner.notice = None;
    }

    /// Select a page within the current site for editing — moves the cursor to
    /// it (the single tree highlight) and loads it. Expands the folders on the
    /// way so the page is visible.
    pub fn select_page(&self, page: &str) {
        let mut inner = self.inner.lock().unwrap();
        expand_ancestors(&mut inner, page);
        inner.cursor = page.to_string();
        inner.cursor_is_page = true;
        inner.selected_page = Some(page.to_string());
        inner.notice = None;
    }

    /// Move the cursor to a folder (or the root, `""`) — the single tree
    /// highlight. This makes it the add-target but does NOT change the page
    /// loaded in the editor (you can click around folders without losing your
    /// place). Expands the folder so its contents show.
    pub fn cd(&self, slug: &str) {
        let mut inner = self.inner.lock().unwrap();
        let slug = slug.trim_matches('/').to_string();
        if !slug.is_empty() {
            inner.expanded.insert(slug.clone());
        }
        inner.cursor = slug;
        inner.cursor_is_page = false;
        inner.notice = None;
    }

    /// Toggle a folder node open/closed in the tree navigator. Session-only UI
    /// state — the handler `mark_dirty`s so the tree rebuilds.
    pub fn toggle_node(&self, slug: &str) {
        let slug = slug.trim_matches('/').to_string();
        if slug.is_empty() {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        if !inner.expanded.remove(&slug) {
            inner.expanded.insert(slug);
        }
    }

    /// "New folder" — set the add-target into a new sub-directory of the current
    /// one and expand it. Directories are implicit (no entity); the folder
    /// becomes real once a page is added under it, so this just points the
    /// add-target there and hints as much.
    pub fn add_dir(&self, name: &str) {
        let name = name.trim();
        if let Err(reason) = validate::validate_page_slug(name) {
            return self.set_notice(reason, true);
        }
        let mut inner = self.inner.lock().unwrap();
        let target = join_cwd(&add_target(&inner), name);
        // Seed the empty folder so it shows in the tree right away, expand it
        // AND its ancestors (a multi-segment name like `a/b` creates `a` too —
        // without expanding `a` the new folder would be hidden), and move the
        // cursor onto it (it's now the add-target).
        inner.pending_folders.insert(target.clone());
        inner.expanded.insert(target.clone());
        expand_ancestors(&mut inner, &target);
        inner.cursor = target;
        inner.cursor_is_page = false;
        inner.notice = Some(Notice {
            text: format!("Folder '{name}' — add a page here to keep it."),
            is_error: false,
        });
    }

    /// Toggle the live-preview pane.
    pub fn toggle_preview(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.show_preview = !inner.show_preview;
    }

    /// Toggle the "Your sites" list region.
    pub fn toggle_sites(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.sites_open = !inner.sites_open;
    }

    /// Toggle the "New site" create card.
    pub fn toggle_create(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.create_open = !inner.create_open;
    }

    /// Toggle the tree-navigator region.
    pub fn toggle_pages(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.pages_open = !inner.pages_open;
    }

    /// Save the editor buffer as the selected page's markdown body + title. A
    /// non-empty `title` sets the page's frontmatter title; an empty one keeps
    /// the existing title (or derives a readable one from the slug for a new
    /// page). The title feeds breadcrumbs, the `<title>`, and — for a top-level
    /// page — its auto-derived nav label. No-op with a notice if nothing is
    /// selected.
    pub fn save_page(&self, peers: &Peers, peer_id: &str, title: &str, body: &str) {
        let (site, page) = {
            let inner = self.inner.lock().unwrap();
            match (inner.selected_site.clone(), inner.selected_page.clone()) {
                (Some(s), Some(p)) => (s, p),
                _ => {
                    drop(inner);
                    self.set_notice("Select a site and page first.".to_string(), true);
                    return;
                }
            }
        };
        let path = paths::page_path(peer_id, &site, &page);
        let existing = peers.get_entity(peer_id, &path);
        // Hash of the body we're about to supersede — reclaimed after the write.
        let old_hash = existing.as_ref().map(entity_content_hash);
        let mut sp = existing.map(|e| SitePage::from_entity(&e)).unwrap_or_default();
        // A provided title wins; otherwise keep the existing one, deriving a
        // readable fallback from the slug when there's none yet.
        let title = title.trim();
        if !title.is_empty() {
            sp.frontmatter.insert("title".to_string(), title.to_string());
        } else if sp.title().is_empty() {
            let leaf = page.rsplit('/').next().unwrap_or(&page);
            sp.frontmatter.insert("title".to_string(), humanize(leaf));
        }
        sp.format = "markdown".to_string();
        sp.body = body.to_string();
        peers.seed_write(peer_id, &path, sp.to_entity());
        // GC: reclaim the now-superseded body blob (binding-safe — a no-op if
        // the save was identical/deduped, or the blob is still bound elsewhere).
        reclaim_blob(peers, peer_id, old_hash);
        // A save can change a top-level page's title → its nav label; keep the
        // browser nav in sync (a no-op when the derived nav is unchanged).
        self.rebuild_nav(peers, peer_id, &site);
        self.set_notice(format!("Saved '{page}'."), false);
    }

    /// Add a new (empty) page in the **add-target directory** (derived from the
    /// cursor). `name` is a leaf (or a relative path) joined onto it. Validates
    /// the resulting slug and refuses to clobber an existing page; selects the
    /// new page on success.
    pub fn add_page(&self, peers: &Peers, peer_id: &str, name: &str) {
        let (site, slug) = {
            let inner = self.inner.lock().unwrap();
            match inner.selected_site.clone() {
                Some(s) => (s, join_cwd(&add_target(&inner), name.trim())),
                None => {
                    drop(inner);
                    return self.set_notice("Select a site first.".into(), true);
                }
            }
        };
        if let Err(reason) = validate::validate_page_slug(&slug) {
            return self.set_notice(reason, true);
        }
        let path = paths::page_path(peer_id, &site, &slug);
        if peers.get_entity(peer_id, &path).is_some() {
            return self.set_notice(format!("A page '{slug}' already exists."), true);
        }
        let title = humanize(slug.rsplit('/').next().unwrap_or(&slug));
        let body = format!("# {title}\n\n");
        peers.seed_write(peer_id, path, SitePage::markdown(&title, body).to_entity());
        // Keep the browser nav in sync with the top-level page set.
        self.rebuild_nav(peers, peer_id, &site);
        {
            let mut inner = self.inner.lock().unwrap();
            expand_ancestors(&mut inner, &slug);
            inner.cursor = slug.clone();
            inner.cursor_is_page = true;
            inner.selected_page = Some(slug.clone());
            inner.notice = Some(Notice { text: format!("Added page '{slug}'."), is_error: false });
        }
    }

    /// Delete a page from the current site (and reclaim its blob). If it was the
    /// selected page, falls back to the manifest root. Deleting the root page is
    /// allowed — the D13 health line then warns the site won't render until it's
    /// recreated (the browser shows an honest not-found; nothing breaks).
    pub fn delete_page(&self, peers: &Peers, peer_id: &str, slug: &str) {
        let site = match self.inner.lock().unwrap().selected_site.clone() {
            Some(s) => s,
            None => return,
        };
        reclaim_path(peers, peer_id, &paths::page_path(peer_id, &site, slug));
        // Keep the browser nav in sync with the top-level page set.
        self.rebuild_nav(peers, peer_id, &site);
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.selected_page.as_deref() == Some(slug) {
                let root = peers
                    .get_entity(peer_id, &paths::manifest_path(peer_id, &site))
                    .map(|e| SiteManifest::from_entity(&e).root().to_string());
                // Move the cursor with the editor to the fallback root page.
                inner.cursor = root.clone().unwrap_or_default();
                inner.cursor_is_page = root.is_some();
                inner.selected_page = root;
            }
            inner.notice = Some(Notice { text: format!("Deleted page '{slug}'."), is_error: false });
        }
    }

    /// Delete an entire site — the whole `/{me}/sites/{site}/…` subgraph
    /// (manifest + every page + every asset) — reclaiming each blob. Leaves no
    /// orphaned tree entities behind (D9). Clears the selection if it was this
    /// site.
    pub fn delete_site(&self, peers: &Peers, peer_id: &str, site_id: &str) {
        let prefix = paths::site_prefix(peer_id, site_id);
        let paths_to_remove: Vec<String> =
            peers.tree_listing(peer_id, &prefix).into_iter().map(|e| e.path).collect();
        for path in paths_to_remove {
            reclaim_path(peers, peer_id, &path);
        }
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.selected_site.as_deref() == Some(site_id) {
                inner.selected_site = None;
                inner.selected_page = None;
            }
            inner.notice = Some(Notice { text: format!("Deleted site '{site_id}'."), is_error: false });
        }
    }

    /// Re-derive the site's `manifest.nav` from its **top-level** pages and
    /// folders and write it back if it changed. The site title already doubles
    /// as the browser's Home link (→ the root page), so the derived nav lists
    /// the *other* top-level entries — each a root-absolute `/{slug}` link (the
    /// app-generated-link convention). This is what makes a freshly authored
    /// site's browser nav bar populate as pages are added, with zero UI: the
    /// (frozen) browser renders whatever `manifest.nav` says.
    ///
    /// Idempotent: a no-op (no write, no churn) when the derived nav already
    /// matches. On a real change it supersedes the manifest and reclaims the old
    /// manifest blob (D9), Direct/IDB arm.
    fn rebuild_nav(&self, peers: &Peers, peer_id: &str, site_id: &str) {
        let path = paths::manifest_path(peer_id, site_id);
        let Some(existing) = peers.get_entity(peer_id, &path) else {
            return; // No manifest (e.g. mid-delete) → nothing to keep in sync.
        };
        let mut manifest = SiteManifest::from_entity(&existing);
        let nav = build_nav(peers, peer_id, site_id, manifest.root());
        if nav == manifest.nav {
            return;
        }
        let old_hash = entity_content_hash(&existing);
        manifest.nav = nav;
        peers.seed_write(peer_id, &path, manifest.to_entity());
        reclaim_blob(peers, peer_id, Some(old_hash));
    }

    /// Rename / move a page to a new slug — the same content at a new path,
    /// which is what lets the author reshape the folder structure (move a page
    /// into or out of a folder, or just rename it). Validates the target,
    /// refuses to clobber an existing page, repoints the manifest root if the
    /// moved page WAS the root (so the site keeps rendering), refreshes the nav,
    /// and re-points the selection. No-op (with a notice) if the source page is
    /// gone or the target is unchanged.
    pub fn rename_page(&self, peers: &Peers, peer_id: &str, from: &str, to: &str) {
        let site = match self.inner.lock().unwrap().selected_site.clone() {
            Some(s) => s,
            None => return self.set_notice("Select a site first.".into(), true),
        };
        let to = to.trim().trim_matches('/').to_string();
        if let Err(reason) = validate::validate_page_slug(&to) {
            return self.set_notice(reason, true);
        }
        if to == from {
            return self.set_notice("New path is the same as the current one.".into(), true);
        }
        let from_path = paths::page_path(peer_id, &site, from);
        let entity = match peers.get_entity(peer_id, &from_path) {
            Some(e) => e,
            None => return self.set_notice(format!("No page '{from}' to move."), true),
        };
        let to_path = paths::page_path(peer_id, &site, &to);
        if peers.get_entity(peer_id, &to_path).is_some() {
            return self.set_notice(format!("A page '{to}' already exists."), true);
        }

        // Write the same content at the new path, then remove the old path. The
        // body blob is now bound at the new path, so reclaiming the old path's
        // hash is a binding-safe no-op (it's still referenced) — exactly what
        // `content_remove_if_unbound` guarantees.
        peers.seed_write(peer_id, &to_path, entity);
        reclaim_path(peers, peer_id, &from_path);

        // If the moved page was the manifest root, repoint it so the site still
        // renders; fold that together with the nav refresh in one manifest write.
        let mpath = paths::manifest_path(peer_id, &site);
        if let Some(existing) = peers.get_entity(peer_id, &mpath) {
            let mut manifest = SiteManifest::from_entity(&existing);
            let mut changed = false;
            if manifest.root() == from {
                manifest.params.insert("root".to_string(), to.clone());
                changed = true;
            }
            let nav = build_nav(peers, peer_id, &site, manifest.root());
            if nav != manifest.nav {
                manifest.nav = nav;
                changed = true;
            }
            if changed {
                let old_hash = entity_content_hash(&existing);
                peers.seed_write(peer_id, &mpath, manifest.to_entity());
                reclaim_blob(peers, peer_id, Some(old_hash));
            }
        }

        {
            let mut inner = self.inner.lock().unwrap();
            // Open the path to the moved page so it stays visible in the tree.
            expand_ancestors(&mut inner, &to);
            if inner.selected_page.as_deref() == Some(from) {
                inner.selected_page = Some(to.clone());
                inner.cursor = to.clone();
                inner.cursor_is_page = true;
            }
            inner.notice = Some(Notice { text: format!("Moved '{from}' → '{to}'."), is_error: false });
        }
    }

    fn set_notice(&self, text: String, is_error: bool) {
        self.inner.lock().unwrap().notice = Some(Notice { text, is_error });
    }

    // ---- read ----

    /// Build the render output for the bound peer.
    pub fn render_output(&self, peers: &Peers, peer_id: &str) -> SiteEditorOutput {
        use super::output::SiteListItem;
        use super::validate::SiteHealth;

        let site_ids = discovery::list_sites(peers, peer_id);
        let inner = self.inner.lock().unwrap();

        // Each owned site carries its render health as a per-row indicator.
        let sites: Vec<SiteListItem> = site_ids
            .iter()
            .map(|id| match site_health(peers, peer_id, id) {
                SiteHealth::Renderable => {
                    SiteListItem { id: id.clone(), renderable: true, reason: String::new() }
                }
                SiteHealth::NotRenderable(reason) => {
                    SiteListItem { id: id.clone(), renderable: false, reason }
                }
            })
            .collect();

        // Only surface a selection that still exists (a deleted site clears it).
        let selected = inner
            .selected_site
            .as_ref()
            .filter(|s| site_ids.contains(s))
            .map(|site| {
                // Build the site's page tree with the SAME shared tree component
                // the Entity Tree inspector uses (`entity_tree::tree`): seed every
                // page slug + any empty "pending" folders, restore the open-folder
                // set, then flatten to the visible rows. So the whole structure is
                // visible at once with proper expand/collapse, not one level at a
                // time. (auto_expand_below=0 → the `expanded` set is the single
                // source of truth for what's open.)
                let mut root = TreeNode::new_root();
                for slug in all_page_slugs(peers, peer_id, site) {
                    insert_or_update(&mut root, &slug, 0);
                }
                for folder in &inner.pending_folders {
                    insert_folder(&mut root, folder, 0);
                }
                restore_expanded(&mut root, &inner.expanded);
                let rows = flatten_visible(&root);

                let selected_page = inner.selected_page.clone();
                let page = selected_page
                    .as_ref()
                    .and_then(|p| peers.get_entity(peer_id, &paths::page_path(peer_id, site, p)))
                    .map(|e| SitePage::from_entity(&e));
                let page_title = page.as_ref().map(|p| p.title().to_string()).unwrap_or_default();
                let page_body = page.map(|p| p.body).unwrap_or_default();
                SelectedSite {
                    site_id: site.clone(),
                    pages_open: inner.pages_open,
                    rows,
                    cursor: inner.cursor.clone(),
                    add_target: add_target(&inner),
                    selected_page,
                    page_title,
                    page_body,
                    show_preview: inner.show_preview,
                }
            });

        SiteEditorOutput {
            sites,
            sites_open: inner.sites_open,
            create_open: inner.create_open,
            selected,
            notice: inner.notice.clone(),
        }
    }
}

impl Default for SiteEditorModel {
    fn default() -> Self {
        Self::new()
    }
}

/// Insert the ancestor folders of `slug` into the expanded set, so the path to
/// a selected/added/moved page is open and the page is visible in the tree.
fn expand_ancestors(inner: &mut Inner, slug: &str) {
    for a in ancestor_slugs(slug) {
        inner.expanded.insert(a);
    }
}

/// Every page slug in a site (full slugs from the site root), read from the
/// local/cached tree — a body-free index scan under the pages prefix. The whole
/// set at once (not one level) is what lets us build the full nested tree.
/// Arm-aware via [`Peers::tree_listing`]; on the Worker arm the cache mirror
/// only feeds the subscribed `sites/` prefix (which this window observes).
fn all_page_slugs(peers: &Peers, peer_id: &str, site_id: &str) -> Vec<String> {
    let prefix = paths::pages_prefix(peer_id, site_id);
    peers
        .tree_listing(peer_id, &prefix)
        .into_iter()
        .filter_map(|e| e.path.strip_prefix(&prefix).map(|s| s.trim_start_matches('/').to_string()))
        .filter(|s| !s.is_empty())
        .collect()
}

/// The content-store key for an entity — `Hash::compute(type, data)`, the same
/// derivation `content_remove_if_unbound` checks against the location index.
fn entity_content_hash(e: &entity_entity::Entity) -> Hash {
    Hash::compute(&e.entity_type, &e.data)
}

/// Derive the manifest nav menu from a site's **top-level** structure: one
/// root-absolute `/{name}` entry per immediate child of the site root, in the
/// navigator's sorted order, **excluding the root page** (the browser already
/// surfaces the root as the title/Home link). A leaf page's label is its page
/// title; a folder's (or a page-that-is-also-a-section's) label is the
/// humanized segment, and `/{name}` resolves to the browser's generated
/// section-index. A site with only its root page derives an empty nav.
fn build_nav(peers: &Peers, peer_id: &str, site_id: &str, root: &str) -> Vec<NavItem> {
    discovery::list_child_pages(peers, peer_id, site_id, "")
        .into_iter()
        .filter(|c| c.name != root)
        .map(|c| {
            let label = if c.is_page && !c.is_section {
                page_title(peers, peer_id, site_id, &c.name)
            } else {
                humanize(&c.name)
            };
            NavItem::new(label, format!("/{}", c.name))
        })
        .collect()
}

/// The human title of a top-level page: its frontmatter `title`, falling back
/// to the humanized slug when unset.
fn page_title(peers: &Peers, peer_id: &str, site_id: &str, slug: &str) -> String {
    peers
        .get_entity(peer_id, &paths::page_path(peer_id, site_id, slug))
        .map(|e| SitePage::from_entity(&e))
        .map(|p| if p.title().is_empty() { humanize(slug) } else { p.title().to_string() })
        .unwrap_or_else(|| humanize(slug))
}

/// Reclaim a superseded/removed content blob by hash, binding-safe. `None` (no
/// prior entity) is a no-op. Direct/IDB arm only (Worker is a no-op — kernel GC
/// reclaims there); our target arm is Direct, so this is where the saving lands.
fn reclaim_blob(peers: &Peers, peer_id: &str, hash: Option<Hash>) {
    if let (Some(h), Some(handle)) = (hash, peers.writer_handle_for(peer_id)) {
        handle.content_remove(h);
    }
}

/// Remove a tree path and reclaim the blob it bound: read the entity (for its
/// hash) → `dispatch_remove` (arm-aware) → `content_remove` the now-unbound
/// blob. The "clean up after yourself" primitive behind page/site delete.
fn reclaim_path(peers: &Peers, peer_id: &str, path: &str) {
    let old_hash = peers.get_entity(peer_id, path).as_ref().map(entity_content_hash);
    peers.dispatch_remove(peer_id, path);
    reclaim_blob(peers, peer_id, old_hash);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::site_editor::validate::SiteHealth;

    #[test]
    fn lists_owned_sites_present_in_my_store() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        assert!(SiteEditorModel::new().render_output(&peers, &me).sites.is_empty());

        peers.seed_write(
            &me,
            paths::manifest_path(&me, "mysite"),
            SiteManifest::new("mysite", "My Site", "index", vec![]).to_entity(),
        );
        let out = SiteEditorModel::new().render_output(&peers, &me);
        assert_eq!(out.sites.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["mysite"]);
    }

    #[test]
    fn create_site_writes_a_renderable_site_and_selects_it() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();

        m.create_site(&peers, &me, "blog", "My Blog");

        let out = m.render_output(&peers, &me);
        assert!(
            out.sites.iter().any(|s| s.id == "blog" && s.renderable),
            "new site listed + flagged renderable"
        );
        assert!(out.notice.as_ref().is_some_and(|n| !n.is_error), "success notice");
        assert!(!out.create_open, "the create card collapses after a successful create");
        let sel = out.selected.expect("the new site is selected");
        assert_eq!(sel.site_id, "blog");
        assert_eq!(sel.selected_page.as_deref(), Some("index"));
        assert!(sel.page_body.contains("My Blog"), "index body seeded: {}", sel.page_body);
        assert!(
            sel.rows.iter().any(|r| r.path == "index" && r.has_entry && !r.has_children),
            "the index page shows at the top of the tree: {:?}",
            sel.rows.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn create_rejects_bad_id_and_collision_without_writing() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();

        // Invalid id → error notice, nothing written.
        m.create_site(&peers, &me, "bad id", "T");
        assert!(m.render_output(&peers, &me).notice.unwrap().is_error);
        assert!(peers.get_entity(&me, &paths::manifest_path(&me, "bad id")).is_none());

        // Create, then a colliding create → error, original untouched.
        m.create_site(&peers, &me, "site", "First");
        m.create_site(&peers, &me, "site", "Second");
        let out = m.render_output(&peers, &me);
        assert!(out.notice.unwrap().is_error, "collision is reported");
        let mani = SiteManifest::from_entity(
            &peers.get_entity(&me, &paths::manifest_path(&me, "site")).unwrap(),
        );
        assert_eq!(mani.title, "First", "the colliding create did not overwrite");
    }

    #[test]
    fn save_page_overwrites_body_and_keeps_title() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");

        m.save_page(&peers, &me, "", "# Changed\n\nNew content.");
        let page = SitePage::from_entity(
            &peers.get_entity(&me, &paths::page_path(&me, "s", "index")).unwrap(),
        );
        assert!(page.body.contains("New content"), "body saved");
        assert_eq!(page.title(), "S", "an empty title preserves the seeded title");
    }

    #[test]
    fn save_page_sets_a_provided_title_and_updates_the_nav_label() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");
        m.add_page(&peers, &me, "about");

        // Save the top-level 'about' page with an explicit title.
        m.save_page(&peers, &me, "  About Us  ", "# About\n\nHi.");
        let page = SitePage::from_entity(
            &peers.get_entity(&me, &paths::page_path(&me, "s", "about")).unwrap(),
        );
        assert_eq!(page.title(), "About Us", "the provided title is set (trimmed)");
        // The derived nav label tracks the new title.
        let nav = SiteManifest::from_entity(
            &peers.get_entity(&me, &paths::manifest_path(&me, "s")).unwrap(),
        )
        .nav;
        assert!(
            nav.iter().any(|n| n.label == "About Us" && n.target == "/about"),
            "nav label follows the saved title: {nav:?}"
        );
    }

    #[test]
    fn save_reclaims_the_superseded_body_blob() {
        // GC: explicit-save churn must not leak orphan blobs. After two distinct
        // saves the content-store count is stable — the prior body is reclaimed.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");

        m.save_page(&peers, &me, "", "# Version A");
        let after_a = peers.entity_count(&me);
        m.save_page(&peers, &me, "", "# Version B");
        let after_b = peers.entity_count(&me);
        assert_eq!(after_b, after_a, "the superseded body blob is reclaimed (no growth)");
    }

    #[test]
    fn add_page_creates_selects_and_refuses_dupes_and_bad_slugs() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");

        m.add_page(&peers, &me, "about");
        assert!(peers.get_entity(&me, &paths::page_path(&me, "s", "about")).is_some());
        let out = m.render_output(&peers, &me);
        let sel = out.selected.unwrap();
        assert_eq!(sel.selected_page.as_deref(), Some("about"), "new page selected");
        assert!(sel.rows.iter().any(|r| r.path == "about" && r.has_entry && !r.has_children));

        // Duplicate + invalid slug both refused with an error notice.
        m.add_page(&peers, &me, "about");
        assert!(m.render_output(&peers, &me).notice.unwrap().is_error, "dup refused");
        m.add_page(&peers, &me, "../escape");
        assert!(m.render_output(&peers, &me).notice.unwrap().is_error, "bad slug refused");
    }

    // Find the visible row at a given slug, if present.
    fn row<'a>(
        rows: &'a [crate::views::entity_tree::tree::VisibleRow],
        path: &str,
    ) -> Option<&'a crate::views::entity_tree::tree::VisibleRow> {
        rows.iter().find(|r| r.path == path)
    }

    #[test]
    fn one_cursor_clicking_a_folder_keeps_the_loaded_page() {
        // The tree has exactly ONE highlight (the cursor). Clicking a folder
        // moves the cursor to it but does NOT change the page in the editor —
        // so there are never two "selected" things at once.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");
        m.add_page(&peers, &me, "about");
        m.add_dir("guide");

        // Click the page → cursor + editor both on it.
        m.select_page("about");
        let sel = m.render_output(&peers, &me).selected.unwrap();
        assert_eq!(sel.cursor, "about", "cursor on the page");
        assert_eq!(sel.selected_page.as_deref(), Some("about"));

        // Click the folder → cursor moves to it; the editor stays on the page.
        m.cd("guide");
        let sel = m.render_output(&peers, &me).selected.unwrap();
        assert_eq!(sel.cursor, "guide", "the one highlight is now the folder");
        assert_eq!(sel.selected_page.as_deref(), Some("about"), "editor unchanged");
        assert_eq!(sel.add_target, "guide", "folder is the add-target");
    }

    #[test]
    fn tree_view_shows_the_whole_nested_structure_with_expand_collapse() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");

        // New folder 'guide' (add-target moves in) → add a page there.
        m.add_dir("guide");
        m.add_page(&peers, &me, "intro");
        assert!(
            peers.get_entity(&me, &paths::page_path(&me, "s", "guide/intro")).is_some(),
            "page written at the nested slug"
        );

        // The whole tree flattens to visible rows: 'guide' (folder, sorted
        // first), its nested 'guide/intro' page (adding it auto-expanded guide),
        // and the root 'index' page.
        let sel = m.render_output(&peers, &me).selected.unwrap();
        let paths: Vec<&str> = sel.rows.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, vec!["guide", "guide/intro", "index"], "depth-first visible rows");
        let guide = row(&sel.rows, "guide").unwrap();
        assert!(guide.has_children && !guide.has_entry, "guide is a folder, not a page");
        assert!(guide.expanded, "the added page's folder is expanded");
        assert_eq!(guide.depth, 0);
        let intro = row(&sel.rows, "guide/intro").unwrap();
        assert!(intro.has_entry && !intro.has_children && intro.depth == 1, "nested page row");

        // Collapse 'guide' → its descendants drop out of the visible rows.
        m.toggle_node("guide");
        let sel = m.render_output(&peers, &me).selected.unwrap();
        assert!(!row(&sel.rows, "guide").unwrap().expanded, "collapsed");
        assert!(row(&sel.rows, "guide/intro").is_none(), "child hidden when collapsed");

        // Toggle again → expanded once more, child visible.
        m.toggle_node("guide");
        let sel = m.render_output(&peers, &me).selected.unwrap();
        assert!(row(&sel.rows, "guide").unwrap().expanded, "re-expanded");
        assert!(row(&sel.rows, "guide/intro").is_some(), "child visible again");
    }

    #[test]
    fn add_dir_with_a_nested_name_expands_the_whole_path() {
        // '+ Add folder' with a multi-segment name (e.g. a/b) creates the
        // intermediate folder too — both levels must be visible, not hidden
        // behind a collapsed ancestor.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");

        m.add_dir("docs/api");
        let sel = m.render_output(&peers, &me).selected.unwrap();
        let docs = row(&sel.rows, "docs").expect("intermediate folder visible");
        assert!(docs.has_children && docs.expanded, "ancestor expanded so its child shows");
        assert!(row(&sel.rows, "docs/api").is_some(), "the nested folder is visible");
        assert_eq!(sel.cursor, "docs/api", "cursor lands on the new folder");
    }

    #[test]
    fn an_added_empty_folder_shows_in_the_tree() {
        // The bug this guards: '+ Add folder' creates no entity (folders are
        // implicit), so a freshly-made empty folder must still appear in the
        // tree (seeded as a pending folder), or it looks like nothing happened.
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");

        m.add_dir("guide");
        let sel = m.render_output(&peers, &me).selected.unwrap();
        let guide = row(&sel.rows, "guide").expect("empty folder shows immediately");
        assert!(!guide.has_entry, "it's a folder, not a page");
        assert!(!guide.has_children, "still empty");
        assert_eq!(sel.add_target, "guide", "and it's the add-target");

        // Add a page into it → it becomes a real folder with a child.
        m.add_page(&peers, &me, "intro");
        let sel = m.render_output(&peers, &me).selected.unwrap();
        assert!(row(&sel.rows, "guide").unwrap().has_children, "now has a child");
        assert!(row(&sel.rows, "guide/intro").unwrap().has_entry, "the page is under it");
    }

    #[test]
    fn deep_nesting_builds_and_clicking_a_folder_sets_the_add_target() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");
        // A three-level page: guide/advanced/internals.
        m.add_page(&peers, &me, "guide/advanced/internals");

        let sel = m.render_output(&peers, &me).selected.unwrap();
        let guide = row(&sel.rows, "guide").unwrap();
        assert!(guide.has_children && !guide.has_entry && guide.depth == 0);
        let advanced = row(&sel.rows, "guide/advanced").unwrap();
        assert!(advanced.has_children && advanced.depth == 1);
        let internals = row(&sel.rows, "guide/advanced/internals").unwrap();
        assert!(internals.has_entry && internals.depth == 2);
        // Adding the deep page expanded the whole path to it.
        assert!(guide.expanded && advanced.expanded, "ancestors auto-expanded");

        // Clicking a folder sets it as the add-target (where + Add page lands).
        m.cd("guide/advanced");
        assert_eq!(m.render_output(&peers, &me).selected.unwrap().add_target, "guide/advanced");
        m.add_page(&peers, &me, "tips");
        assert!(
            peers.get_entity(&me, &paths::page_path(&me, "s", "guide/advanced/tips")).is_some(),
            "the new page landed under the clicked folder"
        );
    }

    #[test]
    fn nav_is_auto_derived_from_top_level_pages() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        let read_nav = || {
            SiteManifest::from_entity(
                &peers.get_entity(&me, &paths::manifest_path(&me, "s")).unwrap(),
            )
            .nav
        };

        // Fresh site: only the root page → nav is empty (title doubles as Home).
        m.create_site(&peers, &me, "s", "S");
        assert!(read_nav().is_empty(), "a one-page site derives an empty nav");

        // Add a top-level page → it appears in the nav as a root-absolute link,
        // labelled by its title.
        m.add_page(&peers, &me, "about");
        let nav = read_nav();
        assert_eq!(nav.len(), 1, "the about page is in the nav: {nav:?}");
        assert_eq!(nav[0].label, "About");
        assert_eq!(nav[0].target, "/about");

        // Add a nested page (under a folder) → the FOLDER shows top-level, not
        // the nested page; the root page ('index') never appears.
        m.cd("");
        m.add_dir("guide");
        m.add_page(&peers, &me, "intro");
        let nav = read_nav();
        let labels: Vec<&str> = nav.iter().map(|n| n.label.as_str()).collect();
        assert!(labels.contains(&"About"));
        assert!(labels.contains(&"Guide"), "folder shows in nav: {nav:?}");
        assert!(nav.iter().all(|n| n.target != "/index"), "root page is not duplicated in nav");
        assert!(nav.iter().any(|n| n.target == "/guide"), "folder links to its section index");

        // Delete the top-level page → it leaves the nav.
        m.delete_page(&peers, &me, "about");
        assert!(
            read_nav().iter().all(|n| n.target != "/about"),
            "a deleted top-level page leaves the nav"
        );
    }

    #[test]
    fn toggles_flip_ui_state() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");

        let out = m.render_output(&peers, &me);
        assert!(out.sites_open, "sites open by default");
        assert!(!out.create_open, "create card closed by default");
        let sel = out.selected.unwrap();
        assert!(sel.pages_open && !sel.show_preview, "pages open, preview off by default");

        m.toggle_preview();
        m.toggle_pages();
        m.toggle_sites();
        m.toggle_create();
        let out = m.render_output(&peers, &me);
        assert!(!out.sites_open);
        assert!(out.create_open, "create card toggled open");
        let sel = out.selected.unwrap();
        assert!(!sel.pages_open && sel.show_preview, "focus-mode toggles applied");
    }

    #[test]
    fn rename_page_moves_content_updates_nav_and_selection() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");
        m.add_page(&peers, &me, "about");
        m.save_page(&peers, &me, "About", "# About\n\nbody here");

        // Move 'about' into a folder: 'about' → 'company/about'. Content moves,
        // old path is gone, selection follows, nav reflects the new top-level.
        m.rename_page(&peers, &me, "about", "company/about");
        assert!(peers.get_entity(&me, &paths::page_path(&me, "s", "about")).is_none(), "old path gone");
        let moved = SitePage::from_entity(
            &peers.get_entity(&me, &paths::page_path(&me, "s", "company/about")).unwrap(),
        );
        assert!(moved.body.contains("body here"), "content moved intact");
        assert_eq!(moved.title(), "About", "title preserved");

        let out = m.render_output(&peers, &me);
        assert_eq!(out.selected.unwrap().selected_page.as_deref(), Some("company/about"), "selection follows");
        let nav = SiteManifest::from_entity(
            &peers.get_entity(&me, &paths::manifest_path(&me, "s")).unwrap(),
        )
        .nav;
        assert!(nav.iter().any(|n| n.target == "/company"), "the new folder shows in nav: {nav:?}");
        assert!(nav.iter().all(|n| n.target != "/about"), "old top-level page left the nav");
    }

    #[test]
    fn rename_refuses_collision_and_bad_target() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");
        m.add_page(&peers, &me, "about");

        // Collision with the existing root page is refused; original untouched.
        m.rename_page(&peers, &me, "about", "index");
        assert!(m.render_output(&peers, &me).notice.unwrap().is_error, "collision refused");
        assert!(peers.get_entity(&me, &paths::page_path(&me, "s", "about")).is_some(), "source untouched");

        // A path-unsafe target is refused.
        m.rename_page(&peers, &me, "about", "../escape");
        assert!(m.render_output(&peers, &me).notice.unwrap().is_error, "bad target refused");
    }

    #[test]
    fn renaming_the_root_page_repoints_the_manifest_so_the_site_still_renders() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");

        // Rename the root page 'index' → 'home'. The manifest root must follow,
        // or the site would stop rendering.
        m.rename_page(&peers, &me, "index", "home");
        let mani = SiteManifest::from_entity(
            &peers.get_entity(&me, &paths::manifest_path(&me, "s")).unwrap(),
        );
        assert_eq!(mani.root(), "home", "root repointed to the moved page");
        assert_eq!(site_health(&peers, &me, "s"), SiteHealth::Renderable, "site still renders");
    }

    #[test]
    fn delete_page_removes_it_and_reclaims() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");
        m.add_page(&peers, &me, "about");
        assert!(peers.get_entity(&me, &paths::page_path(&me, "s", "about")).is_some());

        m.delete_page(&peers, &me, "about");
        assert!(
            peers.get_entity(&me, &paths::page_path(&me, "s", "about")).is_none(),
            "page entity removed"
        );
        // Selection fell back to the manifest root.
        assert_eq!(
            m.render_output(&peers, &me).selected.unwrap().selected_page.as_deref(),
            Some("index")
        );
    }

    #[test]
    fn delete_site_removes_the_whole_subgraph() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        let m = SiteEditorModel::new();
        m.create_site(&peers, &me, "s", "S");
        m.add_page(&peers, &me, "guide/intro");

        m.delete_site(&peers, &me, "s");

        // No tree entity remains under the site subgraph (manifest + all pages).
        assert!(
            peers.tree_listing(&me, &paths::site_prefix(&me, "s")).is_empty(),
            "the entire site subgraph is gone"
        );
        let out = m.render_output(&peers, &me);
        assert!(out.sites.iter().all(|s| s.id != "s"), "site no longer listed");
        assert!(out.selected.is_none(), "selection cleared after deleting the open site");
    }

    /// The tree-only interface proof: the editor writes; the (frozen) Content
    /// Site model reads the SAME tree and renders it — no shared code.
    #[test]
    fn created_site_renders_through_the_content_site_model() {
        use crate::views::content_site::model::ContentSiteModel;

        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        SiteEditorModel::new().create_site(&peers, &me, "proof", "Proof Site");

        // Point the browser model at the just-created owned site and render it.
        let browser = ContentSiteModel::new(99, me.clone());
        browser.open_site("", "proof", &peers);
        let out = browser.render_output(&peers);

        assert!(out.error.is_none(), "browser renders the editor-created site: {:?}", out.error);
        assert_eq!(out.site_title, "Proof Site");
        assert_eq!(out.current_page, "index");
        assert!(out.body_html.contains("Proof Site"), "rendered body: {}", out.body_html);
    }
}
