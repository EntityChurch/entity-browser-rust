//! Peer Connections DOM renderer — pure consumer of
//! [`PeerConnectionsOutput`](crate::views::peer_connections::output::PeerConnectionsOutput).
//!
//! Structured per REFERENCE-UI-DESIGN: one job per group, bounded cards (S2),
//! repeated records as tables (S7), one authorization vocabulary via the shared
//! `auth_chip` (S4). Organized outbound-first (Direction A): "reach a device"
//! is the primary job; device authorizations (inbound) and pairing follow.

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::connection_health::Liveness;
use crate::dom::components::{self, ConnState};
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::peer_display::PeerDisplay;
use crate::views::peer_connections::model::generate_qr_svg;
use crate::views::peer_connections::output::PeerConnectionsOutput;

use web_sys::Element;

pub fn render(container: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "peer-connections");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    util::set_text(&h2, "Peer Connections");
    util::append(&wrapper, &h2);

    render_bound_header(&wrapper, output);
    render_known_devices(&wrapper, output, ctx);
    render_connect(&wrapper, output, ctx);
    render_pairing_qr(&wrapper, output, ctx);

    util::append(container, &wrapper);
}

/// A slim "this is the peer you're acting as" line — not a card, just context
/// under the title (S2: fold bound info into a header line).
fn render_bound_header(parent: &Element, output: &PeerConnectionsOutput) {
    let line = util::create_element("p");
    line.set_attribute(
        "style",
        &format!("margin:0 0 {} 0;font-size:12px;color:var(--text-dim,#888)", theme::SP_3),
    )
    .ok();
    let mut html = format!(
        "This peer <code>{}</code> · {}",
        util::escape_html(&output.bound_peer.short_pid),
        util::escape_html(&output.bound_peer.kind.to_string()),
    );
    if output.bound_peer.kind == PeerDisplay::Primary {
        if let Some(addr) = &output.bound_peer.ws_listen_addr {
            html.push_str(&format!(
                " · listening <code style='color:var(--status-ok,#4c4)'>{}</code>",
                util::escape_html(addr)
            ));
        }
    }
    line.set_inner_html(&html);
    util::append(parent, &line);
}

/// Outbound: devices we've reached before, each with its live status. A proper
/// table (S7) — Device · Status · Address · action. Status comes from the
/// subscribed connection-health mirror (S4 chip), so a live device reads
/// "Connected" instead of always offering "Reconnect". Hidden when empty.
fn render_known_devices(parent: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    if output.known_peers.is_empty() {
        return;
    }
    let card = components::card("Known devices");
    let (tbl, body) = components::table(&["Device", "Status", "Address", ""]);

    for kp in &output.known_peers {
        // Live status chip (S4). Unknown → a quiet dash (we've paired but have
        // no current signal), not a misleading "connecting".
        let status_cell = match kp.liveness {
            Liveness::Connected => components::td(&components::conn_chip(ConnState::Connected)),
            Liveness::Connecting => components::td(&components::conn_chip(ConnState::Connecting)),
            Liveness::Unreachable => components::td(&components::conn_chip(ConnState::Offline)),
            Liveness::Unknown => components::td_text("—"),
        };

        let addr_cell = if kp.addr.is_empty() {
            components::td_text("—")
        } else {
            components::td_text(&kp.addr)
        };

        // Offer Reconnect unless it's already connected (that's the confusing
        // case) — and only when there's an address to dial.
        let action_cell = if kp.addr.is_empty() || kp.liveness == Liveness::Connected {
            components::td_text("")
        } else {
            let btn = util::create_element("button");
            util::set_text(&btn, "Reconnect");
            btn.set_attribute("style", theme::BTN_SECONDARY).ok();
            ctx.on_action(
                &btn,
                "click",
                Action::ConnectPeer {
                    peer_id: output.bound_peer.peer_id.clone(),
                    addr: kp.addr.clone(),
                },
            );
            components::td(&btn)
        };

        util::append(
            &body,
            &components::tr(vec![
                components::td_text(&kp.display),
                status_cell,
                addr_cell,
                action_cell,
            ]),
        );
    }
    util::append(&card, &tbl);
    util::append(parent, &card);
}

/// Outbound primary: dial a device by address, or scan its QR to fill the
/// address in. One bounded card (S2) — this is what the old free-floating
/// "Connect to Address" + scanner become.
fn render_connect(parent: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    let card = components::card("Connect to a device");

    let input = util::create_element("input");
    input.set_attribute("type", "text").ok();
    input.set_attribute("value", &output.address_input_initial).ok();
    input.set_attribute("placeholder", "ws://192.168.1.10:4041").ok();
    input.set_attribute("data-field", "address").ok();
    input.set_attribute("style", theme::INPUT).ok();
    util::append(&card, &input);

    let btn = util::create_element("button");
    util::set_text(&btn, "Connect");
    btn.set_attribute("style", theme::BTN_PRIMARY).ok();
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        let card_ref = card.clone();
        let from_pid = output.bound_peer.peer_id.clone();
        ctx.listen(&btn, "click", move |_| {
            let addr = card_ref
                .query_selector("[data-field='address']")
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
                .map(|inp| inp.value())
                .unwrap_or_default();
            if !addr.is_empty() {
                let mut acts = actions.borrow_mut();
                acts.push(Action::ConnectPeer { peer_id: from_pid.clone(), addr });
                acts.push(Action::WindowEvent {
                    window_id: wid,
                    event: "clear_address".into(),
                    value: String::new(),
                });
                rp();
            }
        });
    }
    util::append(&card, &btn);

    // Scan a device's QR to populate the address (outbound: I scan them).
    render_scan_qr(&card, ctx);

    util::append(parent, &card);
}

/// The QR *scanner* — a lazy-on-open disclosure that fills the address input in
/// the same card when it decodes a code. (Advertising *our* QR is the inbound
/// job — [`render_pairing_qr`].)
fn render_scan_qr(card: &Element, ctx: &DomCtx) {
    let scan_details = util::create_element("details");
    scan_details.set_attribute("style", &format!("margin-top:{}", theme::SP_2)).ok();
    let scan_summary = util::create_element("summary");
    scan_summary
        .set_attribute("style", "cursor:pointer;font-size:12px;padding:4px 0;color:var(--accent-2,#c0c0e0)")
        .ok();
    util::set_text(&scan_summary, "Scan a QR code");
    util::append(&scan_details, &scan_summary);

    let scan_container = util::create_element("div");
    scan_container.set_attribute("style", &format!("margin-top:{}", theme::SP_2)).ok();

    let scanner_initialized = std::rc::Rc::new(std::cell::RefCell::new(false));
    {
        let container_ref = scan_container.clone();
        let init_ref = scanner_initialized;
        let scan_closures = ctx.closures.clone();
        // Our QR payload is `{ws_addr}|{peer_id}`; take the address (before the
        // first '|') and drop it into the connect input in this card.
        let on_scan: std::rc::Rc<dyn Fn(String)> = {
            let root = card.clone();
            std::rc::Rc::new(move |scanned: String| {
                let addr = scanned.split('|').next().unwrap_or(&scanned).trim();
                if addr.is_empty() {
                    return;
                }
                if let Ok(Some(el)) = root.query_selector("[data-field='address']") {
                    if let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() {
                        input.set_value(addr);
                    }
                }
            })
        };
        let active = std::rc::Rc::new(std::cell::RefCell::new(true));
        ctx.listen(&scan_details, "toggle", move |_| {
            if !*init_ref.borrow() {
                *init_ref.borrow_mut() = true;
                crate::dom::scanner::create_scanner(
                    &container_ref,
                    on_scan.clone(),
                    active.clone(),
                    &scan_closures,
                );
            }
        });
    }
    util::append(&scan_details, &scan_container);
    util::append(card, &scan_details);
}

/// Inbound: advertise *this* backend's pairing target so another device can
/// scan it. Lazy-generated on open (QR build is the window's dominant cost) and
/// only shown when there's a listener here to advertise.
fn render_pairing_qr(parent: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    let Some(payload) = output.qr_payload.clone() else {
        return;
    };
    let card = components::card("Pair a device (QR)");

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, "Scan this from another device to connect it here.");
    util::append(&card, &hint);

    let qr_details = util::create_element("details");
    let qr_summary = util::create_element("summary");
    qr_summary
        .set_attribute("style", "cursor:pointer;font-size:12px;padding:4px 0;color:var(--accent-2,#c0c0e0)")
        .ok();
    util::set_text(&qr_summary, "Show QR code");
    util::append(&qr_details, &qr_summary);

    let qr_content = util::create_element("div");
    qr_content.set_attribute("style", &format!("margin-top:{}", theme::SP_2)).ok();
    {
        let content_ref = qr_content.clone();
        let qr_initialized = std::rc::Rc::new(std::cell::RefCell::new(false));
        ctx.listen(&qr_details, "toggle", move |_| {
            if *qr_initialized.borrow() {
                return;
            }
            *qr_initialized.borrow_mut() = true;
            let svg = generate_qr_svg(&payload);
            content_ref.set_inner_html(&format!(
                "<div style='background:white;display:inline-block;max-width:100%;box-sizing:border-box;\
                 padding:12px;border-radius:4px'>{}</div>\
                 <p style='margin-top:4px;max-width:100%'>\
                 <code style='font-size:11px;word-break:break-all'>{}</code></p>",
                svg,
                util::escape_html(&payload)
            ));
        });
    }
    util::append(&qr_details, &qr_content);
    util::append(&card, &qr_details);

    util::append(parent, &card);
}
