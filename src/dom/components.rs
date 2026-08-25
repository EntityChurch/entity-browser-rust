//! Shared UI primitives — the single source for grouping containers, status
//! chips, and empty/loading/error states (REFERENCE-UI-DESIGN §3).
//!
//! Windows consume these so the design standard is the *default* path, not
//! per-window discipline:
//! - [`card`] — a bounded group container (S2 common region).
//! - [`conn_chip`] / [`auth_chip`] — the ONE status vocabulary (S4): every
//!   window renders connection / authorization status through these, so the
//!   word + color + glyph can never drift between screens.
//! - [`loading`] / [`empty`] / [`error`] — the three non-content states (S5),
//!   so no async/list surface ever ships a blank.

use crate::dom::theme;
use crate::dom::util;
use web_sys::Element;

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
                "text-align:left;padding:{} {};font-size:11px;font-weight:600;\
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
