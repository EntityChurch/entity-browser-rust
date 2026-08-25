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
    if !output.bound {
        render_start_picker(&wrapper, output, ctx);
    }
    render_messages(&wrapper, output);
    render_compose(&wrapper, ctx);

    util::append(container, &wrapper);
}

/// The start-a-chat picker, shown on the default single-peer conversation: a
/// button per connected peer that binds a 1:1 with them (`ChatStartWith`), or a
/// hint to connect a peer first. Once bound this section disappears.
fn render_start_picker(parent: &Element, output: &ChatOutput, ctx: &DomCtx) {
    let bar = util::create_element("div");
    bar.set_attribute(
        "style",
        "display:flex;flex-wrap:wrap;align-items:center;gap:6px;\
         margin-bottom:8px;flex-shrink:0",
    )
    .ok();

    if output.startable.is_empty() {
        let hint = util::create_element("span");
        hint.set_attribute("style", "font-size:12px;color:var(--text-faint, #666)")
            .ok();
        util::set_text(&hint, &crate::i18n::t("chat.start_hint", &[]));
        util::append(&bar, &hint);
    } else {
        let label = util::create_element("span");
        label
            .set_attribute("style", "font-size:12px;color:var(--text-dim, #888)")
            .ok();
        util::set_text(&label, &crate::i18n::t("chat.start_prompt", &[]));
        util::append(&bar, &label);

        let window_id = ctx.window_id;
        for peer in &output.startable {
            let btn = crate::dom::components::button_action(
                ctx,
                &peer.name,
                crate::dom::components::ButtonKind::Small,
                Action::ChatStartWith {
                    window_id,
                    peer_id: peer.peer_id.clone(),
                },
            );
            util::append(&bar, &btn);
        }
    }

    // Always offer a by-id start: paste a peer id + Enter. This is the
    // transport-agnostic bind — delivery establishes the connection (WebRTC or
    // WebSocket) lazily on the first dispatch — and it is what the two-browser
    // e2e drives, standing in for the not-yet-designed discovery/selection UI.
    render_start_by_id(&bar, ctx);

    util::append(parent, &bar);
}

/// The by-id start affordance: a draft-tracked peer-id input whose Enter binds a
/// 1:1 with that peer (`ChatStartWith`). Same all-Rust `<input>` + keydown
/// discipline as the compose box.
fn render_start_by_id(parent: &Element, ctx: &DomCtx) {
    let field_id = "chat-start-peer";
    let placeholder = crate::i18n::t("chat.start_by_id_placeholder", &[]);
    let input = crate::dom::components::text_input(ctx, field_id, "", &placeholder);
    input
        .set_attribute("data-field", "chat-start-peer")
        .ok();

    // Validation feedback: a malformed peer id would otherwise bind a
    // conversation that can never deliver and fail silently (empty list, no
    // error). The error span is populated by the handler and cleared on a valid
    // bind. Styled via the `.chat-start-error` class (dom/style.rs), not an
    // inline style, to keep the ui-lint atom/style ratchet flat.
    let error = util::create_element_with_class("span", "chat-start-error");
    error.set_attribute("data-field", "chat-start-error").ok();

    let window_id = ctx.window_id;
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let drafts = ctx.drafts.clone();
    let error_el = error.clone();

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
        let peer_id = target.value().trim().to_string();
        if peer_id.is_empty() {
            return;
        }
        // Reject a malformed id with visible feedback rather than binding a dead
        // conversation. Same Base58/46-char rule the write path enforces
        // (`content_site::resolver` uses it too).
        if !entity_entity::EntityUri::is_peer_id(&peer_id) {
            util::set_text(&error_el, &crate::i18n::t("chat.invalid_peer_id", &[]));
            return;
        }
        util::set_text(&error_el, "");
        target.set_value("");
        drafts.borrow_mut().remove(field_id);
        actions
            .borrow_mut()
            .push(Action::ChatStartWith { window_id, peer_id });
        rp();
    });

    util::append(parent, &input);
    util::append(parent, &error);
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

    render_reachability(parent, output);
}

/// Who is in this conversation and whether we can actually reach them — one
/// chip per other participant, in the app's single connection vocabulary (S4).
///
/// Nothing is painted for an unbound window: the self-conversation has no
/// remote, so a chip there would be a status about nobody.
fn render_reachability(parent: &Element, output: &ChatOutput) {
    if output.reachability.is_empty() && !output.no_establisher {
        return;
    }

    let row = util::create_element_with_class("div", "chat-reach");
    row.set_attribute("data-field", "chat-reachability").ok();

    for who in &output.reachability {
        let cell = util::create_element_with_class("span", "chat-reach-cell");

        let name = util::create_element_with_class("span", "chat-reach-name");
        util::set_text(&name, &who.label);
        util::append(&cell, &name);

        // `Unknown` has no chip by design — paired, but no current signal. A
        // quiet dash is the honest paint; "Connecting" would be a guess and
        // "Offline" would be a claim the kernel never made.
        match crate::dom::components::ConnState::from_display(who.status) {
            Some(state) => {
                util::append(&cell, &crate::dom::components::conn_chip(state));
            }
            None => {
                let dash = util::create_element_with_class("span", "chat-reach-unknown");
                util::set_text(&dash, "\u{2014}"); // — i18n-ignore — punctuation, not prose
                util::append(&cell, &dash);
            }
        }

        util::append(&row, &cell);
    }

    // The reason, when we have one. Deliberately a note and not a refusal: the
    // conversation is legitimate and messages still queue — it is the silence
    // about *why* nothing arrives that was the defect.
    if output.no_establisher {
        util::append(
            &row,
            &crate::dom::components::notice(&crate::i18n::t("chat.no_establisher", &[])),
        );
    }

    // *Why* the network could not carry it, when our own ICE agent can say.
    // Subordinate to the chips above, never a replacement: they say whether,
    // this says why, and the point of the whole classifier is that "this network
    // needs a relay" and "your friend is offline" stop arriving as one sentence.
    if let Some(advice) = output.reachability_advice {
        if let Some(key) = reachability_message_key(advice) {
            util::append(&row, &crate::dom::components::notice(&crate::i18n::t(key, &[])));
        }
    }

    util::append(parent, &row);
}

/// The catalog key for one advisory verdict, or `None` for the verdicts that
/// must stay silent.
///
/// The `None` arms are the enforcement of §3.2's rule, one layer out from the
/// classifier: `Unknown` and `Connected` have nothing to add to a state the
/// kernel already published, and a surface that invented a sentence for them
/// would be crying wolf during ordinary establishment.
fn reachability_message_key(v: crate::reachability::Reachability) -> Option<&'static str> {
    use crate::reachability::Reachability as R;
    match v {
        R::Unknown | R::Connected => None,
        R::NoCounterpart => Some("chat.reach_no_counterpart"),
        R::NoReflector => Some("chat.reach_no_reflector"),
        R::ReflectorUnreachable => Some("chat.reach_reflector_unreachable"),
        R::NoDirectPath => Some("chat.reach_no_direct_path"),
    }
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
            let who = m.author_label.as_str();
            html.push_str(&format!(
                "<div style='margin:3px 0;text-align:{align}'>\
                   <span style='font-size:10px;color:var(--text-dim, #888)'>{who} </span>\
                   <span style='font-size:13px;color:{color};white-space:pre-wrap;overflow-wrap:anywhere'>{body}</span>\
                 </div>",
                align = align,
                who = util::escape_html(who),
                color = color,
                body = util::escape_html(&m.body),
            ));
        }
        list.set_inner_html(&html);
    }

    util::append(parent, &list);
    util::schedule_scroll_to_bottom(&list);
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
    input.set_attribute("data-field", "chat-compose").ok();

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
