//! Wire Recorder DOM renderer — pure consumer of
//! [`WireRecorderOutput`](crate::views::wire_recorder::output::WireRecorderOutput).

use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::wire_recorder::output::{WireDirection, WireRecorderOutput, WireRow};

use web_sys::Element;

pub fn render(container: &Element, output: &WireRecorderOutput, _ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "wire-recorder");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, "Wire Recorder");
    util::append(&wrapper, &h2);

    let hint = util::create_element("div");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(
        &hint,
        "Live wire frames for this peer (newest first; ring buffer). Only \
         populates when cross-peer traffic flows.",
    );
    util::append(&wrapper, &hint);

    let counters = util::create_element("div");
    counters.set_attribute("style", theme::HINT).ok();
    let _ = counters.set_attribute("data-field", "wire-recorder-counts");
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
            "<span style='color:var(--text-dim, #888)'>(no wire frames yet — connect to a remote peer or \
             accept an inbound dial to see traffic)</span>",
        );
        util::append(&wrapper, &pre);
        util::append(container, &wrapper);
        return;
    }

    render_rows(&wrapper, &output.rows);
    util::append(container, &wrapper);
}

fn render_rows(parent: &Element, rows: &[WireRow]) {
    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();

    let mut html = String::new();
    for r in rows {
        let (dir_glyph, dir_color) = match r.direction {
            WireDirection::Inbound => ("←", "var(--status-info, #9cf)"),
            WireDirection::Outbound => ("→", "var(--status-warn, #fc9)"),
        };
        let remote = r.peer_remote.as_deref().unwrap_or("?");
        html.push_str(&format!(
            "<div><span style='color:{}'>{}</span> <span style='color:var(--status-ok, #9c9)'>{}</span> \
             <span style='color:var(--text-dim, #888)'>{} B</span> · <span style='color:#aaa'>{}</span></div>\n",
            dir_color,
            dir_glyph,
            util::escape_html(&r.frame_kind),
            r.bytes,
            util::escape_html(remote),
        ));
        if let Some(rid) = &r.request_id {
            html.push_str(&format!(
                "<div style='color:var(--text-dim, #888);margin:0 0 6px 12px'>req {}</div>\n",
                util::escape_html(rid),
            ));
        } else {
            html.push_str("<div style='margin:0 0 6px 0'></div>\n");
        }
    }
    pre.set_inner_html(&html);
    util::append(parent, &pre);
}
