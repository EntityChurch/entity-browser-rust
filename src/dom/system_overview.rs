//! System Overview DOM renderer — pure consumer of
//! [`SystemOverviewOutput`](crate::views::system_overview::output::SystemOverviewOutput).

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::components::{self, AuthState, ConnState};
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::system_overview::output::{
    AppServerView, AuthorizationsView, AuthRow, BackendStatusView, PairScope, SystemOverviewOutput,
};
use crate::views::system_peers::output::SystemPeersOutput;

use web_sys::Element;

/// Render the merged System Overview window: the system peer(s) + posture
/// (`overview`) up top, then the native peer's live detail (`output` — status,
/// device authorizations, logs, share). One window, one job (S2): the old
/// standalone "System Peer (Native)" window and its drill-in are retired.
pub fn render(
    container: &Element,
    output: &SystemOverviewOutput,
    overview: &SystemPeersOutput,
    ctx: &DomCtx,
) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "system-backend");
    wrapper.set_attribute("style", theme::SECTION).ok();

    // --- Header (title + log controls) ---
    let header = util::create_element("div");
    header.set_attribute("style", theme::HEADER_ROW).ok();
    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::TITLE_INLINE).ok();
    util::set_text(&h2, &crate::i18n::t("window.system_overview", &[]));
    util::append(&header, &h2);
    if output.tauri {
        // Right-aligned controls group: level selector + Clear.
        let controls = util::create_element("div");
        controls
            .set_attribute("style", "display:flex;gap:8px;align-items:center")
            .ok();

        let lvl_label = util::create_element("span");
        lvl_label
            .set_attribute("style", "color:var(--text-dim, #888);font-size:12px")
            .ok();
        util::set_text(&lvl_label, &crate::i18n::t("label.level", &[]));
        util::append(&controls, &lvl_label);

        let levels: Vec<(&str, &str)> = crate::views::system_overview::model::LEVELS
            .iter()
            .map(|l| (*l, *l))
            .collect();
        let select = components::select(ctx, &levels, &output.log_level, "sb_set_level");
        util::append(&controls, &select);

        let clear_btn =
            components::button(
                ctx,
                &crate::i18n::t("sysoverview.clear_logs", &[]),
                components::ButtonKind::Small,
                "sb_clear_logs",
            );
        util::append(&controls, &clear_btn);

        util::append(&header, &controls);
    }
    util::append(&wrapper, &header);

    // System peers + posture (always — this is the browser view too). The
    // native peer's identity card here is terse; its live detail follows below.
    crate::dom::system_peers::render_system_peers(&wrapper, overview);

    if !output.tauri {
        // Not a dead end — the backend simply isn't reachable from a plain
        // browser session today (its logs come over a desktop-local IPC).
        // Exposing them to an authorized remote peer is a planned direction.
        let note = util::create_element("p");
        note.set_attribute("style", "color:var(--text-dim, #888);margin:4px 0")
            .ok();
        util::set_text(&note, &crate::i18n::t("sysoverview.backend_logs_note", &[]));
        util::append(&wrapper, &note);
        util::append(container, &wrapper);
        return;
    }

    // --- Three cards, each answering ONE question ---
    //
    // This was one "Status" card carrying identity facts, three service
    // switches and the pairing lines as identical `label: value` rows. Every
    // row was individually correct and nothing told you which KIND of row you
    // were reading, so finding "the address to type on the laptop" meant
    // scanning eleven rows and knowing which two to combine. The split is the
    // whole fix:
    //
    //   A. This device                     — what am I, and where do my files go
    //   B. What other devices can do here  — the switches
    //   C. Connect another device          — the strings you carry elsewhere
    //
    // The order is causal: B changes what C can offer, and C renders nothing at
    // all until B is on. Do not fold them back together, and do not add a row
    // to one that answers another's question — that is how this became a pile
    // the first time.
    match &output.backend {
        Some(b) => {
            render_device_card(&wrapper, output, b);
            render_services_card(&wrapper, output, b, ctx);
            render_connect_card(&wrapper, output, b, ctx);
        }
        None => {
            // Non-content states (S5): still loading vs genuinely absent. One
            // card, because with no backend there are no services and nothing
            // to connect to — three empty cards would be three copies of the
            // same absence.
            let status = components::card(&crate::i18n::t("sysoverview.card_device", &[]));
            if output.fetched {
                util::append(&status, &components::empty(&crate::i18n::t("sysoverview.no_backend", &[])));
            } else {
                util::append(&status, &components::loading(&crate::i18n::t("sysoverview.loading_status", &[])));
            }
            util::append(&wrapper, &status);
        }
    }

    // --- Device authorizations card (inbound; Direction A home) ---
    render_authorizations(&wrapper, output, ctx);

    // Inbound access ("who reached into B's share") lives in the unified Access
    // Log window now (fed by the same backend poll, tagged ← Inbound) — one place
    // to review access, not a second copy here.

    // --- Live log card ---
    let logs = components::card(&crate::i18n::t("sysoverview.logs", &[]));
    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();
    if output.log_lines.is_empty() {
        pre.set_inner_html(&format!(
            "<span style='color:var(--text-dim, #888)'>{}</span>",
            util::escape_html(&crate::i18n::t("sysoverview.no_logs", &[]))
        ));
    } else {
        // The backend keeps ANSI color on its `tracing` output; convert the
        // escapes to colored HTML so the logs render with their level/field
        // coloring (crate::ansi HTML-escapes the text, so this is injection-safe).
        let mut html = String::new();
        for line in &output.log_lines {
            html.push_str(&crate::ansi::ansi_to_html(line));
            html.push('\n');
        }
        pre.set_inner_html(&html);
    }
    util::append(&logs, &pre);
    util::append(&wrapper, &logs);
    // Keep the newest lines in view (a live tail scrolls to the bottom).
    util::schedule_scroll_to_bottom(&pre);

    util::append(container, &wrapper);
}

/// **Card A — This device.** Identity and local facts only: what this machine
/// is, whether it is up, what it is listening on, where its shared files live,
/// and whether the browser session is linked to it.
///
/// Nothing here is a control and nothing here is a string to carry to another
/// machine. If a row you are adding is either of those, it belongs in card B or
/// card C.
fn render_device_card(parent: &Element, output: &SystemOverviewOutput, b: &BackendStatusView) {
    let card = components::card(&crate::i18n::t("sysoverview.card_device", &[]));
    add_row(
        &card,
        &crate::i18n::t("sysoverview.native_peer", &[]),
        &b.short_id,
        Some(&b.peer_id),
        None,
    );
    add_row(
        &card,
        &crate::i18n::t("label.status", &[]),
        &b.status,
        None,
        Some(status_color_for(&b.status)),
    );
    add_row(
        &card,
        &crate::i18n::t("sysoverview.listen", &[]),
        &b.ws_addr.clone().unwrap_or_else(|| crate::i18n::t("sysoverview.not_listening", &[])),
        None,
        None,
    );
    // Link status via the shared connection vocabulary (S4). Honest liveness:
    // "Connected" only when the auth read actually succeeded over the transport
    // (the real probe) — a remembered-but-stale link (read failing) shows
    // Connecting, not a false Connected.
    add_chip_row(
        &card,
        &crate::i18n::t("sysoverview.link", &[]),
        components::conn_chip(link_state(output)),
    );
    // Share: the tree prefix a paired device browses, plus the real on-disk
    // directory (answers "where do shared files actually live").
    add_row(
        &card,
        &crate::i18n::t("sysoverview.share_tree", &[]),
        &output.share_prefix,
        Some(&crate::i18n::t("sysoverview.share_tree_hint", &[])),
        None,
    );
    if let Some(path) = &output.share_path {
        add_row(&card, &crate::i18n::t("sysoverview.share_disk", &[]), path, Some(path), None);
    }
    util::append(parent, &card);
}

/// **Card B — What other devices can do here.** The three switches, and nothing
/// else.
///
/// Each row is `what it is: what that means for other devices`, with a button
/// that says only which way it goes. The address a person types is deliberately
/// **not** here — it is card C's job — because mixing "what this switch does"
/// with "the string to carry" is what made every one of these rows read as the
/// same undifferentiated fact.
fn render_services_card(
    parent: &Element,
    output: &SystemOverviewOutput,
    b: &BackendStatusView,
    ctx: &DomCtx,
) {
    let card = components::card(&crate::i18n::t("sysoverview.card_services", &[]));
    // Rendezvous: whether browsers on this LAN can meet through this desktop.
    // Two browsers need no STUN and no TURN on one LAN (`e2e-webrtc-lan`) — but
    // they do need somewhere to exchange the offer, and this is the only thing
    // that can be it without anyone running server infrastructure.
    render_rendezvous_row(&card, b, ctx);
    // Directly under the rendezvous, because the unprovisioned state of this
    // row is fixed by the row above it and its own string says so.
    render_app_server_row(&card, b, &output.app_server, ctx);
    render_port_mapping_row(&card, b, ctx);
    util::append(parent, &card);
}

/// **Card C — Connect another device.** The strings a person carries to another
/// machine, best first.
///
/// Rendered only when there is something to carry. An empty version of this
/// card is a question with no answer, and the remedy — the switches — is
/// already on screen directly above it.
///
/// The composition (which steps, in what order, and whether the URL claims to
/// be the finished flow) lives in
/// [`connect_steps`](crate::views::system_overview::output::connect_steps),
/// where native tests can read it. What would be wrong here is the ORDER, and
/// that is not observable from inside `create_element` calls.
fn render_connect_card(
    parent: &Element,
    output: &SystemOverviewOutput,
    b: &BackendStatusView,
    ctx: &DomCtx,
) {
    use crate::views::system_overview::output::{connect_steps, ConnectStep};

    let steps = connect_steps(b, &output.app_server);
    if steps.is_empty() {
        return;
    }
    let card = components::card(&crate::i18n::t("sysoverview.card_connect", &[]));
    for step in steps {
        match step {
            // The URL gets a copy button for the pairing line's reason: it is
            // going to be typed on a *different* device, and every character
            // retyped by hand is a chance to produce a failure that reads as
            // connectivity.
            ConnectStep::OpenUrl { url, provisioned } => {
                let hint = if provisioned {
                    crate::i18n::t("sysoverview.connect_url_hint", &[])
                } else {
                    crate::i18n::t("sysoverview.connect_url_hint_unprovisioned", &[])
                };
                add_chip_row(
                    &card,
                    &crate::i18n::t("sysoverview.connect_url", &[]),
                    components::copy_code(ctx, &url, Some(&hint)),
                );
            }
            ConnectStep::PasteCommand { scope, command } => {
                let label = match scope {
                    PairScope::Lan => crate::i18n::t("sysoverview.pair_lan", &[]),
                    PairScope::Internet => crate::i18n::t("sysoverview.pair_wan", &[]),
                };
                add_command_row(&card, &label, &command, ctx);
            }
        }
    }
    util::append(parent, &card);
}

/// Build a compact `label: value` row. `title` sets a hover tooltip (e.g. the
/// full peer id when the value is truncated); `color` overrides the value color.
fn add_row(parent: &Element, label: &str, value: &str, title: Option<&str>, color: Option<&str>) {
    let row = util::create_element("div");
    row.set_attribute("style", "display:flex;gap:8px;margin:2px 0;font-size:13px")
        .ok();

    let key = util::create_element("span");
    key.set_attribute(
        "style",
        "color:var(--text-dim, #888);min-width:88px;flex:0 0 auto",
    )
    .ok();
    util::set_text(&key, label);
    util::append(&row, &key);

    let val = util::create_element("span");
    let style = match color {
        Some(c) => format!("color:{c};word-break:break-all"),
        None => "color:var(--text, #ddd);word-break:break-all".to_string(),
    };
    val.set_attribute("style", &style).ok();
    if let Some(t) = title {
        val.set_attribute("title", t).ok();
    }
    util::set_text(&val, value);
    util::append(&row, &val);

    util::append(parent, &row);
}

/// The **Rendezvous** row: is this desktop acting as a §6.5 signaling node, and
/// a button to change that.
///
/// # Why this row exists at all
///
/// A browser peer has no listener, so two browsers cannot exchange a WebRTC
/// offer without a third party holding it — even on the same Wi-Fi, where
/// `e2e-webrtc-lan` proved the media itself needs neither STUN nor TURN. This
/// desktop already binds a WebSocket a browser can reach, so it can be that
/// third party, and then nobody has to run server infrastructure.
///
/// # What the strings must and must not claim
///
/// When it is **on** the row shows the address a browser types into
/// `connector add`, because a rendezvous nobody can address is not usable and
/// "it's on" is not an actionable answer. When it is **off** the row says so
/// plainly rather than hiding — a person looking for "why can't my two browsers
/// meet" needs to find this, and an absent row answers nothing.
///
/// The button warns that flipping restarts the backend, because it does
/// (`system/signaling` mounts on `PeerBuilder`; there is no live add). That is
/// cheap — a node holds nothing durable (§1.3) — but it drops in-flight
/// connections, and a control that silently hangs up deserves to say so.
fn render_rendezvous_row(parent: &Element, b: &BackendStatusView, ctx: &DomCtx) {
    // Built from the card's own two row helpers rather than hand-rolled markup,
    // so this surface adds no raw styles to the `ui-lint` baseline — the state
    // reads as a normal `label: value` row and the control sits under it in the
    // value column.
    let (text, color) = if b.signaling_node {
        // The address is the actionable half: "it is on" is not something a
        // user can do anything with, and this is the string the other browser
        // types into `connector add`.
        let addr = b.ws_addr.clone().unwrap_or_default();
        (
            crate::i18n::t("sysoverview.rendezvous_on", &[("addr", addr.as_str())]),
            crate::theme_tokens::STATUS_OK,
        )
    } else {
        (
            crate::i18n::t("sysoverview.rendezvous_off", &[]),
            "var(--text-muted, #c0c0c0)",
        )
    };
    add_row(
        parent,
        &crate::i18n::t("sysoverview.rendezvous", &[]),
        &text,
        Some(&crate::i18n::t("sysoverview.rendezvous_hint", &[])),
        Some(color),
    );

    // `\x1f`-packed: WHICH backend and WHICH direction. The row is the only
    // place that knows both, and the handler must not have to re-derive the
    // current state to work out what the click meant — a toggle that reads
    // "the opposite of whatever I last painted" flips the wrong way whenever a
    // poll lands between the render and the click.
    let want = if b.signaling_node { "0" } else { "1" };
    let label = if b.signaling_node {
        crate::i18n::t("label.turn_off", &[])
    } else {
        crate::i18n::t("label.turn_on", &[])
    };
    let btn = components::button_value(
        ctx,
        &label,
        components::ButtonKind::Small,
        "sb_set_signaling_node",
        &format!("{}\u{1f}{}", b.peer_id, want),
    );
    // Empty label: the control belongs to the row above, aligned under its
    // value rather than introducing a second key nobody needs to read.
    add_chip_row(parent, "", btn);
}

/// The **serve-the-app** row: can another device on this network load Tori from
/// this desktop, and does it arrive able to connect.
///
/// # Why this is not just a convenience
///
/// Without it the two-machine flow needs two commands and a second release
/// build (`make tauri-run` plus `make pair-serve`), which is one command too
/// many for the most ordinary thing anyone will do with this product. With it,
/// the process already running the rendezvous is the one handing out the app, so
/// it knows its own node peer-id and address and puts them in the URL it
/// redirects to — a browser that loads the bare address is provisioned **before
/// boot**, with nothing typed, nothing scanned and no reload.
///
/// # Three states, and the middle one is the point
///
/// Serving *with* a rendezvous is "type this URL and you are done". Serving
/// *without* one hands over a working app that still cannot reach anybody, and
/// says so, pointing at the Rendezvous row directly above — which is why the two
/// rows sit together. Collapsing them into "on" would promise the first while
/// delivering the second.
///
/// Unlike its two neighbours this toggle **does not restart the backend**: the
/// server is an independent listener, not a handler mounted on `PeerBuilder` nor
/// a lease bound to this run's port. The button says so, because the other two
/// buttons here warn that they *do*, and an unexplained difference between three
/// adjacent controls is read as an oversight.
fn render_app_server_row(parent: &Element, b: &BackendStatusView, s: &AppServerView, ctx: &DomCtx) {
    // The URL is deliberately NOT interpolated into these sentences: it is
    // rendered below as a copyable code row, because it is going to be typed on
    // a different device and a string inside a sentence cannot be copied.
    let (text, color) = match s {
        AppServerView::Serving { .. } => (
            crate::i18n::t("sysoverview.appserver_on", &[]),
            crate::theme_tokens::STATUS_OK,
        ),
        AppServerView::ServingUnprovisioned { .. } => (
            crate::i18n::t("sysoverview.appserver_unprovisioned", &[]),
            crate::theme_tokens::STATUS_WARN,
        ),
        AppServerView::Off => (
            crate::i18n::t("sysoverview.appserver_off", &[]),
            "var(--text-muted, #c0c0c0)",
        ),
    };
    add_row(
        parent,
        &crate::i18n::t("sysoverview.appserver", &[]),
        &text,
        Some(&crate::i18n::t("sysoverview.appserver_hint", &[])),
        Some(color),
    );

    // The URL is NOT rendered here — it lives in the "Connect another device"
    // card below, whose whole job is the strings you carry to another machine.
    // It used to sit under this row, which meant the one thing a person came to
    // this window for was buried among three switches and six identity rows.

    // `\x1f`-packed backend id + wanted direction, for `render_rendezvous_row`'s
    // reason: the handler must not re-derive the current state, or a poll
    // landing between render and click flips the wrong way.
    let want = if s.is_serving() { "0" } else { "1" };
    let label = if s.is_serving() {
        crate::i18n::t("label.turn_off", &[])
    } else {
        crate::i18n::t("label.turn_on", &[])
    };
    let btn = components::button_value(
        ctx,
        &label,
        components::ButtonKind::Small,
        "sb_set_app_server",
        &format!("{}\u{1f}{}", b.peer_id, want),
    );
    add_chip_row(parent, "", btn);
}

// (`render_pairing_row` retired — the pairing lines are steps in
// `render_connect_card`, beside the served URL that replaces them in the common
// case. Its reasoning survives where it belongs: *why a command and not two
// facts* is now `pairing_commands`' own doc, and *why two addresses and never
// silently one* is enforced by that function's tests.)

/// One `label: <code>command</code> [Copy]` row.
///
/// Built from the card's own row helper plus the shared identity-code style, so
/// it adds no raw styles to the `ui-lint` baseline and reads like the peer-id
/// row in Peer Connections — which is the other place in the app that exists to
/// get a long string onto another device.
fn add_command_row(parent: &Element, label: &str, command: &str, ctx: &DomCtx) {
    // The shared affordance (`components::copy_code`) — this row and Peer
    // Connections' device rows are the two places in the app whose job is
    // getting a long string onto another machine, and they had one copy button
    // between them.
    //
    // The hint names the RELOAD as well as the address: the node is read once
    // at boot, so a paste that stops at "added connector" leaves a correct
    // registry doing nothing.
    let holder = components::copy_code(
        ctx,
        command,
        Some(&crate::i18n::t("sysoverview.pair_hint", &[])),
    );
    add_chip_row(parent, label, holder);
}

/// The port-mapping row: whether a router is forwarding this backend from the
/// internet, and the address if so.
///
/// **Four different `None`s, and they must not read as one.** The backend
/// reports `external_addr` alongside `port_mapping` (are we asking at all) and
/// `port_mapping_note` (why not), precisely so this row can tell apart *nobody
/// asked*, *still asking*, *the router refused*, and *your ISP has you behind
/// carrier-grade NAT*. Collapsing them into "not reachable" would be the
/// one-sentence failure the reachability work exists to end, rebuilt on a
/// different surface.
fn render_port_mapping_row(parent: &Element, b: &BackendStatusView, ctx: &DomCtx) {
    let (text, color) = match (&b.external_addr, b.port_mapping) {
        // A door is open. The address is the actionable half — it is what
        // someone off this network puts into `connector add`.
        (Some(addr), _) => (
            crate::i18n::t("sysoverview.portmap_on", &[("addr", addr.as_str())]),
            crate::theme_tokens::STATUS_OK,
        ),
        // Asking, no answer yet. Deliberately NOT the note: the backend sends
        // none while probing, and inventing "checking…" as a failure string
        // would report a refusal that has not happened.
        (None, true) => match &b.port_mapping_note {
            Some(note) => (
                crate::i18n::t("sysoverview.portmap_none", &[("why", note.as_str())]),
                "var(--text-muted, #c0c0c0)",
            ),
            None => (
                crate::i18n::t("sysoverview.portmap_asking", &[]),
                "var(--text-muted, #c0c0c0)",
            ),
        },
        (None, false) => (
            crate::i18n::t("sysoverview.portmap_off", &[]),
            "var(--text-muted, #c0c0c0)",
        ),
    };
    add_row(
        parent,
        &crate::i18n::t("sysoverview.portmap", &[]),
        &text,
        Some(&crate::i18n::t("sysoverview.portmap_hint", &[])),
        Some(color),
    );

    // Same `\x1f`-packed which-backend + which-direction as the row above, and
    // for the same reason: a handler that reads "the opposite of what I last
    // painted" flips the wrong way when a poll lands between render and click.
    let want = if b.port_mapping { "0" } else { "1" };
    let label = if b.port_mapping {
        crate::i18n::t("sysoverview.portmap_stop", &[])
    } else {
        crate::i18n::t("sysoverview.portmap_start", &[])
    };
    let btn = components::button_value(
        ctx,
        &label,
        components::ButtonKind::Small,
        "sb_set_port_mapping",
        &format!("{}\u{1f}{}", b.peer_id, want),
    );
    add_chip_row(parent, "", btn);
}

/// A `label:` row whose value is an element (e.g. a status chip) rather than
/// text — same left column as [`add_row`].
fn add_chip_row(parent: &Element, label: &str, value: Element) {
    let row = util::create_element("div");
    row.set_attribute("style", "display:flex;gap:8px;margin:2px 0;font-size:13px;align-items:center")
        .ok();
    let key = util::create_element("span");
    key.set_attribute("style", "color:var(--text-dim, #888);min-width:88px;flex:0 0 auto")
        .ok();
    util::set_text(&key, label);
    util::append(&row, &key);
    util::append(&row, &value);
    util::append(parent, &row);
}

/// The honest S↔B **transport** link state, driven by the live connection-health
/// mirror (via `connected`/`dialing`), NOT by the orthogonal backend-auth read:
/// - actively dialing (`liveness == Connecting`) → Connecting
/// - registered and not Unreachable → Connected
/// - otherwise → Offline
///
/// Previously this gated "Connected" on the device-auth read succeeding
/// (`authorizations.checked && no error`), so a genuinely-connected link read
/// "Connecting…" forever whenever that async read was slow, absent, or failing —
/// the "always connecting" bug. The auth-read status is its own concern, shown in
/// the Device authorizations card; it must not mask the transport state.
fn link_state(output: &SystemOverviewOutput) -> ConnState {
    if output.dialing {
        return ConnState::Connecting;
    }
    if output.connected {
        return ConnState::Connected;
    }
    ConnState::Offline
}

/// Inbound-device authorizations for the canonical backend — the authority
/// surface (moved here from Peer Connections, Direction A). A table (S7) of
/// connected devices with the shared status chip (S4) and a per-device
/// Authorize control. Refreshes **reactively** (auto-fired on a throttle by the
/// window; see `mod.rs`) — no manual refresh button, per the subscription-first
/// design.
fn render_authorizations(parent: &Element, output: &SystemOverviewOutput, ctx: &DomCtx) {
    let Some(auth) = &output.authorizations else {
        return;
    };
    let card = components::card(&crate::i18n::t("sysoverview.device_auth", &[]));

    if !output.connected {
        util::append(
            &card,
            &components::empty(&crate::i18n::t("sysoverview.waiting_link", &[])),
        );
    } else if !auth.checked {
        util::append(&card, &components::loading(&crate::i18n::t("sysoverview.checking_devices", &[])));
    } else if let Some(err) = &auth.error {
        // Clean, human presentation (operator ask): a plain headline + the raw
        // reason kept as dim detail rather than dumped as the whole message.
        util::append(
            &card,
            &components::error(&crate::i18n::t("sysoverview.devices_error", &[])),
        );
        let detail = util::create_element("p");
        detail
            .set_attribute("style", "color:var(--text-dim, #888);font-size:11px;margin:2px 0")
            .ok();
        util::set_text(&detail, err);
        util::append(&card, &detail);
    } else if auth.pending.is_empty() && auth.authorized.is_empty() {
        util::append(&card, &components::empty(&crate::i18n::t("sysoverview.no_devices", &[])));
    } else {
        let (tbl, body) = components::table(&[
            &crate::i18n::t("label.device", &[]),
            &crate::i18n::t("label.status", &[]),
            &crate::i18n::t("sysoverview.col_grant", &[]),
            "",
        ]);
        for row in &auth.pending {
            append_pending_row(&body, auth, row, ctx);
        }
        for row in &auth.authorized {
            util::append(
                &body,
                &components::tr(vec![
                    components::td_text(&row.display),
                    components::td(&components::auth_chip(AuthState::Authorized)),
                    components::td(&grant_cell(row.profile.as_deref())),
                    components::td_text(""),
                ]),
            );
        }
        util::append(&card, &tbl);
    }

    util::append(parent, &card);
}

/// One pending-device row: status chip, a grant-profile picker, and Authorize.
fn append_pending_row(body: &Element, auth: &AuthorizationsView, row: &AuthRow, ctx: &DomCtx) {
    // Unwired select — the grant profile is read at Authorize-click time, so a
    // change must NOT dispatch (a dispatch would rebuild the row and reset the
    // pick).
    let select = components::select_el(
        &[
            ("file-transfer", &crate::i18n::t("sysoverview.profile_pull", &[])),
            ("file-transfer-rw", &crate::i18n::t("sysoverview.profile_twoway", &[])),
        ],
        "file-transfer",
    );

    let authorize = components::button_el(&crate::i18n::t("btn.authorize", &[]), components::ButtonKind::Primary);
    {
        let actions = ctx.actions.clone();
        let rp = ctx.repaint.clone();
        let local = auth.manager_pid.clone();
        let backend = auth.backend_pid.clone();
        let target = row.peer_id.clone();
        let select_ref = select.clone();
        ctx.listen(&authorize, "click", move |_| {
            let profile = select_ref
                .clone()
                .dyn_into::<web_sys::HtmlSelectElement>()
                .map(|s| s.value())
                .unwrap_or_else(|_| "file-transfer".to_string());
            actions.borrow_mut().push(Action::AuthorizePeer {
                local_peer_id: local.clone(),
                backend_pid: backend.clone(),
                target_pid: target.clone(),
                profile,
            });
            rp();
        });
    }

    util::append(
        body,
        &components::tr(vec![
            components::td_text(&row.display),
            components::td(&components::auth_chip(AuthState::Pending)),
            components::td(&select),
            components::td(&authorize),
        ]),
    );
}

/// The "Grant" cell for an authorized device — the legibility surface: *what*
/// this device can do, not just that it's authorized. Shows the granted
/// profile's human label with its plain-English scope as a hover tooltip. An
/// unrecognized / unrecorded profile falls back to an honest "granted" with a
/// note, rather than a misleading blank.
fn grant_cell(profile: Option<&str>) -> Element {
    let span = util::create_element("span");
    span.set_attribute("style", "font-size:12px").ok();
    match profile.and_then(crate::backend_auth::GrantProfile::from_token) {
        Some(p) => {
            util::set_text(&span, &crate::i18n::t(p.label(), &[]));
            span.set_attribute("title", &crate::i18n::t(p.scope_summary(), &[])).ok();
        }
        None => {
            util::set_text(&span, &crate::i18n::t("status.granted", &[]));
            span.set_attribute(
                "title",
                &crate::i18n::t("sysoverview.grant_backend_hint", &[]),
            )
            .ok();
        }
    }
    span
}

/// Color for the Status row, by scanning the lifecycle string.
fn status_color_for(status: &str) -> &'static str {
    let up = status.to_ascii_uppercase();
    if up.contains("RUN") {
        crate::theme_tokens::STATUS_OK
    } else if up.contains("STOP") {
        crate::theme_tokens::STATUS_WARN
    } else {
        "var(--text-muted, #c0c0c0)"
    }
}

