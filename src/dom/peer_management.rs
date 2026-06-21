//! Peer Management DOM renderer — pure consumer of
//! [`PeerManagementOutput`](crate::views::peer_management::output::PeerManagementOutput).

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::peer_display::PeerDisplay;
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

    // 1b capability gate (MAP §10): a deployment that disables peer creation
    // hides the whole create panel — alias input, the three `+ …` buttons, and
    // the Tauri backend button. The `CreatePeerWithMode` action guard is the
    // hard backstop; this is the defense-in-depth UI half (CR-5).
    if !output.show_peer_create {
        util::append(container, &header);
        return;
    }

    // Mini create panel: an alias input + one button per peer mode.
    // The alias feeds the already-existing `label` param (was always
    // None); blank = no label (falls back to short-pid on display).
    // Each button reads the input at click time so the typed alias
    // applies to whichever mode is chosen.
    let create_panel = util::create_element_with_class("div", "peer-create-panel");

    // Class-styled (NOT theme::INPUT): theme::INPUT is
    // display:block;width:100%, which collapses to a sliver inside the
    // button flex row on narrow screens. `.peer-create-alias` keeps a
    // usable min-width and goes full-width when the panel wraps.
    let alias_input = util::create_element_with_class("input", "peer-create-alias");
    util::set_attr(&alias_input, "type", "text");
    util::set_attr(&alias_input, "placeholder", "alias (optional)");
    util::set_attr(&alias_input, "data-field", "peer-alias");
    util::append(&create_panel, &alias_input);

    // Frontend = main-thread + in-memory; Backend Memory = worker +
    // in-memory; Backend OPFS = worker + OPFS-persisted.
    use crate::peer_mode::PeerMode;
    let create_buttons: [(&str, PeerMode, &str); 3] = [
        ("+ Frontend", PeerMode::Frontend, theme::BTN_PRIMARY),
        ("+ Backend (Memory)", PeerMode::BackendMemory, theme::BTN_SECONDARY),
        ("+ Backend (OPFS)", PeerMode::BackendOpfs, theme::BTN_SECONDARY),
    ];
    for (label, mode, style) in create_buttons {
        let btn = util::create_element("button");
        util::set_text(&btn, label);
        util::set_attr(&btn, "style", style);
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let input_ref = alias_input.clone();
        ctx.listen(&btn, "click", move |_| {
            let label = read_alias(&input_ref);
            actions
                .borrow_mut()
                .push(Action::CreatePeerWithMode { label, mode });
            rp();
        });
        util::append(&create_panel, &btn);
    }

    if output.show_backend_create {
        let backend_btn = util::create_element("button");
        util::set_text(&backend_btn, "+ Tauri Backend");
        util::set_attr(&backend_btn, "style", theme::BTN_SECONDARY);
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let input_ref = alias_input.clone();
        ctx.listen(&backend_btn, "click", move |_| {
            let label = read_alias(&input_ref);
            actions
                .borrow_mut()
                .push(Action::CreateBackendPeer { label });
            rp();
        });
        util::append(&create_panel, &backend_btn);
    }

    util::append(&header, &create_panel);
    util::append(container, &header);
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

    let tbody = util::create_element("tbody");
    for row in &output.rows {
        render_row(&tbody, row, ctx);
    }
    util::append(&table, &tbody);
    util::append(&wrap, &table);
    util::append(container, &wrap);
}

fn render_row(tbody: &Element, row: &PeerRow, ctx: &DomCtx) {
    let tr = util::create_element("tr");

    // Glyph prefix lets you scan type without reading the badge.
    let td_id = util::create_element_with_class("td", "id");
    util::set_text(&td_id, &format!("{} {}", row.role_glyph, row.short_pid));
    util::append(&tr, &td_id);

    let td_kind = util::create_element("td");
    // role_name distinguishes backend-opfs vs backend-memory, which
    // the bare primary/local/remote kind does not.
    let badge_kind_class = match row.kind {
        PeerDisplay::Primary => "peer-badge primary",
        PeerDisplay::Local => "peer-badge local",
        PeerDisplay::Remote => "peer-badge remote",
    };
    let badge = util::create_element_with_class("span", badge_kind_class);
    util::set_text(&badge, &row.role_name);
    util::append(&td_kind, &badge);
    if row.persisted {
        let saved = util::create_element_with_class("span", "peer-saved");
        util::set_text(&saved, " saved");
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

