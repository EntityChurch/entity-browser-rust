//! Shell window DOM renderer — status header + `<pre>` scrollback +
//! `<input>` prompt with keydown interception.
//!
//! The all-Rust pillar forbids xterm.js / readline; `<input>` +
//! Rust-side keydown handling gives history, completion, copy-paste,
//! and accessibility for free.

use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{Element, HtmlInputElement, KeyboardEvent};

use crate::action::Action;
use crate::dom::util::{self, DomCtx};
use crate::views::shell::output::{render_verb_output_lines, ScrollbackEntry, ShellOutput};
use entity_shell::{DispatchChunk, StreamChunk, VerbOutput};

pub fn render(container: &Element, output: &ShellOutput, ctx: &DomCtx) {
    util::clear_children(container);

    // Vertical flex column that fills the window-content area. Status
    // header + scrollback grow to fill; the prompt row pins at the
    // bottom so the input never disappears below the fold no matter
    // how much scrollback accumulates. The scrollback itself owns the
    // single scrollable region; outer chrome doesn't double-scroll.
    let wrapper = util::create_element_with_class("div", "shell");
    wrapper
        .set_attribute(
            "style",
            "display:flex;flex-direction:column;padding:12px;\
             min-height:400px;height:100%;box-sizing:border-box",
        )
        .ok();

    render_status(&wrapper, output);
    render_scrollback(&wrapper, output);
    render_prompt(&wrapper, output, ctx);
    render_footer(&wrapper, output);

    util::append(container, &wrapper);
}

fn render_status(parent: &Element, output: &ShellOutput) {
    let status = util::create_element("div");
    status
        .set_attribute(
            "style",
            "font-family:monospace;font-size:11px;color:#8a8;\
             margin-bottom:6px;flex-shrink:0",
        )
        .ok();
    util::set_text(&status, &format!("wd: {}", output.wd));
    util::append(parent, &status);
}

fn render_scrollback(parent: &Element, output: &ShellOutput) {
    let pre = util::create_element("pre");
    // Own style instead of the shared `theme::PRE_OUTPUT` — the
    // shell wants `flex:1` so the scrollback fills available vertical
    // space (rather than the shared 400px cap), and `min-height:0` so
    // the flex parent actually lets it shrink to fit.
    pre.set_attribute(
        "style",
        "background:var(--surface-sunken, #0a0a1a);padding:8px;border-radius:4px;\
         font-size:11px;flex:1 1 0;min-height:120px;overflow:auto;\
         white-space:pre-wrap;margin:0",
    )
    .ok();
    pre.set_attribute("data-field", "shell-scrollback").ok();

    if output.scrollback.is_empty() {
        pre.set_inner_html("<span style='color:var(--text-dim, #888)'>(scrollback cleared)</span>");
    } else {
        let mut html = String::new();
        for entry in &output.scrollback {
            append_entry_html(&mut html, entry.as_ref());
        }
        pre.set_inner_html(&html);
    }

    util::append(parent, &pre);

    // Auto-scroll on rebuild — shared with event log / exec console /
    // query results. See `util::schedule_scroll_to_bottom`.
    util::schedule_scroll_to_bottom(&pre);
}

/// Append the HTML for one scrollback entry. Each variant picks its
/// own color; multi-row variants (verb listings, tree dumps, entity
/// bodies) emit one `<span>` per row.
fn append_entry_html(html: &mut String, entry: &ScrollbackEntry) {
    match entry {
        ScrollbackEntry::PromptEcho { wd, line } => {
            push_row(html, COLOR_PROMPT, &format!("{} > {}", wd, line));
        }
        ScrollbackEntry::Result(out) => append_verb_output_html(html, out),
        ScrollbackEntry::Error(e) => push_row(html, COLOR_ERROR, &e.to_string()),
        ScrollbackEntry::StreamChunk(c) => match c {
            StreamChunk::Dispatched(t) => push_row(html, COLOR_INFO, t),
            StreamChunk::Line(t) => push_row(html, COLOR_LISTING, t),
            StreamChunk::Complete(t) => push_row(html, COLOR_SUCCESS, t),
            StreamChunk::Failed(e) => push_row(html, COLOR_ERROR, &e.to_string()),
        },
        ScrollbackEntry::DispatchChunk(c) => match c {
            DispatchChunk::Dispatched(t) | DispatchChunk::Progress(t) => {
                push_row(html, COLOR_INFO, t);
            }
            DispatchChunk::Complete(t) => push_row(html, COLOR_SUCCESS, t),
            DispatchChunk::Failed(e) => push_row(html, COLOR_ERROR, &e.to_string()),
        },
        ScrollbackEntry::Info(t) => push_row(html, COLOR_INFO, t),
        ScrollbackEntry::ErrorText(t) => push_row(html, COLOR_ERROR, t),
        ScrollbackEntry::Listing(t) => push_row(html, COLOR_LISTING, t),
    }
}

fn append_verb_output_html(html: &mut String, out: &VerbOutput) {
    match out {
        VerbOutput::Path(p) => push_row(html, COLOR_SUCCESS, p),
        VerbOutput::Message(m) => push_row(html, COLOR_SUCCESS, m),
        VerbOutput::Listing { sections } => {
            for section in sections {
                if let Some(h) = &section.header {
                    push_row(html, COLOR_INFO, h);
                }
                for e in &section.entries {
                    push_row(html, COLOR_LISTING, e);
                }
            }
        }
        VerbOutput::Entity(_) | VerbOutput::Tree(_) | VerbOutput::Info(_) => {
            for row in render_verb_output_lines(out) {
                let color = match out {
                    VerbOutput::Entity(_) => COLOR_ENTITY,
                    VerbOutput::Tree(_) => COLOR_LISTING,
                    _ => COLOR_INFO,
                };
                push_row(html, color, &row);
            }
        }
        // Streaming variants never reach the renderer as `Result` —
        // they are decomposed into per-chunk `ScrollbackEntry` rows
        // upstream. Defensive: render the placeholder.
        VerbOutput::Lines(_) | VerbOutput::Dispatch(_) => {
            push_row(html, COLOR_INFO, "(streaming output)");
        }
    }
}

fn push_row(html: &mut String, color: &str, text: &str) {
    let escaped = util::escape_html(text);
    html.push_str(&format!("<span style='color:{}'>{}</span>\n", color, escaped));
}

const COLOR_PROMPT: &str = "var(--text-dim, #888)";
const COLOR_INFO: &str = "#bbb";
const COLOR_SUCCESS: &str = crate::theme_tokens::STATUS_OK;
const COLOR_ERROR: &str = crate::theme_tokens::STATUS_ERR;
const COLOR_LISTING: &str = "#9ac";
const COLOR_ENTITY: &str = "#cb8";

fn render_prompt(parent: &Element, output: &ShellOutput, ctx: &DomCtx) {
    let row = util::create_element("div");
    row.set_attribute(
        "style",
        "display:flex;flex-wrap:wrap;align-items:center;gap:4px;\
         margin-top:6px;flex-shrink:0",
    )
    .ok();

    let label = util::create_element("span");
    label
        .set_attribute(
            "style",
            "font-family:monospace;font-size:12px;color:#6c6;flex-shrink:0",
        )
        .ok();
    util::set_text(&label, ">");
    util::append(&row, &label);

    let input = util::create_element("input");
    input.set_attribute("type", "text").ok();
    input.set_attribute("data-field", "shell-input").ok();
    input.set_attribute("value", &output.draft).ok();
    input.set_attribute("autocomplete", "off").ok();
    input.set_attribute("spellcheck", "false").ok();
    input.set_attribute("autofocus", "").ok();
    input
        .set_attribute(
            "style",
            "flex:1 1 200px;background:var(--surface-sunken, #0a0a1a);color:var(--text, #e0e0e0);border:1px solid var(--border-strong, #444);\
             padding:4px 8px;font-family:monospace;font-size:12px;border-radius:3px",
        )
        .ok();

    let window_id = ctx.window_id;
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();

    // Track in-progress typing into the model so any rebuild (exec
    // completion, scrollback append) restores the value into the
    // freshly-created input. handle_action returns false for
    // `set_draft` → no save_state → no subscription churn.
    {
        let actions = actions.clone();
        let rp = rp.clone();
        ctx.listen(&input, "input", move |evt: web_sys::Event| {
            let Some(target) = evt
                .target()
                .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
            else { return };
            actions.borrow_mut().push(Action::WindowEvent {
                window_id,
                event: "set_draft".into(),
                value: target.value(),
            });
            rp();
        });
    }

    ctx.listen(&input, "keydown", move |evt: web_sys::Event| {
        let Ok(kev) = evt.dyn_into::<KeyboardEvent>() else {
            return;
        };
        let key = kev.key();
        let target = match kev.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
            Some(t) => t,
            None => return,
        };

        match key.as_str() {
            "Enter" => {
                kev.prevent_default();
                let line = target.value();
                target.set_value("");
                actions.borrow_mut().push(Action::ShellSubmit {
                    window_id,
                    line,
                });
                rp();
            }
            "ArrowUp" => {
                kev.prevent_default();
                actions.borrow_mut().push(Action::ShellHistoryPrev {
                    window_id,
                    current: target.value(),
                });
                rp();
            }
            "ArrowDown" => {
                kev.prevent_default();
                actions.borrow_mut().push(Action::ShellHistoryNext {
                    window_id,
                    current: target.value(),
                });
                rp();
            }
            "Tab" => {
                kev.prevent_default();
                let partial = target.value();
                actions.borrow_mut().push(Action::ShellTabComplete {
                    window_id,
                    partial,
                });
                rp();
            }
            "l" | "L" if kev.ctrl_key() => {
                kev.prevent_default();
                actions.borrow_mut().push(Action::ShellClear(window_id));
                rp();
            }
            _ => {}
        }
    });

    util::append(&row, &input);
    util::append(parent, &row);

    // Restore focus to the input if the previous rebuild orphaned it.
    // `autofocus` only fires reliably on the first connection of an
    // element; subsequent rebuilds (Enter, history nav, exec
    // completion) lose focus because the prior input was detached.
    //
    // Heuristic: only refocus when focus is *orphaned* — `document
    // .activeElement` is BODY/HTML/the shadow host. If the user has
    // since clicked into another input/button, leave them alone.
    let input_for_focus = input.clone();
    let cb = Closure::once_into_js(move || {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
        let orphaned = match doc.active_element() {
            None => true,
            Some(el) => {
                let tag = el.tag_name().to_uppercase();
                tag == "BODY" || tag == "HTML" || el.id() == "dom-layer"
            }
        };
        if orphaned {
            if let Ok(input) = input_for_focus.dyn_into::<HtmlInputElement>() {
                input.focus().ok();
            }
        }
    });
    if let Some(win) = web_sys::window() {
        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.unchecked_ref(),
            0,
        );
    }
}

fn render_footer(parent: &Element, output: &ShellOutput) {
    let info = util::create_element("p");
    info.set_attribute(
        "style",
        "color:var(--text-faint, #666);margin-top:4px;font-size:10px;flex-shrink:0;\
         text-overflow:ellipsis;overflow:hidden;white-space:nowrap",
    )
    .ok();
    util::set_text(&info, &format!("State: {}", output.state_path));
    util::append(parent, &info);
}

