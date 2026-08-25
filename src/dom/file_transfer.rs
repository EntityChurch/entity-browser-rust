//! File Transfer DOM renderer — pure consumer of
//! [`FileTransferOutput`](crate::views::file_transfer::output::FileTransferOutput).
//!
//! List/Pull both dispatch through the shared `Action::Execute` path
//! (Action → handle_execute → ops::execute → event log), so results land in
//! the results pane below. Remote targets are reached as
//! `entity://{target}/local/files` — the bound peer's connection pool
//! resolves them (never the local peer registry).

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::event_log;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::file_transfer::output::FileTransferOutput;

use web_sys::Element;

pub fn render(container: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "file-transfer");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, "File Transfer");
    util::append(&wrapper, &h2);

    if !output.has_target {
        render_no_target_hint(&wrapper);
        util::append(container, &wrapper);
        return;
    }

    render_target_selector(&wrapper, output, ctx);
    render_list_button(&wrapper, output, ctx);
    render_pull_controls(&wrapper, output, ctx);
    render_upload_controls(&wrapper, output, ctx);
    render_results(&wrapper, output);

    util::append(container, &wrapper);
}

fn render_no_target_hint(parent: &Element) {
    let hint = util::create_element("p");
    hint.set_attribute("style", "color:var(--text-dim, #888);font-size:13px").ok();
    hint.set_inner_html(
        "No peer connected. Open <strong>Peer Connections</strong> and pair a \
         Tori-native backend (scan its QR or connect its <code>ws://</code> \
         address), then come back here to transfer files.",
    );
    util::append(parent, &hint);
}

fn render_target_selector(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&label, "From peer:");
    util::append(parent, &label);

    let select = util::create_element("select");
    select.set_attribute("style", theme::SELECT).ok();
    for option in &output.target_options {
        let opt = util::create_element("option");
        opt.set_attribute("value", &option.value).ok();
        if option.selected {
            opt.set_attribute("selected", "").ok();
        }
        util::set_text(&opt, &option.label);
        util::append(&select, &opt);
    }
    ctx.on_select_change(&select, "select_target");
    util::append(parent, &select);
}

fn render_list_button(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    let btn = util::create_element("button");
    util::set_text(&btn, "List shared files");
    btn.set_attribute("style", theme::BTN_SECONDARY).ok();

    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let peer_id = output.peer_id.clone();
    let target = output.selected_target.clone();
    let prefix = output.share_prefix.clone();

    ctx.listen(&btn, "click", move |_| {
        if target.is_empty() {
            return;
        }
        actions.borrow_mut().push(Action::Execute {
            peer_id: peer_id.clone(),
            handler_uri: format!("entity://{}/local/files", target),
            operation: "list".into(),
            resource: Some(prefix.clone()),
            params: None,
        });
        rp();
    });
    util::append(parent, &btn);
}

fn render_pull_controls(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&label, "File to pull:");
    util::append(parent, &label);

    // `tracked_input` preserves the typed value across the results-pane
    // rebuild the Execute triggers (event-log subscription → re-render).
    util::tracked_input(parent, ctx, "filename", &output.filename_initial, theme::INPUT);

    let btn = util::create_element("button");
    util::set_text(&btn, "Pull file");
    btn.set_attribute("style", theme::BTN_PRIMARY).ok();

    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let parent_ref = parent.clone();
    let peer_id = output.peer_id.clone();
    let target = output.selected_target.clone();
    let prefix = output.share_prefix.clone();
    let filename_fallback = output.filename_initial.clone();
    let window_id = ctx.window_id;

    ctx.listen(&btn, "click", move |_| {
        if target.is_empty() {
            return;
        }
        let filename = parent_ref
            .query_selector("[data-field='filename']")
            .ok()
            .flatten()
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
            .map(|inp| inp.value())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| filename_fallback.clone());

        // Persist the typed filename to the model before the Execute-driven
        // re-render reads `filename_initial` back (mirrors Execute Console's
        // set_resource-before-execute).
        actions.borrow_mut().push(Action::WindowEvent {
            window_id,
            event: "set_filename".into(),
            value: filename.clone(),
        });
        actions.borrow_mut().push(Action::DownloadFile {
            peer_id: peer_id.clone(),
            handler_uri: format!("entity://{}/local/files", target),
            path: format!("{}{}", prefix, filename),
            filename: filename.clone(),
        });
        rp();
    });
    util::append(parent, &btn);
}

fn render_upload_controls(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL).ok();
    util::set_text(&label, "Send a file to this peer:");
    util::append(parent, &label);

    // Hidden native file picker; the visible button triggers it.
    let input = util::create_element("input");
    input.set_attribute("type", "file").ok();
    input.set_attribute("style", "display:none").ok();
    util::append(parent, &input);

    let btn = util::create_element("button");
    util::set_text(&btn, "Upload a file");
    btn.set_attribute("style", theme::BTN_SECONDARY).ok();
    {
        let input_for_click = input.clone();
        ctx.listen(&btn, "click", move |_| {
            if let Ok(el) = input_for_click.clone().dyn_into::<web_sys::HtmlElement>() {
                el.click();
            }
        });
    }
    util::append(parent, &btn);

    // On selection, read the file's bytes and push an UploadFile action.
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let peer_id = output.peer_id.clone();
    let target = output.selected_target.clone();
    let prefix = output.share_prefix.clone();
    let input_ref = input.clone();
    ctx.listen(&input, "change", move |_| {
        if target.is_empty() {
            return;
        }
        let Ok(inp) = input_ref.clone().dyn_into::<web_sys::HtmlInputElement>() else {
            return;
        };
        let Some(files) = inp.files() else { return };
        if files.length() == 0 {
            return;
        }
        let Some(file) = files.get(0) else { return };
        let filename = file.name();
        let path = format!("{}{}", prefix, filename);
        let handler_uri = format!("entity://{}/local/files", target);

        let actions = actions.clone();
        let rp = rp.clone();
        let peer_id = peer_id.clone();
        // Consume the (fallible) array_buffer promise via JsFuture — a
        // dropped rejecting promise would reload the whole app (index.html
        // unhandledrejection guard).
        wasm_bindgen_futures::spawn_local(async move {
            match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
                Ok(buf) => {
                    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                    actions.borrow_mut().push(Action::UploadFile {
                        peer_id,
                        handler_uri,
                        path,
                        bytes,
                    });
                    rp();
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("file read failed for {}: {:?}", path, e).into(),
                    );
                }
            }
        });
    });
}

fn render_results(parent: &Element, output: &FileTransferOutput) {
    let header = util::create_element("h3");
    header.set_attribute("style", "margin:12px 0 4px;font-size:14px").ok();
    util::set_text(&header, "Results");
    util::append(parent, &header);

    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();
    if output.events.is_empty() {
        pre.set_inner_html(
            "<span style='color:var(--text-dim, #888)'>List or pull a file to see \
             results.</span>",
        );
    } else {
        let mut html = String::new();
        for entry in &output.events {
            let color = event_log::color_for(entry.category);
            let escaped = util::escape_html(&entry.message);
            html.push_str(&format!("<span style='color:{}'>{}</span>\n", color, escaped));
        }
        pre.set_inner_html(&html);
    }
    util::append(parent, &pre);
    util::schedule_scroll_to_bottom(&pre);
}
