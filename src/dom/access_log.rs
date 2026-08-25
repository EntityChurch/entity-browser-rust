//! Access Log DOM renderer — pure consumer of
//! [`AccessLogOutput`](crate::views::access_log::output::AccessLogOutput).
//!
//! A legible table of recent operations: Peer · Actor · Target · Operation ·
//! Result. The **Peer** column + dropdown answer "whose access log is this" —
//! the System peer (its own dispatches) vs the System backend (who reached into
//! it), each its own log, consolidated in one window. The result
//! chip surfaces allow/deny — the enforcement signal — in the shared status
//! colors. The visible half of the capability-audit direction
//! (`RESEARCH-CAPABILITY-MANAGEMENT-UX §4 Step 1`).

use crate::dom::components;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::theme_tokens;
use crate::views::access_log::output::{
    subject_key, AccessDirection, AccessEntry, AccessLogOutput, AccessOutcome, DirectionFilter,
};

use std::collections::HashMap;
use web_sys::Element;

/// Bounded, scrollable container for the rows. Without a max-height the table
/// grew the window without limit (no scrollbar, whole app janks laying out
/// hundreds of unclipped rows on every new record) — this is the fix for
/// "scrolls insanely / can't scroll." `overflow:auto` gives the scrollbar; the
/// bounded height keeps the reflow proportional.
const TABLE_SCROLL: &str =
    "max-height:60vh;overflow:auto;border:1px solid var(--border,#222);border-radius:4px";

pub fn render(container: &Element, output: &AccessLogOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "access-log");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", "margin:0").ok();
    util::set_text(&h2, "Access Log");
    util::append(&wrapper, &h2);

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    // Honest scope: dispatched operations only. → outbound (this app called a
    // remote peer), ← inbound (a remote peer reached into this device's backend
    // share, via the native backend), · local (a dispatch on this app's own
    // peer). Browsing cached data isn't a dispatch and won't appear.
    util::set_text(
        &hint,
        "Access crossing the boundary — → out (you called a peer), ← in \
         (a peer called this device), · local — showing who, the target, the \
         operation, and whether it was allowed or denied (newest first).",
    );
    util::append(&wrapper, &hint);

    // Two filters, side by side: which peer's log (the frontend system peer vs
    // the native backend, each keeps its own log), and which direction. The peer
    // axis is what the operator manages by — see + separate both peers' logs.
    util::append(&wrapper, &controls_bar(output, ctx));

    if output.entries.is_empty() {
        let peer_scoped = !output.peer_filter.is_empty();
        let msg = match (output.direction, peer_scoped) {
            (DirectionFilter::All, false) => {
                "No operations yet. Dispatch something — browse a peer, run a shell \
                 verb, transfer a file — and it appears here."
            }
            _ => "Nothing for this peer / direction yet. Widen the filters (peer \
                  “All peers”, direction “All”) to see every access.",
        };
        util::append(&wrapper, &components::empty(msg));
        util::append(container, &wrapper);
        return;
    }

    // data-field lets the e2e assert the row count without scraping the table.
    let count = util::create_element("div");
    count.set_attribute("style", theme::HINT).ok();
    count.set_attribute("data-field", "access-log-count").ok();
    util::set_text(&count, &format!("operations: {}", output.entries.len()));
    util::append(&wrapper, &count);

    let scroll = util::create_element("div");
    scroll.set_attribute("style", TABLE_SCROLL).ok();
    let (tbl, body) = components::table(&["", "Peer", "Actor", "Target", "Operation", "Result"]);
    for entry in &output.entries {
        util::append(&body, &row(entry, &output.backend_key, &output.labels));
    }
    util::append(&scroll, &tbl);
    util::append(&wrapper, &scroll);

    util::append(container, &wrapper);
}

/// The filter controls: a Peer `<select>` (whose log) + a direction `<select>`.
/// Each change fires its window event, handled in the window to re-render.
fn controls_bar(output: &AccessLogOutput, ctx: &DomCtx) -> Element {
    let bar = util::create_element("div");
    bar.set_attribute(
        "style",
        "display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin:4px 0 8px 0",
    )
    .ok();

    // --- Peer filter ---
    util::append(&bar, &field_label("Peer:"));
    let peer_select = compact_select("access-log-peer");
    // "All peers" first, then each subject present.
    append_option(&peer_select, "", "All peers", output.peer_filter.is_empty());
    for opt in &output.peer_options {
        append_option(&peer_select, &opt.key, &opt.label, opt.key == output.peer_filter);
    }
    ctx.on_select_change(&peer_select, "set_peer_filter");
    util::append(&bar, &peer_select);

    // --- Direction filter ---
    util::append(&bar, &field_label("Direction:"));
    let dir_select = compact_select("access-log-direction");
    for opt in DirectionFilter::ALL {
        append_option(&dir_select, opt.as_value(), opt.label(), opt == output.direction);
    }
    ctx.on_select_change(&dir_select, "set_direction_filter");
    util::append(&bar, &dir_select);

    bar
}

/// A `<select>` sized to its content, not the full row — `theme::SELECT` is
/// `width:100%`, which in the flex controls bar would make each dropdown claim a
/// whole line and stack. The trailing `width:auto` (last-wins) keeps them inline.
fn compact_select(data_field: &str) -> Element {
    let select = util::create_element("select");
    select
        .set_attribute("style", &format!("{};width:auto", theme::SELECT))
        .ok();
    select.set_attribute("data-field", data_field).ok();
    select
}

fn field_label(text: &str) -> Element {
    let label = util::create_element("label");
    label.set_attribute("style", theme::HINT).ok();
    util::set_text(&label, text);
    label
}

fn append_option(select: &Element, value: &str, label: &str, selected: bool) {
    let o = util::create_element("option");
    o.set_attribute("value", value).ok();
    if selected {
        o.set_attribute("selected", "selected").ok();
    }
    util::set_text(&o, label);
    util::append(select, &o);
}

fn row(entry: &AccessEntry, backend_key: &str, labels: &HashMap<String, String>) -> Element {
    components::tr(vec![
        components::td(&direction_glyph(entry)),
        components::td_text(&peer_label(entry, backend_key, labels)),
        components::td_text(&short_peer(&entry.actor)),
        components::td_text(&target_label(entry)),
        components::td_text(&operation_label(entry)),
        components::td(&outcome_chip(entry)),
    ])
}

/// The subject peer of this row — whose access log it belongs to. Uses the
/// model's friendly label; falls back to a short id for a subject not in the map
/// (shouldn't happen — the map is a superset — but never render a blank cell).
fn peer_label(entry: &AccessEntry, backend_key: &str, labels: &HashMap<String, String>) -> String {
    let key = subject_key(entry, backend_key);
    labels
        .get(&key)
        .cloned()
        .unwrap_or_else(|| short_peer(&key))
}

/// A direction marker so both halves read in one table: `→` outbound (I called
/// out), `←` inbound (someone called me), `·` a local dispatch on my own peer.
/// The full word is the hover tooltip.
fn direction_glyph(entry: &AccessEntry) -> Element {
    let (glyph, title) = match entry.direction {
        AccessDirection::Outbound => ("→", "Outbound — this app called a remote peer"),
        AccessDirection::Inbound => ("←", "Inbound — a remote peer called this device"),
        AccessDirection::Local => ("·", "Local — a dispatch on this app's own peer"),
    };
    let span = util::create_element("span");
    span.set_attribute("style", "font-variant-numeric:tabular-nums").ok();
    span.set_attribute("title", title).ok();
    util::set_text(&span, glyph);
    span
}

/// First 12 chars of a peer id — enough to disambiguate at a glance.
fn short_peer(peer: &str) -> String {
    peer.chars().take(12).collect()
}

/// "handler @ peerXXXX" for a remote target, or just the handler for a local
/// dispatch — the "where" of the operation.
fn target_label(entry: &AccessEntry) -> String {
    match &entry.target_peer {
        Some(p) => {
            let short = short_peer(p);
            if entry.handler.is_empty() {
                short
            } else {
                format!("{} @ {}", entry.handler, short)
            }
        }
        None => {
            if entry.handler.is_empty() {
                "(local)".to_string()
            } else {
                entry.handler.clone()
            }
        }
    }
}

/// The operation, with the resource path appended when present — "read a.txt"
/// reads better than a bare "read".
fn operation_label(entry: &AccessEntry) -> String {
    match &entry.resource {
        Some(r) if !r.is_empty() => format!("{} {}", entry.operation, r),
        _ => entry.operation.clone(),
    }
}

/// A small colored chip for the outcome; the raw detail is a hover tooltip.
fn outcome_chip(entry: &AccessEntry) -> Element {
    let (label, color) = match entry.outcome {
        AccessOutcome::Allowed => ("Allowed", theme_tokens::STATUS_OK),
        AccessOutcome::Denied => ("Denied", theme_tokens::STATUS_ERR),
        AccessOutcome::Error => ("Error", theme_tokens::STATUS_WARN),
    };
    let chip = util::create_element("span");
    chip.set_attribute(
        "style",
        &format!("color:{color};font-size:12px;font-variant-numeric:tabular-nums"),
    )
    .ok();
    chip.set_attribute("title", &entry.detail).ok();
    util::set_text(&chip, label);
    chip
}
