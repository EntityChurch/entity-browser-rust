//! Query Console DOM renderer — pure consumer of
//! [`QueryConsoleOutput`](crate::views::query_console::output::QueryConsoleOutput).

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::event_log;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::query_console::model::build_expression_from_fields;
use crate::views::query_console::output::{QueryConsoleOutput, QueryFields};
use crate::window::WindowId;

use web_sys::Element;

pub fn render(container: &Element, output: &QueryConsoleOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "query-console");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, "Query Console");
    util::append(&wrapper, &h2);

    text_field(
        &wrapper,
        ctx,
        "Type Filter:",
        "Exact type, glob (app/*), or * for all",
        "type_filter",
        &output.fields.type_filter,
        None,
    );
    text_field(
        &wrapper,
        ctx,
        "Path Prefix:",
        "Filter results by path prefix (optional)",
        "path_prefix",
        &output.fields.path_prefix,
        None,
    );
    text_field(
        &wrapper,
        ctx,
        "Ref Filter (hash):",
        "Find entities referencing a content hash (hex, optional)",
        "ref_filter",
        &output.fields.ref_filter,
        Some("00aabbccdd..."),
    );
    text_field(
        &wrapper,
        ctx,
        "Path Filter:",
        "Find entities linking to this path (optional)",
        "path_filter",
        &output.fields.path_filter,
        None,
    );
    text_field(
        &wrapper,
        ctx,
        "Limit:",
        "",
        "limit",
        &output.fields.limit,
        None,
    );

    render_include_entities(&wrapper, output, ctx);
    render_action_buttons(&wrapper, output, ctx);
    render_results(&wrapper, output);

    util::append(container, &wrapper);
}

fn text_field(
    parent: &Element,
    ctx: &DomCtx,
    label_text: &str,
    hint: &str,
    field_name: &str,
    value: &str,
    placeholder: Option<&str>,
) {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&label, label_text);
    util::append(parent, &label);

    if !hint.is_empty() {
        let hint_div = util::create_element("div");
        hint_div.set_attribute("style", theme::HINT).ok();
        util::set_text(&hint_div, hint);
        util::append(parent, &hint_div);
    }

    // `tracked_input` preserves the live value across section
    // rebuilds — query consoles get rebuild storms when results
    // land or the event log ticks, and used to lose mid-typed
    // filters. The placeholder attribute is set after creation
    // since the helper doesn't take one.
    let input = util::tracked_input(parent, ctx, field_name, value, theme::INPUT);
    if let Some(ph) = placeholder {
        input.set_attribute("placeholder", ph).ok();
    }
}

fn render_include_entities(parent: &Element, output: &QueryConsoleOutput, ctx: &DomCtx) {
    let row = util::create_element("div");
    row.set_attribute("style", theme::CHECKBOX_ROW).ok();

    let cb = util::create_element("input");
    cb.set_attribute("type", "checkbox").ok();
    cb.set_attribute("data-field", "include_entities").ok();
    if output.fields.include_entities {
        cb.set_attribute("checked", "").ok();
    }
    ctx.on_window_event(&cb, "change", "toggle_include_entities", "");
    util::append(&row, &cb);

    let label = util::create_element("label");
    label.set_attribute("style", "font-size:12px").ok();
    util::set_text(&label, "Include full entities in results");
    util::append(&row, &label);
    util::append(parent, &row);
}

fn render_action_buttons(parent: &Element, output: &QueryConsoleOutput, ctx: &DomCtx) {
    let row = util::create_element("div");
    row.set_attribute("style", theme::BTN_ROW).ok();

    let find_btn = util::create_element("button");
    util::set_text(&find_btn, "Find");
    find_btn.set_attribute("style", theme::BTN_PRIMARY).ok();
    bind_query_click(&find_btn, parent, "find", output.window_id, output.peer_id.clone(), ctx);
    util::append(&row, &find_btn);

    let count_btn = util::create_element("button");
    util::set_text(&count_btn, "Count");
    count_btn.set_attribute("style", theme::BTN_SECONDARY).ok();
    bind_query_click(&count_btn, parent, "count", output.window_id, output.peer_id.clone(), ctx);
    util::append(&row, &count_btn);

    util::append(parent, &row);
}

fn bind_query_click(
    button: &Element,
    parent: &Element,
    operation: &'static str,
    window_id: WindowId,
    peer_id: String,
    ctx: &DomCtx,
) {
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let parent_ref = parent.clone();

    ctx.listen(button, "click", move |_| {
        let peer_id = peer_id.clone();
        let fields = read_fields_from_dom(&parent_ref);
        let expression = build_expression_from_fields(
            &fields.type_filter,
            &fields.path_prefix,
            &fields.ref_filter,
            &fields.path_filter,
            &fields.limit,
            fields.include_entities,
        );
        // Both `find` and `count` go through their typed L1 helpers.
        match operation {
            "find" => actions.borrow_mut().push(Action::Query { peer_id, expression }),
            "count" => actions.borrow_mut().push(Action::Count { peer_id, expression }),
            _ => actions.borrow_mut().push(Action::Execute {
                peer_id,
                handler_uri: "system/query".into(),
                operation: operation.into(),
                resource: None,
                params: Some(expression),
            }),
        }
        push_sync_events(&actions, window_id, &fields);
        rp();
    });
}

fn render_results(parent: &Element, output: &QueryConsoleOutput) {
    let header = util::create_element("h3");
    header.set_attribute("style", "margin:12px 0 4px;font-size:14px").ok();
    util::set_text(&header, "Results");
    util::append(parent, &header);

    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();
    if output.events.is_empty() {
        pre.set_inner_html("<span style='color:var(--text-dim, #888)'>(no results yet — run a query)</span>");
    } else {
        let mut html = String::new();
        for entry in &output.events {
            let color = event_log::color_for(entry.category);
            let escaped = util::escape_html(&entry.message);
            html.push_str(&format!("<span style='color:{}'>{}</span>\n", color, escaped));
        }
        pre.set_inner_html(&html);
    }
    util::append(parent, &pre);
    util::schedule_scroll_to_bottom(&pre);
}

fn read_fields_from_dom(wrapper: &Element) -> QueryFields {
    let get_val = |field: &str| -> String {
        wrapper
            .query_selector(&format!("[data-field='{}']", field))
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|inp| inp.value())
            .unwrap_or_default()
    };
    let include_entities = wrapper
        .query_selector("[data-field='include_entities']")
        .ok()
        .flatten()
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|inp| inp.checked())
        .unwrap_or(false);
    QueryFields {
        type_filter: get_val("type_filter"),
        path_prefix: get_val("path_prefix"),
        ref_filter: get_val("ref_filter"),
        path_filter: get_val("path_filter"),
        limit: get_val("limit"),
        include_entities,
    }
}

/// Sync DOM field values back to tree state via `WindowEvent`s. The
/// click handler reads the live DOM for the query expression; these
/// events ensure the next render reflects the same values.
fn push_sync_events(actions: &Rc<RefCell<Vec<Action>>>, window_id: WindowId, fields: &QueryFields) {
    let mut q = actions.borrow_mut();
    for (event, value) in [
        ("set_type_filter", &fields.type_filter),
        ("set_path_prefix", &fields.path_prefix),
        ("set_ref_filter", &fields.ref_filter),
        ("set_path_filter", &fields.path_filter),
        ("set_limit", &fields.limit),
    ] {
        q.push(Action::WindowEvent {
            window_id,
            event: event.into(),
            value: value.clone(),
        });
    }
}
