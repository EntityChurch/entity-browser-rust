//! Peer Connections DOM renderer — pure consumer of
//! [`PeerConnectionsOutput`](crate::views::peer_connections::output::PeerConnectionsOutput).
//!
//! Structured per REFERENCE-UI-DESIGN: one job per group, bounded cards (S2),
//! repeated records as tables (S7), one authorization vocabulary via the shared
//! `auth_chip` (S4). Organized outbound-first (Direction A): "reach a device"
//! is the primary job; device authorizations (inbound) and pairing follow.

use wasm_bindgen::JsCast;

use crate::action::Action;
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

    render_bound_header(&wrapper, output, ctx);
    render_known_devices(&wrapper, output, ctx);
    render_connect(&wrapper, output, ctx);
    render_pairing_qr(&wrapper, output, ctx);

    util::append(container, &wrapper);
}

/// A slim "this is the peer you're acting as" line — not a card, just context
/// under the title (S2: fold bound info into a header line).
fn render_bound_header(parent: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
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
                &util::escape_html(&output.bound_peer.kind.display_label()),
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

    // The FULL peer id, selectable and copyable. The line above shows the
    // display name, which is the 8…6 truncation whenever the peer has no label
    // — fine for recognizing a peer, useless for the thing people actually need
    // an id for (pasting it into another device, a chat participant list, a bug
    // report). The full value existed in the output all along
    // (`BoundPeerInfo::peer_id`) and was simply never rendered.
    let id_row = util::create_element("div");
    id_row.set_attribute("style", theme::ID_ROW).ok();

    let code = util::create_element("code");
    code.set_attribute("style", theme::ID_CODE).ok();
    util::set_text(&code, &output.bound_peer.peer_id);
    util::append(&id_row, &code);

    let copy = components::button_el(
        &crate::i18n::t("btn.copy", &[]),
        components::ButtonKind::Secondary,
    );
    {
        let pid = output.bound_peer.peer_id.clone();
        let el = copy.clone();
        ctx.listen(&copy, "click", move |_| {
            if let Some(win) = web_sys::window() {
                let promise = win.navigator().clipboard().write_text(&pid);
                // MUST consume the promise. Clipboard writes reject on denied
                // permission / no focus / insecure context, and a DROPPED
                // rejected promise hits index.html's `unhandledrejection`
                // guard, which reloads the whole app (AGENTS.md).
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                });
            }
            // Confirm regardless — the write may be denied, but the click was
            // still received, and a button that never acknowledges reads broken.
            el.set_text_content(Some(&crate::i18n::t("status.copied", &[])));
        });
    }
    util::append(&id_row, &copy);
    util::append(parent, &id_row);
}

/// What became of the last Connect press — in-flight, failed (with the reason),
/// or connected (naming who answered). Rendered inside the Connect card, next to
/// the button that caused it.
///
/// This is the fix for the reported bug, and the failure it closes was silence
/// in BOTH directions: a failed dial reported only to the Event Log window and
/// a `tracing::error!`, and a *successful* dial reported nowhere the user was
/// looking. Pressing Connect and being told nothing is indistinguishable from a
/// dead button (D13).
fn render_connect_outcome(card: &Element, output: &PeerConnectionsOutput) {
    use crate::connect_attempt::ConnectOutcome;
    let Some((addr, outcome)) = &output.last_attempt else {
        return;
    };
    let line = match outcome {
        ConnectOutcome::Dialing => {
            components::loading(&crate::i18n::t("peerconn.connect_dialing", &[("addr", addr)]))
        }
        // The reason travels verbatim from the connect future — "couldn't
        // connect" without a cause leaves the user exactly as stuck.
        ConnectOutcome::Failed(reason) => components::error(&crate::i18n::t(
            "peerconn.connect_failed",
            &[("addr", addr), ("reason", reason)],
        )),
        ConnectOutcome::Connected(remote) => components::success(&crate::i18n::t(
            "peerconn.connect_ok",
            &[("peer", remote)],
        )),
    };
    util::append(card, &line);
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
        // Live status chip (S4), from the one §4c vocabulary. Unknown → a quiet
        // dash (we've paired but have no current signal), not a misleading
        // "connecting".
        let status_cell = match ConnState::from_display(kp.status) {
            Some(state) => components::td(&components::conn_chip(state)),
            None => components::td_text("—"),
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
            if !kp.addr.is_empty() && kp.status != crate::peer_liveness::ConnDisplay::Connected {
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
            // Forget on every REMEMBERED row — a remembered link can read a
            // stale "Connected" (no liveness probe for arbitrary peers), so the
            // operator must be able to clear those too. Forget only drops the
            // remembered entry; it doesn't sever a live transport.
            //
            // Omitted on a metadata-derived row (the auto-provisioned system
            // backend): there is no registry entry to drop, so the click would
            // remove nothing and the row would stay — a visible no-op, which is
            // the same disease as the silent Connect this window just fixed.
            if kp.forgettable {
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
        // (No `window_id` needed any more — the press no longer fires a
        // `clear_address` WindowEvent; see the click handler.)
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
                // The address STAYS in the box. It used to be consumed here, in
                // the click handler — i.e. before the dial had produced any
                // result — so a failed connect silently ate what the user had
                // typed and left them retyping an address they could no longer
                // see. The outcome line below reports what happened to it; the
                // user clears the field when they're done with it, not us.
                let mut acts = actions.borrow_mut();
                acts.push(Action::ConnectPeer { peer_id: from_pid.clone(), addr });
                rp();
            }
        });
    }
    util::append(&card, &btn);

    // What happened to the last press. Without this the action was silent in
    // both directions — a failure showed nothing, and a success whose row the
    // user wasn't watching also showed nothing (D13, the reported bug).
    render_connect_outcome(&card, output);

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
