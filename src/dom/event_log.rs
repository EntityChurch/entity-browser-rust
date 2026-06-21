//! Event Log DOM renderer — pure consumer of
//! [`EventLogOutput`](crate::views::event_log::output::EventLogOutput).

use crate::action::Action;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::event_log::output::EventLogOutput;
use crate::views::EventCategory;

use web_sys::Element;

pub fn render(container: &Element, output: &EventLogOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "event-log");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let header = util::create_element("div");
    header.set_attribute("style", theme::HEADER_ROW).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", "margin:0").ok();
    util::set_text(&h2, "Event Log");
    util::append(&header, &h2);

    let clear_btn = util::create_element("button");
    util::set_text(&clear_btn, "Clear");
    clear_btn.set_attribute("style", theme::BTN_SMALL).ok();
    ctx.on_action(&clear_btn, "click", Action::ClearEventLog);
    util::append(&header, &clear_btn);
    util::append(&wrapper, &header);

    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();

    if output.events.is_empty() {
        pre.set_inner_html("<span style='color:var(--text-dim, #888)'>(no events yet)</span>");
    } else {
        let mut html = String::new();
        for entry in &output.events {
            let color = color_for(entry.category);
            let escaped = util::escape_html(&entry.message);
            html.push_str(&format!("<span style='color:{}'>{}</span>\n", color, escaped));
        }
        pre.set_inner_html(&html);
    }

    util::append(&wrapper, &pre);
    // Pin the scroll to the bottom after attachment so new event-log
    // entries are visible without manual scrolling.
    util::schedule_scroll_to_bottom(&pre);
    util::append(container, &wrapper);
}

pub(crate) fn color_for(category: EventCategory) -> &'static str {
    match category {
        EventCategory::Success => crate::theme_tokens::STATUS_OK,
        EventCategory::Failure => crate::theme_tokens::STATUS_ERR,
        EventCategory::Info => crate::theme_tokens::STATUS_INFO,
        EventCategory::Neutral => "#ccc",
    }
}
