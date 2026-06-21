//! Site Editor window DOM renderer — pure consumer of
//! [`SiteEditorOutput`](crate::views::site_editor::output::SiteEditorOutput).
//!
//! Layout: a collapsible "Your sites" list (each row a ✓/⚠ render-health glyph
//! + the site name) and a "New site" create card (its own expander), then (when
//! a site is selected) a slim header + delete, a collapsible **tree navigator**
//! (the site's nested folder/page tree, rendered as flat indented `VisibleRow`s
//! like the Entity Tree inspector, + one add page/folder row), and a markdown
//! editor with an unsaved-changes marker and a toggleable live preview. Collapse
//! the regions + hide preview → focus mode. All edits flow through
//! `WindowEvent`s; writes land in the tree and the Content Site browser picks
//! them up (the tree-only interface).

use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::action::Action;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::entity_tree::tree::VisibleRow;
use crate::views::site_editor::output::{SelectedSite, SiteEditorOutput, SiteListItem};
use crate::views::site_editor::{
    EV_ADD_DIR, EV_ADD_PAGE, EV_CD, EV_CREATE, EV_DELETE_PAGE, EV_DELETE_SITE, EV_RENAME_PAGE,
    EV_SAVE_PAGE, EV_SELECT_PAGE, EV_SELECT_SITE, EV_TOGGLE_CREATE, EV_TOGGLE_NODE,
    EV_TOGGLE_PAGES, EV_TOGGLE_PREVIEW, EV_TOGGLE_SITES,
};

const ROW: &str = "display:flex;flex-wrap:wrap;gap:6px;align-items:center;margin:6px 0";
const CHIP_ON: &str = "background:var(--accent,#3a6ea5);color:var(--accent-text,#fff);border:none;\
    border-radius:4px;padding:4px 12px;font-size:14px;cursor:pointer";
// Collapsible section header — styled as a clear bar so it reads as "expandable".
const HEADER_BTN: &str = "display:flex;align-items:center;gap:8px;width:100%;text-align:left;\
    background:var(--surface-sunken,#15152a);color:var(--text,#e0e0e0);border:1px solid \
    var(--border,#2a2a4e);border-radius:6px;padding:9px 12px;margin-top:12px;font-size:15px;\
    font-weight:bold;cursor:pointer";
const EDITOR_COLS: &str =
    "display:flex;flex-wrap:wrap;gap:10px;align-items:stretch;margin-top:6px";
const PANE: &str = "flex:1 1 280px;min-width:240px";
const TEXTAREA: &str = "display:block;width:100%;min-height:420px;box-sizing:border-box;\
    background:var(--input-bg,#0e0e1e);color:var(--text,#e0e0e0);border:1px solid \
    var(--border,#2a2a4e);border-radius:4px;padding:10px;font-family:monospace;font-size:15px;\
    line-height:1.5";
const PREVIEW: &str = "min-height:420px;background:var(--surface-sunken,#0a0a1a);\
    border:1px solid var(--border,#2a2a4e);border-radius:4px;padding:10px;overflow:auto";
const BTN_DANGER: &str = "background:transparent;color:var(--status-err,#f66);\
    border:1px solid var(--status-err,#f66);border-radius:4px;padding:5px 12px;\
    font-size:13px;cursor:pointer";
// Tree navigator rows — scaled up so the carets/labels read as real menu rows.
const TREE_ROW: &str = "display:flex;align-items:center;gap:4px;margin:2px 0";
const CARET: &str = "background:transparent;border:none;color:var(--text,#e0e0e0);\
    cursor:pointer;font-size:15px;width:22px;padding:0;line-height:1;flex:0 0 22px";
const NODE_BTN: &str = "flex:1 1 auto;text-align:left;background:transparent;\
    color:var(--text,#e0e0e0);border:1px solid transparent;border-radius:4px;\
    padding:5px 8px;font-size:14px;cursor:pointer;overflow:hidden;text-overflow:ellipsis";
// One shared "selected" highlight for BOTH the open page and the add-target
// folder — a clear blue (accent border + accent text), so they read the same.
const NODE_BTN_SELECTED: &str = "flex:1 1 auto;text-align:left;background:transparent;\
    color:var(--accent,#3a6ea5);border:2px solid var(--accent,#3a6ea5);\
    border-radius:4px;padding:4px 7px;font-size:14px;font-weight:600;cursor:pointer";

/// Native confirm dialog — guards the destructive deletes. Returns `false` when
/// unavailable (an automation context that suppresses prompts), so a missing
/// dialog fails safe (no delete) rather than deleting unconfirmed.
fn confirm(msg: &str) -> bool {
    web_sys::window().and_then(|w| w.confirm_with_message(msg).ok()).unwrap_or(false)
}

/// Push a `WindowEvent` after an optional confirm — the shared shape for the
/// value-carrying buttons that aren't a static `on_window_event`.
fn on_confirmed_event(ctx: &DomCtx, el: &Element, confirm_msg: Option<String>, event: &str, value: String) {
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let wid = ctx.window_id;
    let event = event.to_string();
    ctx.listen(el, "click", move |_| {
        if let Some(msg) = &confirm_msg {
            if !confirm(msg) {
                return;
            }
        }
        actions.borrow_mut().push(Action::WindowEvent {
            window_id: wid,
            event: event.clone(),
            value: value.clone(),
        });
        rp();
    });
}

/// A full-width clickable section header with a ▾/▸ caret that toggles `event`.
fn collapsible_header(ctx: &DomCtx, label: &str, open: bool, event: &str) -> Element {
    let h = util::create_element("button");
    h.set_attribute("style", HEADER_BTN).ok();
    util::set_text(&h, &format!("{} {label}", if open { "\u{25be}" } else { "\u{25b8}" }));
    ctx.on_window_event(&h, "click", event, "");
    h
}

pub fn render(container: &Element, output: &SiteEditorOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "site-editor");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", "margin:0").ok();
    util::set_text(&h2, "Site Creator");
    util::append(&wrapper, &h2);

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(
        &hint,
        "Build a site as a tree of folders and pages. Saves write to your \
         peer's tree; the Site Browser window picks them up automatically.",
    );
    util::append(&wrapper, &hint);

    if let Some(notice) = &output.notice {
        util::append(&wrapper, &notice_block(&notice.text, notice.is_error));
    }

    // "Your sites" (collapsible) — a clean list of sites, each with its render
    // health as a small ✓/⚠ next to the name.
    util::append(&wrapper, &collapsible_header(ctx, "Your sites", output.sites_open, EV_TOGGLE_SITES));
    if output.sites_open {
        util::append(&wrapper, &sites_block(output, ctx));
        // "New site" is its own expander → a tidy card on demand, not a row of
        // input boxes always sitting under the list.
        util::append(&wrapper, &collapsible_header(ctx, "New site", output.create_open, EV_TOGGLE_CREATE));
        if output.create_open {
            util::append(&wrapper, &create_block(ctx));
        }
    }

    if let Some(sel) = &output.selected {
        util::append(&wrapper, &editor_block(sel, ctx));
    }

    util::append(container, &wrapper);
}

fn notice_block(text: &str, is_error: bool) -> Element {
    let el = util::create_element("div");
    let color = if is_error { crate::theme_tokens::STATUS_ERR } else { crate::theme_tokens::STATUS_OK };
    el.set_attribute(
        "style",
        &format!("font-size:12px;margin:6px 0;padding:6px 8px;border-radius:4px;border:1px solid {color};color:{color}"),
    )
    .ok();
    util::set_text(&el, text);
    el
}

/// The owned-site list — one row per site: a render-health glyph (✓ renders /
/// ⚠ won't, with the reason as a tooltip) then the clickable name. The open
/// site is highlighted with the same blue used in the page tree.
fn sites_block(output: &SiteEditorOutput, ctx: &DomCtx) -> Element {
    let block = util::create_element("div");
    block.set_attribute("style", "margin:6px 0").ok();
    if output.sites.is_empty() {
        let empty = util::create_element("p");
        empty.set_attribute("style", theme::HINT).ok();
        util::set_text(&empty, "(none yet — use “New site” below)");
        util::append(&block, &empty);
        return block;
    }
    let current = output.selected.as_ref().map(|s| s.site_id.as_str());
    for site in &output.sites {
        util::append(&block, &site_row(site, current == Some(site.id.as_str()), ctx));
    }
    block
}

fn site_row(site: &SiteListItem, selected: bool, ctx: &DomCtx) -> Element {
    let row = util::create_element("div");
    row.set_attribute("style", "display:flex;align-items:center;gap:6px;margin:2px 0").ok();

    // Health glyph: ✓ renders / ⚠ won't (tooltip carries the reason).
    let health = util::create_element("span");
    let (glyph, color, tip) = if site.renderable {
        ("\u{2713}", crate::theme_tokens::STATUS_OK, "Renders in the browser".to_string())
    } else {
        ("\u{26a0}", crate::theme_tokens::STATUS_WARN, format!("Won't render: {}", site.reason))
    };
    health.set_attribute("style", &format!("flex:0 0 16px;text-align:center;font-size:14px;color:{color}")).ok();
    health.set_attribute("title", &tip).ok();
    util::set_text(&health, glyph);
    util::append(&row, &health);

    let btn = util::create_element("button");
    btn.set_attribute("style", if selected { CHIP_ON } else { theme::BTN_SMALL }).ok();
    util::set_text(&btn, &site.id);
    ctx.on_window_event(&btn, "click", EV_SELECT_SITE, &site.id);
    util::append(&row, &btn);
    row
}

/// The "New site" card (shown when the create expander is open): an id field, a
/// title field, and a Create button that fires `EV_CREATE` and clears the drafts.
fn create_block(ctx: &DomCtx) -> Element {
    let block = util::create_element("div");
    block.set_attribute("style", "margin:6px 0 4px;padding:12px;border:1px solid \
        var(--border,#2a2a4e);border-radius:6px;background:var(--surface-sunken,#15152a)").ok();

    let id_input = util::tracked_input(&block, ctx, "new_site_id", "", theme::INPUT);
    id_input.set_attribute("placeholder", "new site-id (letters, digits, - _)").ok();
    let title_input = util::tracked_input(&block, ctx, "new_site_title", "", theme::INPUT);
    title_input.set_attribute("placeholder", "Title (optional)").ok();

    let create = util::create_element("button");
    create.set_attribute("style", theme::BTN_PRIMARY).ok();
    util::set_text(&create, "Create site");
    {
        let drafts = ctx.drafts.clone();
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        ctx.listen(&create, "click", move |_| {
            let id = drafts.borrow().get("new_site_id").cloned().unwrap_or_default();
            let title = drafts.borrow().get("new_site_title").cloned().unwrap_or_default();
            actions.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: EV_CREATE.to_string(),
                value: format!("{id}\n{title}"),
            });
            drafts.borrow_mut().remove("new_site_id");
            drafts.borrow_mut().remove("new_site_title");
            rp();
        });
    }
    util::append(&block, &create);
    block
}

fn editor_block(sel: &SelectedSite, ctx: &DomCtx) -> Element {
    let block = util::create_element("div");

    // Slim header: which site is open + a right-aligned "Delete site"
    // (destructive → confirmed). Render health lives next to the site in the
    // list above, not here.
    let head = util::create_element("div");
    head.set_attribute("style", "display:flex;justify-content:space-between;align-items:center;gap:8px;margin:12px 0 4px").ok();
    let title = util::create_element("div");
    title.set_attribute("style", "font-weight:bold;font-size:14px").ok();
    util::set_text(&title, &format!("Editing: {}", sel.site_id));
    util::append(&head, &title);
    let del_site = util::create_element("button");
    del_site.set_attribute("style", BTN_DANGER).ok();
    util::set_text(&del_site, "Delete site");
    on_confirmed_event(
        ctx,
        &del_site,
        Some(format!("Delete the entire site '{}' and all its pages? This cannot be undone.", sel.site_id)),
        EV_DELETE_SITE,
        sel.site_id.clone(),
    );
    util::append(&head, &del_site);
    util::append(&block, &head);

    // Tree navigator (collapsible).
    util::append(&block, &collapsible_header(ctx, "Pages", sel.pages_open, EV_TOGGLE_PAGES));
    if sel.pages_open {
        util::append(&block, &navigator(sel, ctx));
    }

    // Markdown editor for the selected page.
    if let Some(page) = &sel.selected_page {
        util::append(
            &block,
            &page_editor(&sel.site_id, page, &sel.page_title, &sel.page_body, sel.show_preview, ctx),
        );
    }

    block
}

fn navigator(sel: &SelectedSite, ctx: &DomCtx) -> Element {
    let nav = util::create_element("div");

    // Site-root row — click to make the site root the add-target. Highlighted
    // (same blue as a selected page) when it's the current target.
    let root_row = util::create_element("div");
    root_row.set_attribute("style", TREE_ROW).ok();
    let spacer = util::create_element("span");
    spacer.set_attribute("style", "flex:0 0 22px").ok();
    util::append(&root_row, &spacer);
    let root_btn = util::create_element("button");
    let root_style = if sel.cursor.is_empty() { NODE_BTN_SELECTED } else { NODE_BTN };
    root_btn.set_attribute("style", root_style).ok();
    util::set_text(&root_btn, "\u{1f3e0} / (site root)"); // 🏠
    ctx.on_window_event(&root_btn, "click", EV_CD, "");
    util::append(&root_row, &root_btn);
    util::append(&nav, &root_row);

    // The page tree, flattened to visible rows (the same shape the Entity Tree
    // inspector renders): one indented row per visible node, folders carry a
    // ▾/▸ toggle.
    let list = util::create_element("div");
    list.set_attribute("style", "margin:2px 0").ok();
    if sel.rows.is_empty() {
        let empty = util::create_element("p");
        empty.set_attribute("style", theme::HINT).ok();
        util::set_text(&empty, "(no pages yet — add one below)");
        util::append(&list, &empty);
    } else {
        for row in &sel.rows {
            render_node(&list, row, sel, ctx);
        }
    }
    util::append(&nav, &list);

    // One name box on one line, feeding both "+ Add page" and "+ Add folder".
    // They land in the current add-target directory.
    let target_label = util::create_element("div");
    target_label.set_attribute("style", "font-size:12px;color:var(--text-dim,#888);margin-top:8px").ok();
    let where_ = if sel.add_target.is_empty() {
        "site root".to_string()
    } else {
        format!("/{}", sel.add_target)
    };
    util::set_text(&target_label, &format!("Adding to: {where_}"));
    util::append(&nav, &target_label);
    util::append(&nav, &add_controls(ctx));
    nav
}

/// A single name input + "+ Add page" and "+ Add folder" buttons on one line,
/// both sourcing the same field. (Field name kept as `new_page_slug` so the
/// add-page e2e selector is stable.)
fn add_controls(ctx: &DomCtx) -> Element {
    let row = util::create_element("div");
    row.set_attribute("style", ROW).ok();
    let field = "new_page_slug";
    let input = util::tracked_input(&row, ctx, field, "", &format!("{};flex:1 1 160px;max-width:280px", theme::INPUT));
    input.set_attribute("placeholder", "page or folder name").ok();

    for (label, event) in [("+ Add page", EV_ADD_PAGE), ("+ Add folder", EV_ADD_DIR)] {
        let btn = util::create_element("button");
        btn.set_attribute("style", theme::BTN_SMALL).ok();
        util::set_text(&btn, label);
        let drafts = ctx.drafts.clone();
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        let event = event.to_string();
        ctx.listen(&btn, "click", move |_| {
            let value = drafts.borrow().get(field).cloned().unwrap_or_default();
            actions.borrow_mut().push(Action::WindowEvent { window_id: wid, event: event.clone(), value });
            drafts.borrow_mut().remove(field);
            rp();
        });
        util::append(&row, &btn);
    }
    row
}

/// Render one visible tree row as an indented line. A folder carries a ▾/▸
/// toggle (caret); clicking the name edits a page (`has_entry`) or — for a
/// folder — sets it as the add-target. The selected page is highlighted; the
/// add-target folder gets a dashed outline.
fn render_node(list: &Element, node: &VisibleRow, sel: &SelectedSite, ctx: &DomCtx) {
    let row = util::create_element("div");
    row.set_attribute("style", &format!("{TREE_ROW};padding-left:{}px", node.depth * 16)).ok();

    // A page is a row that binds an entity; everything else is a folder.
    let is_page = node.has_entry;

    // Caret (folders with children) or an aligning spacer.
    if node.has_children {
        let caret = util::create_element("button");
        caret.set_attribute("style", CARET).ok();
        util::set_text(&caret, if node.expanded { "\u{25be}" } else { "\u{25b8}" }); // ▾ / ▸
        ctx.on_window_event(&caret, "click", EV_TOGGLE_NODE, &node.path);
        util::append(&row, &caret);
    } else {
        let spacer = util::create_element("span");
        spacer.set_attribute("style", "flex:0 0 22px").ok();
        util::append(&row, &spacer);
    }

    // Name button. EXACTLY ONE highlight in the tree: the cursor (the last node
    // clicked, page or folder). Which page is loaded in the editor and whether
    // it has unsaved edits are shown as label markers (✎ / ●), never as a second
    // highlight — so clicking a folder doesn't leave a page looking "selected."
    let btn = util::create_element("button");
    let is_cursor = node.path == sel.cursor;
    btn.set_attribute("style", if is_cursor { NODE_BTN_SELECTED } else { NODE_BTN }).ok();
    let icon = if is_page { "\u{1f4c4}" } else { "\u{1f4c1}" }; // 📄 page / 📁 folder
    let is_editing = is_page && sel.selected_page.as_deref() == Some(node.path.as_str());
    let mut label = match node.leaf_count {
        Some(n) => format!("{icon} {} ({n})", node.segment),
        None => format!("{icon} {}", node.segment),
    };
    if is_editing {
        label.push_str(" \u{270e}"); // ✎ loaded in the editor
    }
    util::set_text(&btn, &label);
    // ● = unsaved changes, in red so it's unmistakable (don't lose work). The
    // open page is compared precisely (buffer vs saved); any OTHER page with an
    // outstanding draft (edited then navigated away without saving) is flagged
    // too, so the marker follows you across the tree. Its own span → red even
    // when the row label is otherwise the cursor's accent colour.
    let unsaved = if is_editing {
        page_dirty(sel, ctx)
    } else {
        is_page && row_has_unsaved(&sel.site_id, &node.path, ctx)
    };
    if unsaved {
        let dot = util::create_element("span");
        dot.set_attribute(
            "style",
            &format!("color:{};font-weight:700;margin-left:5px", crate::theme_tokens::STATUS_ERR),
        )
        .ok();
        util::set_text(&dot, "\u{25cf}");
        dot.set_attribute("title", "Unsaved changes").ok();
        util::append(&btn, &dot);
    }
    if is_page {
        ctx.on_window_event(&btn, "click", EV_SELECT_PAGE, &node.path);
    } else {
        ctx.on_window_event(&btn, "click", EV_CD, &node.path);
    }
    util::append(&row, &btn);
    util::append(list, &row);
}

/// Does a page (other than the open one) have an outstanding draft? A draft
/// only exists once the user has typed into that page's field, and the Save
/// handler clears it — so a present draft means "edited but not saved." Used to
/// flag pages you've edited and navigated away from with the ● tree marker.
fn row_has_unsaved(site: &str, slug: &str, ctx: &DomCtx) -> bool {
    let drafts = ctx.drafts.borrow();
    drafts.contains_key(&format!("body::{site}::{slug}"))
        || drafts.contains_key(&format!("title::{site}::{slug}"))
}

/// Does the loaded page have unsaved edits? Compares the per-page draft buffers
/// (body + title) against the saved values. Only the loaded page is assessable
/// (it's the one whose saved content we hold) — which is exactly the page the
/// tree's ● marker tracks.
fn page_dirty(sel: &SelectedSite, ctx: &DomCtx) -> bool {
    let Some(page) = sel.selected_page.as_deref() else { return false };
    let body_field = format!("body::{}::{}", sel.site_id, page);
    let title_field = format!("title::{}::{}", sel.site_id, page);
    let drafts = ctx.drafts.borrow();
    let body = drafts.get(&body_field).map(String::as_str).unwrap_or(sel.page_body.as_str());
    let title = drafts.get(&title_field).map(String::as_str).unwrap_or(sel.page_title.as_str());
    body != sel.page_body || title != sel.page_title
}

fn page_editor(
    site: &str,
    page: &str,
    title: &str,
    body: &str,
    show_preview: bool,
    ctx: &DomCtx,
) -> Element {
    let block = util::create_element("div");
    block.set_attribute("style", "margin-top:8px").ok();

    // Title field — feeds the page title (breadcrumbs, <title>, nav label).
    // Draft-keyed per (site,page) so switching pages never clobbers it.
    let title_field = format!("title::{site}::{page}");
    let title_input =
        util::tracked_input(&block, ctx, &title_field, title, &format!("{};margin-bottom:6px", theme::INPUT));
    title_input.set_attribute("placeholder", "Page title").ok();

    // The current buffer = the live draft if present, else the saved body — so
    // both the textarea and the preview reflect unsaved edits across rebuilds.
    let field = format!("body::{site}::{page}");
    let buffer = ctx.drafts.borrow().get(&field).cloned().unwrap_or_else(|| body.to_string());
    let title_buffer =
        ctx.drafts.borrow().get(&title_field).cloned().unwrap_or_else(|| title.to_string());
    // Drafts persist per (site,page), so switching pages doesn't lose edits —
    // but that means an edited page that you clicked away from still has unsaved
    // changes. Show an honest marker whenever the buffer differs from the saved
    // page (cleared automatically on the post-save rebuild, since the draft then
    // equals the saved text).
    let dirty = buffer != body || title_buffer != title;

    // Toolbar: which page + an unsaved marker + a preview toggle.
    let bar = util::create_element("div");
    bar.set_attribute("style", "display:flex;justify-content:space-between;align-items:center;gap:8px;margin-bottom:4px").ok();
    let label = util::create_element("div");
    label.set_attribute("style", "display:flex;align-items:center;gap:8px;font-size:12px;color:var(--text-dim,#888)").ok();
    let name = util::create_element("span");
    util::set_text(&name, &format!("Markdown — {page}"));
    util::append(&label, &name);
    let dirty_marker = util::create_element("span");
    dirty_marker.set_attribute(
        "style",
        &format!(
            "font-weight:600;color:{};{}",
            crate::theme_tokens::STATUS_ERR,
            if dirty { "" } else { "display:none" }
        ),
    )
    .ok();
    util::set_text(&dirty_marker, "\u{25cf} Unsaved changes");
    util::append(&label, &dirty_marker);
    util::append(&bar, &label);
    let prev_toggle = util::create_element("button");
    prev_toggle.set_attribute("style", theme::BTN_SMALL).ok();
    util::set_text(&prev_toggle, if show_preview { "Hide preview" } else { "Show preview" });
    ctx.on_window_event(&prev_toggle, "click", EV_TOGGLE_PREVIEW, "");
    util::append(&bar, &prev_toggle);
    util::append(&block, &bar);

    // Reveal the marker live as soon as the title is edited (no rebuild → the
    // render-time `dirty` above wouldn't catch keystrokes otherwise).
    {
        let marker = dirty_marker.clone();
        ctx.listen(&title_input, "input", move |_| {
            marker.set_attribute("style", &format!("font-weight:600;color:{}", crate::theme_tokens::STATUS_ERR)).ok();
        });
    }

    if show_preview {
        let cols = util::create_element("div");
        cols.set_attribute("style", EDITOR_COLS).ok();
        let edit_pane = util::create_element("div");
        edit_pane.set_attribute("style", PANE).ok();
        let textarea = util::tracked_textarea(&edit_pane, ctx, &field, body, TEXTAREA);
        util::append(&cols, &edit_pane);

        let prev_pane = util::create_element("div");
        prev_pane.set_attribute("style", PANE).ok();
        let preview = util::create_element("div");
        preview.set_attribute("style", PREVIEW).ok();
        preview.set_inner_html(&crate::content_site::render_page_body("markdown", &buffer));
        util::append(&prev_pane, &preview);
        util::append(&cols, &prev_pane);
        util::append(&block, &cols);

        // Live preview: re-render the buffer on each keystroke (no rebuild → no
        // focus loss). Safe: render_page_body escapes raw HTML (F-CONTENT-1).
        // Also flip the unsaved marker on.
        let preview_ref = preview.clone();
        let marker = dirty_marker.clone();
        ctx.listen(&textarea, "input", move |evt| {
            let val = evt
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
                .map(|t| t.value())
                .unwrap_or_default();
            preview_ref.set_inner_html(&crate::content_site::render_page_body("markdown", &val));
            marker.set_attribute("style", &format!("font-weight:600;color:{}", crate::theme_tokens::STATUS_ERR)).ok();
        });
    } else {
        // Focus mode — just the textarea, full width.
        let textarea = util::tracked_textarea(&block, ctx, &field, body, TEXTAREA);
        let marker = dirty_marker.clone();
        ctx.listen(&textarea, "input", move |_| {
            marker.set_attribute("style", &format!("font-weight:600;color:{}", crate::theme_tokens::STATUS_ERR)).ok();
        });
    }

    // Save + Delete page.
    let actions_row = util::create_element("div");
    actions_row.set_attribute("style", ROW).ok();
    let save = util::create_element("button");
    save.set_attribute("style", theme::BTN_PRIMARY).ok();
    util::set_text(&save, "Save page");
    {
        let drafts = ctx.drafts.clone();
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        let body_key = field.clone();
        let title_key = title_field.clone();
        let body_fallback = body.to_string();
        let title_fallback = title.to_string();
        ctx.listen(&save, "click", move |_| {
            let (title, body) = {
                let drafts = drafts.borrow();
                let title = drafts.get(&title_key).cloned().unwrap_or_else(|| title_fallback.clone());
                let body = drafts.get(&body_key).cloned().unwrap_or_else(|| body_fallback.clone());
                (title, body)
            };
            // Pack "{title}\n{body}" — the title is one line, the body follows.
            actions.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: EV_SAVE_PAGE.to_string(),
                value: format!("{}\n{}", title.replace('\n', " "), body),
            });
            // Drop the drafts so this page reads "saved" (no unsaved marker in
            // the tree) — the rebuild reseeds the fields from the saved entity.
            drafts.borrow_mut().remove(&body_key);
            drafts.borrow_mut().remove(&title_key);
            rp();
        });
    }
    util::append(&actions_row, &save);
    let del_page = util::create_element("button");
    del_page.set_attribute("style", BTN_DANGER).ok();
    util::set_text(&del_page, "Delete page");
    on_confirmed_event(
        ctx,
        &del_page,
        Some(format!("Delete the page '{page}'? This cannot be undone.")),
        EV_DELETE_PAGE,
        page.to_string(),
    );
    util::append(&actions_row, &del_page);
    util::append(&block, &actions_row);

    // Move / rename: an input pre-filled with the current full slug. Editing the
    // path (e.g. `guide/intro` → `manual/intro`) moves the page; the author
    // reshapes the folder structure here. Draft-keyed per (site,page).
    let move_row = util::create_element("div");
    move_row.set_attribute("style", ROW).ok();
    let move_label = util::create_element("span");
    move_label.set_attribute("style", "font-size:12px;color:var(--text-dim,#888)").ok();
    util::set_text(&move_label, "Move/rename to:");
    util::append(&move_row, &move_label);
    let move_field = format!("rename::{site}::{page}");
    let move_input =
        util::tracked_input(&move_row, ctx, &move_field, page, &format!("{};max-width:240px", theme::INPUT));
    move_input.set_attribute("placeholder", "new/path/slug").ok();
    let move_btn = util::create_element("button");
    move_btn.set_attribute("style", theme::BTN_SMALL).ok();
    util::set_text(&move_btn, "Move");
    {
        let drafts = ctx.drafts.clone();
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        let key = move_field.clone();
        let from = page.to_string();
        let body_key = field.clone();
        let title_key = title_field.clone();
        let saved_body = body.to_string();
        let saved_title = title.to_string();
        ctx.listen(&move_btn, "click", move |_| {
            // A move relocates the SAVED page; unsaved edits to it are keyed to
            // the old slug and would be dropped. Don't lose work silently —
            // confirm first.
            let dirty = {
                let d = drafts.borrow();
                d.get(&body_key).is_some_and(|b| b != &saved_body)
                    || d.get(&title_key).is_some_and(|t| t != &saved_title)
            };
            if dirty
                && !confirm("This page has unsaved changes that will be lost when it moves. Move anyway?")
            {
                return;
            }
            let to = drafts.borrow().get(&key).cloned().unwrap_or_default();
            actions.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: EV_RENAME_PAGE.to_string(),
                value: format!("{from}\n{}", to.replace('\n', " ")),
            });
            // Clear the rename draft + the now-orphaned old-slug body/title
            // drafts (the page moves; those keys would otherwise linger and
            // mis-flag a vanished slug as unsaved).
            let mut d = drafts.borrow_mut();
            d.remove(&key);
            d.remove(&body_key);
            d.remove(&title_key);
            rp();
        });
    }
    util::append(&move_row, &move_btn);
    util::append(&block, &move_row);

    block
}
