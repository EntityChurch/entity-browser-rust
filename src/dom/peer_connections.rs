//! Peer Connections DOM renderer — pure consumer of
//! [`PeerConnectionsOutput`](crate::views::peer_connections::output::PeerConnectionsOutput).

use wasm_bindgen::JsCast;

use crate::action::Action;
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

    render_bound_info(&wrapper, output);
    render_connected(&wrapper, output);
    render_backend_peers(&wrapper, output, ctx);
    render_manual_connect(&wrapper, output, ctx);
    render_qr_section(&wrapper, output, ctx);

    util::append(container, &wrapper);
}

fn render_bound_info(parent: &Element, output: &PeerConnectionsOutput) {
    let info = util::create_element("div");
    info.set_attribute("style", theme::SECTION_GROUP).ok();
    let mut html = format!(
        "<strong>Peer</strong><br>\
         <span style='color:var(--text-dim, #888)'>ID:</span> <code>{}</code>\
         <br><span style='color:var(--text-dim, #888)'>Kind:</span> <code>{}</code>",
        util::escape_html(&output.bound_peer.short_pid),
        output.bound_peer.kind,
    );
    if output.bound_peer.kind == PeerDisplay::Primary {
        if let Some(addr) = &output.bound_peer.ws_listen_addr {
            html.push_str(&format!(
                "<br><span style='color:var(--text-dim, #888)'>WebSocket:</span> <code style='color:var(--status-ok, #0f0)'>{}</code>",
                util::escape_html(addr)
            ));
        }
    }
    info.set_inner_html(&html);
    util::append(parent, &info);
}

fn render_connected(parent: &Element, output: &PeerConnectionsOutput) {
    if output.connected.is_empty() {
        return;
    }
    let conn_div = util::create_element("div");
    conn_div.set_attribute("style", theme::SECTION_GROUP).ok();
    let mut html = String::from("<strong>Connected Peers</strong><br>");
    for rpid in &output.connected {
        html.push_str(&format!(
            "<span style='color:var(--status-ok, #0f0)'>●</span> <code>{}</code><br>",
            util::escape_html(&crate::views::short_pid(rpid))
        ));
    }
    conn_div.set_inner_html(&html);
    util::append(parent, &conn_div);
}

fn render_backend_peers(parent: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    if output.backend_peers.is_empty() {
        return;
    }
    let known_div = util::create_element("div");
    known_div.set_attribute("style", theme::SECTION_GROUP).ok();
    let label = util::create_element("strong");
    util::set_text(&label, "Backend Peers");
    util::append(&known_div, &label);

    for peer in &output.backend_peers {
        let row = util::create_element_with_class("div", "peer-conn-backend-row");

        let info_span =
            util::create_element_with_class("span", "peer-conn-backend-info");
        util::set_text(&info_span, &peer.display);
        util::append(&row, &info_span);

        for addr in &peer.connect_addresses {
            let connect_btn = util::create_element("button");
            util::set_text(&connect_btn, addr);
            connect_btn.set_attribute("style", theme::BTN_SECONDARY).ok();
            ctx.on_action(
                &connect_btn,
                "click",
                Action::ConnectPeer {
                    peer_id: output.bound_peer.peer_id.clone(),
                    addr: addr.clone(),
                },
            );
            util::append(&row, &connect_btn);
        }

        util::append(&known_div, &row);
    }
    util::append(parent, &known_div);
}

fn render_manual_connect(parent: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    let label = util::create_element("strong");
    util::set_text(&label, "Connect to Address");
    util::append(parent, &label);

    let input = util::create_element("input");
    input.set_attribute("type", "text").ok();
    input
        .set_attribute("value", &output.address_input_initial)
        .ok();
    input
        .set_attribute("placeholder", "ws://192.168.1.10:4041")
        .ok();
    input.set_attribute("data-field", "address").ok();
    input.set_attribute("style", theme::INPUT).ok();
    util::append(parent, &input);

    let btn = util::create_element("button");
    util::set_text(&btn, "Connect");
    btn.set_attribute("style", theme::BTN_PRIMARY).ok();
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let wid = ctx.window_id;
        let parent_ref = parent.clone();
        let from_pid = output.bound_peer.peer_id.clone();
        ctx.listen(&btn, "click", move |_| {
            let addr = parent_ref
                .query_selector("[data-field='address']")
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
                .map(|inp| inp.value())
                .unwrap_or_default();
            if !addr.is_empty() {
                let mut acts = actions.borrow_mut();
                acts.push(Action::ConnectPeer {
                    peer_id: from_pid.clone(),
                    addr,
                });
                acts.push(Action::WindowEvent {
                    window_id: wid,
                    event: "clear_address".into(),
                    value: String::new(),
                });
                rp();
            }
        });
    }
    util::append(parent, &btn);
}

fn render_qr_section(parent: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    let qr_section = util::create_element("div");
    qr_section
        .set_attribute(
            "style",
            "margin-top:12px;border-top:1px solid var(--border, #333);padding-top:8px",
        )
        .ok();

    // QR Pairing DISPLAY — only when there's a listener here to advertise
    // (this process's native listener, or the system's Tauri backend).
    // A `no-listener` QR is useless, so when `qr_payload` is None we skip
    // the display entirely; the scanner below stays available regardless
    // (you can pair by scanning from any peer). See the model's
    // `qr_payload` derivation and output.rs.
    if let Some(payload) = output.qr_payload.clone() {
        let qr_details = util::create_element("details");
        let qr_summary = util::create_element("summary");
        qr_summary
            .set_attribute("style", "cursor:pointer;font-weight:bold;padding:4px 0")
            .ok();
        util::set_text(&qr_summary, "QR Pairing");
        util::append(&qr_details, &qr_summary);

        // QR generation (Reed-Solomon encode + per-module SVG build + the
        // set_inner_html parse) is the dominant render cost for this
        // window. The <details> ships collapsed and the payload almost
        // never changes, so defer generation until the user opens it —
        // same lazy-on-toggle pattern as the "Scan QR Code" sibling below.
        let qr_content = util::create_element("div");
        qr_content.set_attribute("style", "margin-top:8px").ok();
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
                    svg, util::escape_html(&payload)
                ));
            });
        }
        util::append(&qr_details, &qr_content);
        util::append(&qr_section, &qr_details);
    }

    let scan_details = util::create_element("details");
    scan_details.set_attribute("style", "margin-top:8px").ok();
    let scan_summary = util::create_element("summary");
    scan_summary
        .set_attribute("style", "cursor:pointer;font-weight:bold;padding:4px 0")
        .ok();
    util::set_text(&scan_summary, "Scan QR Code");
    util::append(&scan_details, &scan_summary);

    let scan_container = util::create_element("div");
    scan_container.set_attribute("style", "margin-top:8px").ok();

    let scanner_initialized = std::rc::Rc::new(std::cell::RefCell::new(false));
    {
        let container_ref = scan_container.clone();
        let init_ref = scanner_initialized;
        let scan_closures = ctx.closures.clone();
        // A scanned QR populates the connect-address input so the user doesn't
        // have to retype it. Our QR payload is `{ws_addr}|{peer_id}`, so take
        // the address (before the first '|'); a bare address scans through
        // unchanged. The input is found by its `data-field` within this
        // window's wrapper — the same handle the Connect button reads.
        let on_scan: std::rc::Rc<dyn Fn(String)> = {
            let root = parent.clone();
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
    util::append(&qr_section, &scan_details);
    util::append(parent, &qr_section);
}
