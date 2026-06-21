//! Site Editor window — create and edit your own content sites (markdown).
//!
//! **Its own isolated app.** It writes the
//! existing site data model (`manifest`/`page`/`asset` at `/{peer}/sites/{site}/…`)
//! so a site it creates is picked up by the **frozen** Content Site browser for
//! free — the tree is the only interface between them. Additive & disableable:
//! drop the registration in `window_registry.rs` and the browser is untouched.
//!
//! Commit 1 is the empty shell: a read-only list of the sites I own on this
//! peer, proving the window registers without touching any read/browse/publish
//! path. Create / edit / delete + validation + GC land in Commit 2/3.

pub mod model;
pub mod output;
pub mod validate;

use crate::action::Action;
use crate::peers::Peers;
use crate::window::{WindowType, WindowView};
use crate::window_watch::WindowWatch;
use model::SiteEditorModel;

// `WindowEvent` names this window interprets (routed by window_id). The
// renderer emits these; `handle_action` dispatches them. Kept here so the
// renderer and the handler can't drift on the string.
/// Create a site. Value = `"{site_id}\n{title}"` (the two create-form fields).
pub const EV_CREATE: &str = "site_editor_create";
/// Select an existing owned site for editing. Value = the site id.
pub const EV_SELECT_SITE: &str = "site_editor_select_site";
/// Select a page within the current site. Value = the page slug.
pub const EV_SELECT_PAGE: &str = "site_editor_select_page";
/// Save the editor buffer to the selected page. Value = `"{title}\n{body}"`
/// (the title is a single line; the body — which may contain newlines — is
/// everything after the first newline).
pub const EV_SAVE_PAGE: &str = "site_editor_save_page";
/// Add a new page to the current site. Value = the page slug.
pub const EV_ADD_PAGE: &str = "site_editor_add_page";
/// Delete a page from the current site. Value = the page slug.
pub const EV_DELETE_PAGE: &str = "site_editor_delete_page";
/// Rename / move a page. Value = `"{from_slug}\n{to_slug}"`.
pub const EV_RENAME_PAGE: &str = "site_editor_rename_page";
/// Delete an entire site (whole subgraph). Value = the site id.
pub const EV_DELETE_SITE: &str = "site_editor_delete_site";
/// Set the add-target directory (click a folder). Value = the slug (`""` = root).
pub const EV_CD: &str = "site_editor_cd";
/// Toggle a folder open/closed in the tree navigator. Value = the folder slug.
pub const EV_TOGGLE_NODE: &str = "site_editor_toggle_node";
/// New folder: move the cursor into a new sub-directory. Value = the leaf name.
pub const EV_ADD_DIR: &str = "site_editor_add_dir";
/// Toggle the live-preview pane. Value unused.
pub const EV_TOGGLE_PREVIEW: &str = "site_editor_toggle_preview";
/// Toggle the "Your sites" list region. Value unused.
pub const EV_TOGGLE_SITES: &str = "site_editor_toggle_sites";
/// Toggle the "New site" create card. Value unused.
pub const EV_TOGGLE_CREATE: &str = "site_editor_toggle_create";
/// Toggle the tree-navigator region. Value unused.
pub const EV_TOGGLE_PAGES: &str = "site_editor_toggle_pages";

pub struct SiteEditorWindow {
    peer_id: String,
    model: SiteEditorModel,
    watch: WindowWatch,
}

impl SiteEditorWindow {
    pub fn new(peer_id: String) -> Self {
        Self {
            peer_id,
            model: SiteEditorModel::new(),
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Site Creator",
            description: "Create and edit your own content sites (markdown)",
            scope: crate::window::WindowScope::Peer,
            create: |_id, peer_id, pm| {
                let mut window = SiteEditorWindow::new(peer_id.to_string());
                // Reactive: the owned-site list rebuilds when this peer's
                // `sites/` subgraph changes — a site created/edited/deleted,
                // whether by a future editor save or by anything else writing
                // the tree. Subscribing the prefix is also what makes the
                // editor see its OWN writes reflect (the cross-window save→view
                // loop) on either arm — the worker-cache subscription rule.
                pm.watch_prefix(
                    &mut window.watch,
                    peer_id,
                    crate::content_site::paths::sites_prefix(peer_id),
                );
                Box::new(window)
            },
        }
    }
}

impl WindowView for SiteEditorWindow {
    fn title(&self) -> String {
        "Site Creator".into()
    }

    fn type_name(&self) -> &'static str {
        "Site Creator"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let Action::WindowEvent { event, value, .. } = action else { return };
        match event.as_str() {
            EV_CREATE => {
                // Value packs the two create-form fields as "{site_id}\n{title}".
                let mut parts = value.splitn(2, '\n');
                let site_id = parts.next().unwrap_or("");
                let title = parts.next().unwrap_or("");
                self.model.create_site(peers, &self.peer_id, site_id, title);
            }
            EV_SELECT_SITE => self.model.select_site(peers, &self.peer_id, value),
            EV_SELECT_PAGE => self.model.select_page(value),
            EV_SAVE_PAGE => {
                // Value packs "{title}\n{body}"; the title is the first line.
                let mut parts = value.splitn(2, '\n');
                let title = parts.next().unwrap_or("");
                let body = parts.next().unwrap_or("");
                self.model.save_page(peers, &self.peer_id, title, body);
            }
            EV_ADD_PAGE => self.model.add_page(peers, &self.peer_id, value),
            EV_DELETE_PAGE => self.model.delete_page(peers, &self.peer_id, value),
            EV_RENAME_PAGE => {
                // Value packs "{from}\n{to}".
                let mut parts = value.splitn(2, '\n');
                let from = parts.next().unwrap_or("");
                let to = parts.next().unwrap_or("");
                self.model.rename_page(peers, &self.peer_id, from, to);
            }
            EV_DELETE_SITE => self.model.delete_site(peers, &self.peer_id, value),
            EV_CD => self.model.cd(value),
            EV_TOGGLE_NODE => self.model.toggle_node(value),
            EV_ADD_DIR => self.model.add_dir(value),
            EV_TOGGLE_PREVIEW => self.model.toggle_preview(),
            EV_TOGGLE_SITES => self.model.toggle_sites(),
            EV_TOGGLE_CREATE => self.model.toggle_create(),
            EV_TOGGLE_PAGES => self.model.toggle_pages(),
            _ => return,
        }
        // The bug this fixes: selection / navigation / toggle actions change
        // only in-memory model state (no tree write), so the subscription never
        // fires and the section would never rebuild — clicks "didn't register."
        // Marking dirty here forces a rebuild that reflects the new state. The
        // tree-writing actions also fire the subscription (a redundant, harmless
        // extra dirty).
        self.watch.mark_dirty();
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output(peers, &self.peer_id);
        crate::dom::site_editor::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_type_is_peer_scoped() {
        let t = SiteEditorWindow::window_type();
        assert_eq!(t.name, "Site Creator");
        assert!(matches!(t.scope, crate::window::WindowScope::Peer));
    }
}
