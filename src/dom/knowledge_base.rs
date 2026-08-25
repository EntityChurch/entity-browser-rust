//! DOM renderer for the Knowledge Base window.
//!
//! Pure construction: takes a `KnowledgeBaseOutput` and builds DOM
//! into the given container. Does not access the tree, the model, or
//! any peer state directly.
//!
//! Layout uses block-level sections with margin between them — no
//! flexbox header crowding, no overlays, no toast/status banners.
//! Title elements are full-width on their own line; button rows are
//! on their own line below.
//!
//! **Drafts live in the DOM, not in the tree.** While the user is in
//! Editor or New mode, the input and textarea elements own their
//! current value (browser handles input state). On save, a click
//! handler reads both values via DOM query, packs them with the
//! unit-separator delimiter, and dispatches a single `save` event.
//! No per-keystroke action is dispatched.
//!
//! This is intentionally the only file in the knowledge base feature
//! that depends on web-sys. The model layer (`views::knowledge_base`)
//! is renderer-independent.

use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::dom::{components, theme, util, DomCtx};
use crate::views::knowledge_base::output::{
    DraftInitial, KbTreeRow, KnowledgeBaseOutput, ViewMode,
};
use crate::views::knowledge_base::SAVE_FIELD_SEP;

/// Build the Knowledge Base window content into `container`.
pub fn render(container: &Element, output: &KnowledgeBaseOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "knowledge-base");
    util::set_attr(&wrapper, "style", theme::SECTION);

    match output.view_mode {
        ViewMode::List => render_list_view(&wrapper, output, ctx),
        ViewMode::Reader => render_reader_view(&wrapper, output, ctx),
        ViewMode::Editor => render_editor_view(&wrapper, output, ctx),
        ViewMode::New => render_new_view(&wrapper, output, ctx),
    }

    util::append(container, &wrapper);
}

// ---------------------------------------------------------------------------
// List view
// ---------------------------------------------------------------------------

fn render_list_view(parent: &Element, output: &KnowledgeBaseOutput, ctx: &DomCtx) {
    // Title on its own line.
    let h2 = util::create_element("h2");
    util::set_text(&h2, &crate::i18n::t("window.knowledge_base", &[]));
    util::set_attr(&h2, "style", theme::HEADING);
    util::append(parent, &h2);

    // Action button row on its own line.
    let actions_row = util::create_element("div");
    util::set_attr(&actions_row, "style", theme::BTN_ROW);
    let new_btn = components::button(
        ctx,
        &crate::i18n::t("kb.new_article", &[]),
        components::ButtonKind::Primary,
        "new",
    );
    util::append(&actions_row, &new_btn);
    util::append(parent, &actions_row);

    // Empty state (S5) — a helpful line with the next action named.
    if output.articles.is_empty() {
        let empty = util::create_element("div");
        util::set_attr(
            &empty,
            "style",
            "padding:20px;text-align:center;color:var(--text-dim, #888);font-style:italic;\
             border:1px dashed var(--border-strong, #444);border-radius:4px;",
        );
        util::set_text(
            &empty,
            &format!(
                "No articles yet on peer {}. Click \"+ New article\" to create one.",
                output.peer_label
            ),
        );
        util::append(parent, &empty);
        return;
    }

    // Article count.
    let count = util::create_element("div");
    util::set_attr(&count, "style", theme::HINT);
    util::set_text(
        &count,
        &format!(
            "{} article{}",
            output.articles.len(),
            if output.articles.len() == 1 { "" } else { "s" }
        ),
    );
    util::append(parent, &count);

    // Collapsible docs tree, mirroring the on-disk directory layout.
    // Folder rows toggle expand; article leaves open the reader.
    let tree = util::create_element("div");
    util::set_attr(
        &tree,
        "style",
        "border:1px solid var(--border, #333);border-radius:3px;overflow:auto;\
         max-height:70vh;font-family:var(--font-mono,monospace);font-size:13px;",
    );
    for row in &output.tree_rows {
        render_tree_row(&tree, row, ctx);
    }
    util::append(parent, &tree);
}

/// Pixels of indentation per tree depth level.
const INDENT_PX: usize = 14;

fn render_tree_row(parent: &Element, row: &KbTreeRow, ctx: &DomCtx) {
    // `.kb-tree-row` is the universal row selector; article leaves
    // also get `.has-entry` (mirrors the entity_tree convention so
    // CSS/tests can distinguish navigable leaves from folder nodes).
    //
    // Deliberately NOT `components::tree_row`: these are whole-row
    // clickable text lines (a doc index), not caret + node-button
    // controls — a different affordance, themed via tokens below.
    let class = if row.has_entry {
        "kb-tree-row has-entry"
    } else {
        "kb-tree-row"
    };
    let item = util::create_element_with_class("div", class);
    util::set_attr(
        &item,
        "style",
        &format!(
            "padding:5px 8px;padding-inline-start:{}px;cursor:pointer;\
             border-bottom:1px solid var(--border, #1a1a2a);white-space:nowrap;\
             overflow:hidden;text-overflow:ellipsis;",
            8 + row.depth * INDENT_PX
        ),
    );

    if row.has_children {
        // Directory node: ▼/▶ toggle glyph, name, collapsed count.
        let glyph = util::create_element("span");
        util::set_text(&glyph, if row.expanded { "▼ " } else { "▶ " });
        util::set_attr(&glyph, "style", "color:var(--status-ok, #7a7)");
        util::append(&item, &glyph);

        let name = util::create_element("span");
        util::set_text(&name, &row.segment);
        util::set_attr(&name, "style", "color:var(--accent-2, #cdf);font-weight:bold");
        util::append(&item, &name);

        if let Some(n) = row.leaf_count {
            let hint = util::create_element("span");
            util::set_text(&hint, &format!("  ({})", n));
            util::set_attr(&hint, "style", "color:var(--text-dim, #777);font-size:11px");
            util::append(&item, &hint);
        }

        ctx.on_window_event(&item, "click", "toggle", &row.path);
    } else {
        // Article leaf: aligned with sibling toggles, opens the reader.
        let spacer = util::create_element("span");
        util::set_text(&spacer, "  ");
        util::append(&item, &spacer);

        let name = util::create_element("span");
        util::set_text(&name, &row.segment);
        util::set_attr(&name, "style", "color:var(--text, #dde)");
        util::append(&item, &name);

        ctx.on_window_event(&item, "click", "select", &row.path);
    }

    util::append(parent, &item);
}

// ---------------------------------------------------------------------------
// Reader view
// ---------------------------------------------------------------------------

/// The dim mono slug line under a title (S6: raw ids ride below the human
/// name, never replace it).
fn slug_line(text: &str) -> Element {
    let line = util::create_element("div");
    util::set_text(&line, text);
    util::set_attr(
        &line,
        "style",
        "color:var(--text-dim, #888);font-family:var(--font-mono,monospace);\
         font-size:11px;margin:0 0 16px 0",
    );
    line
}

fn render_reader_view(parent: &Element, output: &KnowledgeBaseOutput, ctx: &DomCtx) {
    let detail = match &output.current {
        Some(d) => d,
        None => {
            // Selected article disappeared (race or external delete) —
            // loud error state (S5) + the way back.
            util::append(
                parent,
                &components::error("The selected article is no longer available."),
            );
            let row = util::create_element("div");
            let btn =
                components::button(
                    ctx,
                    &crate::i18n::t("kb.back_to_list", &[]),
                    components::ButtonKind::Small,
                    "show_list",
                );
            util::append(&row, &btn);
            util::append(parent, &row);
            return;
        }
    };

    // Title on its own line.
    let h2 = util::create_element("h2");
    util::set_text(&h2, &detail.title);
    util::set_attr(&h2, "style", theme::HEADING);
    util::append(parent, &h2);

    // Slug line on its own line.
    util::append(parent, &slug_line(&detail.slug));

    // Action button row on its own line — one navigational, one secondary,
    // and the destructive delete in the shared destructive look (S3).
    let row = util::create_element("div");
    util::set_attr(&row, "style", theme::BTN_ROW);
    util::append(
        &row,
        &components::button(
            ctx,
            &crate::i18n::t("btn.back", &[]),
            components::ButtonKind::Small,
            "show_list",
        ),
    );
    util::append(
        &row,
        &components::button(
            ctx,
            &crate::i18n::t("btn.edit", &[]),
            components::ButtonKind::Secondary,
            "edit",
        ),
    );
    util::append(
        &row,
        &components::button(
            ctx,
            &crate::i18n::t("btn.delete", &[]),
            components::ButtonKind::Destructive,
            "delete",
        ),
    );
    util::append(parent, &row);

    // Body content.
    let body = util::create_element("pre");
    util::set_attr(
        &body,
        "style",
        "background:var(--surface-sunken, #0a0a1a);color:var(--text, #dde);padding:12px;\
         border-radius:4px;font-family:var(--font-mono,monospace);font-size:13px;line-height:1.5;\
         white-space:pre-wrap;word-wrap:break-word;margin:0;\
         max-height:60vh;overflow:auto;border:1px solid var(--border, #222);",
    );
    util::set_text(&body, &detail.content);
    util::append(parent, &body);
}

// ---------------------------------------------------------------------------
// Editor / New view
// ---------------------------------------------------------------------------

fn render_editor_view(parent: &Element, output: &KnowledgeBaseOutput, ctx: &DomCtx) {
    let draft = match &output.draft_initial {
        Some(d) => d,
        None => return,
    };
    render_draft_form(parent, draft, ctx);
}

fn render_new_view(parent: &Element, output: &KnowledgeBaseOutput, ctx: &DomCtx) {
    let draft = match &output.draft_initial {
        Some(d) => d,
        None => return,
    };
    render_draft_form(parent, draft, ctx);
}

fn render_draft_form(parent: &Element, draft: &DraftInitial, ctx: &DomCtx) {
    // Heading on its own line.
    let h2 = util::create_element("h2");
    util::set_attr(&h2, "style", "margin:0 0 4px 0");
    let label = if draft.is_new { "New Article" } else { "Edit Article" };
    util::set_text(&h2, label);
    util::append(parent, &h2);

    // Slug line (Editor mode only).
    if let Some(slug) = &draft.editing_slug {
        util::append(parent, &slug_line(slug));
    } else {
        let spacer = util::create_element("div");
        util::set_attr(&spacer, "style", "margin-bottom:16px");
        util::append(parent, &spacer);
    }

    // Title field — the draft-tracked atom preserves typing across section
    // rebuilds (the previous set_value-on-rebuild pattern clobbered edits).
    let title_input = components::text_input(
        ctx,
        "title",
        &draft.initial_title,
        &crate::i18n::t("kb.title_placeholder", &[]),
    );
    util::set_attr(&title_input, "autofocus", "");
    util::append(
        parent,
        &components::field(&crate::i18n::t("label.title", &[]), "", &title_input),
    );

    // Content field block.
    let content_label = util::create_element("label");
    util::set_text(&content_label, &crate::i18n::t("label.content", &[]));
    util::set_attr(&content_label, "style", theme::LABEL);
    util::append(parent, &content_label);

    let textarea = util::tracked_textarea(parent, ctx, "content", &draft.initial_content, theme::TEXTAREA);
    util::set_attr(&textarea, "placeholder", &crate::i18n::t("kb.body_placeholder", &[]));

    // Button row on its own line, below the form.
    let row = util::create_element("div");
    util::set_attr(&row, "style", theme::BTN_ROW);

    let save_btn = components::button_el(&crate::i18n::t("btn.save", &[]), components::ButtonKind::Primary);
    // The save handler reads both DOM values at click time and
    // dispatches a single packed action — no per-keystroke writes.
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        let parent_ref = parent.clone();
        ctx.listen(&save_btn, "click", move |_| {
            let title = read_field_value(&parent_ref, "title");
            let content = read_field_value(&parent_ref, "content");
            let packed = format!("{}{}{}", title, SAVE_FIELD_SEP, content);
            actions.borrow_mut().push(crate::action::Action::WindowEvent {
                window_id: wid,
                event: "save".into(),
                value: packed,
            });
            rp();
        });
    }
    util::append(&row, &save_btn);

    let cancel_btn = components::button(
        ctx,
        &crate::i18n::t("btn.cancel", &[]),
        components::ButtonKind::Small,
        "cancel",
    );
    util::append(&row, &cancel_btn);

    util::append(parent, &row);
}

/// Read the current value of an input or textarea by its `data-field`
/// attribute. Searches inside `parent`. Returns empty string if not
/// found.
fn read_field_value(parent: &Element, field: &str) -> String {
    let selector = format!("[data-field='{}']", field);
    let el = match parent.query_selector(&selector).ok().flatten() {
        Some(el) => el,
        None => return String::new(),
    };
    if let Some(input) = el.dyn_ref::<web_sys::HtmlInputElement>() {
        return input.value();
    }
    if let Some(textarea) = el.dyn_ref::<web_sys::HtmlTextAreaElement>() {
        return textarea.value();
    }
    String::new()
}
