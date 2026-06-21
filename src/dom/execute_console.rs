//! Execute Console DOM renderer — pure consumer of
//! [`ExecuteConsoleOutput`](crate::views::execute_console::output::ExecuteConsoleOutput).

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::event_log;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::execute_console::output::{ExecuteConsoleOutput, ExecuteMode};

use web_sys::Element;

pub fn render(container: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "execute-console");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, "Execute Console");
    util::append(&wrapper, &h2);

    render_mode_toggle(&wrapper, output, ctx);
    render_peer_selector(&wrapper, output, ctx);

    match output.mode {
        ExecuteMode::Guided => render_guided(&wrapper, output, ctx),
        ExecuteMode::Raw => render_raw(&wrapper, output, ctx),
    }

    render_resource(&wrapper, output, ctx);
    render_execute_button(&wrapper, output, ctx);
    render_results(&wrapper, output);

    util::append(container, &wrapper);
}

fn render_mode_toggle(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let is_guided = output.mode == ExecuteMode::Guided;

    let mode_div = util::create_element("div");
    mode_div
        .set_attribute("style", "margin-bottom:8px;display:flex;flex-wrap:wrap;gap:4px")
        .ok();

    let guided_btn = util::create_element("button");
    guided_btn
        .set_attribute(
            "style",
            if is_guided { theme::TOGGLE_ACTIVE } else { theme::TOGGLE_INACTIVE },
        )
        .ok();
    util::set_text(&guided_btn, "Guided");
    ctx.on_window_event(&guided_btn, "click", "set_mode", "guided");
    util::append(&mode_div, &guided_btn);

    let raw_btn = util::create_element("button");
    raw_btn
        .set_attribute(
            "style",
            if is_guided { theme::TOGGLE_INACTIVE } else { theme::TOGGLE_ACTIVE },
        )
        .ok();
    util::set_text(&raw_btn, "Raw");
    ctx.on_window_event(&raw_btn, "click", "set_mode", "raw");
    util::append(&mode_div, &raw_btn);

    util::append(parent, &mode_div);
}

fn render_peer_selector(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&label, "Peer:");
    util::append(parent, &label);

    let select = util::create_element("select");
    select.set_attribute("style", theme::SELECT).ok();
    for option in &output.peer_options {
        let opt = util::create_element("option");
        opt.set_attribute("value", &option.value).ok();
        if option.selected {
            opt.set_attribute("selected", "").ok();
        }
        util::set_text(&opt, &option.label);
        util::append(&select, &opt);
    }
    ctx.on_select_change(&select, "select_peer");
    util::append(parent, &select);
}

fn render_guided(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let Some(guided) = &output.guided else { return };

    let h_label = util::create_element("label");
    h_label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&h_label, "Handler:");
    util::append(parent, &h_label);

    let h_select = util::create_element("select");
    h_select.set_attribute("style", theme::SELECT).ok();
    for h in &guided.handlers {
        let opt = util::create_element("option");
        opt.set_attribute("value", &h.index.to_string()).ok();
        if h.selected {
            opt.set_attribute("selected", "").ok();
        }
        util::set_text(&opt, &h.label);
        util::append(&h_select, &opt);
    }
    ctx.on_select_change(&h_select, "select_handler");
    util::append(parent, &h_select);

    let op_label = util::create_element("label");
    op_label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&op_label, "Operation:");
    util::append(parent, &op_label);

    let op_select = util::create_element("select");
    op_select.set_attribute("style", theme::SELECT).ok();
    for op in &guided.operations {
        let opt = util::create_element("option");
        opt.set_attribute("value", &op.index.to_string()).ok();
        if op.selected {
            opt.set_attribute("selected", "").ok();
        }
        util::set_text(&opt, &op.name);
        util::append(&op_select, &opt);
    }
    ctx.on_select_change(&op_select, "select_operation");
    util::append(parent, &op_select);
}

fn render_raw(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let Some(raw) = &output.raw else { return };

    let uri_label = util::create_element("label");
    uri_label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&uri_label, "Handler URI:");
    util::append(parent, &uri_label);

    // `tracked_input` persists typing across section rebuilds — exec
    // completion + event log subscription used to clobber the value
    // mid-edit before this. The data-field attribute (matches the
    // execute button's query_selector) is set by the helper.
    util::tracked_input(parent, ctx, "raw_uri", &raw.handler_uri_initial, theme::INPUT);

    let op_label = util::create_element("label");
    op_label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&op_label, "Operation:");
    util::append(parent, &op_label);

    util::tracked_input(parent, ctx, "raw_op", &raw.operation_initial, theme::INPUT);
}

fn render_resource(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&label, "Resource:");
    util::append(parent, &label);

    util::tracked_input(parent, ctx, "resource", &output.resource_initial, theme::INPUT);
}

fn render_execute_button(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let exec_btn = util::create_element("button");
    util::set_text(&exec_btn, "Execute");
    exec_btn.set_attribute("style", theme::BTN_PRIMARY).ok();

    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let parent_ref = parent.clone();
    let is_raw = output.mode == ExecuteMode::Raw;
    let resolved_uri = output.resolved.handler_uri.clone();
    let resolved_op = output.resolved.operation.clone();
    let exec_peer_id = output.peer_id.clone();
    let window_id = ctx.window_id;

    ctx.listen(&exec_btn, "click", move |_| {
        let get_val = |field: &str| -> Option<String> {
            parent_ref
                .query_selector(&format!("[data-field='{}']", field))
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
                .map(|inp| inp.value())
        };

        let (uri, op) = if is_raw {
            (
                get_val("raw_uri").unwrap_or_else(|| resolved_uri.clone()),
                get_val("raw_op").unwrap_or_else(|| resolved_op.clone()),
            )
        } else {
            (resolved_uri.clone(), resolved_op.clone())
        };
        let res = get_val("resource").unwrap_or_default();

        if !uri.is_empty() && !op.is_empty() {
            // Persist the live DOM input values to the model BEFORE
            // firing Execute. Without this, the Execute action
            // triggers an event-log subscription Change → re-render,
            // and the rebuilt DOM reads `output.*_initial` from the
            // model — which still holds the pre-edit values, so the
            // user's typed inputs appear to vanish. These set_*
            // events run through handle_action's save_state path and
            // bring state.resource / raw_uri / raw_op up to date for
            // the subsequent re-render.
            let push = |event: &str, value: String| {
                actions.borrow_mut().push(Action::WindowEvent {
                    window_id,
                    event: event.into(),
                    value,
                });
            };
            push("set_resource", res.clone());
            if is_raw {
                push("set_raw_uri", uri.clone());
                push("set_raw_operation", op.clone());
            }

            actions.borrow_mut().push(Action::Execute {
                peer_id: exec_peer_id.clone(),
                handler_uri: uri,
                operation: op,
                resource: if res.is_empty() { None } else { Some(res) },
                params: None,
            });
            rp();
        }
    });
    util::append(parent, &exec_btn);
}

fn render_results(parent: &Element, output: &ExecuteConsoleOutput) {
    let header = util::create_element("h3");
    header.set_attribute("style", "margin:12px 0 4px;font-size:14px").ok();
    util::set_text(&header, "Results");
    util::append(parent, &header);

    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();
    if output.events.is_empty() {
        pre.set_inner_html("<span style='color:var(--text-dim, #888)'>(no events yet)</span>");
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
