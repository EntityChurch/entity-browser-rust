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
use crate::dom::components;
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
const EDITOR_COLS: &str =
    "display:flex;flex-wrap:wrap;gap:10px;align-items:stretch;margin-top:6px";
const PANE: &str = "flex:1 1 280px;min-width:240px";
const TEXTAREA: &str = "display:block;width:100%;min-height:420px;box-sizing:border-box;\
    background:var(--input-bg,#0e0e1e);color:var(--text,#e0e0e0);border:1px solid \
    var(--border,#2a2a4e);border-radius:4px;padding:10px;font-family:monospace;font-size:15px;\
    line-height:1.5";
// The preview pane paints the SITE background (not chrome tokens) and pairs
// with the `.cs-doc` class + the shared doc stylesheet, so it matches the
// overlay / a published page instead of a chrome-styled approximation.
const PREVIEW: &str = "min-height:420px;background:var(--site-bg, #101018);\
    border:1px solid var(--border,#2a2a4e);border-radius:4px;padding:10px;overflow:auto";
// (Destructive buttons use theme::BTN_DESTRUCTIVE — promoted from the local
// BTN_DANGER. Tree rows use components::tree_row / theme::TREE_* — promoted
// when File Transfer shipped a second copy of the consts that lived here.)

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

pub fn render(container: &Element, output: &SiteEditorOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "site-editor");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::TITLE_INLINE).ok();
    util::set_text(&h2, &crate::i18n::t("window.site_creator", &[]));
    util::append(&wrapper, &h2);

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &crate::i18n::t("siteeditor.hint", &[]));
    util::append(&wrapper, &hint);

    if let Some(notice) = &output.notice {
        util::append(&wrapper, &notice_block(&notice.text, notice.is_error));
    }

    // "Your sites" (collapsible) — a clean list of sites, each with its render
    // health as a small ✓/⚠ next to the name.
    util::append(&wrapper, &components::collapsible_header(ctx, &crate::i18n::t("siteeditor.your_sites", &[]), output.sites_open, EV_TOGGLE_SITES));
    if output.sites_open {
        util::append(&wrapper, &sites_block(output, ctx));
        // "New site" is its own expander → a tidy card on demand, not a row of
        // input boxes always sitting under the list.
        util::append(&wrapper, &components::collapsible_header(ctx, &crate::i18n::t("siteeditor.new_site", &[]), output.create_open, EV_TOGGLE_CREATE));
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
        util::set_text(&empty, &crate::i18n::t("siteeditor.no_sites", &[]));
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
    row.set_attribute("style", theme::TREE_ROW).ok();

    // Health glyph: ✓ renders / ⚠ won't (tooltip carries the reason).
    let health = util::create_element("span");
    let (glyph, color, tip) = if site.renderable {
        ("\u{2713}", crate::theme_tokens::STATUS_OK, crate::i18n::t("siteeditor.renders_ok", &[]))
    } else {
        ("\u{26a0}", crate::theme_tokens::STATUS_WARN, crate::i18n::t("siteeditor.wont_render", &[("reason", &site.reason)]))
    };
    health.set_attribute("style", &format!("flex:0 0 16px;text-align:center;font-size:14px;color:{color}")).ok();
    health.set_attribute("title", &tip).ok();
    util::set_text(&health, glyph);
    util::append(&row, &health);

    let btn = components::button_el(&site.id, components::ButtonKind::Small);
    if selected {
        btn.set_attribute("style", CHIP_ON).ok();
    }
    ctx.on_window_event(&btn, "click", EV_SELECT_SITE, &site.id);
    util::append(&row, &btn);
    row
}

/// The "New site" card (shown when the create expander is open): an id field, a
/// title field, and a Create button that fires `EV_CREATE` and clears the drafts.
fn create_block(ctx: &DomCtx) -> Element {
    // The create-form card (S8 convention: collapsed disclosure → a card with
    // exactly one primary that collapses on success).
    let block = components::card("");

    let id_input = util::tracked_input(&block, ctx, "new_site_id", "", theme::INPUT);
    id_input.set_attribute("placeholder", &crate::i18n::t("siteeditor.site_id_placeholder", &[])).ok();
    let title_input = util::tracked_input(&block, ctx, "new_site_title", "", theme::INPUT);
    title_input.set_attribute("placeholder", &crate::i18n::t("siteeditor.title_optional", &[])).ok();

    let create = components::button_el(&crate::i18n::t("siteeditor.create_site", &[]), components::ButtonKind::Primary);
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
    head.set_attribute("style", theme::HEADER_ROW).ok();
    let title = util::create_element("div");
    title.set_attribute("style", "font-weight:bold;font-size:14px").ok();
    util::set_text(&title, &crate::i18n::t("siteeditor.editing", &[("id", &sel.site_id)]));
    util::append(&head, &title);
    let del_site = components::button_el(&crate::i18n::t("siteeditor.delete_site", &[]), components::ButtonKind::Destructive);
    on_confirmed_event(
        ctx,
        &del_site,
        Some(crate::i18n::t("siteeditor.confirm_delete_site", &[("id", &sel.site_id)])),
        EV_DELETE_SITE,
        sel.site_id.clone(),
    );
    util::append(&head, &del_site);
    util::append(&block, &head);

    // Tree navigator (collapsible).
    util::append(&block, &components::collapsible_header(ctx, &crate::i18n::t("siteeditor.pages", &[]), sel.pages_open, EV_TOGGLE_PAGES));
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
    let (root_row, _, root_btn) =
        components::tree_row(0, false, false, sel.cursor.is_empty(), &crate::i18n::t("siteeditor.site_root_row", &[])); // 🏠
    ctx.on_window_event(&root_btn, "click", EV_CD, "");
    util::append(&nav, &root_row);

    // The page tree, flattened to visible rows (the same shape the Entity Tree
    // inspector renders): one indented row per visible node, folders carry a
    // ▾/▸ toggle.
    let list = util::create_element("div");
    list.set_attribute("style", "margin:2px 0").ok();
    if sel.rows.is_empty() {
        let empty = util::create_element("p");
        empty.set_attribute("style", theme::HINT).ok();
        util::set_text(&empty, &crate::i18n::t("siteeditor.no_pages", &[]));
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
        crate::i18n::t("siteeditor.site_root", &[])
    } else {
        format!("/{}", sel.add_target)
    };
    util::set_text(&target_label, &crate::i18n::t("siteeditor.adding_to", &[("target", &where_)]));
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
    input.set_attribute("placeholder", &crate::i18n::t("siteeditor.page_name_placeholder", &[])).ok();

    for (label_key, event) in [("siteeditor.add_page", EV_ADD_PAGE), ("siteeditor.add_folder", EV_ADD_DIR)] {
        let btn = components::button_el(&crate::i18n::t(label_key, &[]), components::ButtonKind::Small);
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
    // A page is a row that binds an entity; everything else is a folder.
    let is_page = node.has_entry;

    // Label. EXACTLY ONE highlight in the tree: the cursor (the last node
    // clicked, page or folder). Which page is loaded in the editor and whether
    // it has unsaved edits are shown as label markers (✎ / ●), never as a second
    // highlight — so clicking a folder doesn't leave a page looking "selected."
    let is_cursor = node.path == sel.cursor;
    let icon = if is_page { "\u{1f4c4}" } else { "\u{1f4c1}" }; // 📄 page / 📁 folder
    let is_editing = is_page && sel.selected_page.as_deref() == Some(node.path.as_str());
    let mut label = match node.leaf_count {
        Some(n) => format!("{icon} {} ({n})", node.segment),
        None => format!("{icon} {}", node.segment),
    };
    if is_editing {
        label.push_str(" \u{270e}"); // ✎ loaded in the editor
    }

    let (row, caret, btn) =
        components::tree_row(node.depth, node.has_children, node.expanded, is_cursor, &label);
    if let Some(caret) = caret {
        ctx.on_window_event(&caret, "click", EV_TOGGLE_NODE, &node.path);
    }
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
            &format!("color:{};font-weight:700;margin-inline-start:5px", crate::theme_tokens::STATUS_ERR),
        )
        .ok();
        util::set_text(&dot, "\u{25cf}");
        dot.set_attribute("title", &crate::i18n::t("tooltip.unsaved", &[])).ok();
        util::append(&btn, &dot);
    }
    if is_page {
        ctx.on_window_event(&btn, "click", EV_SELECT_PAGE, &node.path);
    } else {
        ctx.on_window_event(&btn, "click", EV_CD, &node.path);
    }
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
    title_input.set_attribute("placeholder", &crate::i18n::t("siteeditor.page_title_ph", &[])).ok();

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
    bar.set_attribute("style", theme::HEADER_ROW).ok();
    let label = util::create_element("div");
    label.set_attribute("style", "display:flex;align-items:center;gap:8px;font-size:12px;color:var(--text-dim,#888)").ok();
    let name = util::create_element("span");
    util::set_text(&name, &crate::i18n::t("siteeditor.markdown_label", &[("page", page)]));
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
    util::set_text(&dirty_marker, &crate::i18n::t("siteeditor.unsaved", &[]));
    util::append(&label, &dirty_marker);
    util::append(&bar, &label);
    let prev_toggle = components::button(
        ctx,
        &if show_preview { crate::i18n::t("siteeditor.hide_preview", &[]) } else { crate::i18n::t("siteeditor.show_preview", &[]) },
        components::ButtonKind::Small,
        EV_TOGGLE_PREVIEW,
    );
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
        // The preview renders in the SAME content-document context as the
        // overlay and a published page: the `.cs-doc` class + the shared
        // rule table (`content_site::doc_css`, live form) + the site
        // background — so what you preview is what the site shows (S-T1).
        // The <style> is per-render inside the shadow root, like the
        // overlay's; `--site-*` inherits through, so the preview follows
        // the "Site appearance" setting exactly as the overlay does.
        static DOC_CSS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let doc_style = util::create_element("style");
        util::set_text(
            &doc_style,
            DOC_CSS.get_or_init(|| {
                crate::content_site::doc_css::doc_css(
                    ".cs-doc",
                    crate::content_site::doc_css::PaletteMode::Live,
                )
            }),
        );
        util::append(&prev_pane, &doc_style);
        let preview = util::create_element_with_class("div", "cs-doc");
        preview.set_attribute("style", PREVIEW).ok();
        preview.set_inner_html(&crate::content_site::markdown_to_html(&buffer));
        util::append(&prev_pane, &preview);
        util::append(&cols, &prev_pane);
        util::append(&block, &cols);

        // Live preview: re-render the buffer on each keystroke (no rebuild → no
        // focus loss). Safe: markdown_to_html escapes raw HTML (F-CONTENT-1).
        // Also flip the unsaved marker on.
        let preview_ref = preview.clone();
        let marker = dirty_marker.clone();
        ctx.listen(&textarea, "input", move |evt| {
            let val = evt
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlTextAreaElement>().ok())
                .map(|t| t.value())
                .unwrap_or_default();
            preview_ref.set_inner_html(&crate::content_site::markdown_to_html(&val));
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
    let save = components::button_el(&crate::i18n::t("siteeditor.save_page", &[]), components::ButtonKind::Primary);
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
    let del_page = components::button_el(&crate::i18n::t("siteeditor.delete_page", &[]), components::ButtonKind::Destructive);
    on_confirmed_event(
        ctx,
        &del_page,
        Some(crate::i18n::t("siteeditor.confirm_delete_page", &[("page", page)])),
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
    util::set_text(&move_label, &crate::i18n::t("siteeditor.move_rename", &[]));
    util::append(&move_row, &move_label);
    let move_field = format!("rename::{site}::{page}");
    let move_input =
        util::tracked_input(&move_row, ctx, &move_field, page, &format!("{};max-width:240px", theme::INPUT));
    move_input.set_attribute("placeholder", "new/path/slug").ok();
    let move_btn = components::button_el(&crate::i18n::t("btn.move", &[]), components::ButtonKind::Small);
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
                && !confirm(&crate::i18n::t("siteeditor.confirm_move_unsaved", &[]))
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
