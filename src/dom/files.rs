//! Files window DOM renderer — pure consumer of
//! [`FilesOutput`](crate::views::files::output::FilesOutput).
//!
//! Places as filter chips, then the files in the selected place, each row
//! carrying exactly the actions its kind permits (`file_kinds::FileRef::actions`).
//! Every place says, in one line, whether other devices can see what is in it —
//! the private/listed distinction is the thing a person most needs to know
//! before pressing anything.

use crate::action::Action;
use crate::dom::components::{self, ButtonKind};
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::file_kinds::{FileAction, FileRef, FileRow, Place};
use crate::user_files::PrivateStore;
use crate::views::files::output::{FilesNotice, FilesOutput};
use crate::views::files::{self, AddInbox, Inbox};

use web_sys::Element;

pub(crate) fn place_label(place: Place) -> String {
    crate::i18n::t(
        match place {
            Place::MyFiles => "files.place_my_files",
            Place::AppFiles => "files.place_app_files",
            Place::Saves => "files.place_saves",
            Place::Workspace => "files.place_workspace",
            Place::Offered => "files.place_offered",
        },
        &[],
    )
}

fn place_hint(place: Place) -> String {
    crate::i18n::t(
        match place {
            Place::MyFiles => "files.hint_my_files",
            Place::AppFiles => "files.hint_app_files",
            Place::Saves => "files.hint_saves",
            Place::Workspace => "files.hint_workspace",
            Place::Offered => "files.hint_offered",
        },
        &[],
    )
}

pub fn render(container: &Element, output: &FilesOutput, ctx: &DomCtx, inbox: Inbox, adds: AddInbox) {
    util::clear_children(container);
    let wrapper = util::create_element_with_class("div", "files");
    wrapper.set_attribute("style", theme::SECTION).ok();
    wrapper.set_attribute("data-field", "files-window").ok();

    let header = util::create_element("div");
    header.set_attribute("style", theme::HEADER_ROW).ok();
    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::TITLE_INLINE).ok();
    util::set_text(&h2, &crate::i18n::t("window.files", &[]));
    util::append(&header, &h2);
    util::append(&wrapper, &header);

    let bar = components::filter_bar();
    bar.set_attribute("data-field", "files-places").ok();
    for (place, count) in &output.counts {
        let chip = components::filter_chip(
            ctx,
            &place_label(*place),
            *count,
            *place == output.place,
            files::PLACE_EVENT,
            place.key(),
        );
        util::append(&bar, &chip);
    }
    util::append(&wrapper, &bar);

    let hint_row = util::create_element("div");
    hint_row.set_attribute("style", theme::ROW_INLINE).ok();
    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    hint.set_attribute("data-field", "files-place-hint").ok();
    util::set_text(&hint, &place_hint(output.place));
    util::append(&hint_row, &hint);
    // Everything in this place, as one archive a person can open anywhere: a
    // `.zip` first, because it is the one every desktop opens by double-click
    // (Windows 10's Explorer cannot open a `.tar.gz`).
    if !output.rows.is_empty() {
        for (key, field, event) in [
            ("files.export_zip", "files-export-zip", files::EXPORT_ZIP_EVENT),
            ("files.export_tar", "files-export-tar", files::EXPORT_TAR_EVENT),
        ] {
            let b = components::button_el(&crate::i18n::t(key, &[]), ButtonKind::Small);
            b.set_attribute("data-field", field).ok();
            ctx.on_window_event(&b, "click", event, output.place.key());
            util::append(&hint_row, &b);
        }
    }
    util::append(&wrapper, &hint_row);

    if output.place == Place::Saves {
        render_import(&wrapper, ctx, inbox);
    }
    if output.place == Place::MyFiles {
        render_add(&wrapper, ctx, adds.clone());
    }
    // A file dropped anywhere on the window lands in My files, whichever place
    // is showing: dropping is the gesture, and making someone switch places
    // before it works would be a trap.
    wire_drop(&wrapper, ctx, adds);

    crate::dom::file_transfer::render_offer_status(&wrapper, ctx);
    if let Some(notice) = &output.notice {
        let el = match notice {
            FilesNotice::Info(msg) => components::success(msg),
            FilesNotice::Error(msg) => components::error(msg),
        };
        el.set_attribute("data-field", "files-notice").ok();
        util::append(&wrapper, &el);
    }

    if output.rows.is_empty() {
        let empty = components::empty(&crate::i18n::t("files.empty", &[]));
        empty.set_attribute("data-field", "files-empty").ok();
        util::append(&wrapper, &empty);
    } else {
        let (table, tbody) = components::table(&[
            &crate::i18n::t("filetransfer.col_file", &[]),
            &crate::i18n::t("filetransfer.col_size", &[]),
            "",
        ]);
        for row in &output.rows {
            util::append(&tbody, &render_row(row, output, ctx));
        }
        util::append(&wrapper, &table);
    }

    let note = util::create_element("p");
    note.set_attribute("style", theme::NOTE).ok();
    util::set_text(&note, &crate::i18n::t("files.private_note", &[]));
    util::append(&wrapper, &note);

    util::append(container, &wrapper);
}

/// Bring a file from this device into My files — privately. The drop hint
/// beside it says the other way in.
fn render_add(parent: &Element, ctx: &DomCtx, adds: AddInbox) {
    let row = util::create_element("div");
    row.set_attribute("style", theme::ROW_INLINE).ok();
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let window_id = ctx.window_id;
    let attempt = ctx.offer_attempt.clone();
    crate::dom::file_transfer::file_picker(
        &row,
        ctx,
        &crate::i18n::t("files.add_file", &[]),
        ButtonKind::Primary,
        "files-add",
        move |name, bytes| {
            attempt.clear();
            adds.borrow_mut().push((name, Ok(bytes)));
            actions.borrow_mut().push(Action::WindowEvent {
                window_id,
                event: files::ADD_EVENT.into(),
                value: String::new(),
            });
            rp();
        },
    );
    let hint = util::create_element("span");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &crate::i18n::t("files.drop_hint", &[]));
    util::append(&row, &hint);
    util::append(parent, &row);
}

/// Files dropped onto the window. Every file is read, size-checked first (a
/// camera-sized file must be refused before it is read into a phone's memory),
/// and queued for the window; the window keeps them.
fn wire_drop(wrapper: &Element, ctx: &DomCtx, adds: AddInbox) {
    use wasm_bindgen::JsCast;
    let over = |el: &Element, on: bool| {
        if on {
            el.set_attribute("data-drop-over", "").ok();
        } else {
            el.remove_attribute("data-drop-over").ok();
        }
    };
    {
        let el = wrapper.clone();
        ctx.listen(wrapper, "dragover", move |ev| {
            ev.prevent_default();
            over(&el, true);
        });
    }
    {
        let el = wrapper.clone();
        ctx.listen(wrapper, "dragleave", move |ev| {
            // Leaving a child is not leaving the window.
            let inside = ev
                .dyn_ref::<web_sys::DragEvent>()
                .and_then(|d| d.related_target())
                .and_then(|t| t.dyn_into::<web_sys::Node>().ok())
                .is_some_and(|n| el.contains(Some(&n)));
            if !inside {
                over(&el, false);
            }
        });
    }
    let el = wrapper.clone();
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let window_id = ctx.window_id;
    ctx.listen(wrapper, "drop", move |ev| {
        ev.prevent_default();
        over(&el, false);
        let Some(files) = ev.dyn_ref::<web_sys::DragEvent>().and_then(|d| d.data_transfer()).and_then(|t| t.files()) else {
            return;
        };
        let wake = {
            let actions = actions.clone();
            let rp = rp.clone();
            move || {
                actions.borrow_mut().push(Action::WindowEvent {
                    window_id,
                    event: files::ADD_EVENT.into(),
                    value: String::new(),
                });
                rp();
            }
        };
        for i in 0..files.length() {
            let Some(file) = files.get(i) else { continue };
            let name = file.name();
            let size = file.size() as u64;
            if size > crate::file_offer::MAX_OFFER_BYTES {
                adds.borrow_mut().push((name.clone(), Err(crate::file_offer::too_large_message(&name, size))));
                wake();
                continue;
            }
            let adds = adds.clone();
            let wake = wake.clone();
            // Consume the (fallible) promise: a dropped rejecting promise reloads
            // the app (index.html's unhandledrejection guard).
            wasm_bindgen_futures::spawn_local(async move {
                let got = wasm_bindgen_futures::JsFuture::from(file.array_buffer())
                    .await
                    .map(|buf| js_sys::Uint8Array::new(&buf).to_vec())
                    .map_err(|e| format!("{e:?}"));
                adds.borrow_mut().push((name, got));
                wake();
            });
        }
    });
}

/// The other half of the USB flow: pick a `.entitysave` file from this device.
fn render_import(parent: &Element, ctx: &DomCtx, inbox: Inbox) {
    let row = util::create_element("div");
    row.set_attribute("style", theme::ROW_INLINE).ok();
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let window_id = ctx.window_id;
    let attempt = ctx.offer_attempt.clone();
    crate::dom::file_transfer::file_picker(
        &row,
        ctx,
        &crate::i18n::t("files.import_save", &[]),
        ButtonKind::Secondary,
        "files-import-save",
        move |name, bytes| {
            // The picker reports a slow read on the shared offer slot; this is
            // not an offer, so the slot is cleared once the bytes are here and
            // the window's own notice takes over.
            attempt.clear();
            *inbox.borrow_mut() = Some((name, bytes));
            actions.borrow_mut().push(Action::WindowEvent {
                window_id,
                event: files::IMPORT_EVENT.into(),
                value: String::new(),
            });
            rp();
        },
    );
    util::append(parent, &row);
}

fn render_row(row: &FileRow, output: &FilesOutput, ctx: &DomCtx) -> Element {
    let name_cell = util::create_element("div");
    let name_el = util::create_element("div");
    util::set_text(&name_el, &row.name);
    util::append(&name_cell, &name_el);
    let mut detail = Vec::new();
    if !row.owner.is_empty() {
        detail.push(row.owner.clone());
    }
    if let Some(ms) = row.when_ms {
        detail.push(crate::dom::app_saves::stamp_label(ms));
    }
    if matches!(row.file, FileRef::Backup { .. }) {
        detail.push(crate::i18n::t("files.backup", &[]));
    }
    if !detail.is_empty() {
        let d = util::create_element("div");
        d.set_attribute("style", theme::HINT).ok();
        util::set_text(&d, &detail.join(" · ")); // i18n-ignore — separator between localized parts
        util::append(&name_cell, &d);
    }

    let buttons = util::create_element("div");
    buttons.set_attribute("style", theme::ROW_INLINE).ok();
    for action in row.file.actions() {
        if let Some(b) = action_button(action, row, output, ctx) {
            util::append(&buttons, &b);
        }
    }
    // Every file here is an entity at a path; say which, and open the tree on
    // it. A file you cannot find in the Entity Tree is not in the system.
    let tree_path = row.file.tree_path(&output.peer_id);
    let reveal = components::button_el(&crate::i18n::t("files.show_in_tree", &[]), ButtonKind::Small);
    reveal.set_attribute("data-field", "files-show-in-tree").ok();
    reveal.set_attribute("title", &tree_path).ok();
    ctx.on_action(&reveal, "click", Action::RevealInEntityTree { path: tree_path });
    util::append(&buttons, &reveal);

    let tr = components::tr(vec![
        components::td(&name_cell),
        components::td_text(&crate::file_offer::human_bytes(row.size)),
        components::td(&buttons),
    ]);
    tr.set_attribute("data-field", "files-row").ok();
    tr.set_attribute("data-file-name", &row.name).ok();
    tr.set_attribute("data-file-ref", &row.file.encode()).ok();
    tr
}

fn action_button(action: FileAction, row: &FileRow, output: &FilesOutput, ctx: &DomCtx) -> Option<Element> {
    let peer_id = output.peer_id.clone();
    let name = row.name.clone();
    let (label, field) = match action {
        FileAction::SaveToDevice => ("filetransfer.save_to_device", "files-save"),
        FileAction::DownloadSaveFile => ("files.download_save", "files-download-save"),
        FileAction::OfferToPeers => ("filetransfer.offer_to_peers", "files-offer"),
        FileAction::StopOffering => ("files.stop_offering", "files-stop-offering"),
        FileAction::Remove => ("files.remove", "files-remove"),
    };
    let kind = if action == FileAction::Remove { ButtonKind::Destructive } else { ButtonKind::Small };
    let b = components::button_el(&crate::i18n::t(label, &[]), kind);
    b.set_attribute("data-field", field).ok();
    b.set_attribute("data-file-name", &name).ok();

    match (action, &row.file) {
        (FileAction::SaveToDevice, FileRef::Kept { blob_hex, .. }) => ctx.on_action(
            &b,
            "click",
            Action::SaveKeptFile { peer_id, file_id: blob_hex.clone(), filename: name, store: PrivateStore::AppFiles },
        ),
        (FileAction::SaveToDevice, FileRef::Mine { blob_hex }) => ctx.on_action(
            &b,
            "click",
            Action::SaveKeptFile { peer_id, file_id: blob_hex.clone(), filename: name, store: PrivateStore::MyFiles },
        ),
        (FileAction::SaveToDevice, FileRef::Offer { blob_hex }) => ctx.on_action(
            &b,
            "click",
            Action::SaveOwnOffer { peer_id, offer_id: blob_hex.clone(), filename: name },
        ),
        (FileAction::SaveToDevice, FileRef::Work { .. }) => {
            ctx.on_window_event(&b, "click", files::DOWNLOAD_WORK_EVENT, &row.file.encode())
        }
        (FileAction::DownloadSaveFile, _) => {
            ctx.on_window_event(&b, "click", files::DOWNLOAD_SAVE_EVENT, &row.file.encode())
        }
        (FileAction::OfferToPeers, FileRef::Kept { blob_hex, .. }) => ctx.on_action(
            &b,
            "click",
            Action::OfferKeptFile { peer_id, file_id: blob_hex.clone(), filename: name, store: PrivateStore::AppFiles },
        ),
        (FileAction::OfferToPeers, FileRef::Mine { blob_hex }) => ctx.on_action(
            &b,
            "click",
            Action::OfferKeptFile { peer_id, file_id: blob_hex.clone(), filename: name, store: PrivateStore::MyFiles },
        ),
        (FileAction::StopOffering, FileRef::Offer { blob_hex }) => ctx.on_action(
            &b,
            "click",
            Action::WithdrawOffer { peer_id, offer_id: blob_hex.clone(), filename: name },
        ),
        (FileAction::Remove, _) => {
            // Removing is the one action here that loses something; ask first,
            // naming the file.
            let actions = ctx.actions.clone();
            let rp = ctx.repaint.clone();
            let window_id = ctx.window_id;
            let value = row.file.encode();
            let question = crate::i18n::t("files.confirm_remove", &[("name", &row.name)]);
            ctx.listen(&b, "click", move |_| {
                let yes = web_sys::window()
                    .and_then(|w| w.confirm_with_message(&question).ok())
                    .unwrap_or(false);
                if yes {
                    actions.borrow_mut().push(Action::WindowEvent {
                        window_id,
                        event: files::REMOVE_EVENT.into(),
                        value: value.clone(),
                    });
                    rp();
                }
            });
        }
        // A kind/ref pairing the table does not produce: no button rather than
        // a button that does nothing.
        _ => return None,
    }
    Some(b)
}
