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
    subject_key, AccessDirection, AccessEntry, AccessLogOutput, AccessOutcome, AccessView,
    CapabilityMapOutput, DirectionFilter, ObservedGrant, PeerCapabilities,
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

pub fn render(container: &Element, output: &AccessLogOutput, view: AccessView, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "access-log");
    wrapper.set_attribute("style", theme::SECTION).ok();
    window_header(&wrapper, view, ctx);

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    // Honest scope: dispatched operations only. → outbound (this app called a
    // remote peer), ← inbound (a remote peer reached into this device's backend
    // share, via the native backend), · local (a dispatch on this app's own
    // peer). Browsing cached data isn't a dispatch and won't appear.
    util::set_text(&hint, &crate::i18n::t("accesslog.hint", &[]));
    util::append(&wrapper, &hint);

    // Two filters, side by side: which peer's log (the frontend system peer vs
    // the native backend, each keeps its own log), and which direction. The peer
    // axis is what the operator manages by — see + separate both peers' logs.
    util::append(&wrapper, &controls_bar(output, ctx));

    if output.entries.is_empty() {
        let peer_scoped = !output.peer_filter.is_empty();
        let msg = match (output.direction, peer_scoped) {
            (DirectionFilter::All, false) => crate::i18n::t("accesslog.empty_all", &[]),
            _ => crate::i18n::t(
                "accesslog.empty_filtered",
                &[
                    ("peers", &crate::i18n::t("accesslog.all_peers", &[])),
                    ("all", &crate::i18n::t("accesslog.filter_all", &[])),
                ],
            ),
        };
        util::append(&wrapper, &components::empty(&msg));
        util::append(container, &wrapper);
        return;
    }

    // data-field lets the e2e assert the row count without scraping the table.
    let count = util::create_element("div");
    count.set_attribute("style", theme::HINT).ok();
    count.set_attribute("data-field", "access-log-count").ok();
    util::set_text(
        &count,
        &crate::i18n::t("accesslog.operations", &[("n", &output.entries.len().to_string())]),
    );
    util::append(&wrapper, &count);

    let scroll = util::create_element("div");
    scroll.set_attribute("style", TABLE_SCROLL).ok();
    let (tbl, body) = components::table(&[
        "",
        &crate::i18n::t("label.peer", &[]),
        &crate::i18n::t("accesslog.col_actor", &[]),
        &crate::i18n::t("label.target", &[]),
        &crate::i18n::t("execute.operation", &[]),
        &crate::i18n::t("accesslog.col_result", &[]),
    ]);
    for entry in &output.entries {
        util::append(&body, &row(entry, &output.backend_key, &output.labels));
    }
    util::append(&scroll, &tbl);
    util::append(&wrapper, &scroll);

    util::append(container, &wrapper);
}

/// Capabilities view: the observed-capability map — per acting peer, the distinct
/// grants it exercised (= the minimal grant it would need under enforcement). The
/// analytical projection of the log, and the raw material for "observed vs.
/// authored" (PLAN-OF-RECORD-capability-enforcement.md).
pub fn render_capabilities(
    container: &Element,
    output: &CapabilityMapOutput,
    view: AccessView,
    ctx: &DomCtx,
) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "access-log");
    wrapper.set_attribute("style", theme::SECTION).ok();
    window_header(&wrapper, view, ctx);

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &crate::i18n::t("accesslog.caps_hint", &[]));
    util::append(&wrapper, &hint);

    if output.peers.is_empty() {
        util::append(
            &wrapper,
            &components::empty(&crate::i18n::t("accesslog.caps_empty", &[])),
        );
        util::append(container, &wrapper);
        return;
    }

    let scroll = util::create_element("div");
    scroll.set_attribute("style", TABLE_SCROLL).ok();
    for peer in &output.peers {
        util::append(&scroll, &capability_section(peer));
    }
    util::append(&wrapper, &scroll);

    util::append(container, &wrapper);
}

/// One peer's observed grants: a labelled heading + a table of its distinct
/// (target, handler, operation, resource) tuples with use counts.
fn capability_section(peer: &PeerCapabilities) -> Element {
    let section = util::create_element("div");
    section.set_attribute("style", "margin:8px 0 12px 0").ok();

    let heading = util::create_element("div");
    heading
        .set_attribute("style", "font-weight:600;font-size:13px;margin:0 0 4px 0")
        .ok();
    heading.set_attribute("data-field", "capability-peer").ok();
    util::set_text(
        &heading,
        &crate::i18n::t_plural(
            "accesslog.capabilities_observed",
            peer.grants.len() as i64,
            &[("actor", &peer.actor_label), ("n", &peer.grants.len().to_string())],
        ),
    );
    util::append(&section, &heading);

    // Authorized (authored) grant, beside what's observed — the gap the operator
    // reads to decide whether the profile matches reality.
    util::append(&section, &authorized_line(peer));

    let (tbl, body) = components::table(&[
        &crate::i18n::t("label.target", &[]),
        &crate::i18n::t("execute.handler", &[]),
        &crate::i18n::t("execute.operation", &[]),
        &crate::i18n::t("execute.resource", &[]),
        &crate::i18n::t("accesslog.col_uses", &[]),
    ]);
    for g in &peer.grants {
        util::append(&body, &capability_row(g));
    }
    util::append(&section, &tbl);
    section
}

/// The peer's authored grant summarized in one block: profile name + scope, and
/// the granted handler/operation/path bits. Dim "not authorized" when there's no
/// recorded grant (our own peers, or a device not yet granted).
fn authorized_line(peer: &PeerCapabilities) -> Element {
    let line = util::create_element("div");
    line.set_attribute(
        "style",
        "font-size:12px;margin:0 0 6px 0;padding:4px 8px;border-inline-start:2px solid var(--border,#333)",
    )
    .ok();
    line.set_attribute("data-field", "capability-authorized").ok();

    match &peer.authorized {
        Some(a) => {
            let head = util::create_element("div");
            head.set_attribute("style", &format!("color:{}", theme_tokens::STATUS_OK)).ok();
            util::set_text(
                &head,
                &crate::i18n::t(
                    "accesslog.authorized_head",
                    &[("profile", &a.profile_label), ("summary", &a.summary)],
                ),
            );
            util::append(&line, &head);

            let bits = util::create_element("div");
            bits.set_attribute("style", theme::HINT).ok();
            util::set_text(
                &bits,
                &crate::i18n::t(
                    "accesslog.grants_summary",
                    &[
                        ("handlers", &join_or_dash(&a.handlers)),
                        ("operations", &join_or_dash(&a.operations)),
                        ("paths", &join_or_dash(&a.resources)),
                    ],
                ),
            );
            util::append(&line, &bits);
        }
        None => {
            let none = util::create_element("div");
            none.set_attribute("style", theme::HINT).ok();
            util::set_text(&none, &crate::i18n::t("accesslog.no_grant", &[]));
            util::append(&line, &none);
        }
    }
    line
}

fn join_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "—".to_string()
    } else {
        items.join(", ")
    }
}

fn capability_row(g: &ObservedGrant) -> Element {
    let count = if g.any_denied {
        format!("{} · denied", g.count)
    } else {
        g.count.to_string()
    };
    let cells = vec![
        components::td_text(
            &g.target_label.clone().unwrap_or_else(|| crate::i18n::t("accesslog.own_peer", &[])),
        ),
        components::td_text(&g.handler),
        components::td_text(&g.operation),
        components::td_text(g.resource.as_deref().unwrap_or("—")),
        components::td_text(&count),
    ];
    components::tr(cells)
}

/// Shared window header: the title + a small view switcher (Activity ↔ observed
/// Capabilities). Kept identical across both views so the toggle doesn't jump.
fn window_header(wrapper: &Element, view: AccessView, ctx: &DomCtx) {
    let bar = util::create_element("div");
    bar.set_attribute("style", theme::HEADER_ROW).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", "margin:0").ok();
    util::set_text(&h2, &crate::i18n::t("window.access_log", &[]));
    util::append(&bar, &h2);

    let switch = compact_select("access-log-view");
    for v in AccessView::ALL {
        append_option(&switch, v.as_value(), &crate::i18n::t(v.label(), &[]), v == view);
    }
    ctx.on_select_change(&switch, "set_access_view");
    util::append(&bar, &switch);

    util::append(wrapper, &bar);
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
    util::append(&bar, &field_label(&crate::i18n::t("accesslog.peer_filter_label", &[])));
    let peer_select = compact_select("access-log-peer");
    // "All peers" first, then each subject present.
    append_option(
        &peer_select,
        "",
        &crate::i18n::t("accesslog.all_peers", &[]),
        output.peer_filter.is_empty(),
    );
    for opt in &output.peer_options {
        append_option(&peer_select, &opt.key, &opt.label, opt.key == output.peer_filter);
    }
    ctx.on_select_change(&peer_select, "set_peer_filter");
    util::append(&bar, &peer_select);

    // --- Direction filter ---
    util::append(&bar, &field_label(&crate::i18n::t("accesslog.direction_label", &[])));
    let dir_select = compact_select("access-log-direction");
    for opt in DirectionFilter::ALL {
        append_option(&dir_select, opt.as_value(), &crate::i18n::t(opt.label(), &[]), opt == output.direction);
    }
    ctx.on_select_change(&dir_select, "set_direction_filter");
    util::append(&bar, &dir_select);

    bar
}

/// A `<select>` sized to its content, not the full row — `theme::SELECT` is
/// `width:100%`, which in the flex controls bar would make each dropdown claim a
/// whole line and stack. The trailing `width:auto` (last-wins) keeps them inline.
fn compact_select(data_field: &str) -> Element {
    // The shared unwired select atom, inline-sized (width:auto last-wins) so
    // the filter row stays compact; options are appended by the caller.
    let select = crate::dom::components::select_el(&[], "");
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
    let (glyph, title_key) = match entry.direction {
        AccessDirection::Outbound => ("→", "accesslog.dir_outbound"),
        AccessDirection::Inbound => ("←", "accesslog.dir_inbound"),
        AccessDirection::Local => ("·", "accesslog.dir_local"),
    };
    let span = util::create_element("span");
    span.set_attribute("style", "font-variant-numeric:tabular-nums").ok();
    span.set_attribute("title", &crate::i18n::t(title_key, &[])).ok();
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
                crate::i18n::t("accesslog.target_local", &[])
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
    let (label_key, color) = match entry.outcome {
        AccessOutcome::Allowed => ("accesslog.result_allowed", theme_tokens::STATUS_OK),
        AccessOutcome::Denied => ("accesslog.result_denied", theme_tokens::STATUS_ERR),
        AccessOutcome::Error => ("accesslog.result_error", theme_tokens::STATUS_WARN),
    };
    let label = crate::i18n::t(label_key, &[]);
    let chip = util::create_element("span");
    chip.set_attribute(
        "style",
        &format!("color:{color};font-size:12px;font-variant-numeric:tabular-nums"),
    )
    .ok();
    chip.set_attribute("title", &entry.detail).ok();
    util::set_text(&chip, &label);
    chip
}
