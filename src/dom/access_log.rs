//! Access Log DOM renderer — pure consumer of
//! [`AccessLogOutput`](crate::views::access_log::output::AccessLogOutput).
//!
//! A legible table of recent operations: Target · Operation · Result. The
//! result chip surfaces allow/deny — the enforcement signal — in the shared
//! status colors (S4-adjacent). The visible half of the capability-audit
//! direction (`RESEARCH-CAPABILITY-MANAGEMENT-UX §4 Step 1`).

use crate::dom::components;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::theme_tokens;
use crate::views::access_log::output::{AccessEntry, AccessLogOutput, AccessOutcome};

use web_sys::Element;

pub fn render(container: &Element, output: &AccessLogOutput, _ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "access-log");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", "margin:0").ok();
    util::set_text(&h2, "Access Log");
    util::append(&wrapper, &h2);

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(
        &hint,
        "Live operations crossing the dispatch boundary — what ran, against which \
         target, and whether it was allowed or denied. Newest first.",
    );
    util::append(&wrapper, &hint);

    if !output.routing_active {
        util::append(
            &wrapper,
            &components::empty(
                "Access routing isn't wired for this peer — no operations to show.",
            ),
        );
        util::append(container, &wrapper);
        return;
    }

    if output.entries.is_empty() {
        util::append(
            &wrapper,
            &components::empty("No operations yet. Activity appears here as it happens."),
        );
        util::append(container, &wrapper);
        return;
    }

    // data-field lets the e2e assert the row count without scraping the table.
    let count = util::create_element("div");
    count.set_attribute("style", theme::HINT).ok();
    count.set_attribute("data-field", "access-log-count").ok();
    util::set_text(&count, &format!("operations: {}", output.entries.len()));
    util::append(&wrapper, &count);

    let (tbl, body) = components::table(&["Target", "Operation", "Result"]);
    for entry in &output.entries {
        util::append(&body, &row(entry));
    }
    util::append(&wrapper, &tbl);

    util::append(container, &wrapper);
}

fn row(entry: &AccessEntry) -> Element {
    components::tr(vec![
        components::td_text(&target_label(entry)),
        components::td_text(&entry.operation),
        components::td(&outcome_chip(entry)),
    ])
}

/// "handler @ peer12345" for a remote target, or just the handler for a local
/// dispatch — the "where" of the operation.
fn target_label(entry: &AccessEntry) -> String {
    match &entry.peer {
        Some(p) => {
            let short: String = p.chars().take(12).collect();
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

/// A small colored chip for the outcome; the raw status is a hover tooltip.
fn outcome_chip(entry: &AccessEntry) -> Element {
    let (label, color) = match entry.outcome {
        AccessOutcome::Allowed => ("Allowed", theme_tokens::STATUS_OK),
        AccessOutcome::Denied => ("Denied", theme_tokens::STATUS_ERR),
        AccessOutcome::Error => ("Error", theme_tokens::STATUS_WARN),
        AccessOutcome::Pending => ("…", theme_tokens::STATUS_INFO),
    };
    let chip = util::create_element("span");
    chip.set_attribute(
        "style",
        &format!("color:{color};font-size:12px;font-variant-numeric:tabular-nums"),
    )
    .ok();
    chip.set_attribute("title", &format!("status {}", entry.status)).ok();
    util::set_text(&chip, label);
    chip
}
