//! Entity Tree window DOM renderer — pure consumer of
//! [`EntityTreeOutput`](crate::views::entity_tree::output::EntityTreeOutput).
//!
//! Builds three side-by-side panels (`<nav>` tree, `<main>` document,
//! `<aside>` inspector). Tree panel renders a flat list of [`TreeRow`]
//! values with depth-based indentation; one delegated click handler
//! splits navigation vs group-toggle by inspecting the `data-action`
//! attribute on the clicked element. No `Peers` access here.

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::util::{self, DomCtx};
use crate::views::entity_tree::output::{
    DocumentBody, DocumentView, EntityTreeOutput, InspectorView, TreeRow,
};

use web_sys::Element;

/// One indent level, in pixels. Matches existing tree-group nesting feel.
const INDENT_PX: usize = 14;

/// Build the Entity Tree window's three panels and return them appended
/// to `container`.
pub fn render(container: &Element, output: &EntityTreeOutput, ctx: &DomCtx) {
    let tree_panel = util::create_element_with_class("nav", "tree-panel");
    // The tree panel scrolls (`overflow-y:auto`); mark it so its scroll position
    // survives the section rebuild that an expand/collapse triggers (otherwise
    // the user is snapped back to the top on every toggle).
    util::set_attr(&tree_panel, "data-scroll-key", "entity-tree");
    render_tree_panel(&tree_panel, output, ctx);
    util::append(container, &tree_panel);

    let doc_panel = util::create_element_with_class("main", "document-panel");
    render_document_panel(&doc_panel, &output.document);
    util::append(container, &doc_panel);

    let inspector_panel = util::create_element_with_class("aside", "inspector-panel");
    render_inspector_panel(&inspector_panel, &output.inspector);
    util::append(container, &inspector_panel);
}

fn render_tree_panel(container: &Element, output: &EntityTreeOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let h2 = util::create_element("h2");
    util::set_text(&h2, "Entity Tree");
    util::append(container, &h2);

    render_selection_source(container, output, ctx);

    if output.current_path.is_some() {
        let btn = util::create_element_with_class("button", "nav-up");
        util::set_text(&btn, "Up");
        util::set_attr(&btn, "data-action", "navigate-up");
        util::append(container, &btn);
    }

    if output.rows.is_empty() {
        let p = util::create_element("p");
        util::set_text(&p, "(empty tree)");
        util::append(container, &p);
    } else {
        for row in &output.rows {
            render_tree_row(container, row);
        }
    }

    let footer = util::create_element("footer");
    util::set_text(
        &footer,
        &format!(
            "{} entities, {} paths",
            output.footer.entity_count, output.footer.path_count
        ),
    );
    util::append(container, &footer);

    // Single delegated click handler. Distinguishes:
    //   - "Up" button (data-action=navigate-up)
    //   - Group-toggle glyph (data-action=toggle-expand on a span
    //     carrying data-path)
    //   - Row body (data-path on the row element)
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let wid = ctx.window_id;
    ctx.listen(container, "click", move |event: web_sys::Event| {
        let Some(target) = event.target() else {
            return;
        };
        let Some(el) = target.dyn_ref::<web_sys::Element>() else {
            return;
        };

        // Climb up to find the nearest element with a data-action or
        // data-path. The toggle glyph and the row body share a parent;
        // we want clicks anywhere on the row (except the glyph) to
        // navigate.
        let mut node: Option<web_sys::Element> = Some(el.clone());
        let mut to_push: Vec<Action> = Vec::new();
        while let Some(n) = node {
            let data_action = n.get_attribute("data-action");
            let data_path = n.get_attribute("data-path");
            if data_action.is_some() || data_path.is_some() {
                let has_children = n.get_attribute("data-has-children").is_some();
                to_push = crate::tree_click::click_actions(
                    wid,
                    data_action.as_deref(),
                    data_path.as_deref(),
                    has_children,
                );
                break;
            }
            node = n.parent_element();
        }

        if !to_push.is_empty() {
            let mut q = actions.borrow_mut();
            for a in to_push {
                q.push(a);
            }
            drop(q);
            rp();
        }
    });
}

/// "Selection source" dropdown. v1 offers None + App aggregate
/// (per-panel rows deferred — design §4.1). On change pushes
/// `Action::SetSelectionSource(window_id, wire)`.
fn render_selection_source(container: &Element, output: &EntityTreeOutput, ctx: &DomCtx) {
    let label = util::create_element_with_class("label", "selection-source-label");
    util::set_text(&label, "Selection source");
    util::append(container, &label);

    // The "app" source is implicitly bound-peer-scoped — it resolves
    // to THIS window's peer's `workspace/selection`, not a global
    // app slot. Two panels on the same peer co-orient; panels on
    // different peers each follow their own peer. Make that peer
    // scope visible in the label (the wire value stays "app" — it is
    // correctly resolved relative to the bound peer).
    let select = util::create_element_with_class("select", "selection-source");
    let app_label = format!("App aggregate (peer: {})", output.peer_label);
    let options: [(&str, &str); 2] =
        [("none", "None (manual)"), ("app", app_label.as_str())];
    for (value, text) in options {
        let opt = util::create_element("option");
        util::set_attr(&opt, "value", value);
        util::set_text(&opt, text);
        if output.selection_source == value {
            util::set_attr(&opt, "selected", "selected");
        }
        util::append(&select, &opt);
    }

    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let wid = ctx.window_id;
    let sel_ref = select.clone();
    ctx.listen(&select, "change", move |_| {
        let val = sel_ref
            .dyn_ref::<web_sys::HtmlSelectElement>()
            .map(|s| s.value())
            .unwrap_or_else(|| "none".to_string());
        actions
            .borrow_mut()
            .push(Action::SetSelectionSource(wid, val));
        rp();
    });
    util::append(container, &select);
}

fn render_tree_row(parent: &Element, row: &TreeRow) {
    // Two classes — `.tree-row` is the universal selector; rows that
    // also bind an entity get `.has-entry` so renderers, CSS, and
    // tests can distinguish "navigable to an entity" from "folder
    // node only." Folder-only rows still navigate (the model updates
    // current_path), but the inspector will surface NotFound.
    let class = if row.has_entry {
        "tree-row has-entry"
    } else {
        "tree-row"
    };
    let item = util::create_element_with_class("div", class);
    util::set_attr(
        &item,
        "style",
        &format!("padding-left:{}px", row.depth * INDENT_PX),
    );
    util::set_attr(&item, "data-path", &row.path);
    util::set_attr(&item, "role", "treeitem");
    if row.is_selected {
        util::set_attr(&item, "aria-selected", "true");
    }

    if row.has_children {
        // Mark the row so a click anywhere on its body toggles expansion (not
        // just the small disclosure glyph — a hard hit-box, esp. on touch).
        util::set_attr(&item, "data-has-children", "1");
        let toggle = util::create_element_with_class("span", "tree-toggle");
        util::set_text(&toggle, if row.expanded { "▼ " } else { "▶ " });
        util::set_attr(&toggle, "data-action", "toggle-expand");
        util::set_attr(&toggle, "data-path", &row.path);
        util::append(&item, &toggle);
    } else {
        // Spacer so leaf rows align with siblings that have toggles.
        let spacer = util::create_element_with_class("span", "tree-toggle-spacer");
        util::set_text(&spacer, "  ");
        util::append(&item, &spacer);
    }

    let label = util::create_element_with_class("span", "tree-label");
    util::set_text(&label, &row.segment);
    util::append(&item, &label);

    // Hint for collapsed groups.
    if let Some(n) = row.leaf_count {
        let hint = util::create_element_with_class("span", "tree-leaf-count");
        util::set_text(&hint, &format!(" ({})", n));
        util::append(&item, &hint);
    }

    util::append(parent, &item);
}

fn render_document_panel(container: &Element, view: &DocumentView) {
    util::clear_children(container);

    match view {
        DocumentView::Empty => {
            let p = util::create_element_with_class("p", "placeholder");
            util::set_text(&p, "Select an entity from the tree");
            util::append(container, &p);
        }
        DocumentView::NotFound { path } => {
            let h1 = util::create_element("h1");
            util::set_text(&h1, &format!("No entity at: {}", path));
            util::append(container, &h1);
        }
        DocumentView::Entity {
            path,
            entity_type,
            body,
        } => {
            let article = util::create_element("article");

            let h1 = util::create_element("h1");
            util::set_text(&h1, path);
            util::append(&article, &h1);

            let p_type = util::create_element_with_class("p", "entity-type");
            util::set_text(&p_type, &format!("Type: {}", entity_type));
            util::append(&article, &p_type);

            let hr = util::create_element("hr");
            util::append(&article, &hr);

            let pre = util::create_element_with_class("pre", "entity-content");
            match body {
                DocumentBody::Text(t) => util::set_text(&pre, t),
                DocumentBody::Formatted(s) => util::set_text(&pre, s),
            }
            util::append(&article, &pre);

            util::append(container, &article);
        }
    }
}

fn render_inspector_panel(container: &Element, view: &InspectorView) {
    util::clear_children(container);

    let h2 = util::create_element("h2");
    util::set_text(&h2, "Inspector");
    util::append(container, &h2);

    match view {
        InspectorView::Empty => {
            let p = util::create_element("p");
            util::set_text(&p, "No entity selected");
            util::append(container, &p);
        }
        InspectorView::NotFound { path } => {
            let p = util::create_element("p");
            util::set_text(&p, &format!("No entity at: {}", path));
            util::append(container, &p);
        }
        InspectorView::Entity {
            fields,
            raw_hash_hex,
            ..
        } => {
            let dl = util::create_element("dl");
            for (label, value) in fields {
                let dt = util::create_element("dt");
                util::set_text(&dt, label);
                util::append(&dl, &dt);

                let dd = util::create_element("dd");
                let code = util::create_element("code");
                util::set_text(&code, value);
                util::append(&dd, &code);
                util::append(&dl, &dd);
            }
            util::append(container, &dl);

            let h2_hash = util::create_element("h2");
            util::set_text(&h2_hash, "Raw Hash");
            util::append(container, &h2_hash);

            let pre = util::create_element_with_class("pre", "raw-hash");
            util::set_text(&pre, raw_hash_hex);
            util::append(container, &pre);
        }
    }
}
