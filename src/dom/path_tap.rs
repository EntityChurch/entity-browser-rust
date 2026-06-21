//! Path Tap DOM renderer — pure consumer of
//! [`PathTapOutput`](crate::views::path_tap::output::PathTapOutput).

use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::path_tap::output::{DispatchRow, PathTapOutput};

use web_sys::Element;

pub fn render(container: &Element, output: &PathTapOutput, _ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "path-tap");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, "Path Tap");
    util::append(&wrapper, &h2);

    let hint = util::create_element("div");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(
        &hint,
        "Live dispatch facts from this peer (newest first; ring buffer).",
    );
    util::append(&wrapper, &hint);

    // Diagnostic counters — visible to E2E + users so an empty ring
    // doesn't look like a wiring break when the upstream is just
    // emitting non-Dispatch facts. data-field used by `tests/e2e_worker.rs`.
    let counters = util::create_element("div");
    counters.set_attribute("style", theme::HINT).ok();
    let _ = counters.set_attribute("data-field", "path-tap-counts");
    util::set_text(
        &counters,
        &format!(
            "facts: dispatch={} wire={} binding={}",
            output.counts.dispatch, output.counts.wire, output.counts.binding
        ),
    );
    util::append(&wrapper, &counters);

    if !output.routing_active {
        let warn = util::create_element("pre");
        warn.set_attribute("style", theme::PRE_OUTPUT).ok();
        warn.set_inner_html(
            "<span style='color:var(--status-err, #f66)'>Inspect routing failed to attach on this peer.</span>\n\
             <span style='color:var(--text-dim, #888)'>No facts will arrive. Check tracing logs for the \
             install_inspect_sink error.</span>",
        );
        util::append(&wrapper, &warn);
        util::append(container, &wrapper);
        return;
    }

    if output.rows.is_empty() {
        let pre = util::create_element("pre");
        pre.set_attribute("style", theme::PRE_OUTPUT).ok();
        pre.set_inner_html(
            "<span style='color:var(--text-dim, #888)'>(no dispatch facts yet — trigger an exec, query, \
             put, etc. on this peer)</span>",
        );
        util::append(&wrapper, &pre);
        util::append(container, &wrapper);
        return;
    }

    render_rows(&wrapper, &output.rows);
    util::append(container, &wrapper);
}

fn render_rows(parent: &Element, rows: &[DispatchRow]) {
    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();

    let mut html = String::new();
    for r in rows {
        let status_color = if r.status >= 400 { "var(--status-err, #f66)" } else { "var(--status-ok, #9c9)" };
        html.push_str(&format!(
            "<div><span style='color:{}'><b>{}</b></span> · <span style='color:var(--status-info, #9cf)'>{}</span> :: <span style='color:var(--status-warn, #fc9)'>{}</span></div>\n",
            status_color,
            r.status,
            util::escape_html(&r.handler_uri),
            util::escape_html(&r.operation),
        ));
        html.push_str(&format!(
            "<div style='color:var(--text-dim, #888);margin:0 0 6px 12px'>req {}</div>\n",
            util::escape_html(&r.request_id),
        ));
    }
    pre.set_inner_html(&html);
    util::append(parent, &pre);
}
