//! Execute Console DOM renderer — pure consumer of
//! [`ExecuteConsoleOutput`](crate::views::execute_console::output::ExecuteConsoleOutput).

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::components;
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

    // Toggle pair — the atoms' look comes from theme::TOGGLE_*; built on the
    // shared button_el so the element itself isn't hand-rolled.
    let guided_btn = components::button_el(&crate::i18n::t("execute.guided", &[]), components::ButtonKind::Small);
    guided_btn
        .set_attribute(
            "style",
            if is_guided { theme::TOGGLE_ACTIVE } else { theme::TOGGLE_INACTIVE },
        )
        .ok();
    ctx.on_window_event(&guided_btn, "click", "set_mode", "guided");
    util::append(&mode_div, &guided_btn);

    let raw_btn = components::button_el(&crate::i18n::t("execute.raw", &[]), components::ButtonKind::Small);
    raw_btn
        .set_attribute(
            "style",
            if is_guided { theme::TOGGLE_INACTIVE } else { theme::TOGGLE_ACTIVE },
        )
        .ok();
    ctx.on_window_event(&raw_btn, "click", "set_mode", "raw");
    util::append(&mode_div, &raw_btn);

    util::append(parent, &mode_div);
}

fn render_peer_selector(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let options: Vec<(&str, &str)> = output
        .peer_options
        .iter()
        .map(|o| (o.value.as_str(), o.label.as_str()))
        .collect();
    let selected = output
        .peer_options
        .iter()
        .find(|o| o.selected)
        .map(|o| o.value.as_str())
        .unwrap_or("");
    let select = components::select(ctx, &options, selected, "select_peer");
    util::append(
        parent,
        &components::field(&crate::i18n::t("label.peer", &[]), "", &select),
    );
}

fn render_guided(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let Some(guided) = &output.guided else { return };

    let h_values: Vec<String> = guided.handlers.iter().map(|h| h.index.to_string()).collect();
    let h_options: Vec<(&str, &str)> = h_values
        .iter()
        .zip(&guided.handlers)
        .map(|(v, h)| (v.as_str(), h.label.as_str()))
        .collect();
    let h_selected = h_values
        .iter()
        .zip(&guided.handlers)
        .find(|(_, h)| h.selected)
        .map(|(v, _)| v.as_str())
        .unwrap_or("");
    let h_select = components::select(ctx, &h_options, h_selected, "select_handler");
    util::append(
        parent,
        &components::field(&crate::i18n::t("execute.handler", &[]), "", &h_select),
    );

    let op_values: Vec<String> = guided.operations.iter().map(|o| o.index.to_string()).collect();
    let op_options: Vec<(&str, &str)> = op_values
        .iter()
        .zip(&guided.operations)
        .map(|(v, o)| (v.as_str(), o.name.as_str()))
        .collect();
    let op_selected = op_values
        .iter()
        .zip(&guided.operations)
        .find(|(_, o)| o.selected)
        .map(|(v, _)| v.as_str())
        .unwrap_or("");
    let op_select = components::select(ctx, &op_options, op_selected, "select_operation");
    util::append(
        parent,
        &components::field(&crate::i18n::t("execute.operation", &[]), "", &op_select),
    );
}

fn render_raw(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let Some(raw) = &output.raw else { return };

    // Draft-tracked atoms — typing persists across section rebuilds (exec
    // completion + event log subscription used to clobber the value mid-edit).
    // The data-field attribute (matching the execute button's query_selector)
    // comes with the atom.
    let uri = components::text_input(ctx, "raw_uri", &raw.handler_uri_initial, "");
    util::append(
        parent,
        &components::field(&crate::i18n::t("execute.handler_uri", &[]), "", &uri),
    );

    let op = components::text_input(ctx, "raw_op", &raw.operation_initial, "");
    util::append(
        parent,
        &components::field(&crate::i18n::t("execute.operation", &[]), "", &op),
    );
}

fn render_resource(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let resource = components::text_input(ctx, "resource", &output.resource_initial, "");
    util::append(
        parent,
        &components::field(&crate::i18n::t("execute.resource", &[]), "", &resource),
    );
}

fn render_execute_button(parent: &Element, output: &ExecuteConsoleOutput, ctx: &DomCtx) {
    let exec_btn = components::button_el(&crate::i18n::t("execute.execute", &[]), components::ButtonKind::Primary);

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
