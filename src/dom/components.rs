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

/// [`button`], but carrying a `value` alongside the event name.
///
/// The third sibling of [`button`] / [`button_action`], for the common shape
/// where one event serves several rows and the row identifies itself — a
/// per-item Select/Delete, or a toggle that must say *which* thing and *which
/// direction*. Without it a view reaches for raw `button_el` +
/// `on_window_event` (which several already do), and the UI lint cannot see it.
pub fn button_value(
    ctx: &util::DomCtx,
    label: &str,
    kind: ButtonKind,
    event: &str,
    value: &str,
) -> Element {
    let b = button_el(label, kind);
    ctx.on_window_event(&b, "click", event, value);
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

/// An unwired `<input type=file>`, **visually hidden rather than
/// `display:none`**, tagged `data-field="{field}"`. Open it from a real
/// button through [`util::show_file_picker`], which reports a refused chooser.
///
/// Why hidden this way: a `display:none` input is not rendered at all, and
/// several mobile browsers are documented to decline a chooser for a control
/// that is not rendered. Keeping it laid out at zero size costs nothing.
///
/// **It is NOT what makes Android work, and the record used to imply it was.**
/// Measured 2026-08-24 on Firefox for Android with a nine-row matrix
/// (`tools/picker-probe.html`): a clipped input, a laid-out `opacity:0` input
/// and a plainly visible one tapped directly ALL have their chooser dismissed
/// by the engine in ~200-250ms, while Chrome on the same phone opens it. So
/// the hiding style is not the discriminator; this is cheap insurance for
/// other engines, not a fix. Two consumers (File Transfer, the Apps player's
/// file verbs), which is why it lives here and not twice.
pub fn hidden_file_input(field: &str) -> Element {
    let input = util::create_element("input");
    input.set_attribute("type", "file").ok();
    input
        .set_attribute(
            "style",
            "position:absolute;width:1px;height:1px;padding:0;margin:-1px;\
             overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap;border:0",
        )
        .ok();
    input.set_attribute("data-field", field).ok();
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

impl ConnState {
    /// Map the app's one connection vocabulary
    /// ([`crate::peer_liveness::ConnDisplay`]) to a chip. `Unknown` has no chip —
    /// the caller renders a quiet dash (paired, no current signal), never a
    /// misleading "connecting".
    ///
    /// `Reconnecting` (kernel `suspect`) currently renders as `Connecting` (calm,
    /// non-red — a correct intermediate). Its distinct §4c "Reconnecting…"
    /// label + amber is Piece C's status-vocabulary work (needs the i18n key
    /// across the catalog); folded in there, not here.
    pub fn from_display(d: crate::peer_liveness::ConnDisplay) -> Option<Self> {
        use crate::peer_liveness::ConnDisplay;
        match d {
            ConnDisplay::Connected => Some(ConnState::Connected),
            ConnDisplay::Dialing | ConnDisplay::Reconnecting => Some(ConnState::Connecting),
            ConnDisplay::Offline => Some(ConnState::Offline),
            ConnDisplay::Unknown => None,
        }
    }
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
    /// (glyph, word-key, color) — the single definition of how each state looks.
    /// The middle element is an i18n catalog key resolved by [`chip`].
    fn parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            ConnState::Connected => ("\u{25cf}", "chip.connected", "var(--status-ok,#4c4)"), // ●
            ConnState::Connecting => ("\u{25d0}", "chip.connecting", "var(--text-dim,#888)"), // ◐
            ConnState::Offline => ("\u{25cb}", "chip.offline", "var(--text-dim,#888)"),       // ○
        }
    }
}

impl AuthState {
    fn parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            AuthState::Authorized => ("\u{2713}", "chip.authorized", "var(--status-ok,#4c4)"), // ✓
            AuthState::Pending => ("\u{2022}", "chip.pending", "var(--text-dim,#888)"),         // •
            AuthState::NotAuthorized => ("\u{26d4}", "chip.not_authorized", "var(--status-err,#f66)"), // ⛔
            AuthState::Unverified => ("\u{2022}", "chip.not_verified", "var(--text-dim,#888)"), // •
        }
    }
}

/// The one chip look — a compact pill (glyph + word) in the state's color.
/// Private so every chip in the app is byte-identical in shape. `word_key` is
/// an i18n catalog key, resolved here so every chip localizes in one place.
fn chip(glyph: &str, word_key: &str, color: &str) -> Element {
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
    util::set_text(&el, &format!("{glyph} {}", crate::i18n::t(word_key, &[])));
    el
}

/// Connection-status chip (S4). Use everywhere a connection state is shown.
#[allow(dead_code)] // consumed as System Backend / Peer Connections adopt S4
pub fn conn_chip(state: ConnState) -> Element {
    let (g, w, c) = state.parts();
    chip(g, w, c)
}

/// Health-verdict chip (S4) — the System Overview *Problems* section.
///
/// A fourth chip family rather than a reuse of [`ServiceState`], because the
/// vocabulary genuinely differs and S4's rule is *one vocabulary per kind of
/// status*, not *one chip for everything*: a service is on or off, and a check
/// can be **unestablished**, which is a state no service chip has a word for.
/// Folding "could not check" into `Pending` or `Off` is precisely the collapse
/// `doctor::Verdict` exists to prevent.
///
/// **Three glyphs, not two, and none of them is a tick.** Colour alone is not a
/// state anyone can read on a bad monitor (the note on [`ServiceState`]), and
/// more importantly the *unknown* band must not borrow either the good or the
/// bad one — a check that could not run is neither.
///
/// The word is passed in rather than looked up, because these strings live with
/// the rest of the section's copy in `doctor::Verdict::chip` — one file to
/// extract when this surface is translated.
#[allow(dead_code)] // wasm-only consumer
pub fn health_chip(word: &str, tone: HealthTone) -> Element {
    let (glyph, color) = match tone {
        HealthTone::Attention => ("\u{26a0}", "var(--status-warn,#fc9)"), // ⚠
        HealthTone::Unknown => ("\u{25cc}", "var(--text-dim,#888)"),      // ◌
        HealthTone::Clear => ("\u{25cf}", "var(--status-ok,#4c4)"),       // ●
    };
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

/// The three bands a health verdict renders in. Note there are three, not five:
/// the five verdicts differ in *what they mean*, and the chip only has to say
/// how much of the user's attention this deserves. The distinction survives in
/// the words beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // wasm-only consumer
pub enum HealthTone {
    /// Something is wrong and the user can see the effects.
    Attention,
    /// Not established — never rendered as either good or bad.
    Unknown,
    /// Checked, and fine.
    Clear,
}

/// Authorization-status chip (S4). Use everywhere an authorization state is
/// shown — the authoritative surface AND read-only reflections (File Transfer).
pub fn auth_chip(state: AuthState) -> Element {
    let (g, w, c) = state.parts();
    chip(g, w, c)
}

/// A filter-chip bar — the wrapping row [`filter_chip`]s sit in.
pub fn filter_bar() -> Element {
    let bar = util::create_element("div");
    bar.set_attribute("style", theme::ROW_START).ok();
    bar
}

/// One filter chip: a two-state toggle carrying the value it selects.
///
/// The look is the app's existing toggle pair (`TOGGLE_ACTIVE` /
/// `TOGGLE_INACTIVE`) rather than a new chip style — a filter chip *is* a
/// toggle, and giving it its own colors is how two surfaces that mean the same
/// thing end up looking different.
///
/// `count` rides in the label deliberately: a chip that says how many things it
/// holds cannot be mistaken for a chip that is broken, which is exactly what a
/// filter row that silently shows nothing looks like.
pub fn filter_chip(
    ctx: &util::DomCtx,
    label: &str,
    count: usize,
    selected: bool,
    event: &str,
    value: &str,
) -> Element {
    let b = util::create_element("button");
    b.set_attribute("type", "button").ok();
    b.set_attribute(
        "style",
        if selected {
            theme::TOGGLE_ACTIVE
        } else {
            theme::TOGGLE_INACTIVE
        },
    )
    .ok();
    if selected {
        // Which chip is active must be recoverable without reading colors back
        // out of an inline style — by assistive tech, and by the e2e.
        b.set_attribute("aria-pressed", "true").ok();
    }
    b.set_attribute("data-chip", value).ok();
    util::set_text(&b, &format!("{label} {count}"));
    ctx.on_window_event(&b, "click", event, value);
    b
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

/// A native `<details>` disclosure with the app's summary look — returns
/// `(details, body)`. Append the section's content to `body`, then `details` to
/// your card. Closed on first render.
///
/// **Not a replacement for [`collapsible_header`], and the difference decides
/// which to use.** That one holds `open` in the model, so a subscription firing
/// mid-entry cannot collapse the section under you — which is what you need for
/// a form somebody is *typing into*. A `<details>` re-renders closed on every
/// repaint, which is exactly right for a section that is closed by default and
/// costs something to open (a camera, a QR render) or that holds fields most
/// people never touch.
///
/// It exists as an atom because three surfaces had hand-rolled the same
/// `<details>` + inline-styled `<summary>` (the QR scanner, the QR display, the
/// connector form's advanced fields), which is how the same affordance ends up
/// three slightly different shades of blue.
pub fn disclosure(label: &str) -> (Element, Element) {
    let details = util::create_element("details");
    details
        .set_attribute("style", &format!("margin-top:{}", theme::SP_2))
        .ok();

    let summary = util::create_element("summary");
    summary
        .set_attribute(
            "style",
            &format!(
                "cursor:pointer;font-size:12px;padding:{} 0;color:var(--accent-2,#c0c0e0)",
                theme::SP_1
            ),
        )
        .ok();
    util::set_text(&summary, label);
    util::append(&details, &summary);

    let body = util::create_element("div");
    body.set_attribute("style", &format!("margin-top:{}", theme::SP_2)).ok();
    util::append(&details, &body);
    (details, body)
}

// --- Services this device offers (S4, third vocabulary) ---------------------

/// Whether a service this device offers is doing anything for other devices.
///
/// The third §S4 vocabulary beside [`ConnState`] and [`AuthState`], and a
/// genuinely different axis: those describe a *peer*, this describes a *switch
/// on this machine*. Five states, not two, because collapsing them rebuilds the
/// one-sentence failure the reachability work exists to end — a person who is
/// told "not reachable" cannot tell whether to wait, to flip another switch, to
/// reconfigure a router, or to give up:
///
/// - [`On`](Self::On) — asked for, and working.
/// - [`Incomplete`](Self::Incomplete) — on, but something *else* is missing
///   before it does the job you wanted (the app server serving without a
///   rendezvous: a visitor gets a working app and still cannot reach anybody).
/// - [`Pending`](Self::Pending) — asked, no answer yet. Never a failure string:
///   inventing one reports a refusal that has not happened.
/// - [`Refused`](Self::Refused) — asked, and told no. Someone said no; the
///   reason belongs in the row's detail line.
/// - [`Off`](Self::Off) — not asked for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    On,
    Incomplete,
    Pending,
    Refused,
    Off,
}

impl ServiceState {
    fn parts(self) -> (&'static str, &'static str, &'static str) {
        match self {
            // Distinct GLYPHS as well as colors: color alone is not a state
            // anyone can read on a bad monitor, and these five sit in one column.
            ServiceState::On => ("\u{25cf}", "chip.service_on", "var(--status-ok,#4c4)"), // ●
            ServiceState::Incomplete => {
                ("\u{25d0}", "chip.service_incomplete", "var(--status-warn,#fc9)")
            } // ◐
            ServiceState::Pending => ("\u{25cc}", "chip.service_pending", "var(--text-dim,#888)"), // ◌
            ServiceState::Refused => ("\u{26a0}", "chip.service_refused", "var(--status-warn,#fc9)"), // ⚠
            ServiceState::Off => ("\u{25cb}", "chip.service_off", "var(--text-dim,#888)"), // ○
        }
    }
}

/// Service-state chip (S4). Use everywhere a switch on this device is shown.
pub fn service_chip(state: ServiceState) -> Element {
    let (g, w, c) = state.parts();
    chip(g, w, c)
}

/// **One service this device offers**, as a block rather than a `label: value`
/// line: its name and state, the control that changes it, what that state means
/// for other devices, and room underneath for the string it produces.
///
/// Append the carry-strings (a [`copy_code`]) to the returned element — that
/// placement is the point. These rows used to be an undifferentiated stack of
/// dim `label: value` text, with the strings a person came here for collected in
/// a separate card further down; so "turn this on" and "here is what to type on
/// the other machine" were two unconnected reading tasks. Attaching the string
/// to the switch that produced it makes flipping a switch visibly *yield* the
/// thing you carry.
pub fn service_row(name: &str, state: ServiceState, detail: &str, control: Element) -> Element {
    let row = util::create_element("div");
    row.set_attribute(
        "style",
        &format!(
            "padding:{} 0;border-top:1px solid var(--border,#333)",
            theme::SP_3
        ),
    )
    .ok();

    // Name + chip on the left, the control hard right — so the switches line up
    // as a column you can run an eye down, whatever the detail text does.
    let head = util::create_element("div");
    head.set_attribute("style", theme::HEADER_ROW).ok();
    let left = util::create_element("div");
    left.set_attribute("style", theme::ROW_INLINE).ok();
    let title = util::create_element("span");
    title
        .set_attribute("style", "font-size:13px;font-weight:600")
        .ok();
    util::set_text(&title, name);
    util::append(&left, &title);
    util::append(&left, &service_chip(state));
    util::append(&head, &left);
    util::append(&head, &control);
    util::append(&row, &head);

    if !detail.is_empty() {
        let p = util::create_element("p");
        p.set_attribute(
            "style",
            &format!("font-size:12px;color:var(--text-muted,#c0c0c0);margin:{} 0 0 0", theme::SP_1),
        )
        .ok();
        util::set_text(&p, detail);
        util::append(&row, &p);
    }
    row
}

/// A labelled string to carry to another device, sitting under the
/// [`service_row`] that produced it. The arrow marks it as an *output* of the
/// switch above rather than another fact about it.
pub fn carry_line(ctx: &util::DomCtx, label: &str, value: &str, hint: Option<&str>) -> Element {
    let wrap = util::create_element("div");
    wrap.set_attribute("style", &format!("margin-top:{}", theme::SP_2)).ok();
    let cap = util::create_element("div");
    cap.set_attribute("style", theme::HINT).ok();
    util::set_text(&cap, &format!("\u{2192} {label}"));
    util::append(&wrap, &cap);
    util::append(&wrap, &copy_code(ctx, value, hint));
    wrap
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
    // The empty-arg default is a real on-screen string, so it resolves through
    // the catalog like any other. `components.rs` is ALLOW-listed in the prose
    // scan (atoms hold the default labels they emit), which means a hardcoded
    // default here is invisible to the gate by construction — the one place
    // where "the atom owns it" has to mean "the atom localizes it".
    let fallback;
    let text = if msg.is_empty() {
        fallback = crate::i18n::t("state.loading", &[]);
        fallback.as_str()
    } else {
        msg
    };
    util::set_text(&p, text);
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

/// A `<pre>` output pane holding a single dim message — the empty state of the
/// scrollback-style dev tools (Event Log, Execute Console, Shell, Path Tap,
/// Wire Recorder, Content Stream), which render their body as innerHTML.
pub fn pre_notice(msg: &str) -> Element {
    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();
    pre.set_inner_html(&format!(
        "<span style='color:var(--text-dim, #888)'>{}</span>",
        util::escape_html(msg)
    ));
    pre
}

/// The "inspect routing failed to attach" warning pane — a loud first line and
/// a dim line telling the developer where to look. Identical in Path Tap, Wire
/// Recorder and Content Stream, which is why it lives here rather than being
/// written out three times.
pub fn inspect_attach_warning() -> Element {
    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();
    pre.set_inner_html(&format!(
        "<span style='color:var(--status-err, #f66)'>{}</span>\n\
         <span style='color:var(--text-dim, #888)'>{}</span>",
        util::escape_html(&crate::i18n::t("inspect.attach_failed", &[])),
        util::escape_html(&crate::i18n::t("inspect.attach_failed_detail", &[])),
    ));
    pre
}

/// Success state — the mirror of [`error`], for an action that reported back
/// well. Same shape and weight so a result line doesn't move or resize when it
/// flips between the two. Exists because a *silent success* is its own bug: the
/// Peer Connections connect had no success surface, so a working connect whose
/// device row the user wasn't watching was indistinguishable from a no-op.
pub fn success(msg: &str) -> Element {
    let p = util::create_element("p");
    p.set_attribute(
        "style",
        &format!(
            "color:var(--status-ok,#6c6);font-size:12px;margin:{} 0",
            theme::SP_1
        ),
    )
    .ok();
    util::set_text(&p, &format!("\u{2713} {msg}")); // ✓
    p
}

/// Error state — loud, specific, actionable. Renders in the error color.
/// An **advisory** line — something the user should know and can act on, which
/// is neither a completed action ([`success`]) nor a failure ([`error`]).
///
/// The third tone exists because both of its first consumers had invented it
/// privately: Chat's "this peer can't be reached back" note and the connector
/// reload notice are the same thing — true, unalarming, and actionable — and
/// were on their way to two different colors. S4 is one vocabulary per status;
/// that has to include the tones, or every window drifts its own amber.
pub fn notice(msg: &str) -> Element {
    let p = util::create_element("p");
    p.set_attribute(
        "style",
        &format!(
            "color:var(--status-warn,#fc9);font-size:12px;margin:{} 0",
            theme::SP_1
        ),
    )
    .ok();
    util::set_text(&p, &format!("\u{26a0} {msg}")); // ⚠
    p
}

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

/// **A long string plus a Copy button** — the shared affordance for getting an
/// identifier onto another device.
///
/// This exists because the two surfaces whose whole job is that (System
/// Overview's pairing rows, Peer Connections' devices) had one copy button
/// between them, and the table that lists *the peers you can transfer files to*
/// rendered only a friendly name — so "which peer is this" and "let me send it
/// to the other machine" had no answer on the screen that raised the question.
///
/// `title` is the hover hint; pass `None` when the surrounding label already
/// says what the string is.
pub fn copy_code(ctx: &crate::dom::util::DomCtx, text: &str, title: Option<&str>) -> Element {
    let holder = crate::dom::util::create_element("span");
    holder.set_attribute("style", crate::dom::theme::ID_ROW).ok();

    let code = crate::dom::util::create_element("code");
    code.set_attribute("style", crate::dom::theme::ID_CODE).ok();
    if let Some(t) = title {
        code.set_attribute("title", t).ok();
    }
    crate::dom::util::set_text(&code, text);
    crate::dom::util::append(&holder, &code);

    let copy = button_el(&crate::i18n::t("btn.copy", &[]), ButtonKind::Secondary);
    {
        let value = text.to_string();
        let el = copy.clone();
        ctx.listen(&copy, "click", move |_| {
            if let Some(win) = web_sys::window() {
                let promise = win.navigator().clipboard().write_text(&value);
                // MUST consume the promise: a dropped *rejecting* promise hits
                // index.html's `unhandledrejection` guard, which reloads the
                // whole app. Clipboard writes reject on denied permission, no
                // focus, or an insecure context — and an insecure context is
                // exactly where someone pairing two machines will be.
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                });
            }
            // Acknowledge regardless: the write may be denied, and a button
            // that never responds reads as broken. The text stays selectable,
            // which is the fallback.
            el.set_text_content(Some(&crate::i18n::t("status.copied", &[])));
        });
    }
    crate::dom::util::append(&holder, &copy);
    holder
}
