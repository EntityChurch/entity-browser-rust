//! Theme Editor DOM renderer — pure consumer of
//! [`ThemeEditorOutput`](crate::views::theme_editor::model::ThemeEditorOutput).
//!
//! Edit buffers live HERE, in the DOM (`data-token` inputs), not in the
//! tree — a per-keystroke tree write would rebuild the section and destroy
//! focus. Save/preview read the live values back out of the DOM:
//! - **Live preview**: every `input` event rebuilds `#theme-vars` from the
//!   draft (`theme_tokens::install_preview`) — no registry, no tree, no
//!   rebuild, so focus survives. Revert/save/load re-install the real theme.
//! - **Save** packs `label\x1fscheme\x1ftoken=value…` into one
//!   `save_theme` window event (the multi-field `\x1f` convention).
//!
//! Field ids carry the model's `revision` (`tok-{rev}-{token}`), so a
//! load/revert/save bumps every id and stale drafts stop shadowing the
//! re-initialized values.

use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::dom::components::{self, ButtonKind};
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::theme_tokens;
use crate::views::theme_editor::model::{EditingTheme, ThemeEditorOutput};

const SEP: &str = "\x1f";

pub fn render(container: &Element, output: &ThemeEditorOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element("div");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::HEADING).ok();
    util::set_text(&h2, "Theme Editor");
    util::append(&wrapper, &h2);

    render_picker(&wrapper, output, ctx);

    match &output.editing {
        Some(editing) => render_editor(&wrapper, editing, output, ctx),
        None => util::append(
            &wrapper,
            &components::empty("No theme loaded — duplicate one above to start."),
        ),
    }

    // Status line (D13): create/save/delete outcomes land here, including
    // refusals with their reason. Stable hook for the e2e.
    if !output.status.is_empty() {
        let status = util::create_element("p");
        status.set_attribute("style", theme::NOTE).ok();
        status.set_attribute("data-field", "theme-editor-status").ok();
        util::set_text(&status, &output.status);
        util::append(&wrapper, &status);
    }

    util::append(container, &wrapper);
}

/// "Themes" card: load an existing user theme + duplicate-a-base creation row.
fn render_picker(parent: &Element, output: &ThemeEditorOutput, ctx: &DomCtx) {
    let card = components::card("Themes");

    if !output.user_themes.is_empty() {
        let options: Vec<(&str, &str)> =
            output.user_themes.iter().map(|t| (t.name.as_str(), t.label.as_str())).collect();
        let selected =
            output.user_themes.iter().find(|t| t.selected).map(|t| t.name.as_str()).unwrap_or("");
        let load = components::select(ctx, &options, selected, "load_theme");
        load.set_attribute("name", &format!("theme-editor-load-{}", output.window_id)).ok();
        util::append(&card, &components::field("Theme", "", &load));
    }

    // New-from row: base select + name input + Create (submit-time reads).
    let base_options: Vec<(&str, &str)> =
        output.base_themes.iter().map(|t| (t.name.as_str(), t.label.as_str())).collect();
    let base_selected =
        output.base_themes.iter().find(|t| t.selected).map(|t| t.name.as_str()).unwrap_or("");
    let base = components::select_el(&base_options, base_selected);
    base.set_attribute("data-field", "theme-new-base").ok();
    util::append(&card, &components::field("New theme from", "", &base));

    let name_field = format!("theme-new-name-{}", output.revision);
    let name = components::text_input(ctx, &name_field, "", "name (a-z, 0-9, dashes)");
    util::append(&card, &components::field("Name", "", &name));

    let create = components::button_el("Create", ButtonKind::Primary);
    create.set_attribute("data-field", "theme-create").ok();
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let window_id = ctx.window_id;
        let card_ref = card.clone();
        ctx.listen(&create, "click", move |_| {
            let base = card_ref
                .query_selector("[data-field='theme-new-base']")
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<web_sys::HtmlSelectElement>().ok())
                .map(|s| s.value())
                .unwrap_or_default();
            let name = input_value(&card_ref, &name_field).unwrap_or_default();
            // No empty-name early-out: the model's validation reports
            // "theme name is empty" through the status line (D13 — a
            // click that does nothing is a silent no-op).
            actions.borrow_mut().push(crate::action::Action::WindowEvent {
                window_id,
                event: "create_theme".into(),
                value: format!("{base}{SEP}{name}"),
            });
            rp();
        });
    }
    let row = util::create_element("div");
    row.set_attribute("style", theme::BTN_ROW).ok();
    util::append(&row, &create);
    util::append(&card, &row);

    util::append(parent, &card);
}

/// The editor card for the loaded theme: label/scheme + grouped token rows
/// (S7 — repeated records are tables) + Save/Revert/Delete.
fn render_editor(parent: &Element, editing: &EditingTheme, output: &ThemeEditorOutput, ctx: &DomCtx) {
    let card = components::card(&format!("Edit \"{}\"", editing.label));
    let rev = output.revision;

    // The live preview repaints the WHOLE app from the draft — say so up
    // front, or the first keystroke reads as "my theme changed?!".
    let preview_hint = util::create_element("p");
    preview_hint.set_attribute("style", theme::HINT).ok();
    util::set_text(
        &preview_hint,
        "Edits preview live across the whole app. Save keeps them; Revert restores the real theme.",
    );
    util::append(&card, &preview_hint);

    let label_field = format!("theme-label-{rev}");
    let label = components::text_input(ctx, &label_field, &editing.label, "display label");
    util::append(&card, &components::field("Label", "", &label));

    let scheme = components::select_el(&[("dark", "Dark"), ("light", "Light")], &editing.scheme);
    scheme.set_attribute("data-field", "theme-scheme").ok();
    {
        let card_ref = card.clone();
        ctx.listen(&scheme, "change", move |_| preview_from_dom(&card_ref));
    }
    util::append(
        &card,
        &components::field(
            "Scheme",
            "Not a color: browsers render native widgets (dropdown popups, scrollbars, carets) \
             in one of exactly two modes, dark or light — pick the one your palette sits closest \
             to so those widgets match.",
            &scheme,
        ),
    );

    for group in &editing.groups {
        util::append(&card, &components::subheading(group.title));
        let (table, tbody) = components::table(&["Token", "Value"]);
        for (token, value) in &group.rows {
            let cell = util::create_element("div");
            cell.set_attribute("style", theme::TREE_ROW).ok();

            let field_id = format!("tok-{rev}-{token}");
            let text = components::text_input(ctx, &field_id, value, "");
            text.set_attribute("data-token", token).ok();
            {
                let card_ref = card.clone();
                ctx.listen(&text, "input", move |_| preview_from_dom(&card_ref));
            }

            // Hex-parseable values get a native picker synced INTO the text
            // input (the text input stays authoritative — non-hex edits just
            // leave the swatch behind).
            if let Some(hex) = hex6(value) {
                let swatch = components::color_swatch_el(&hex);
                let card_ref = card.clone();
                let text_ref = text.clone();
                let drafts = ctx.drafts.clone();
                let field_for_draft = field_id.clone();
                ctx.listen(&swatch, "input", move |ev| {
                    let Some(picked) = ev
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                        .map(|i| i.value())
                    else {
                        return;
                    };
                    if let Some(input) = text_ref.dyn_ref::<web_sys::HtmlInputElement>() {
                        input.set_value(&picked);
                    }
                    // A programmatic set_value fires no `input` event — keep
                    // the draft map in step by hand, or a rebuild would
                    // resurrect the pre-pick text.
                    drafts.borrow_mut().insert(field_for_draft.clone(), picked);
                    preview_from_dom(&card_ref);
                });
                util::append(&cell, &swatch);
            }

            util::append(&cell, &text);
            let row = components::tr(vec![components::td_text(token), components::td(&cell)]);
            util::append(&tbody, &row);
        }
        util::append(&card, &table);
    }

    let buttons = util::create_element("div");
    buttons.set_attribute("style", theme::BTN_ROW).ok();

    let save = components::button_el("Save", ButtonKind::Primary);
    save.set_attribute("data-field", "theme-save").ok();
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let window_id = ctx.window_id;
        let card_ref = card.clone();
        let label_field = label_field.clone();
        ctx.listen(&save, "click", move |_| {
            let label = input_value(&card_ref, &label_field).unwrap_or_default();
            let scheme = card_ref
                .query_selector("[data-field='theme-scheme']")
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<web_sys::HtmlSelectElement>().ok())
                .map(|s| s.value())
                .unwrap_or_else(|| "dark".to_string());
            let mut parts = vec![label, scheme];
            for (token, value) in token_values(&card_ref) {
                parts.push(format!("{token}={value}"));
            }
            actions.borrow_mut().push(crate::action::Action::WindowEvent {
                window_id,
                event: "save_theme".into(),
                value: parts.join(SEP),
            });
            rp();
        });
    }
    util::append(&buttons, &save);

    util::append(
        &buttons,
        &components::button(ctx, "Revert", ButtonKind::Secondary, "revert_theme"),
    );

    let delete = components::button(ctx, "Delete", ButtonKind::Destructive, "delete_theme");
    delete.set_attribute("data-field", "theme-delete").ok();
    if editing.in_use {
        components::disable(&delete);
    }
    util::append(&buttons, &delete);
    util::append(&card, &buttons);

    if editing.in_use {
        let hint = util::create_element("p");
        hint.set_attribute("style", theme::HINT).ok();
        util::set_text(
            &hint,
            "This theme is in use (chrome theme or site override) — switch first to delete.",
        );
        util::append(&card, &hint);
    }

    util::append(parent, &card);
}

/// Rebuild `#theme-vars` from the DRAFT currently in the DOM — the live
/// preview. Reads every `data-token` input + the scheme select under `root`.
fn preview_from_dom(root: &Element) {
    let scheme = root
        .query_selector("[data-field='theme-scheme']")
        .ok()
        .flatten()
        .and_then(|el| el.dyn_into::<web_sys::HtmlSelectElement>().ok())
        .map(|s| s.value())
        .unwrap_or_else(|| "dark".to_string());
    theme_tokens::install_preview(&scheme, &token_values(root));
}

/// Collect `(token, live value)` from every `data-token` input under `root`,
/// in DOM order (= theme order).
fn token_values(root: &Element) -> Vec<(String, String)> {
    let mut vars = Vec::new();
    let Ok(list) = root.query_selector_all("[data-token]") else {
        return vars;
    };
    for i in 0..list.length() {
        let Some(el) = list.get(i).and_then(|n| n.dyn_into::<web_sys::HtmlInputElement>().ok())
        else {
            continue;
        };
        let Some(token) = el.get_attribute("data-token") else {
            continue;
        };
        vars.push((token, el.value()));
    }
    vars
}

/// The live value of the tracked input with `data-field == field` under
/// `root` (the submit-time read; the DOM value IS the draft).
fn input_value(root: &Element, field: &str) -> Option<String> {
    root.query_selector(&format!("[data-field='{field}']"))
        .ok()
        .flatten()
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|i| i.value())
}

/// Normalize a CSS hex color to the 6-digit `#rrggbb` the native color input
/// requires. `None` for anything else (fonts, percentages, 4/8-digit alpha
/// forms — those rows render without a swatch).
fn hex6(value: &str) -> Option<String> {
    let digits = value.strip_prefix('#')?;
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        3 => {
            let mut out = String::from("#");
            for c in digits.chars() {
                out.push(c);
                out.push(c);
            }
            Some(out.to_ascii_lowercase())
        }
        6 => Some(value.to_ascii_lowercase()),
        _ => None,
    }
}
