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
///
/// The address is the only required one. The peer-id used to sit at the top of
/// this form as a mandatory Base58 string; it is
/// [`ConnectorDraft::expect_peer_id`](crate::connectors::ConnectorDraft), an
/// optional pin, and it lives behind Advanced with the rest of what most people
/// never fill in.
const CONNECTOR_ADDR_FIELD: &str = "connector_addr";
const CONNECTOR_LABEL_FIELD: &str = "connector_label";
const CONNECTOR_EXPECT_FIELD: &str = "connector_expect";
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
    // **Meet before the node list, and that order was asked for twice.** A
    // rendezvous node is plumbing you configure once; meeting somebody is the
    // thing you came here to do. The list used to sit above it, so the first
    // thing between "connect to a device" and "find a person" was a table of
    // infrastructure and a seven-field form.
    render_meet(&wrapper, output, ctx);
    render_connectors(&wrapper, output, ctx);
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
    util::append(parent, &components::copy_code(ctx, &output.bound_peer.peer_id, None));
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

        // **The device's peer id, copyable.** The Device column showed only a
        // friendly name ("system-backend"), so the table listing *the peers you
        // can send a file to* could not answer "which peer is this" or "get it
        // onto the other machine" — on the surface that raises both questions.
        // A peer id is the one string every pairing, meet and offer is keyed
        // on, and it was on screen nowhere you could select it.
        let device_cell = {
            let wrap = util::create_element("div");
            let name = util::create_element("div");
            util::set_text(&name, &kp.display);
            util::append(&wrap, &name);
            util::append(&wrap, &components::copy_code(ctx, &kp.remote_pid, None));
            components::td(&wrap)
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
                device_cell,
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
    let (scan_details, scan_container) =
        components::disclosure(&crate::i18n::t("peers.scan_qr", &[]));

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

    let (qr_details, qr_content) = components::disclosure(&crate::i18n::t("peers.show_qr", &[]));
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
///
/// The two mode helpers below it are shared by the initial render and the
/// change handler — one mapping each, because two copies drift and the drift is
/// invisible until someone picks a mode and reads a sentence about another one.
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
    // **The pick survives a rebuild, the same way typing does.** The selected
    // value was hardcoded to `tag`, so any repaint put the picker back to "A
    // word we agreed on" — and this card repaints constantly, because a running
    // meet marks it dirty on every status change. Choosing Lobby and watching
    // the form quietly revert is the same class of lie as the rest of this
    // arc; drafts is the mechanism `text_input` already uses for exactly it.
    //
    // **`secret` is deliberately not offered here, because it is `tag`.** Both
    // derive the same way from the same input — `derive(mode, input)`, a
    // SHA-256 over a domain string, the mode tag and the bytes you typed — so
    // the node sees 33 opaque bytes either way and *neither* word ever leaves
    // this device. The only difference is which domain the hash lands in, which
    // is invisible to the person choosing. What it is not invisible about is the
    // failure it causes: two people typing the same word under different modes
    // derive different keys, meet nobody, and are told "nobody else was there".
    // The operator read the two options, said they are the same thing, and was
    // right. The Shell keeps `meet secret <phrase>` for anyone talking to a
    // client that uses it; entropy, not the mode, is what makes a name private.
    let offered = [
        ("tag", crate::i18n::t("peerconn.meet_mode_tag", &[])),
        ("lobby", crate::i18n::t("peerconn.meet_mode_lobby", &[])),
    ];
    let chosen_mode = ctx
        .drafts
        .borrow()
        .get(MEET_MODE_FIELD)
        .filter(|m| offered.iter().any(|(v, _)| v == m))
        .cloned()
        .unwrap_or_else(|| "tag".to_string());
    let mode = components::select_el(
        &offered.iter().map(|(v, l)| (*v, l.as_str())).collect::<Vec<_>>(),
        &chosen_mode,
    );
    mode.set_attribute("data-field", MEET_MODE_FIELD).ok();
    util::append(
        &card,
        &components::field(&crate::i18n::t("peerconn.meet_mode", &[]), "", &mode),
    );

    // **What the chosen mode actually does**, updated as the pick changes.
    // Three names in a dropdown are not self-explanatory — "Label (public)" and
    // "Lobby (anyone here)" were reported as indistinguishable, which they are
    // if nothing says that one meets at a word you agree out loud and the other
    // meets at whatever the node itself publishes.
    let mode_note = util::create_element("p");
    mode_note.set_attribute("style", theme::HINT).ok();
    util::set_text(&mode_note, &crate::i18n::t(meet_mode_note_key(&chosen_mode), &[]));
    util::append(&card, &mode_note);

    // The name field, which **Lobby must not show**. Lobby takes no input by
    // construction (its input is the node's own constant), so rendering a box
    // for it invites a value the mode will then refuse — which is exactly what
    // happened: selecting Lobby with a word still in the box answered "lobby
    // takes no input", and the refusal read as the option being broken. You
    // cannot type into a field that is not there, so nothing is silently
    // dropped and `Mode::parse`'s refusal stays as the backstop it is.
    let name = components::text_input(ctx, MEET_INPUT_FIELD, "", "");
    let name_field = components::field(&crate::i18n::t("label.name", &[]), "", &name);
    name_field.set_attribute("style", name_field_style(&chosen_mode)).ok();
    util::append(&card, &name_field);

    {
        // Direct DOM edits on change rather than a repaint: the select is
        // deliberately unwired so that picking a mode cannot rebuild the card
        // and lose what the user has typed. The pick itself is recorded in
        // drafts so it survives the repaints that happen anyway.
        let note = mode_note.clone();
        let field = name_field.clone();
        let sel = mode.clone();
        let drafts = ctx.drafts.clone();
        ctx.listen(&mode, "change", move |_| {
            let chosen = sel
                .clone()
                .dyn_into::<web_sys::HtmlSelectElement>()
                .map(|s| s.value())
                .unwrap_or_else(|_| "tag".to_string());
            util::set_text(&note, &crate::i18n::t(meet_mode_note_key(&chosen), &[]));
            field.set_attribute("style", name_field_style(&chosen)).ok();
            drafts.borrow_mut().insert(MEET_MODE_FIELD.to_string(), chosen);
        });
    }

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
            // Lobby's field is hidden, so whatever is left in its draft is not
            // the user's intent — a word typed before switching modes. Sending
            // it would refuse the press ("lobby takes no input") over a box
            // nobody can see, which is how this read as a broken option.
            let input = if chosen == "lobby" {
                String::new()
            } else {
                drafts.borrow().get(MEET_INPUT_FIELD).cloned().unwrap_or_default()
            };
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

/// **Rendezvous nodes** — the nodes this peer may meet through, and which one
/// is in force.
///
/// Same four operations as the `connector` shell verb (add / use / rm / check),
/// driving the same [`crate::connectors`] functions: one model, two surfaces.
/// The rows are a table (S7 — repeated records), the whole thing one bounded
/// card (S2), and the actions are the shared button atoms (S8).
///
/// # "Connector" is the code's word, not the screen's
///
/// The module, the shell verb, the tree path and the entity type all still say
/// *connector*, and renaming those is a data migration for no gain. What the
/// operator sees is *rendezvous node*, which is the same word System Overview
/// already uses for the switch on the other side of the same relationship: a
/// desktop **offers** rendezvous, a browser **picks** one. "Connector" named
/// neither end of that and was reported, correctly, as meaning nothing.
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
            use crate::views::peer_connections::output::ConnectorSource;
            let from_link = c.source == ConnectorSource::Session;

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

            // A session row's "label" is where it came from, because it has no
            // label — nobody named it, and leaving the cell blank next to a row
            // marked *in use* invites the question this answers.
            let label_cell = if from_link {
                components::td_text(&crate::i18n::t("peerconn.connector_from_link", &[]))
            } else {
                components::td_text(&c.label)
            };

            let actions = util::create_element("div");
            actions.set_attribute("style", theme::BTN_ROW).ok();
            // Use / Delete are **omitted on a session row**, not disabled:
            // there is no stored row to reselect or remove, so both would be
            // visible no-ops — the dead-button disease this window has now
            // fixed twice. `Check` stays, because asking a node what it serves
            // is a question about the node and not about the row.
            if !c.selected && !from_link {
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

            if !from_link {
                let rm_btn = components::button_el(
                    &crate::i18n::t("btn.delete", &[]),
                    components::ButtonKind::Destructive,
                );
                ctx.on_window_event(&rm_btn, "click", "connector_rm", &c.node_peer_id);
                util::append(&actions, &rm_btn);
            }

            util::append(
                &tbody,
                &components::tr(vec![
                    id_cell,
                    components::td_text(&c.node_addr),
                    label_cell,
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
        // **The notice asks for a reload, so it hands you one.** Everything else
        // in this card acts on the press; this one told the user to go and do
        // something to the browser itself, which is the only instruction in the
        // window that the window would not carry out. It also removes the one
        // way this state is usually left unresolved — the person reads the line,
        // means to reload, and keeps working on a session that is still
        // rendezvousing somewhere else.
        let reload = components::button_el(
            &crate::i18n::t("btn.reload", &[]),
            components::ButtonKind::Secondary,
        );
        ctx.listen(&reload, "click", move |_| {
            // No `let _ = promise` here to worry about: `reload()` returns a
            // Result, not a Promise, so there is no rejecting future to drop
            // (the `unhandledrejection` guard would reload the app anyway,
            // which would be a comedy rather than a bug).
            if let Some(w) = web_sys::window() {
                let _ = w.location().reload();
            }
        });
        util::append(&card, &reload);
    }

    if let Some(notice) = &output.connector_notice {
        let el = if notice.is_error {
            components::error(&notice.text)
        } else {
            components::success(&notice.text)
        };
        util::append(&card, &el);
    }

    render_add_node_form(&card, output, ctx);
    util::append(parent, &card);
}

/// The add-a-node form: **two visible fields, and everything else folded away.**
///
/// # What this used to ask for
///
/// Seven fields, all at once, the first of them a mandatory Base58 peer-id. Six
/// of the seven are things a person adding their friend's desktop has no
/// business thinking about, and the mandatory one was the worst: it had to be
/// read off another machine's screen and retyped, which is the transcription
/// error that presents as a *connectivity* failure — and it was being demanded
/// for a value the node hands over the moment we dial it.
///
/// So: **Address**, and a **Label** if you want one. The peer-id moved behind
/// Advanced and changed meaning (see
/// [`ConnectorDraft`](crate::connectors::ConnectorDraft)) — it is now an
/// expectation checked against the dial, which is a thing worth having and a
/// thing the old required field never did.
///
/// Advanced is a `<details>`, not a model-held
/// [`collapsible_header`](components::collapsible_header), and that is the
/// right call here for the reason the atom's own doc gives: this section is
/// closed by default and nobody types into it mid-repaint. The fields inside
/// are draft-tracked all the same, so anything typed there survives a rebuild
/// whether the section is open or not.
fn render_add_node_form(card: &Element, output: &PeerConnectionsOutput, ctx: &DomCtx) {
    // Draft-tracked atoms (S8): typing survives an unrelated repaint, and the
    // values are read at SUBMIT time, so no per-keystroke event churns the tree.
    let addr_input =
        components::text_input(ctx, CONNECTOR_ADDR_FIELD, "", "wss://node.example:9000");
    util::append(
        card,
        &components::field(
            &crate::i18n::t("label.address", &[]),
            &crate::i18n::t("peerconn.node_addr_help", &[]),
            &addr_input,
        ),
    );
    let label_input = components::text_input(ctx, CONNECTOR_LABEL_FIELD, "", "");
    util::append(
        card,
        &components::field(&crate::i18n::t("label.label", &[]), "", &label_input),
    );

    let (advanced, body) = components::disclosure(&crate::i18n::t("peerconn.node_advanced", &[]));
    // The expectation. Optional, and the help says exactly what filling it in
    // buys — "pin this node" is a real thing to want and an empty field is not
    // a degraded version of it.
    let expect_input = components::text_input(ctx, CONNECTOR_EXPECT_FIELD, "", "2K…");
    util::append(
        &body,
        &components::field(
            &crate::i18n::t("label.expected_peer_id", &[]),
            &crate::i18n::t("peerconn.expect_help", &[]),
            &expect_input,
        ),
    );
    // Reflectors. Optional, and the help text says what leaving it empty means —
    // "host candidates only" is a real deployment (a LAN), not a broken one, and
    // the user should not have to guess which they are running.
    let ice_input =
        components::text_input(ctx, CONNECTOR_ICE_FIELD, "", "stun:stun.example.org:3478");
    util::append(
        &body,
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
        &body,
        &components::field(
            &crate::i18n::t("label.relay", &[]),
            &crate::i18n::t("peerconn.relay_help", &[]),
            &relay_input,
        ),
    );
    let relay_user_input = components::text_input(ctx, CONNECTOR_RELAY_USER_FIELD, "", "");
    util::append(
        &body,
        &components::field(&crate::i18n::t("label.relay_username", &[]), "", &relay_user_input),
    );
    let relay_cred_input = components::text_input(ctx, CONNECTOR_RELAY_CRED_FIELD, "", "");
    util::append(
        &body,
        &components::field(
            &crate::i18n::t("label.relay_credential", &[]),
            &crate::i18n::t("peerconn.relay_secret_help", &[]),
            &relay_cred_input,
        ),
    );
    util::append(card, &advanced);

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
            // **Address first, because it is the one required field now.** The
            // packing order follows the form; the handler splits on the same
            // count. An empty submit still dispatches — the refusal comes back
            // as a notice, because a press that does nothing at all is the
            // dead-button disease.
            actions.borrow_mut().push(Action::WindowEvent {
                window_id: wid,
                event: "connector_add".to_string(),
                value: [
                    CONNECTOR_ADDR_FIELD,
                    CONNECTOR_LABEL_FIELD,
                    CONNECTOR_EXPECT_FIELD,
                    CONNECTOR_ICE_FIELD,
                    CONNECTOR_RELAY_FIELD,
                    CONNECTOR_RELAY_USER_FIELD,
                    CONNECTOR_RELAY_CRED_FIELD,
                ]
                .map(read)
                .join("\u{1f}"),
            });
            rp();
        });
    }
    util::append(card, &add_btn);
}

/// The catalog key explaining what a meet mode does.
///
/// `secret` has no note of its own any more: it is not offered in the picker
/// (see `render_meet`), and the note it used to carry claimed it was *"not shown
/// to the rendezvous"* — which was false about the distinction it was drawing,
/// because a `tag` is not shown to the rendezvous either. Both are hashed
/// locally. A shell-started `secret` meet falls through to the `tag` note, which
/// is true of it.
fn meet_mode_note_key(mode: &str) -> &'static str {
    match mode {
        "lobby" => "peerconn.meet_mode_note_lobby",
        _ => "peerconn.meet_mode_note_tag",
    }
}

/// Whether the Name field is shown for `mode`. **Lobby takes no input by
/// construction** (its input is the node's own published constant), so a box
/// for it invites a value the mode will then refuse.
fn name_field_style(mode: &str) -> &'static str {
    if mode == "lobby" {
        "display:none"
    } else {
        ""
    }
}
