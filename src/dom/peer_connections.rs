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

/// Drafts keys for the add-a-connector form.
const CONNECTOR_ID_FIELD: &str = "connector_id";
const CONNECTOR_ADDR_FIELD: &str = "connector_addr";
const CONNECTOR_LABEL_FIELD: &str = "connector_label";
const CONNECTOR_ICE_FIELD: &str = "connector_ice";
/// Relay (TURN) URI list + its credentials. Three fields rather than one,
/// because all three must be present together — `parse_relay` refuses a partial
/// set, and naming which half is missing is only possible if they are separate.
const CONNECTOR_RELAY_FIELD: &str = "connector_relay";
const CONNECTOR_RELAY_USER_FIELD: &str = "connector_relay_user";
const CONNECTOR_RELAY_CRED_FIELD: &str = "connector_relay_cred";

/// Drafts keys for the meet-at-a-name form.
const MEET_MODE_FIELD: &str = "meet_mode";
const MEET_INPUT_FIELD: &str = "meet_input";

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
    render_connectors(&wrapper, output, ctx);
    render_meet(&wrapper, output, ctx);
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

/// The localized word for a running meet's mode, plus its input when there is
/// one to show. The mode arrives as the protocol's English tag (`tag` /
/// `secret` / `lobby`) precisely so this can be a locale's own word instead.
fn meet_mode_label(status: &crate::views::peer_connections::output::MeetStatusRow) -> String {
    let key = match status.mode_name.as_str() {
        "secret" => "peerconn.meet_mode_secret",
        "lobby" => "peerconn.meet_mode_lobby",
        _ => "peerconn.meet_mode_tag",
    };
    let word = crate::i18n::t(key, &[]);
    if status.mode_input.is_empty() {
        word
    } else {
        format!("{word} \u{201c}{}\u{201d}", status.mode_input)
    }
}

/// **Meet at a name** — the `lobby` / `tag` / `secret` modes
/// (`crate::rendezvous`), the localized surface over the `meet` shell verb.
///
/// One bounded card (S2), the results a table (S7), shared atoms (S8). The
/// section reports its own state throughout — searching, the reason it stopped,
/// and an honest "nobody was there" — because a search whose state you cannot
/// see is a spinner, and a meet writes nothing to the tree that would otherwise
/// show for it.
fn render_meet(parent: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    let card = components::card(&crate::i18n::t("peerconn.meet", &[]));

    // What a meet is and what it is not, said where the user types a name:
    // rendezvous introduces and never authorizes, a tag is public by design, and
    // a memorable "secret" is a tag in disguise. That is not decoration — it is
    // the difference between a discovery convenience and a gate someone leans on.
    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &crate::i18n::t("peerconn.meet_hint", &[]));
    util::append(&card, &hint);

    if !output.meet.has_connector {
        util::append(
            &card,
            &components::empty(&crate::i18n::t("peerconn.meet_needs_connector", &[])),
        );
        util::append(parent, &card);
        return;
    }

    // --- the form ---------------------------------------------------------
    // Unwired select + draft-tracked input: both are read at Meet-click time, so
    // a change can't trigger a rebuild that resets the pick or the typing.
    let mode = components::select_el(
        &[
            ("tag", &crate::i18n::t("peerconn.meet_mode_tag", &[])),
            ("secret", &crate::i18n::t("peerconn.meet_mode_secret", &[])),
            ("lobby", &crate::i18n::t("peerconn.meet_mode_lobby", &[])),
        ],
        "tag",
    );
    mode.set_attribute("data-field", MEET_MODE_FIELD).ok();
    util::append(
        &card,
        &components::field(&crate::i18n::t("peerconn.meet_mode", &[]), "", &mode),
    );

    let name = components::text_input(ctx, MEET_INPUT_FIELD, "", "");
    util::append(
        &card,
        &components::field(&crate::i18n::t("label.name", &[]), "", &name),
    );

    let actions = util::create_element("div");
    actions.set_attribute("style", theme::BTN_ROW).ok();
    let go = components::button_el(
        &crate::i18n::t("peerconn.meet_start", &[]),
        components::ButtonKind::Primary,
    );
    {
        let queue = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let drafts = ctx.drafts.clone();
        let wid = output.window_id;
        let mode_ref = mode.clone();
        ctx.listen(&go, "click", move |_| {
            let chosen = mode_ref
                .clone()
                .dyn_into::<web_sys::HtmlSelectElement>()
                .map(|s| s.value())
                .unwrap_or_else(|_| "tag".to_string());
            let input = drafts.borrow().get(MEET_INPUT_FIELD).cloned().unwrap_or_default();
            // Dispatch even when the input is empty: `Mode::parse` reports what
            // is wrong (a tag with nothing to meet at, a lobby with a stray
            // input) and the notice shows it — a silently ignored press is the
            // dead-button disease.
            queue.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: "meet_start".to_string(),
                value: format!("{chosen}\u{1f}{input}"),
            });
            rp();
        });
    }
    util::append(&actions, &go);

    if output.meet.status.as_ref().is_some_and(|s| s.searching) {
        let stop = components::button_el(
            &crate::i18n::t("peerconn.meet_stop", &[]),
            components::ButtonKind::Secondary,
        );
        ctx.on_window_event(&stop, "click", "meet_stop", "");
        util::append(&actions, &stop);
    }
    util::append(&card, &actions);

    // A refused press reports here, next to the button that caused it.
    if let Some(notice) = &output.meet.notice {
        let el = if notice.is_error {
            components::error(&notice.text)
        } else {
            components::success(&notice.text)
        };
        util::append(&card, &el);
    }

    // --- what the search is doing ----------------------------------------
    let Some(status) = &output.meet.status else {
        util::append(parent, &card);
        return;
    };

    if let Some(error) = &status.error {
        util::append(&card, &components::error(error));
    } else if status.searching {
        let line = util::create_element("p");
        line.set_attribute("style", theme::HINT).ok();
        util::set_text(
            &line,
            &crate::i18n::t(
                "peerconn.meet_searching",
                &[
                    ("mode", &meet_mode_label(status)),
                    ("node", &output.meet.node_short),
                    ("polls", &status.polls.to_string()),
                    ("max", &status.max_polls.to_string()),
                ],
            ),
        );
        util::append(&card, &line);
    }

    if status.found.is_empty() {
        if !status.searching && status.error.is_none() {
            util::append(&card, &components::empty(&crate::i18n::t("peerconn.meet_none", &[])));
        }
        util::append(parent, &card);
        return;
    }

    let (table, tbody) = components::table(&[
        &crate::i18n::t("label.peer_id", &[]),
        "",
        "",
    ]);
    for found in &status.found {
        // The id in full beside the short form: this is the value the user
        // hands to `connect` or a Chat, and a truncated id is not one.
        let id_cell = components::td_text(&format!("{}  {}", found.short_pid, found.peer_id));
        let claim = if found.verified {
            components::td_text("")
        } else {
            // Not a warning banner: an unverified claim is the *normal* case for
            // the bare framing, and it is checked at the handshake. Saying which
            // is which is the honest amount.
            components::td_text(&crate::i18n::t("peerconn.meet_unverified", &[]))
        };
        let action = if found.remembered {
            components::td_text(&crate::i18n::t("peerconn.meet_remembered", &[]))
        } else {
            let btn = components::button_el(
                &crate::i18n::t("peerconn.meet_remember", &[]),
                components::ButtonKind::Secondary,
            );
            ctx.on_window_event(&btn, "click", "meet_remember", &found.peer_id);
            components::td(&btn)
        };
        util::append(&tbody, &components::tr(vec![id_cell, claim, action]));
    }
    util::append(&card, &table);

    util::append(parent, &card);
}

/// The **connector registry** — signaling nodes this peer may rendezvous
/// through, and which one provisioning uses.
///
/// Same four operations as the `connector` shell verb (add / use / rm / check),
/// driving the same [`crate::connectors`] functions: one model, two surfaces.
/// The rows are a table (S7 — repeated records), the whole thing one bounded
/// card (S2), and the actions are the shared button atoms (S8).
fn render_connectors(parent: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    let card = components::card(&crate::i18n::t("peerconn.connectors", &[]));

    // What a connector *is*, said where the user picks one: it introduces peers
    // and then gets out of the way. That property is what makes running a
    // community node safe to offer, so it belongs next to the choice rather
    // than in a doc nobody opens.
    let hint = util::create_element("p");
    // `theme::HINT`, not an inline style string — tokens are the rule and
    // ui-lint ratchets the inline count DOWN, never up.
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &crate::i18n::t("peerconn.connectors_hint", &[]));
    util::append(&card, &hint);

    if output.connectors.is_empty() {
        util::append(&card, &components::empty(&crate::i18n::t("peerconn.connector_none", &[])));
    } else {
        let (table, tbody) = components::table(&[
            &crate::i18n::t("label.peer_id", &[]),
            &crate::i18n::t("label.address", &[]),
            &crate::i18n::t("label.label", &[]),
            "",
        ]);
        for c in &output.connectors {
            // `●` marks the selection — the same glyph `connector ls` prints,
            // so the two surfaces read identically. The text label rides
            // alongside it rather than relying on the glyph alone.
            let id_cell = if c.selected {
                components::td_text(&format!(
                    "\u{25cf} {}  ({})",
                    c.short_pid,
                    crate::i18n::t("peerconn.connector_in_use", &[])
                ))
            } else {
                components::td_text(&c.short_pid)
            };

            let actions = util::create_element("div");
            actions.set_attribute("style", theme::BTN_ROW).ok();
            if !c.selected {
                let use_btn = components::button_el(
                    &crate::i18n::t("peerconn.connector_use", &[]),
                    components::ButtonKind::Primary,
                );
                ctx.on_window_event(&use_btn, "click", "connector_use", &c.node_peer_id);
                util::append(&actions, &use_btn);
            }
            // `Check` asks the node what it actually serves — the lobby constant
            // especially, which a peer must not assume (a node may override it,
            // and deriving from the default then meets nobody, silently).
            let check_btn = components::button_el(
                &crate::i18n::t("peerconn.connector_check", &[]),
                components::ButtonKind::Secondary,
            );
            ctx.on_window_event(&check_btn, "click", "connector_check", &c.node_peer_id);
            util::append(&actions, &check_btn);

            let rm_btn = components::button_el(
                &crate::i18n::t("btn.delete", &[]),
                components::ButtonKind::Destructive,
            );
            ctx.on_window_event(&rm_btn, "click", "connector_rm", &c.node_peer_id);
            util::append(&actions, &rm_btn);

            util::append(
                &tbody,
                &components::tr(vec![
                    id_cell,
                    components::td_text(&c.node_addr),
                    components::td_text(&c.label),
                    components::td(&actions),
                ]),
            );
        }
        util::append(&card, &table);
    }

    // The outcome of the last Check (or a refused Add/Use). Without it, asking a
    // node what it serves and showing nothing is the dead-button disease Phase
    // 14.5 exists to catch.
    // The selection is durable the moment it is clicked, but the establisher
    // reads it only at Init (`InitParams.webrtc` is Init-only upstream), so the
    // choice is real and not yet in effect. Saying nothing here is what made
    // "I selected it and meet still fails" the expected first experience.
    if output.connector_reload_pending {
        util::append(
            &card,
            &components::notice(&crate::i18n::t("peerconn.connector_reload_pending", &[])),
        );
    }

    if let Some(notice) = &output.connector_notice {
        let el = if notice.is_error {
            components::error(&notice.text)
        } else {
            components::success(&notice.text)
        };
        util::append(&card, &el);
    }

    // --- add a connector -------------------------------------------------
    // Draft-tracked atoms (S8): typing survives an unrelated repaint, and the
    // values are read at SUBMIT time, so no per-keystroke event churns the tree.
    let id_input =
        components::text_input(ctx, CONNECTOR_ID_FIELD, "", "2K…");
    util::append(
        &card,
        &components::field(&crate::i18n::t("label.peer_id", &[]), "", &id_input),
    );
    let addr_input =
        components::text_input(ctx, CONNECTOR_ADDR_FIELD, "", "wss://node.example:9000");
    util::append(
        &card,
        &components::field(&crate::i18n::t("label.address", &[]), "", &addr_input),
    );
    let label_input = components::text_input(ctx, CONNECTOR_LABEL_FIELD, "", "");
    util::append(
        &card,
        &components::field(&crate::i18n::t("label.label", &[]), "", &label_input),
    );
    // Reflectors. Optional, and the help text says what leaving it empty means —
    // "host candidates only" is a real deployment (a LAN), not a broken one, and
    // the user should not have to guess which they are running.
    let ice_input =
        components::text_input(ctx, CONNECTOR_ICE_FIELD, "", "stun:stun.example.org:3478");
    util::append(
        &card,
        &components::field(
            &crate::i18n::t("label.ice_servers", &[]),
            &crate::i18n::t("peerconn.ice_help", &[]),
            &ice_input,
        ),
    );

    // Relay. Optional, and separate from the reflectors above because the two
    // are different kinds of thing: a reflector is a commodity that takes no
    // credentials (§9.3) and that a node may advertise for you; a relay is
    // rented, credentialed, and can never be advertised (§3b has no credential
    // channel). The help text says when a person needs one — which is exactly
    // what the reachability diagnosis now tells them.
    let relay_input =
        components::text_input(ctx, CONNECTOR_RELAY_FIELD, "", "turn:relay.example.org:3478");
    util::append(
        &card,
        &components::field(
            &crate::i18n::t("label.relay", &[]),
            &crate::i18n::t("peerconn.relay_help", &[]),
            &relay_input,
        ),
    );
    let relay_user_input = components::text_input(ctx, CONNECTOR_RELAY_USER_FIELD, "", "");
    util::append(
        &card,
        &components::field(&crate::i18n::t("label.relay_username", &[]), "", &relay_user_input),
    );
    let relay_cred_input = components::text_input(ctx, CONNECTOR_RELAY_CRED_FIELD, "", "");
    util::append(
        &card,
        &components::field(
            &crate::i18n::t("label.relay_credential", &[]),
            &crate::i18n::t("peerconn.relay_secret_help", &[]),
            &relay_cred_input,
        ),
    );

    let add_btn = components::button_el(
        &crate::i18n::t("peerconn.connector_add", &[]),
        components::ButtonKind::Primary,
    );
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let drafts = ctx.drafts.clone();
        let wid = output.window_id;
        ctx.listen(&add_btn, "click", move |_| {
            let read = |k: &str| drafts.borrow().get(k).cloned().unwrap_or_default();
            let (id, addr, label, ice) = (
                read(CONNECTOR_ID_FIELD),
                read(CONNECTOR_ADDR_FIELD),
                read(CONNECTOR_LABEL_FIELD),
                read(CONNECTOR_ICE_FIELD),
            );
            let (relay, ruser, rcred) = (
                read(CONNECTOR_RELAY_FIELD),
                read(CONNECTOR_RELAY_USER_FIELD),
                read(CONNECTOR_RELAY_CRED_FIELD),
            );
            // Both halves are required, and the model says so with a notice —
            // submitting the empty form must not look like a dead button, so we
            // dispatch and let `add_connector` report the refusal.
            actions.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: "connector_add".to_string(),
                // The app's multi-field packing, so one event carries the form.
                value: format!(
                    "{id}\u{1f}{addr}\u{1f}{label}\u{1f}{ice}\u{1f}{relay}\u{1f}{ruser}\u{1f}{rcred}"
                ),
            });
            rp();
        });
    }
    util::append(&card, &add_btn);

    util::append(parent, &card);
}
