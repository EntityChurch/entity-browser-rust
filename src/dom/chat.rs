//! Chat window DOM renderer — conversation header + scrollable message list +
//! a compose `<input>` (Enter → `ChatSend`). Same all-Rust `<input>` + keydown
//! discipline as the shell prompt (no third-party chat widget).

use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlInputElement, KeyboardEvent};

use crate::action::Action;
use crate::dom::util::{self, DomCtx};
use crate::views::chat::output::ChatOutput;

pub fn render(container: &Element, output: &ChatOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "chat");
    wrapper
        .set_attribute(
            "style",
            "display:flex;flex-direction:column;padding:12px;\
             min-height:400px;height:100%;box-sizing:border-box",
        )
        .ok();

    render_header(&wrapper, output);
    render_messages(&wrapper, output);
    render_compose(&wrapper, ctx);

    util::append(container, &wrapper);
}

fn render_header(parent: &Element, output: &ChatOutput) {
    let header = util::create_element("div");
    header
        .set_attribute(
            "style",
            "font-family:monospace;font-size:11px;color:var(--text-dim, #888);\
             margin-bottom:6px;flex-shrink:0",
        )
        .ok();
    util::set_text(
        &header,
        &crate::i18n::t("chat.conversation", &[("id", &output.conversation_id)]),
    );
    util::append(parent, &header);
}

fn render_messages(parent: &Element, output: &ChatOutput) {
    let list = util::create_element("div");
    list.set_attribute(
        "style",
        "background:var(--surface-sunken, #0a0a1a);padding:8px;border-radius:4px;\
         flex:1 1 0;min-height:120px;overflow:auto;margin:0",
    )
    .ok();
    list.set_attribute("data-field", "chat-messages").ok();

    if output.messages.is_empty() {
        list.set_inner_html(&format!(
            "<span style='color:var(--text-faint, #666);font-size:12px'>{}</span>",
            util::escape_html(&crate::i18n::t("chat.empty", &[]))
        ));
    } else {
        let mut html = String::new();
        for m in &output.messages {
            // Own messages accent-colored + end-aligned; others muted + start.
            // Logical `start`/`end` (not `left`/`right`) so RTL locales mirror
            // correctly — and it keeps this file off the i18n physical-direction
            // lint.
            let (align, color) = if m.mine {
                ("end", "var(--accent, #9ac)")
            } else {
                ("start", "var(--text, #e0e0e0)")
            };
            let who = short_author(&m.author);
            html.push_str(&format!(
                "<div style='margin:3px 0;text-align:{align}'>\
                   <span style='font-size:10px;color:var(--text-dim, #888)'>{who} </span>\
                   <span style='font-size:13px;color:{color};white-space:pre-wrap'>{body}</span>\
                 </div>",
                align = align,
                who = util::escape_html(&who),
                color = color,
                body = util::escape_html(&m.body),
            ));
        }
        list.set_inner_html(&html);
    }

    util::append(parent, &list);
    util::schedule_scroll_to_bottom(&list);
}

/// Abbreviate a peer-id for the byline — first 6 chars, enough to tell authors
/// apart without a wall of Base58.
fn short_author(author: &str) -> String {
    let head: String = author.chars().take(6).collect();
    format!("{}…", head)
}

fn render_compose(parent: &Element, ctx: &DomCtx) {
    let row = util::create_element("div");
    row.set_attribute(
        "style",
        "display:flex;align-items:center;gap:4px;margin-top:6px;flex-shrink:0",
    )
    .ok();

    // The shared draft-tracked atom: typing survives a subscription re-render
    // (an arriving message) via `ctx.drafts`, with no per-keystroke tree write.
    // `initial` is empty because `drafts` restores the in-progress value across
    // rebuilds.
    let field_id = "chat-input";
    let placeholder = crate::i18n::t("chat.placeholder", &[]);
    let input = crate::dom::components::text_input(ctx, field_id, "", &placeholder);

    let window_id = ctx.window_id;
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let drafts = ctx.drafts.clone();

    ctx.listen(&input, "keydown", move |evt: web_sys::Event| {
        let Ok(kev) = evt.dyn_into::<KeyboardEvent>() else {
            return;
        };
        if kev.key() != "Enter" {
            return;
        }
        let Some(target) = kev
            .target()
            .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
        else {
            return;
        };
        kev.prevent_default();
        let body = target.value();
        // Clear both the live input AND its tracked draft, or the next rebuild
        // (triggered by the message we just sent landing) would repopulate the
        // box with the text we already sent.
        target.set_value("");
        drafts.borrow_mut().remove(field_id);
        actions.borrow_mut().push(Action::ChatSend { window_id, body });
        rp();
    });

    util::append(&row, &input);
    util::append(parent, &row);
}
