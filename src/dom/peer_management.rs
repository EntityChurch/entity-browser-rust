//! Peer Management DOM renderer — pure consumer of
//! [`PeerManagementOutput`](crate::views::peer_management::output::PeerManagementOutput).

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::components;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::peer_display::PeerRole;
use crate::views::peer_management::output::{
    AddressDisplay, BackendButton, PeerManagementOutput, PeerRow,
};

use web_sys::Element;

// All visual styling lives in `dom/style.rs` under the `.peer-table` /
// `.peer-badge` rules. Switching to classes drops the
// per-row format!() + set_attr("style", ...) churn that contributed
// to the peer-delete rebuild storm — three subscribed windows all
// rebuilt their full DOM each delete; this window had the highest
// row count and the heaviest style-string formatting.

pub fn render(container: &Element, output: &PeerManagementOutput, ctx: &DomCtx) {
    util::clear_children(container);
    // Layout is class-based (wrapper div, mirroring the other window
    // renderers) so the shadow-DOM stylesheet's media queries can make
    // it responsive — inline styles can't be overridden for narrow
    // screens, which is why the alias input collapsed on mobile.
    let root = util::create_element_with_class("div", "peer-mgmt");
    render_header(&root, output, ctx);
    render_table(&root, output, ctx);
    render_footer(&root, output);
    util::append(container, &root);
}

fn render_header(container: &Element, output: &PeerManagementOutput, ctx: &DomCtx) {
    let header = util::create_element_with_class("div", "peer-mgmt-header");

    let h2 = util::create_element("h2");
    util::set_text(&h2, "Peers");
    util::append(&header, &h2);
    // The title row stands alone; the create form is a collapsible block below
    // it, so a peer list isn't permanently topped by a form you rarely use.
    util::append(container, &header);

    // 1b capability gate (MAP §10): a deployment that disables peer creation
    // hides the whole create affordance. The `CreatePeerWithMode` action guard
    // is the hard backstop; this is the defense-in-depth UI half (CR-5).
    if !output.show_peer_create {
        return;
    }

    // Create form: ONE kind selector (where it runs · how it persists) + an
    // optional alias + a single Add button — replaces the button-per-combo, so a
    // new configuration is one more option, not another button. Both fields live
    // in their DOM elements and are read at click time (never tree-backed: a
    // per-change `tree.put` would rebuild the panel and drop focus/selection).
    let create_panel = util::create_element_with_class("div", "peer-create-panel");

    // Kind selector — S6 (speak the user's terms): the label names the two facts
    // a person actually chooses between, **where it runs** and **whether it's
    // saved**, not the runtime/storage machinery (that stays as the honest chips
    // on the management table below). Availability + labels come from the model
    // (`output.create_options`, system-aware); values are `PeerMode::persist_key`
    // + the special `native` → CreateBackendPeer. These are the modes we currently
    // wire — not the full (runtime × storage) matrix; the missing combos (this-tab
    // saved / native temporary) are an unexposed `PeerMode` gap, not a limit.
    let kind_select = util::create_element_with_class("select", "peer-create-kind");
    // System-aware options: the model gates each mode by what THIS runtime can
    // create. Unsupported ones render **disabled with the reason appended** (e.g.
    // "Native app · saved — desktop app only"), so a config we can't build is
    // understood, not silently missing. Values stay the durable persist-keys.
    for spec in &output.create_options {
        let opt = util::create_element("option");
        util::set_attr(&opt, "value", spec.value);
        let text = match spec.reason {
            Some(r) if !spec.available => format!("{} — {r}", spec.label),
            _ => spec.label.to_string(),
        };
        util::set_text(&opt, &text);
        if !spec.available {
            util::set_attr(&opt, "disabled", "");
        }
        util::append(&kind_select, &opt);
    }
    util::append(&create_panel, &kind_select);

    // Class-styled (NOT theme::INPUT): theme::INPUT is
    // display:block;width:100%, which collapses to a sliver inside the
    // flex row on narrow screens. `.peer-create-alias` keeps a
    // usable min-width and goes full-width when the panel wraps.
    let alias_input = util::create_element_with_class("input", "peer-create-alias");
    util::set_attr(&alias_input, "type", "text");
    util::set_attr(&alias_input, "placeholder", "alias (optional)");
    util::set_attr(&alias_input, "data-field", "peer-alias");
    util::append(&create_panel, &alias_input);

    use crate::peer_mode::PeerMode;
    let add_btn = util::create_element("button");
    util::set_text(&add_btn, "Add peer");
    util::set_attr(&add_btn, "style", theme::BTN_PRIMARY);
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let alias_ref = alias_input.clone();
        let select_ref = kind_select.clone();
        let wid = ctx.window_id;
        ctx.listen(&add_btn, "click", move |_| {
            let label = read_alias(&alias_ref);
            let kind = read_select_value(&select_ref);
            let action = if kind == "native" {
                Action::CreateBackendPeer { label }
            } else if let Some(mode) = PeerMode::from_persist_key(&kind) {
                Action::CreatePeerWithMode { label, mode }
            } else {
                // Unknown option value — should be impossible; ignore rather
                // than default-create the wrong kind of peer.
                return;
            };
            actions.borrow_mut().push(action);
            // Collapse the create card now that the peer is queued (S8) — you're
            // back to the list. Model-held close, routed via handle_action.
            actions.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: crate::views::peer_management::EV_CLOSE_CREATE.to_string(),
                value: String::new(),
            });
            rp();
        });
    }
    util::append(&create_panel, &add_btn);

    // S6: a full-sentence, plain-language description of the selected kind,
    // updated live on `change`. Pure DOM text — no tree write, so it never
    // triggers a snapshot rebuild that would drop the select/alias focus.
    let hint = util::create_element_with_class("span", "peer-create-hint");
    util::set_text(&hint, kind_description("frontend"));
    {
        let hint_ref = hint.clone();
        let select_ref = kind_select.clone();
        ctx.listen(&kind_select, "change", move |_| {
            let v = read_select_value(&select_ref);
            util::set_text(&hint_ref, kind_description(&v));
        });
    }
    util::append(&create_panel, &hint);

    // Collapsible card (S8 create affordance): the SHARED `collapsible_header`
    // primitive + the form, hidden when collapsed. Progressive disclosure — you
    // look at your peers, not a form. Open state is model-held (`output.create_open`)
    // so it survives a snapshot rebuild; the header dispatches EV_TOGGLE_CREATE,
    // and the Add closure dispatches EV_CLOSE_CREATE so it tidies away on success.
    // NOT a native `<details>` (which snaps shut on any repaint) — the exact
    // aligned pattern Site Creator uses (REFERENCE-UI-DESIGN S8).
    util::append(
        container,
        &components::collapsible_header(
            ctx,
            "Add a peer",
            output.create_open,
            crate::views::peer_management::EV_TOGGLE_CREATE,
        ),
    );
    if !output.create_open {
        // Hidden (not un-rendered): the form stays in the DOM so its fields are
        // read at submit time and driving is stable; the class provides flex when
        // shown, this inline display:none overrides it when collapsed.
        util::set_attr(&create_panel, "style", "display:none");
    }
    util::append(container, &create_panel);
}

/// One-line, human description of a create-peer kind (S6). Keyed by the option
/// value (`PeerMode::persist_key`, or `"native"`). Describes the modes we
/// currently wire; it does not claim other `(runtime × storage)` combos are
/// impossible — those are an unexposed enum gap, not a substrate limit.
fn kind_description(value: &str) -> &'static str {
    match value {
        "frontend" => "Main thread of this tab, in-memory. Temporary — cleared when you reload.",
        "frontend-idb" => "Main thread of this tab, saved to IndexedDB. Survives reload.",
        "backend-memory" => "A background Web Worker, in-memory. Temporary — cleared when you reload.",
        "backend-opfs" => "A background Web Worker, saved to OPFS. Survives reload.",
        "native" => "A separate native desktop process with its own on-disk store. Saved.",
        _ => "",
    }
}

fn render_table(container: &Element, output: &PeerManagementOutput, ctx: &DomCtx) {
    // Wrapper scrolls horizontally on narrow screens instead of
    // squashing the 5-column table (or pushing the header off-screen).
    let wrap = util::create_element_with_class("div", "peer-table-wrap");
    let table = util::create_element_with_class("table", "peer-table");

    let thead = util::create_element("thead");
    let hrow = util::create_element("tr");
    for heading in &["Peer ID", "Kind", "Label", "Address", ""] {
        let th = util::create_element("th");
        util::set_text(&th, heading);
        util::append(&hrow, &th);
    }
    util::append(&thead, &hrow);
    util::append(&table, &thead);

    // Group the roster: System peers (always-on infrastructure — one in a
    // browser, two on desktop once the native peer is up) are set apart from
    // user-created peers (UI standard S2: bounded, labeled groups). Role comes
    // straight off the descriptor.
    let (system_rows, user_rows): (Vec<&PeerRow>, Vec<&PeerRow>) = output
        .rows
        .iter()
        .partition(|r| r.descriptor.role == PeerRole::System);

    let tbody = util::create_element("tbody");
    // There is always ≥1 system peer; the "Your peers" group only appears once
    // the user has created one (the create form above is how they do it).
    append_group(&tbody, "System peers", "always-on", &system_rows, ctx);
    if !user_rows.is_empty() {
        append_group(&tbody, "Your peers", "created by you", &user_rows, ctx);
    }
    util::append(&table, &tbody);
    util::append(&wrap, &table);
    util::append(container, &wrap);
}

/// Emit a labeled group of peer rows: a full-width group-label row (title +
/// faint subtitle) followed by the rows themselves.
fn append_group(tbody: &Element, title: &str, subtitle: &str, rows: &[&PeerRow], ctx: &DomCtx) {
    let label_tr = util::create_element_with_class("tr", "peer-group");
    let td = util::create_element("td");
    util::set_attr(&td, "colspan", "5");
    util::set_text(&td, title);
    let sub = util::create_element_with_class("span", "peer-group-sub");
    util::set_text(&sub, subtitle);
    util::append(&td, &sub);
    util::append(&label_tr, &td);
    util::append(tbody, &label_tr);

    for row in rows {
        render_row(tbody, row, ctx);
    }
}

fn render_row(tbody: &Element, row: &PeerRow, ctx: &DomCtx) {
    let tr = util::create_element("tr");

    // Glyph prefix lets you scan type without reading the badge.
    let td_id = util::create_element_with_class("td", "id");
    util::set_text(&td_id, &format!("{} {}", row.descriptor.glyph(), row.short_pid));
    util::append(&tr, &td_id);

    // Kind cell: a System/User role badge + a "where it runs" chip + a
    // "how it persists" chip. Replaces the single "backend (memory)"-style
    // string — three orthogonal facts, each scannable.
    let td_kind = util::create_element("td");
    let (role_class, role_text) = match row.descriptor.role {
        PeerRole::System => ("peer-badge system", "System"),
        PeerRole::User => ("peer-badge user", "User"),
    };
    let badge = util::create_element_with_class("span", role_class);
    util::set_text(&badge, role_text);
    util::append(&td_kind, &badge);

    let runtime_chip = util::create_element_with_class("span", "peer-chip");
    util::set_text(&runtime_chip, row.descriptor.runtime.label());
    util::append(&td_kind, &runtime_chip);

    let storage_chip = util::create_element_with_class("span", "peer-chip");
    util::set_text(&storage_chip, row.descriptor.storage.label());
    util::append(&td_kind, &storage_chip);

    if row.persisted {
        let saved = util::create_element_with_class("span", "peer-saved");
        util::set_text(&saved, "saved");
        util::append(&td_kind, &saved);
    }
    util::append(&tr, &td_kind);

    let td_label = util::create_element("td");
    let label_str = row.label.as_deref().unwrap_or("-");
    util::set_text(&td_label, label_str);
    util::append(&tr, &td_label);

    // Address column variants — the conditional class encodes the
    // three states without per-row format!() into the style attr.
    let td_addr = match &row.address {
        AddressDisplay::Stopped => {
            let td = util::create_element_with_class("td", "addr-stopped");
            util::set_text(&td, "stopped");
            td
        }
        AddressDisplay::Addresses(s) => {
            let td = util::create_element_with_class("td", "addr-list");
            util::set_text(&td, s);
            td
        }
        AddressDisplay::None => {
            let td = util::create_element_with_class("td", "addr-none");
            util::set_text(&td, "-");
            td
        }
    };
    util::append(&tr, &td_addr);

    let td_actions = util::create_element_with_class("td", "actions");

    if row.show_open_tree {
        let open_btn = util::create_element("button");
        util::set_text(&open_btn, "Tree");
        util::set_attr(&open_btn, "style", theme::BTN_PRIMARY);
        ctx.on_action(
            &open_btn,
            "click",
            Action::SpawnWindow {
                type_name: "Entity Tree",
                peer_id: Some(row.peer_id.clone()),
            },
        );
        util::append(&td_actions, &open_btn);
    }

    if let Some(button) = row.backend_button {
        match button {
            BackendButton::Stop => {
                let stop_btn = util::create_element("button");
                util::set_text(&stop_btn, "Stop");
                util::set_attr(&stop_btn, "style", theme::BTN_SECONDARY);
                ctx.on_action(&stop_btn, "click", Action::StopBackendPeer(row.peer_id.clone()));
                util::append(&td_actions, &stop_btn);
            }
            BackendButton::Start => {
                let start_btn = util::create_element("button");
                util::set_text(&start_btn, "Start");
                util::set_attr(&start_btn, "style", theme::BTN_PRIMARY);
                ctx.on_action(&start_btn, "click", Action::StartBackendPeer(row.peer_id.clone()));
                util::append(&td_actions, &start_btn);
            }
        }
    }

    if row.show_delete {
        // theme::BTN_SECONDARY for the button itself, plus the
        // peer-action-delete class for the margin-left offset from
        // the preceding button.
        let del_btn = util::create_element_with_class("button", "peer-action-delete");
        util::set_text(&del_btn, "Delete");
        util::set_attr(&del_btn, "style", theme::BTN_SECONDARY);
        ctx.on_action(&del_btn, "click", Action::DeletePeer(row.peer_id.clone()));
        util::append(&td_actions, &del_btn);
    }

    util::append(&tr, &td_actions);
    util::append(tbody, &tr);
}

fn render_footer(container: &Element, output: &PeerManagementOutput) {
    let footer = util::create_element("div");
    util::set_attr(&footer, "style", "color: var(--text-faint, #666); font-size: 0.85em;");
    // "SDK" was internal jargon — what the user actually cares about
    // is how many isolation boundaries are running. SDK slot 0 is the
    // boot host (main thread in Direct mode, boot worker in Worker
    // mode); slots 1+ are dedicated workers spawned for Backend(Memory)
    // / Backend(OPFS) peers.
    let dedicated = output.sdk_count.saturating_sub(1);
    let text = if dedicated == 0 {
        format!("{} peer(s)", output.total_count)
    } else {
        format!(
            "{} peer(s) — 1 boot + {} dedicated worker(s)",
            output.total_count, dedicated
        )
    };
    util::set_text(&footer, &text);
    util::append(container, &footer);
}

/// Read the current value of a `<select>` (the chosen option's `value`).
fn read_select_value(select: &Element) -> String {
    select
        .dyn_ref::<web_sys::HtmlSelectElement>()
        .map(|s| s.value())
        .unwrap_or_default()
}

/// Read + trim the alias input. Empty → `None` (no label; display
/// falls back to short-pid).
fn read_alias(input: &Element) -> Option<String> {
    let v = input
        .dyn_ref::<web_sys::HtmlInputElement>()
        .map(|i| i.value())
        .unwrap_or_default();
    let v = v.trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

