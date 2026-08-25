//! Shared UI primitives — the single source for grouping containers, status
//! chips, and empty/loading/error states (REFERENCE-UI-DESIGN §3).
//!
//! Windows consume these so the design standard is the *default* path, not
//! per-window discipline:
//! - [`button`] / [`button_action`] / [`text_input`] / [`select`] / [`field`]
//!   — the form ATOMS (S3/S8): the one way to build the highest-frequency
//!   controls. `text_input` rides `util::tracked_input`, so typed drafts
//!   survive snapshot rebuilds by construction — no view opts out of that
//!   again (the B3/B4 "typed address wiped by repaint" class of bug).
//! - [`card`] — a bounded group container (S2 common region).
//! - [`conn_chip`] / [`auth_chip`] — the ONE status vocabulary (S4): every
//!   window renders connection / authorization status through these, so the
//!   word + color + glyph can never drift between screens.
//! - [`loading`] / [`empty`] / [`error`] — the three non-content states (S5),
//!   so no async/list surface ever ships a blank.

use crate::action::Action;
use crate::dom::theme;
use crate::dom::util;
use web_sys::Element;

// --- Atoms (S3/S8) -----------------------------------------------------------
// The one way to build a button / input / select / form row. A view never
// `create_element("button")`s again — `tools/ui-lint.sh` fails a diff that
// does (outside this file and the theme/style modules).

/// Button emphasis (S3): exactly one `Primary` per group; `Destructive` is
/// never a group's primary.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // variants adopted as windows migrate (WASM render path)
pub enum ButtonKind {
    Primary,
    Secondary,
    Small,
    Destructive,
}

impl ButtonKind {
    fn style(self) -> &'static str {
        match self {
            ButtonKind::Primary => theme::BTN_PRIMARY,
            ButtonKind::Secondary => theme::BTN_SECONDARY,
            ButtonKind::Small => theme::BTN_SMALL,
            ButtonKind::Destructive => theme::BTN_DESTRUCTIVE,
        }
    }
}

/// An unwired button in the standard look — for call sites whose click handler
/// needs custom logic (submit-time draft reads, confirms, multi-action
/// pushes): build with this, wire with `ctx.listen`. Prefer [`button`] /
/// [`button_action`] when a static event or action suffices.
pub fn button_el(label: &str, kind: ButtonKind) -> Element {
    let b = util::create_element("button");
    b.set_attribute("style", kind.style()).ok();
    util::set_text(&b, label);
    b
}

/// THE button: standard look, dispatches a `WindowEvent` on click.
#[allow(dead_code)] // consumed as windows migrate (WASM render path)
pub fn button(ctx: &util::DomCtx, label: &str, kind: ButtonKind, event: &str) -> Element {
    let b = button_el(label, kind);
    ctx.on_window_event(&b, "click", event, "");
    b
}

/// [`button`]'s twin for a direct [`Action`] (no window-event indirection).
pub fn button_action(ctx: &util::DomCtx, label: &str, kind: ButtonKind, action: Action) -> Element {
    let b = button_el(label, kind);
    ctx.on_action(&b, "click", action);
    b
}

/// THE text input: standard look + draft-tracked value (`util::tracked_input`
/// mechanism), so typing survives any snapshot rebuild. Read the live value at
/// submit time from `ctx.drafts.borrow().get(field_id)` (fall back to the
/// render's `initial` when absent — the user never touched the field).
pub fn text_input(ctx: &util::DomCtx, field_id: &str, initial: &str, placeholder: &str) -> Element {
    let input = util::tracked_input_el(ctx, field_id, initial, theme::INPUT);
    if !placeholder.is_empty() {
        input.set_attribute("placeholder", placeholder).ok();
    }
    input
}

/// An unwired native color swatch (`<input type=color>`, standard look) —
/// for token-editor rows beside a text input. Wire behavior (sync-to-text,
/// live preview) with `ctx.listen`; the value must be a 6-digit `#rrggbb`
/// (the platform control accepts nothing else — normalize `#rgb` first).
pub fn color_swatch_el(value: &str) -> Element {
    let input = util::create_element("input");
    input.set_attribute("type", "color").ok();
    input.set_attribute("value", value).ok();
    input.set_attribute("style", theme::COLOR_SWATCH).ok();
    input
}

/// An unwired select in the standard look — for call sites whose value is
/// read at submit time (no per-change dispatch, so a change can't trigger a
/// rebuild that resets the pick). Prefer [`select`] when a change event is
/// the behavior.
pub fn select_el(options: &[(&str, &str)], selected: &str) -> Element {
    let sel = util::create_element("select");
    sel.set_attribute("style", theme::SELECT).ok();
    for (value, label) in options {
        let opt = util::create_element("option");
        opt.set_attribute("value", value).ok();
        if *value == selected {
            opt.set_attribute("selected", "selected").ok();
        }
        util::set_text(&opt, label);
        util::append(&sel, &opt);
    }
    sel
}

/// THE select: standard look, `(value, label)` options, dispatches `event`
/// with the selected value on change.
pub fn select(ctx: &util::DomCtx, options: &[(&str, &str)], selected: &str, event: &str) -> Element {
    let sel = select_el(options, selected);
    ctx.on_select_change(&sel, event);
    sel
}

/// THE checkbox row: a `LABEL_CHOICE` label wrapping the box + text, firing
/// `event` on change. `name` doubles as the `data-field` hook (the repo's
/// submit-time-read + e2e convention) — always pass one.
pub fn checkbox(
    ctx: &util::DomCtx,
    name: &str,
    checked: bool,
    event: &str,
    label_text: &str,
) -> Element {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL_CHOICE).ok();
    let cb = util::create_element("input");
    cb.set_attribute("type", "checkbox").ok();
    cb.set_attribute("name", name).ok();
    cb.set_attribute("data-field", name).ok();
    if checked {
        cb.set_attribute("checked", "").ok();
    }
    ctx.on_window_event(&cb, "change", event, "");
    util::append(&label, &cb);
    let span = util::create_element("span");
    util::set_text(&span, label_text);
    util::append(&label, &span);
    label
}

/// THE radio row: one option of a `group`, dispatching `event` with `value`
/// on click. Returns `(row, input)` — the input is exposed so a caller can
/// add its own stable hook attribute (e.g. `data-kind`).
pub fn radio(
    ctx: &util::DomCtx,
    group: &str,
    value: &str,
    checked: bool,
    event: &str,
    label_text: &str,
) -> (Element, Element) {
    let label = util::create_element("label");
    label.set_attribute("style", theme::LABEL_CHOICE).ok();
    let input = util::create_element("input");
    input.set_attribute("type", "radio").ok();
    input.set_attribute("name", group).ok();
    input.set_attribute("value", value).ok();
    if checked {
        input.set_attribute("checked", "").ok();
    }
    ctx.on_window_event(&input, "click", event, value);
    util::append(&label, &input);
    let span = util::create_element("span");
    util::set_text(&span, label_text);
    util::append(&label, &span);
    (label, input)
}

/// Render a button inert: disabled attribute + dimmed look. The one way a
/// primary "waits" (S3: a disabled primary must look disabled) — don't
/// hand-roll opacity styles per window.
pub fn disable(btn: &Element) {
    btn.set_attribute("disabled", "").ok();
    let style = btn.get_attribute("style").unwrap_or_default();
    btn.set_attribute("style", &format!("{style};opacity:0.5;cursor:default"))
        .ok();
}

/// One row of a lazy tree browser (S8) — a caret for directories (an aligning
/// spacer for leaves) plus the node button, in the shared tree look
/// (`theme::TREE_*`). Returns `(row, caret, node)` **unwired**: the caller
/// attaches its own events (toggle on the caret, select/cd/toggle on the node)
/// and appends `row`. Promoted when File Transfer shipped a copy of Site
/// Editor's tree consts — the second bespoke variant; never a third.
pub fn tree_row(
    depth: usize,
    is_dir: bool,
    expanded: bool,
    selected: bool,
    label: &str,
) -> (Element, Option<Element>, Element) {
    let row = util::create_element("div");
    row.set_attribute(
        "style",
        &format!("{};padding-inline-start:{}px", theme::TREE_ROW, depth * 16),
    )
    .ok();

    let caret = if is_dir {
        let c = util::create_element("button");
        c.set_attribute("style", theme::TREE_CARET).ok();
        util::set_text(&c, if expanded { "\u{25be}" } else { "\u{25b8}" }); // ▾ / ▸
        util::append(&row, &c);
        Some(c)
    } else {
        let spacer = util::create_element("span");
        spacer.set_attribute("style", "flex:0 0 22px").ok();
        util::append(&row, &spacer);
        None
    };

    let node = util::create_element("button");
    node.set_attribute(
        "style",
        if selected { theme::TREE_NODE_SELECTED } else { theme::TREE_NODE },
    )
    .ok();
    util::set_text(&node, label);
    util::append(&row, &node);

    (row, caret, node)
}

/// The form row (S8): label + control + optional hint (empty `hint` renders
/// none). Append the returned row to a [`card`] — a form is a card of fields
/// with exactly one primary (S3).
pub fn field(label: &str, hint: &str, control: &Element) -> Element {
    let row = util::create_element("div");
    let l = util::create_element("label");
    l.set_attribute("style", theme::LABEL).ok();
    util::set_text(&l, label);
    util::append(&row, &l);
    util::append(&row, control);
    if !hint.is_empty() {
        let h = util::create_element("p");
        h.set_attribute("style", theme::HINT).ok();
        util::set_text(&h, hint);
        util::append(&row, &h);
    }
    row
}

// --- Status vocabulary (S4) -------------------------------------------------
// One state model per recurring status; one word, one color, one glyph each.
// Rendered ONLY through the chip helpers below — no window hand-rolls a status
// span again.

/// Connection status — the one connection vocabulary (S4).
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // consumed as System Backend / Peer Connections adopt S4
pub enum ConnState {
    Connected,
    Connecting,
    Offline,
}

/// Authorization status — the one authorization vocabulary (S4).
///
/// Four honest states; each view uses the subset that applies. The backend's
/// inbound-device view uses `Authorized` / `Pending` / `NotAuthorized`; an
/// outbound consumer (File Transfer, checking its own access to a target) uses
/// `Authorized` / `NotAuthorized` / `Unverified`. `Pending` (a device awaiting
/// *your* decision) and `Unverified` (access not yet checked) are genuinely
/// different states — but both render through this one table so the word/color
/// never drifts.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // `Pending` awaits the inbound-device surface (System Backend, 3b)
pub enum AuthState {
    Authorized,
    Pending,
    NotAuthorized,
    Unverified,
}

impl ConnState {
    /// (glyph, word, color) — the single definition of how each state looks.
    fn parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            ConnState::Connected => ("\u{25cf}", "Connected", "var(--status-ok,#4c4)"), // ●
            ConnState::Connecting => ("\u{25d0}", "Connecting\u{2026}", "var(--text-dim,#888)"), // ◐
            ConnState::Offline => ("\u{25cb}", "Offline", "var(--text-dim,#888)"),       // ○
        }
    }
}

impl AuthState {
    fn parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            AuthState::Authorized => ("\u{2713}", "Authorized", "var(--status-ok,#4c4)"), // ✓
            AuthState::Pending => ("\u{2022}", "Pending", "var(--text-dim,#888)"),         // •
            AuthState::NotAuthorized => ("\u{26d4}", "Not authorized", "var(--status-err,#f66)"), // ⛔
            AuthState::Unverified => ("\u{2022}", "Not verified", "var(--text-dim,#888)"), // •
        }
    }
}

/// The one chip look — a compact pill (glyph + word) in the state's color.
/// Private so every chip in the app is byte-identical in shape.
fn chip(glyph: &str, word: &str, color: &str) -> Element {
    let el = util::create_element("span");
    el.set_attribute(
        "style",
        &format!(
            "display:inline-flex;align-items:center;gap:{};font-size:11px;\
             padding:2px 8px;border-radius:10px;white-space:nowrap;\
             color:{color};border:1px solid {color}",
            theme::SP_1
        ),
    )
    .ok();
    util::set_text(&el, &format!("{glyph} {word}"));
    el
}

/// Connection-status chip (S4). Use everywhere a connection state is shown.
#[allow(dead_code)] // consumed as System Backend / Peer Connections adopt S4
pub fn conn_chip(state: ConnState) -> Element {
    let (g, w, c) = state.parts();
    chip(g, w, c)
}

/// Authorization-status chip (S4). Use everywhere an authorization state is
/// shown — the authoritative surface AND read-only reflections (File Transfer).
pub fn auth_chip(state: AuthState) -> Element {
    let (g, w, c) = state.parts();
    chip(g, w, c)
}

// --- Grouping (S2) ----------------------------------------------------------

/// A subheading for a group/section (uppercase, dim) — the group's label.
pub fn subheading(text: &str) -> Element {
    let h = util::create_element("h3");
    h.set_attribute(
        "style",
        &format!(
            "margin:0 0 {} 0;font-size:12px;text-transform:uppercase;\
             letter-spacing:0.05em;color:var(--text-dim,#888)",
            theme::SP_2
        ),
    )
    .ok();
    util::set_text(&h, text);
    h
}

/// A bounded group container (S2 common region): a sunken, bordered card that
/// visually contains one group of related controls. Append the group's controls
/// to the returned element. A non-empty `title` renders a [`subheading`] at the
/// top. This is the single source for "a group of controls" — nothing floats
/// between cards.
pub fn card(title: &str) -> Element {
    let g = util::create_element("div");
    g.set_attribute(
        "style",
        &format!(
            "margin:{} 0;padding:{};border-radius:6px;\
             border:1px solid var(--border,#333);background:var(--surface-sunken,#0a0a1a)",
            theme::SP_3,
            theme::SP_3,
        ),
    )
    .ok();
    if !title.is_empty() {
        util::append(&g, &subheading(title));
    }
    g
}

// --- Collapsible disclosure (S8) -------------------------------------------
// The ONE way to reveal an on-demand section (a "New/Add {noun}" create form, a
// foldable list). One look, one behaviour, so no window hand-rolls its own
// reveal (Site Creator's expander, a bespoke `<details>`, …) ever again.

/// A collapsible section header: a full-width toggle button, "▾/▸ {label}",
/// that dispatches `event` (a window event) so the window flips the section's
/// **model-held** `open` bool. The caller owns that bool and renders the body
/// only when `open`.
///
/// Model-held open state is the point (S8): it survives a snapshot rebuild, so a
/// subscription firing mid-entry can't collapse the form under you — the failure
/// mode of a native `<details>`, which re-renders closed on every repaint.
pub fn collapsible_header(ctx: &util::DomCtx, label: &str, open: bool, event: &str) -> Element {
    let h = util::create_element("button");
    h.set_attribute("style", theme::COLLAPSIBLE_HEADER).ok();
    util::set_text(&h, &format!("{} {label}", if open { "\u{25be}" } else { "\u{25b8}" }));
    ctx.on_window_event(&h, "click", event, "");
    h
}

// --- Tables (S7) ------------------------------------------------------------
// Repeated records render as an aligned, header-labelled table so a list is
// scannable (read down a column) instead of a wall of free-form rows. Build
// with `table(headers)`, then append `tr(vec![...])` rows to the returned body.

/// A themed table with a header row. Returns `(table, tbody)` — append rows
/// (built with [`tr`]) to `tbody`, then append `table` to your card.
pub fn table(headers: &[&str]) -> (Element, Element) {
    let t = util::create_element("table");
    t.set_attribute(
        "style",
        "width:100%;border-collapse:collapse;font-size:12px",
    )
    .ok();

    let thead = util::create_element("thead");
    let hrow = util::create_element("tr");
    for h in headers {
        let th = util::create_element("th");
        th.set_attribute(
            "style",
            &format!(
                "text-align:start;padding:{} {};font-size:11px;font-weight:600;\
                 text-transform:uppercase;letter-spacing:0.05em;\
                 color:var(--text-dim,#888);border-bottom:1px solid var(--border,#333)",
                theme::SP_1, theme::SP_2
            ),
        )
        .ok();
        util::set_text(&th, h);
        util::append(&hrow, &th);
    }
    util::append(&thead, &hrow);
    util::append(&t, &thead);

    let tbody = util::create_element("tbody");
    util::append(&t, &tbody);
    (t, tbody)
}

/// A table row (`<tr>`) from a set of cell contents. Each element is wrapped in
/// a `<td>` (pass a text cell via [`td_text`], or any element — a button, a
/// chip — via [`td`]).
pub fn tr(cells: Vec<Element>) -> Element {
    let row = util::create_element("tr");
    for c in cells {
        util::append(&row, &c);
    }
    row
}

/// A text `<td>`.
pub fn td_text(text: &str) -> Element {
    let cell = util::create_element("td");
    cell.set_attribute(
        "style",
        &format!(
            "padding:{} {};border-bottom:1px solid var(--border,#333);\
             color:var(--text,#e0e0e0);vertical-align:middle",
            theme::SP_1, theme::SP_2
        ),
    )
    .ok();
    util::set_text(&cell, text);
    cell
}

/// A `<td>` wrapping an element (a button, a status chip, a mono id…).
pub fn td(child: &Element) -> Element {
    let cell = util::create_element("td");
    cell.set_attribute(
        "style",
        &format!(
            "padding:{} {};border-bottom:1px solid var(--border,#333);vertical-align:middle",
            theme::SP_1, theme::SP_2
        ),
    )
    .ok();
    util::append(&cell, child);
    cell
}

// --- Non-content states (S5) ------------------------------------------------

/// Loading placeholder — never a blank while an async op is in flight.
pub fn loading(msg: &str) -> Element {
    let p = util::create_element("p");
    p.set_attribute("style", theme::HINT).ok();
    util::set_text(&p, if msg.is_empty() { "Loading\u{2026}" } else { msg });
    p
}

/// Empty state — a helpful line, not a void. Keep it actionable where possible
/// (append a next-action button to the returned element's parent, or pass the
/// suggestion in `msg`).
pub fn empty(msg: &str) -> Element {
    let p = util::create_element("p");
    p.set_attribute("style", theme::HINT).ok();
    util::set_text(&p, msg);
    p
}

/// Error state — loud, specific, actionable. Renders in the error color.
pub fn error(msg: &str) -> Element {
    let p = util::create_element("p");
    p.set_attribute(
        "style",
        &format!(
            "color:var(--status-err,#f66);font-size:12px;margin:{} 0",
            theme::SP_1
        ),
    )
    .ok();
    util::set_text(&p, &format!("\u{2717} {msg}")); // ✗
    p
}
