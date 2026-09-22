//! The Saves panel — the Apps window's third view.
//!
//! Grid, player, and this. It is a *view*, not a modal: it replaces the window
//! body and its open/expanded state is persisted like every other structural
//! choice in that window, so a reload puts you back where you were.
//!
//! Built entirely from `components::` atoms and `theme::` tokens — a save is
//! ordinary tabular data and gets the app's ordinary table, buttons and cards.
//! Nothing here needs a look of its own, and inventing one is how two surfaces
//! that mean the same thing end up looking different.

use web_sys::Element;

use crate::apps::saves::{BackupRow, SaveRow};
use crate::dom::components::{self, ButtonKind};
use crate::dom::theme;
use crate::dom::util;
use crate::dom::DomCtx;
use crate::views::games::{
    SAVES_BACKUP_EVENT, SAVES_DOWNLOAD_EVENT, SAVES_DROP_EVENT, SAVES_FOCUS_EVENT,
    SAVES_IMPORT_EVENT, SAVES_PANEL_EVENT, SAVES_PICKED_EVENT, SAVES_RESTORE_EVENT,
    SAVES_SCAN_EVENT, SAVES_SEND_EVENT, SAVES_TARGET_EVENT,
};

/// Everything the panel renders, resolved by the window.
pub struct SavesView<'a> {
    /// Every live save on this peer, its display name where the catalog still
    /// knows one, and how many backups it has.
    pub saves: Vec<(SaveRow, String, usize)>,
    /// `"{set}/{id}"` whose backups are expanded, `""` for none.
    pub focus: &'a str,
    /// The expanded row's backups, newest first. Empty unless `focus` names one.
    pub backups: Vec<BackupRow>,
    /// `(peer id, label)` for every peer we could send to or ask.
    pub peers: Vec<(String, String)>,
    /// The peer currently chosen.
    pub target: &'a str,
    /// `(offer id, app name, set, id)` for each save that peer is offering.
    pub found: Vec<(String, String, String, String)>,
    /// The last thing an action said, already written for a person.
    pub status: &'a str,
    pub busy: bool,
    /// Heads the back button — the launcher's own title.
    pub back_label: &'a str,
    /// Where a picked save file waits for the window (`SavesUi::picked`).
    pub saves_ui: std::rc::Rc<std::cell::RefCell<crate::views::games::SavesUi>>,
}

/// Render the panel into `container`.
pub fn render(container: &Element, ctx: &DomCtx, view: &SavesView) {
    util::clear_children(container);

    let wrap = util::create_element("div");
    util::set_attr(&wrap, "style", theme::PANEL_SURFACE);

    let head = util::create_element("div");
    util::set_attr(&head, "style", theme::ROW_START);
    let back = components::button_value(
        ctx,
        &format!("\u{2190} {}", view.back_label),
        ButtonKind::Small,
        SAVES_PANEL_EVENT,
        "",
    );
    util::append(&head, &back);
    let title = util::create_element("div");
    util::set_attr(&title, "style", theme::PANEL_TITLE);
    util::set_text(&title, &crate::i18n::t("saves.open", &[]));
    util::append(&head, &title);
    util::append(&wrap, &head);

    // The status line sits ABOVE the tables, not beside the button that
    // produced it: every action here reports through one place, so a failure
    // cannot land somewhere the user has already scrolled past.
    if !view.status.is_empty() {
        util::append(&wrap, &components::notice(view.status));
    }
    if view.busy {
        util::append(&wrap, &components::loading(&crate::i18n::t("status.loading", &[])));
    }

    util::append(&wrap, &saves_card(ctx, view));
    util::append(&wrap, &file_card(ctx, view));
    util::append(&wrap, &import_card(ctx, view));
    util::append(container, &wrap);
}

/// The list of this peer's saves, each with its actions and (when expanded) its
/// backups.
fn saves_card(ctx: &DomCtx, view: &SavesView) -> Element {
    let card = components::card(&crate::i18n::t("saves.open", &[]));
    if view.saves.is_empty() {
        util::append(&card, &components::empty(&crate::i18n::t("saves.empty", &[])));
        return card;
    }

    let (table, body) = components::table(&[
        &crate::i18n::t("label.name", &[]),
        &crate::i18n::t("saves.col_size", &[]),
        "",
    ]);
    for (row, name, backup_count) in &view.saves {
        let key = format!("{}/{}", row.set, row.id);
        let expanded = view.focus == key;

        let actions = util::create_element("div");
        util::set_attr(&actions, "style", theme::ROW_END);
        util::append(
            &actions,
            &components::button_value(
                ctx,
                &crate::i18n::t("saves.backup", &[]),
                ButtonKind::Small,
                SAVES_BACKUP_EVENT,
                &key,
            ),
        );
        util::append(&actions, &download_button(ctx, &key));
        util::append(
            &actions,
            &components::button_value(
                ctx,
                &crate::i18n::t("saves.send", &[]),
                ButtonKind::Small,
                SAVES_SEND_EVENT,
                &key,
            ),
        );
        // Expanding is a toggle: pressing the open row's own button collapses
        // it, so the list cannot get stuck open on a row you are done with.
        util::append(
            &actions,
            &components::button_value(
                ctx,
                // This row's own count. It used to read `view.backups`, which
                // holds only the EXPANDED row's backups, so every other row
                // showed that row's number (found by the Saves panel e2e).
                &crate::i18n::t("saves.backups", &[("n", &backup_count.to_string())]),
                ButtonKind::Small,
                SAVES_FOCUS_EVENT,
                if expanded { "" } else { &key },
            ),
        );

        util::append(
            &body,
            &components::tr(vec![
                components::td_text(name),
                components::td_text(&crate::file_offer::human_bytes(row.bytes as u64)),
                components::td(&actions),
            ]),
        );

        if expanded {
            util::append(&body, &components::tr(vec![backups_cell(ctx, view, &key)]));
        }
    }
    util::append(&card, &table);
    card
}

/// The expanded row's backups, as a nested cell spanning the row.
fn backups_cell(ctx: &DomCtx, view: &SavesView, key: &str) -> Element {
    let cell = util::create_element("td");
    util::set_attr(&cell, "colspan", "3");
    util::set_attr(&cell, "style", theme::TD_NESTED);
    if view.backups.is_empty() {
        util::append(
            &cell,
            &components::empty(&crate::i18n::t("saves.no_backups", &[])),
        );
        return cell;
    }
    for b in &view.backups {
        let row = util::create_element("div");
        util::set_attr(&row, "style", theme::ROW_INLINE);
        let when = util::create_element("span");
        util::set_attr(&when, "style", theme::HINT);
        util::set_text(
            &when,
            &format!(
                "{}  ·  {}",
                stamp_label(b.stamp_ms),
                crate::file_offer::human_bytes(b.bytes as u64)
            ),
        );
        util::append(&row, &when);
        let reference = format!("{key}/{}", b.stamp_ms);
        util::append(
            &row,
            &components::button_value(
                ctx,
                &crate::i18n::t("saves.restore", &[]),
                ButtonKind::Small,
                SAVES_RESTORE_EVENT,
                &reference,
            ),
        );
        util::append(&row, &download_button(ctx, &reference));
        util::append(
            &row,
            &components::button_value(
                ctx,
                &crate::i18n::t("btn.delete", &[]),
                ButtonKind::Destructive,
                SAVES_DROP_EVENT,
                &reference,
            ),
        );
        util::append(&cell, &row);
    }
    cell
}

/// "Download save file" for a save (`{set}/{id}`) or a backup
/// (`{set}/{id}/{stamp}`) — the same words the Files window uses for the same act.
fn download_button(ctx: &DomCtx, reference: &str) -> Element {
    let b = components::button_value(
        ctx,
        &crate::i18n::t("files.download_save", &[]),
        ButtonKind::Small,
        SAVES_DOWNLOAD_EVENT,
        reference,
    );
    b.set_attribute("data-field", "saves-download").ok();
    b.set_attribute("data-save-ref", reference).ok();
    b
}

/// The device half: bring a `.entitysave` file in from this device — a save
/// someone handed over on a USB stick, or one this profile downloaded before.
fn file_card(ctx: &DomCtx, view: &SavesView) -> Element {
    let card = components::card(&crate::i18n::t("saves.file_heading", &[]));
    let hint = util::create_element("p");
    util::set_attr(&hint, "style", theme::HINT);
    util::set_text(&hint, &crate::i18n::t("saves.file_hint", &[]));
    util::append(&card, &hint);
    let row = util::create_element("div");
    util::set_attr(&row, "style", theme::ROW_INLINE);
    let actions = ctx.actions.clone();
    let rp = ctx.repaint.clone();
    let window_id = ctx.window_id;
    let attempt = ctx.offer_attempt.clone();
    let ui = view.saves_ui.clone();
    crate::dom::file_transfer::file_picker(
        &row,
        ctx,
        &crate::i18n::t("files.import_save", &[]),
        ButtonKind::Secondary,
        "saves-import-file",
        move |name, bytes| {
            // Not an offer: clear the shared slot the picker reports a slow read
            // on, and let the panel's own status line take over.
            attempt.clear();
            ui.borrow_mut().picked = Some((name, bytes));
            actions.borrow_mut().push(crate::action::Action::WindowEvent {
                window_id,
                event: SAVES_PICKED_EVENT.into(),
                value: String::new(),
            });
            rp();
        },
    );
    util::append(&card, &row);
    card
}

/// The cross-peer half: choose a peer, ask what it offers, take one.
fn import_card(ctx: &DomCtx, view: &SavesView) -> Element {
    let card = components::card(&crate::i18n::t("saves.import_heading", &[]));

    if view.peers.is_empty() {
        util::append(
            &card,
            &components::empty(&crate::i18n::t("saves.err_no_peer", &[])),
        );
        return card;
    }

    let options: Vec<(&str, &str)> = view
        .peers
        .iter()
        .map(|(id, label)| (id.as_str(), label.as_str()))
        .collect();
    let picker = components::select(ctx, &options, view.target, SAVES_TARGET_EVENT);
    util::append(
        &card,
        &components::field(&crate::i18n::t("label.peer", &[]), "", &picker),
    );
    util::append(
        &card,
        &components::button(
            ctx,
            &crate::i18n::t("saves.scan", &[]),
            ButtonKind::Secondary,
            SAVES_SCAN_EVENT,
        ),
    );

    if view.found.is_empty() {
        return card;
    }
    let (table, body) = components::table(&[&crate::i18n::t("label.name", &[]), ""]);
    for (offer_id, app_name, set, id) in &view.found {
        // The set/id the bundle names rides in the row's tooltip rather than a
        // column: it is what the import files by, so it must be inspectable,
        // but it is a slug and does not deserve a column of its own.
        let name = components::td_text(app_name);
        util::set_attr(&name, "title", &format!("{set}/{id}"));
        util::append(
            &body,
            &components::tr(vec![
                name,
                components::td(&components::button_value(
                    ctx,
                    &crate::i18n::t("saves.import", &[]),
                    ButtonKind::Primary,
                    SAVES_IMPORT_EVENT,
                    offer_id,
                )),
            ]),
        );
    }
    util::append(&card, &table);
    card
}

/// A backup's timestamp as a local date-time.
///
/// Formatted by the **platform**, in the user's own locale and time zone —
/// `toLocaleString` rather than a hand-rolled `YYYY-MM-DD`, which would be one
/// more place the app decides what a date looks like for someone whose
/// convention it does not know.
pub(crate) fn stamp_label(stamp_ms: u64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(stamp_ms as f64));
    let s: String = d.to_locale_string("default", &js_sys::Object::new()).into();
    if s.is_empty() {
        stamp_ms.to_string()
    } else {
        s
    }
}
