//! Content Stream DOM renderer — pure consumer of
//! [`ContentStreamOutput`](crate::views::content_stream::output::ContentStreamOutput).

use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::content_stream::output::{BindingKind, BindingRow, ContentStreamOutput};

use web_sys::Element;

pub fn render(container: &Element, output: &ContentStreamOutput, _ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "content-stream");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, "Content Stream");
    util::append(&wrapper, &h2);

    let hint = util::create_element("div");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(
        &hint,
        "Live binding events (entity writes/removes/snapshots) for this peer \
         (newest first; ring buffer).",
    );
    util::append(&wrapper, &hint);

    let counters = util::create_element("div");
    counters.set_attribute("style", theme::HINT).ok();
    let _ = counters.set_attribute("data-field", "content-stream-counts");
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
            "<span style='color:var(--text-dim, #888)'>(no binding events yet — trigger a put / remove / \
             snapshot on this peer)</span>",
        );
        util::append(&wrapper, &pre);
        util::append(container, &wrapper);
        return;
    }

    render_rows(&wrapper, &output.rows);
    util::append(container, &wrapper);
}

fn render_rows(parent: &Element, rows: &[BindingRow]) {
    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();

    let mut html = String::new();
    for r in rows {
        let (kind_label, kind_color) = match r.kind {
            BindingKind::Put => (if r.is_new { "PUT*" } else { "PUT" }, "var(--status-ok, #9c9)"),
            BindingKind::Remove => ("DEL", "var(--status-err, #f99)"),
            BindingKind::Snapshot => ("SNAP", "var(--status-info, #9cf)"),
            BindingKind::CacheInvalidate => ("INV", crate::theme_tokens::STATUS_WARN),
        };
        let etype = r.entity_type.as_deref().unwrap_or("?");
        html.push_str(&format!(
            "<div><span style='color:{}'><b>{}</b></span> <span style='color:var(--status-warn, #fc9)'>{}</span> \
             <span style='color:var(--text-dim, #888)'>:: {}</span></div>\n",
            kind_color,
            kind_label,
            util::escape_html(&r.path),
            util::escape_html(etype),
        ));
        if let Some(hash) = &r.content_hash {
            let short = if hash.len() > 12 { &hash[..12] } else { hash.as_str() };
            html.push_str(&format!(
                "<div style='color:var(--text-faint, #666);margin:0 0 6px 12px'>hash {}</div>\n",
                util::escape_html(short),
            ));
        } else {
            html.push_str("<div style='margin:0 0 6px 0'></div>\n");
        }
    }
    pre.set_inner_html(&html);
    util::append(parent, &pre);
}
