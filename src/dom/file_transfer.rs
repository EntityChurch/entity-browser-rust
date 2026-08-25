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
use crate::dom::components::{self, AuthState};
use crate::dom::event_log;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::file_transfer::output::{FileRow, FileTransferOutput, TargetAccess};

use web_sys::Element;

pub fn render(container: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "file-transfer");
    wrapper.set_attribute("style", theme::SECTION).ok();

    // Header row: the job (named for the connected device) + the ONE access
    // chip on the right (S4 — the single source of authorization status; no
    // window below restates it).
    let header = util::create_element("div");
    header.set_attribute("style", theme::HEADER_ROW).ok();
    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::TITLE_INLINE).ok();
    util::set_text(&h2, &title_for(output));
    util::append(&header, &h2);
    if output.has_target {
        util::append(&header, &components::auth_chip(auth_state(&output.access)));
    }
    util::append(&wrapper, &header);

    if !output.has_target {
        render_no_target_hint(&wrapper);
        util::append(container, &wrapper);
        return;
    }

    // Which device (only surfaced when there's a choice) + the actionable
    // "not authorized" path. The status itself is the header chip.
    let denied = matches!(output.access, TargetAccess::Denied);
    if output.target_options.len() > 1 || denied {
        let peer_group = components::card(&crate::i18n::t("filetransfer.device", &[]));
        if output.target_options.len() > 1 {
            render_target_selector(&peer_group, output, ctx);
        }
        render_access(&peer_group, output, ctx);
        util::append(&wrapper, &peer_group);
    }

    // Gate the transfer surfaces behind authorization (progressive disclosure,
    // S5). A denied target can neither browse nor send — showing those cards
    // would only surface the raw 403 as browse_error + a dead upload button.
    // The Device card above already carries the actionable "authorize" path;
    // here we simply withhold the surfaces until access is granted.
    if !denied {
        // Get: browse the share as a tree, then pull a file.
        let get_group = components::card(&crate::i18n::t("filetransfer.shared_files", &[]));
        render_file_browser(&get_group, output, ctx);
        util::append(&wrapper, &get_group);

        // Send: push a file up.
        let send_group = components::card(&crate::i18n::t("filetransfer.send_file", &[]));
        render_upload_controls(&send_group, output, ctx);
        util::append(&wrapper, &send_group);
    }

    render_results(&wrapper, output);

    util::append(container, &wrapper);
}

/// The window's job, named for the connected device (S6 — human terms). Falls
/// back to a plain title when nothing is connected.
fn title_for(output: &FileTransferOutput) -> String {
    if !output.has_target {
        return crate::i18n::window_title("File Transfer"); // i18n-ignore — lookup key
    }
    match output.target_options.iter().find(|o| o.selected) {
        Some(o) if !o.label.is_empty() => {
            crate::i18n::t("filetransfer.title_files", &[("label", &o.label)])
        }
        _ => crate::i18n::window_title("File Transfer"), // i18n-ignore — lookup key
    }
}

/// Map the target's access to the shared authorization vocabulary (S4). File
/// Transfer is a read-only *reflection* of authorization, never its own words.
fn auth_state(access: &TargetAccess) -> AuthState {
    match access {
        TargetAccess::Authorized(_) => AuthState::Authorized,
        TargetAccess::Denied => AuthState::NotAuthorized,
        TargetAccess::Unknown => AuthState::Unverified,
    }
}

fn render_no_target_hint(parent: &Element) {
    let hint = util::create_element("p");
    hint.set_attribute("style", &format!("color:var(--text-dim, #888);{}", theme::NOTE)).ok();
    hint.set_inner_html(
        "No peer connected. Open <strong>Peer Connections</strong> and pair a \
         Tori-native backend (scan its QR or connect its <code>ws://</code> \
         address), then come back here to transfer files.",
    );
    util::append(parent, &hint);
}

fn render_target_selector(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    let options: Vec<(&str, &str)> = output
        .target_options
        .iter()
        .map(|o| (o.value.as_str(), o.label.as_str()))
        .collect();
    let selected = output
        .target_options
        .iter()
        .find(|o| o.selected)
        .map(|o| o.value.as_str())
        .unwrap_or("");
    let select = components::select(ctx, &options, selected, "select_target");
    util::append(
        parent,
        &components::field(&crate::i18n::t("filetransfer.from_peer", &[]), "", &select),
    );
}

/// Access affordance for the effective target (`§2.1`) — honest and
/// evidence-based. Only a **real** refusal (a `403`/`401` from an actual
/// operation, captured as [`TargetAccess::Denied`]) raises the deep-link into
/// **Peer Connections**, the authority surface. An authorized/unknown target
/// shows only a quiet hint — File Transfer never manufactures a "not
/// authorized" claim from an empty local mirror. It is a *consumer* of
/// authorization; it never authors grants inline.
fn render_access(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    // The header chip is the single source of authorization *status* (S4). Here
    // we add only the actionable *path* when access is refused — no restatement.
    if matches!(output.access, TargetAccess::Denied) {
        let msg = util::create_element("p");
        msg.set_attribute("style", theme::NOTE).ok();
        util::set_text(&msg, &crate::i18n::t("filetransfer.needs_auth", &[]));
        util::append(parent, &msg);

        // Deep-link to the authority surface — focuses the singleton Peer
        // Connections window (or spawns it) where the grant is made.
        let btn = components::button_action(
            ctx,
            &crate::i18n::t("filetransfer.authorize_device", &[]),
            components::ButtonKind::Primary,
            Action::SpawnWindow { type_name: "Peer Connections", peer_id: None }, // i18n-ignore — identity key; registry lookup
        );
        util::append(parent, &btn);
    }
}

/// The share browsed as a **tree** — a Refresh control, the flattened rows
/// (built from the shared `TreeNode` machinery in the model), and a Pull button
/// for the selected file. Directories lazy-load on expand; nothing here reaches
/// the tree directly — it renders `output.tree_rows`.
fn render_file_browser(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    // The card already titles this group ("Shared files"); here we only offer a
    // Refresh once the share is listed — right-aligned (external changes only;
    // our own writes re-list themselves per S5).
    if output.root_listed {
        let header = util::create_element("div");
        header.set_attribute("style", theme::ROW_END).ok();
        let refresh = components::button(
            ctx,
            &crate::i18n::t("btn.refresh", &[]),
            components::ButtonKind::Small,
            "ft_refresh",
        );
        util::append(&header, &refresh);
        util::append(parent, &header);
    }

    // Error state (S5) — loud, specific.
    if let Some(err) = &output.browse_error {
        util::append(parent, &components::error(err));
    }

    if !output.root_listed {
        if output.root_loading {
            util::append(parent, &components::loading("")); // "Loading…"
        } else {
            let browse = components::button(
                ctx,
                &crate::i18n::t("filetransfer.browse_shared", &[]),
                components::ButtonKind::Secondary,
                "ft_refresh",
            );
            util::append(parent, &browse);
        }
        return;
    }

    let list = util::create_element("div");
    list.set_attribute("style", theme::SCROLL_LIST).ok();
    list.set_attribute("data-scroll-key", "file-transfer-tree").ok();
    if output.tree_rows.is_empty() {
        // Empty state (S5) — a helpful line, not a void.
        util::append(&list, &components::empty(&crate::i18n::t("filetransfer.share_empty", &[])));
    } else {
        for row in &output.tree_rows {
            render_tree_row(&list, row, ctx);
        }
    }
    util::append(parent, &list);

    render_pull_selected(parent, output, ctx);
}

fn render_tree_row(list: &Element, row: &FileRow, ctx: &DomCtx) {
    let icon = if row.is_dir { "\u{1f4c1}" } else { "\u{1f4c4}" }; // 📁 / 📄
    let mut label = format!("{icon} {}", row.name);
    if row.loading {
        label.push_str(" …");
    } else if let Some(sz) = row.size {
        label.push_str(&format!("  ({})", human_size(sz)));
    }

    let (el, caret, node) =
        components::tree_row(row.depth, row.is_dir, row.expanded, row.selected, &label);
    if let Some(caret) = caret {
        ctx.on_window_event(&caret, "click", "ft_toggle", &row.path);
    }
    // Directory rows toggle; file rows select (highlight → Pull).
    let event = if row.is_dir { "ft_toggle" } else { "ft_select" };
    ctx.on_window_event(&node, "click", event, &row.path);

    util::append(list, &el);
}

/// Pull the currently-selected file. Inert (dimmed, no handler) when nothing is
/// selected — reuses the proven `Action::DownloadFile` path.
fn render_pull_selected(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    let btn = components::button_el(&crate::i18n::t("filetransfer.pull_selected", &[]), components::ButtonKind::Primary);
    match &output.selected_full_path {
        Some(path) => {
            let actions = ctx.actions.clone();
            let rp = ctx.repaint.clone();
            let peer_id = output.peer_id.clone();
            let target = output.selected_target.clone();
            let path = path.clone();
            ctx.listen(&btn, "click", move |_| {
                if target.is_empty() {
                    return;
                }
                let filename = path.rsplit('/').next().unwrap_or("file").to_string();
                actions.borrow_mut().push(Action::DownloadFile {
                    peer_id: peer_id.clone(),
                    handler_uri: format!("entity://{}/local/files", target),
                    path: path.clone(),
                    filename,
                });
                rp();
            });
        }
        None => {
            components::disable(&btn);
        }
    }
    util::append(parent, &btn);
}

fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B") // i18n-ignore — byte unit, language-neutral
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn render_upload_controls(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    // Hidden native file picker; the visible button triggers it.
    let input = util::create_element("input");
    input.set_attribute("type", "file").ok();
    input.set_attribute("style", "display:none").ok();
    util::append(parent, &input);

    let btn = components::button_el(&crate::i18n::t("filetransfer.upload_file", &[]), components::ButtonKind::Secondary);
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
    let window_id = ctx.window_id;
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
                        window_id,
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
    // A bounded group like its siblings (S2) — the results pane is the
    // window's feedback surface, not a floating tail.
    let card = components::card(&crate::i18n::t("filetransfer.results", &[]));

    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();
    if output.events.is_empty() {
        pre.set_inner_html(&format!(
            "<span style='color:var(--text-dim, #888)'>{}</span>",
            util::escape_html(&crate::i18n::t("filetransfer.no_results", &[]))
        ));
    } else {
        let mut html = String::new();
        for entry in &output.events {
            let color = event_log::color_for(entry.category);
            let escaped = util::escape_html(&entry.message);
            html.push_str(&format!("<span style='color:{}'>{}</span>\n", color, escaped));
        }
        pre.set_inner_html(&html);
    }
    util::append(&card, &pre);
    util::append(parent, &card);
    util::schedule_scroll_to_bottom(&pre);
}
