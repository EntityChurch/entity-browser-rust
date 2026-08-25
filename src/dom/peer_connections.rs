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

/// Drafts key for the connect-address input (`components::text_input`).
const ADDRESS_FIELD: &str = "address";

pub fn render(container: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "peer-connections");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let h2 = util::create_element("h2");
    util::set_text(&h2, &crate::i18n::t("window.peer_connections", &[]));
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
    // The `<code>` wrapper travels inside the slot value: `t()` bidi-isolates
    // each arg, so the peer id stays LTR inside an RTL sentence, and the locale
    // keeps control of where the id and the kind sit.
    let mut html = crate::i18n::t(
        "peerconn.this_peer",
        &[
            (
                "pid",
                &format!("<code>{}</code>", util::escape_html(&output.bound_peer.short_pid)),
            ),
            (
                "kind",
                &util::escape_html(&output.bound_peer.kind.to_string()),
            ),
        ],
    );
    if output.bound_peer.kind == PeerDisplay::Primary {
        if let Some(addr) = &output.bound_peer.ws_listen_addr {
            html.push_str(&crate::i18n::t(
                "peerconn.listening",
                &[(
                    "addr",
                    &format!(
                        "<code style='color:var(--status-ok,#4c4)'>{}</code>",
                        util::escape_html(addr)
                    ),
                )],
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
    let card = components::card(&crate::i18n::t("peers.known_devices", &[]));
    let (tbl, body) = components::table(&[
        &crate::i18n::t("label.device", &[]),
        &crate::i18n::t("label.status", &[]),
        &crate::i18n::t("label.address", &[]),
        "",
    ]);

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

        // Actions: Reconnect (when there's an address and it isn't already
        // live) + Forget (drop a dead/stale row so the list can be cleaned up).
        // A live connection gets neither — don't invite dropping the working link.
        let action_cell = {
            let wrap = util::create_element("div");
            wrap.set_attribute("style", "display:flex;gap:6px").ok();
            if !kp.addr.is_empty() && kp.liveness != Liveness::Connected {
                let btn = components::button_action(
                    ctx,
                    &crate::i18n::t("peers.reconnect", &[]),
                    components::ButtonKind::Secondary,
                    Action::ConnectPeer {
                        peer_id: output.bound_peer.peer_id.clone(),
                        addr: kp.addr.clone(),
                    },
                );
                util::append(&wrap, &btn);
            }
            // Forget on every row — a remembered link can read a stale
            // "Connected" (no liveness probe for arbitrary peers), so the
            // operator must be able to clear those too. Forget only drops the
            // remembered entry; it doesn't sever a live transport.
            {
                let btn = components::button_action(
                    ctx,
                    &crate::i18n::t("peers.forget", &[]),
                    components::ButtonKind::Secondary,
                    Action::ForgetConnection { remote_pid: kp.remote_pid.clone() },
                );
                util::append(&wrap, &btn);
            }
            components::td(&wrap)
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
    let card = components::card(&crate::i18n::t("peers.connect_device", &[]));

    // Draft-tracked atom (S8): typing lands in `ctx.drafts`, so an unrelated
    // repaint (e.g. a connection-health change) rebuilds the field with the
    // typed value intact — the mechanism the fresh-connect "typed it, hit
    // Connect, nothing" bug demanded (`BUGLOG-2026-07-14` B3/B4), now via the
    // one shared input instead of a bespoke per-keystroke listener.
    let input = components::text_input(
        ctx,
        ADDRESS_FIELD,
        &output.address_input_initial,
        "ws://192.168.1.10:4041",
    );
    util::append(
        &card,
        &components::field(&crate::i18n::t("label.address", &[]), "", &input),
    );

    let btn = components::button_el(&crate::i18n::t("btn.connect", &[]), components::ButtonKind::Primary);
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        let drafts = ctx.drafts.clone();
        let initial = output.address_input_initial.clone();
        let from_pid = output.bound_peer.peer_id.clone();
        ctx.listen(&btn, "click", move |_| {
            // Submit-time read: the draft when the user typed (or scanned),
            // else the render's initial (the untouched suggestion).
            let addr = drafts
                .borrow()
                .get(ADDRESS_FIELD)
                .cloned()
                .unwrap_or_else(|| initial.clone());
            if !addr.is_empty() {
                // Consume the draft so the repaint clears the field instead of
                // resurrecting the just-dialed address.
                drafts.borrow_mut().remove(ADDRESS_FIELD);
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
    util::set_text(&scan_summary, &crate::i18n::t("peers.scan_qr", &[]));
    util::append(&scan_details, &scan_summary);

    let scan_container = util::create_element("div");
    scan_container.set_attribute("style", &format!("margin-top:{}", theme::SP_2)).ok();

    let scanner_initialized = std::rc::Rc::new(std::cell::RefCell::new(false));
    {
        let container_ref = scan_container.clone();
        let init_ref = scanner_initialized;
        let scan_closures = ctx.closures.clone();
        // Our QR payload is `{ws_addr}|{peer_id}`; take the address (before the
        // first '|') and drop it into the connect input in this card. Write the
        // draft too — the input's value is rebuilt FROM `ctx.drafts` and Connect
        // submits from it, so a DOM-only fill would vanish on the next repaint.
        let on_scan: std::rc::Rc<dyn Fn(String)> = {
            let root = card.clone();
            let drafts = ctx.drafts.clone();
            std::rc::Rc::new(move |scanned: String| {
                let addr = scanned.split('|').next().unwrap_or(&scanned).trim();
                if addr.is_empty() {
                    return;
                }
                drafts
                    .borrow_mut()
                    .insert(ADDRESS_FIELD.to_string(), addr.to_string());
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
    let card = components::card(&crate::i18n::t("peers.pair_qr", &[]));

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &crate::i18n::t("peers.scan_hint", &[]));
    util::append(&card, &hint);

    let qr_details = util::create_element("details");
    let qr_summary = util::create_element("summary");
    qr_summary
        .set_attribute("style", "cursor:pointer;font-size:12px;padding:4px 0;color:var(--accent-2,#c0c0e0)")
        .ok();
    util::set_text(&qr_summary, &crate::i18n::t("peers.show_qr", &[]));
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
