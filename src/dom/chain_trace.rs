//! Chain Trace DOM renderer — pure consumer of
//! [`ChainTraceOutput`](crate::views::chain_trace::output::ChainTraceOutput).

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::chain_trace::output::{ChainTraceOutput, TraceEntry};

use web_sys::Element;

pub fn render(container: &Element, output: &ChainTraceOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "chain-trace");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, "Chain Trace");
    util::append(&wrapper, &h2);

    render_input_row(&wrapper, output, ctx);
    render_results(&wrapper, output);

    util::append(container, &wrapper);
}

fn render_input_row(parent: &Element, output: &ChainTraceOutput, ctx: &DomCtx) {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&label, "Chain ID:");
    util::append(parent, &label);

    let hint = util::create_element("div");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(
        &hint,
        "Enter the chain_id to walk continuation + chain-error markers on this peer.",
    );
    util::append(parent, &hint);

    let field_id = format!("chain_trace_chain_id_{}", output.window_id);
    let input =
        util::tracked_input(parent, ctx, &field_id, &output.chain_id, theme::INPUT);

    let button = util::create_element("button");
    util::set_text(&button, "Trace");
    button.set_attribute("style", theme::BTN_SMALL).ok();

    let window_id = output.window_id;
    let drafts = ctx.drafts.clone();
    let actions = ctx.actions.clone();
    let repaint = ctx.repaint.clone();
    let field_id_for_click = field_id.clone();
    let initial_for_click = output.chain_id.clone();
    let actions_ref: Rc<RefCell<Vec<Action>>> = actions;
    util::listen(
        &button,
        "click",
        move |_| {
            let value = drafts
                .borrow()
                .get(&field_id_for_click)
                .cloned()
                .unwrap_or_else(|| initial_for_click.clone());
            actions_ref.borrow_mut().push(Action::WindowEvent {
                window_id,
                event: "set_chain_id".into(),
                value,
            });
            repaint();
        },
        &ctx.closures,
    );
    util::append(parent, &button);

    // Submit on Enter inside the input.
    let drafts2 = ctx.drafts.clone();
    let actions2 = ctx.actions.clone();
    let repaint2 = ctx.repaint.clone();
    let field_id_for_enter = field_id;
    let initial_for_enter = output.chain_id.clone();
    util::listen(
        &input,
        "keydown",
        move |evt: web_sys::Event| {
            let Ok(kev) = evt.dyn_into::<web_sys::KeyboardEvent>() else { return };
            if kev.key() != "Enter" {
                return;
            }
            let value = drafts2
                .borrow()
                .get(&field_id_for_enter)
                .cloned()
                .unwrap_or_else(|| initial_for_enter.clone());
            actions2.borrow_mut().push(Action::WindowEvent {
                window_id,
                event: "set_chain_id".into(),
                value,
            });
            repaint2();
        },
        &ctx.closures,
    );
}

fn render_results(parent: &Element, output: &ChainTraceOutput) {
    if output.chain_id.is_empty() {
        let pre = util::create_element("pre");
        pre.set_attribute("style", theme::PRE_OUTPUT).ok();
        pre.set_inner_html("<span style='color:var(--text-dim, #888)'>(enter a chain_id and press Trace)</span>");
        util::append(parent, &pre);
        return;
    }

    if !output.chain_known {
        let pre = util::create_element("pre");
        pre.set_attribute("style", theme::PRE_OUTPUT).ok();
        let escaped = util::escape_html(&output.chain_id);
        pre.set_inner_html(&format!(
            "<span style='color:var(--text-dim, #888)'>(no continuation or chain-error marker bound for chain_id <b>{}</b> on peer {})</span>",
            escaped,
            util::escape_html(&output.peer_id),
        ));
        util::append(parent, &pre);
        return;
    }

    if !output.continuations.is_empty() {
        let h3 = util::create_element("h3");
        h3.set_attribute("style", "margin:12px 0 4px").ok();
        util::set_text(&h3, "Continuations");
        util::append(parent, &h3);
        render_entries(parent, &output.continuations, false);
    }

    if !output.markers.is_empty() {
        let h3 = util::create_element("h3");
        h3.set_attribute("style", "margin:12px 0 4px").ok();
        util::set_text(&h3, "Chain-error markers");
        util::append(parent, &h3);
        render_entries(parent, &output.markers, true);
    }
}

fn render_entries(parent: &Element, entries: &[TraceEntry], is_marker: bool) {
    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();

    let mut html = String::new();
    for entry in entries {
        let path_label = util::escape_html(&entry.path);
        let type_label = if entry.entity_type.is_empty() {
            String::from("<unread>")
        } else {
            util::escape_html(&entry.entity_type)
        };

        if is_marker {
            let kind = if entry.kind_label.is_empty() { "?" } else { &entry.kind_label };
            let reason = if entry.reason_label.is_empty() { "?" } else { &entry.reason_label };
            let kind_color = if kind == "rejected" { "var(--status-err, #f66)" } else { "var(--status-warn, #fc9)" };
            html.push_str(&format!(
                "<div><span style='color:{}'><b>{}</b></span> · reason=<span style='color:var(--status-warn, #fc9)'>{}</span> · type=<span style='color:var(--status-info, #9cf)'>{}</span></div>\n",
                kind_color,
                util::escape_html(kind),
                util::escape_html(reason),
                type_label,
            ));
        } else {
            html.push_str(&format!(
                "<div><span style='color:var(--status-info, #9cf)'><b>{}</b></span></div>\n",
                type_label,
            ));
        }
        html.push_str(&format!(
            "<div style='color:var(--text-dim, #888);margin-bottom:6px'>{}</div>\n",
            path_label,
        ));

        if let Some(body) = &entry.body_display {
            let body_escaped = util::escape_html(body);
            html.push_str(&format!(
                "<pre style='margin:0 0 10px 12px;color:#ccc'>{}</pre>\n",
                body_escaped,
            ));
        } else if entry.body_available {
            html.push_str(
                "<div style='margin:0 0 10px 12px;color:#aaa;font-style:italic'>(body redacted per renderer policy)</div>\n",
            );
        } else {
            html.push_str(
                "<div style='margin:0 0 10px 12px;color:var(--text-dim, #888);font-style:italic'>(body not yet decoded)</div>\n",
            );
        }
    }
    pre.set_inner_html(&html);
    util::append(parent, &pre);
}
