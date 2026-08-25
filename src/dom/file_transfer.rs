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
        // TWO chips, not one, because they answer different questions: can we
        // reach this device, and are we allowed to read from it. A target can be
        // authorized and offline, or up and refusing. Reachability first — it is
        // the precondition, so it reads left-to-right as the user's own
        // troubleshooting order.
        //
        // `Unknown` renders no chip (the shared `from_display` contract): a
        // remembered peer nobody has dialed this session has no honest status,
        // and "Offline" would be a claim the kernel never made.
        if let Some(state) = components::ConnState::from_display(output.target_reach) {
            util::append(&header, &components::conn_chip(state));
        }
        util::append(&header, &components::auth_chip(auth_state(&output.access)));
    }
    util::append(&wrapper, &header);

    if !output.has_target {
        render_no_target_hint(&wrapper);
        // ...and then keep going. Offering is NOT addressed to anybody: it
        // publishes on our side, so it works before a peer is remembered, and a
        // person can put the file up first and meet second. Returning here (as
        // this did while "send" meant only "push into their share") would hide
        // the browser↔browser send behind a precondition it does not have.
        render_offer_controls(&wrapper, output, ctx);
        render_results(&wrapper, output);
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

        // Send: push a file up — but only into a share that exists. Upload is
        // `local/files:write`, so against a peer that just answered
        // `handler_not_found` it is a card whose button can only reproduce the
        // error the browse half stopped showing. The way to send a file to a
        // browser peer is the Serve card below, which is why withholding this
        // one is not withholding the capability.
        if !output.share_absent {
            let send_group = components::card(&crate::i18n::t("filetransfer.send_file", &[]));
            render_upload_controls(&send_group, output, ctx);
            util::append(&wrapper, &send_group);
        }
    }

    // Serve: publish a file for the other side to pull. Outside the `denied`
    // gate for the same reason it survives "no target" — it is not a
    // conversation with the selected device.
    render_offer_controls(&wrapper, output, ctx);

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
    hint.set_inner_html(&crate::i18n::t(
        "filetransfer.no_peer_hint",
        &[
            (
                "window",
                &format!(
                    "<strong>{}</strong>",
                    util::escape_html(&crate::i18n::t("window.peer_connections", &[]))
                ),
            ),
            ("scheme", "<code>ws://</code>"),
        ],
    ));
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
    // e2e hook (the repo's `data-field` convention): the target picker is where
    // a harness — and a user — chooses which peer is serving.
    select.set_attribute("data-field", "ft-target").ok();
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
        // A re-ask in flight is shown BESIDE the listing, never instead of it.
        // The rows stay put while both halves (the share and what the peer
        // offers) answer again — clearing them was the "Refresh deletes my
        // file" bug, and a spinner with nothing under it is the same lie in a
        // politer font.
        if output.root_loading {
            util::append(&header, &components::loading(""));
        }
        let refresh = components::button(
            ctx,
            &crate::i18n::t("btn.refresh", &[]),
            components::ButtonKind::Small,
            "ft_refresh",
        );
        // The re-ask hook. Refresh re-lists BOTH sources (the share and what the
        // peer offers), which is what makes a withdrawal visible — so a harness
        // needs to be able to press it without matching on button text.
        refresh.set_attribute("data-field", "ft-refresh").ok();
        util::append(&header, &refresh);
        util::append(parent, &header);
    }

    // Error state (S5) — loud, specific. A peer that serves no share never
    // reaches here: that is an answer, not a fault, and it used to render a red
    // protocol error beside a transfer that was working through the offers half.
    if let Some(err) = &output.browse_error {
        util::append(parent, &components::error(err));
    }
    // …and the quiet counterpart, shown only when it is the reason the pane is
    // empty. Beside a peer's offered files an absent share needs no sentence.
    if output.share_absent && output.tree_rows.is_empty() {
        util::append(
            parent,
            &components::empty(&crate::i18n::t("filetransfer.no_share", &[])),
        );
    }

    if !output.root_listed {
        if output.root_loading {
            util::append(parent, &components::loading(""));
        } else {
            let browse = components::button(
                ctx,
                &crate::i18n::t("filetransfer.browse_shared", &[]),
                components::ButtonKind::Secondary,
                "ft_refresh",
            );
            // Same event, same hook: before the first listing this button IS
            // Refresh, and a harness should not have to know which of the two
            // is on screen.
            browse.set_attribute("data-field", "ft-refresh").ok();
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
    // e2e hooks. A row carries its own name rather than only its tree key,
    // because an OFFER's key is a content hash — a harness (or a human reading
    // the DOM) must be able to find "report.bin", which is the whole reason the
    // manifest exists.
    el.set_attribute("data-field", "ft-row").ok();
    el.set_attribute("data-row-path", &row.path).ok();
    // The NAME goes on the element that actually carries the click handler,
    // not on the row wrapper: a click dispatched at the wrapper does not reach
    // a listener bound to a child (events bubble up, never down), so a harness
    // — or any script — clicking the wrapper would silently select nothing.
    node.set_attribute("data-row-name", &row.name).ok();
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
    btn.set_attribute("data-field", "ft-pull").ok();
    // The plan comes from the model already decided — a share `read` or an
    // offer's content-closure walk. This layer must never learn which.
    match &output.selected_pull {
        Some(plan) => {
            let actions = ctx.actions.clone();
            let rp = ctx.repaint.clone();
            let peer_id = output.peer_id.clone();
            let target = output.selected_target.clone();
            let plan = plan.clone();
            ctx.listen(&btn, "click", move |_| {
                if target.is_empty() {
                    return;
                }
                actions.borrow_mut().push(Action::PullFile {
                    peer_id: peer_id.clone(),
                    target: target.clone(),
                    plan: plan.clone(),
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

/// Byte counts read the same here as in the model's own messages — one
/// implementation, in the model, because the first caller was a *refusal*
/// (`file_offer::MAX_OFFER_BYTES`) and a limit stated by the window must match
/// the limit stated by the error.
fn human_size(n: u64) -> String {
    crate::file_offer::human_bytes(n)
}

/// A hidden native file picker plus the button that opens it. Calls `on_file`
/// with `(filename, bytes)` once the user has chosen one.
///
/// Shared by the two sends this window has — push a file into a peer's share,
/// and offer one for a peer to pull — which are the same nine lines of picker
/// and differ only in the action they raise. `field` is the `data-field` hook on
/// the visible button; the input itself carries `{field}-input`, because a
/// harness cannot open a native file dialog and drives the input directly
/// instead (assigning `files` and dispatching `change` — the same entry point a
/// real choice takes).
fn file_picker(
    parent: &Element,
    ctx: &DomCtx,
    label: &str,
    kind: components::ButtonKind,
    field: &str,
    on_file: impl Fn(String, Vec<u8>) + 'static,
) {
    let input = util::create_element("input");
    input.set_attribute("type", "file").ok();
    // **Visually hidden, not `display:none`.** A `display:none` input is not
    // rendered at all, and several mobile browsers decline to open a picker for
    // an unrendered control — which presents as a button that does nothing,
    // with no error anywhere, because no `change` event is ever dispatched.
    // Reported from a real Android run as exactly that. Keeping it in the layout
    // at zero size costs nothing and removes a whole class of "it just doesn't
    // work on my phone".
    input
        .set_attribute(
            "style",
            "position:absolute;width:1px;height:1px;padding:0;margin:-1px;\
             overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap;border:0",
        )
        .ok();
    input.set_attribute("data-field", &format!("{field}-input")).ok();
    util::append(parent, &input);

    let btn = components::button_el(label, kind);
    btn.set_attribute("data-field", field).ok();
    {
        let input_for_click = input.clone();
        ctx.listen(&btn, "click", move |_| {
            if let Ok(el) = input_for_click.clone().dyn_into::<web_sys::HtmlElement>() {
                el.click();
            }
        });
    }
    util::append(parent, &btn);

    let on_file = std::rc::Rc::new(on_file);
    let input_ref = input.clone();
    let attempt = ctx.offer_attempt.clone();
    // Waking the window is a **dirty mark, not a repaint**. Sections rebuild
    // only when their `WindowWatch` says so, and `offer_attempt` lives in
    // memory precisely so it never becomes a tree entity — so no write fires
    // and no subscription fires. A bare `repaint()` schedules a frame that then
    // rebuilds nothing, which is the same silence one layer in. The window's
    // action handler marks itself dirty for any event it receives, so this
    // rides that; the app-side half of the same slot pokes the `DirtyFlag`
    // directly, which the DOM has no handle on.
    let rp = ctx.repaint.clone();
    let actions = ctx.actions.clone();
    let window_id = ctx.window_id;
    let wake = move || {
        actions.borrow_mut().push(Action::WindowEvent {
            window_id,
            event: "ft_wake".into(),
            value: String::new(),
        });
        rp();
    };
    let wake = std::rc::Rc::new(wake);
    ctx.listen(&input, "change", move |_| {
        let Ok(inp) = input_ref.clone().dyn_into::<web_sys::HtmlInputElement>() else {
            return;
        };
        let Some(files) = inp.files() else { return };
        let Some(file) = files.get(0) else { return };
        let filename = file.name();

        // Refuse on size BEFORE reading. `offer_file` refuses too and is the
        // authority, but it can only do so once the bytes are already in wasm
        // memory — and a camera-sized file is exactly where a phone's tab dies
        // partway through, which is indistinguishable from the button doing
        // nothing. `File.size` is free and needs no read.
        let size = file.size() as u64;
        if size > crate::file_offer::MAX_OFFER_BYTES {
            attempt.set_failed(&filename, &crate::file_offer::too_large_message(&filename, size));
            wake();
            return;
        }

        // Say that something is happening the moment a file is chosen. On a
        // phone this read is the slow step, and the silence across it is most of
        // what "the button doesn't work" meant.
        attempt.set_reading(&filename);
        wake();

        let on_file = on_file.clone();
        let attempt = attempt.clone();
        let wake = wake.clone();
        // Consume the (fallible) array_buffer promise via JsFuture — a
        // dropped rejecting promise would reload the whole app (index.html
        // unhandledrejection guard).
        wasm_bindgen_futures::spawn_local(async move {
            match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
                Ok(buf) => on_file(filename, js_sys::Uint8Array::new(&buf).to_vec()),
                Err(e) => {
                    // A read that fails has no action to ride, so without this
                    // it reached the user through the console and nowhere else.
                    let why = format!("{e:?}");
                    web_sys::console::error_1(
                        &format!("file read failed for {}: {:?}", filename, e).into(),
                    );
                    attempt.set_failed(&filename, &why);
                    wake();
                }
            }
        });
    });
}

fn render_upload_controls(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let peer_id = output.peer_id.clone();
    let target = output.selected_target.clone();
    let prefix = output.share_prefix.clone();
    let window_id = ctx.window_id;
    file_picker(
        parent,
        ctx,
        &crate::i18n::t("filetransfer.upload_file", &[]),
        components::ButtonKind::Secondary,
        "ft-upload",
        move |filename, bytes| {
            if target.is_empty() {
                return;
            }
            actions.borrow_mut().push(Action::UploadFile {
                peer_id: peer_id.clone(),
                handler_uri: format!("entity://{}/local/files", target),
                path: format!("{}{}", prefix, filename),
                bytes,
                window_id,
            });
            rp();
        },
    );
}

/// **Offer a file** — the serving half, and the only send that works between
/// two browsers (neither mounts a `local/files` share for the other to write
/// into). Publishing is untargeted: the file goes up on *our* side and any peer
/// we are willing to be read by can pull it, which is why this card renders with
/// no target selected and outside the authorization gate.
///
/// The list below the button is not decoration — it is the only place a person
/// can see **what strangers can read from them**, and the only place to take it
/// back down.
fn render_offer_controls(parent: &Element, output: &FileTransferOutput, ctx: &DomCtx) {
    let card = components::card(&crate::i18n::t("filetransfer.offering", &[]));

    // State the ceiling before the picker, not after a refusal (D13): the
    // model's limit, formatted here, so the two can never disagree.
    let hint = util::create_element("p");
    hint.set_attribute("style", theme::NOTE).ok();
    util::set_text(
        &hint,
        &crate::i18n::t(
            "filetransfer.offer_hint",
            &[("limit", &human_size(output.offer_limit))],
        ),
    );
    util::append(&card, &hint);

    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let peer_id = output.peer_id.clone();
        file_picker(
            &card,
            ctx,
            &crate::i18n::t("filetransfer.offer_file", &[]),
            components::ButtonKind::Primary,
            "ft-offer",
            move |filename, bytes| {
                actions.borrow_mut().push(Action::OfferFile {
                    peer_id: peer_id.clone(),
                    filename,
                    bytes,
                });
                rp();
            },
        );
    }

    render_offer_status(&card, ctx);

    if output.own_offers.is_empty() {
        util::append(&card, &components::empty(&crate::i18n::t("filetransfer.offer_none", &[])));
    } else {
        let (table, tbody) = components::table(&[
            &crate::i18n::t("filetransfer.col_file", &[]),
            &crate::i18n::t("filetransfer.col_size", &[]),
            "",
        ]);
        for offer in &output.own_offers {
            let stop = components::button_el(
                &crate::i18n::t("filetransfer.stop_offering", &[]),
                components::ButtonKind::Small,
            );
            // The hook goes on the button that carries the handler, and it
            // carries the FILE's name: an offer's id is a content hash, so the
            // name is the only handle a person (or a harness) has.
            stop.set_attribute("data-field", "ft-stop-offer").ok();
            stop.set_attribute("data-offer-name", &offer.name).ok();
            let actions = ctx.actions.clone();
            let rp = ctx.repaint.clone();
            let peer_id = output.peer_id.clone();
            let offer_id = offer.id.clone();
            let filename = offer.name.clone();
            ctx.listen(&stop, "click", move |_| {
                actions.borrow_mut().push(Action::WithdrawOffer {
                    peer_id: peer_id.clone(),
                    offer_id: offer_id.clone(),
                    filename: filename.clone(),
                });
                rp();
            });

            let row = components::tr(vec![
                components::td_text(&offer.name),
                components::td_text(&human_size(offer.size)),
                components::td(&stop),
            ]);
            // Row-level hook too: a harness asserting "this file is offered"
            // should not have to find the button to see the row.
            row.set_attribute("data-field", "ft-offer-row").ok();
            row.set_attribute("data-offer-name", &offer.name).ok();
            util::append(&tbody, &row);
        }
        util::append(&card, &table);
    }

    util::append(parent, &card);
}

/// What became of the last file offered, **beside the button that offered it**.
///
/// Every failure this button has — a file over the ceiling, an unrouted local
/// peer, a refused ingest, a read the browser could not complete — used to
/// arrive only as a line in the Results pane at the bottom of the window, and
/// on a phone that is not where anyone is looking. It was reported as *"I hit
/// the button, it doesn't work, doesn't give an error, doesn't show anything"*,
/// which is a correct report about the feedback whatever the cause turns out to
/// be.
///
/// **Success renders nothing.** The offer appears as a row in the table
/// immediately below, which is the authority on what is being served — a
/// success line above a list containing the same file is noise, and worse, it
/// would survive a withdrawal and contradict the list it sits on.
fn render_offer_status(parent: &Element, ctx: &DomCtx) {
    use crate::offer_attempt::OfferOutcome;
    let Some((filename, outcome)) = ctx.offer_attempt.read() else { return };
    let el = match outcome {
        // The catalog's own "Loading…" plus the file, because on a slow read
        // the *name* is what tells the user the right file was picked.
        OfferOutcome::Reading | OfferOutcome::Preparing => {
            // Slot + punctuation composition, not prose — both halves are
            // already a catalog string and a filename.
            components::loading(&format!("{filename} — {}", crate::i18n::t("state.loading", &[]))) // i18n-ignore
        }
        // The reason is model English inside a localized frame, the same known
        // shape as the port-mapping row's `{why}`. A translated "it failed" with
        // the reason dropped would be worse.
        OfferOutcome::Failed(why) => components::error(&why),
        OfferOutcome::Offered(_) => return,
    };
    el.set_attribute("data-field", "ft-offer-status").ok();
    util::append(parent, &el);
}

fn render_results(parent: &Element, output: &FileTransferOutput) {
    // A bounded group like its siblings (S2) — the results pane is the
    // window's feedback surface, not a floating tail.
    let card = components::card(&crate::i18n::t("filetransfer.results", &[]));

    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();
    // The window's feedback surface — where "✓ saved …" lands, and what a gate
    // reads to know a pull finished rather than merely started.
    pre.set_attribute("data-field", "ft-results").ok();
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
