//! System Overview DOM renderer — pure consumer of
//! [`SystemOverviewOutput`](crate::views::system_overview::output::SystemOverviewOutput).

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::components::{self, AuthState, ConnState};
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::system_overview::output::{
    AuthorizationsView, AuthRow, BackendStatusView, SystemOverviewOutput,
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

    // --- Status card ---
    let status = components::card(&crate::i18n::t("label.status", &[]));
    match &output.backend {
        Some(b) => {
            add_row(&status, &crate::i18n::t("sysoverview.native_peer", &[]), &b.short_id, Some(&b.peer_id), None);
            let status_color = status_color_for(&b.status);
            add_row(&status, &crate::i18n::t("label.status", &[]), &b.status, None, Some(status_color));
            add_row(
                &status,
                &crate::i18n::t("sysoverview.listen", &[]),
                &b.ws_addr.clone().unwrap_or_else(|| crate::i18n::t("sysoverview.not_listening", &[])),
                None,
                None,
            );
            // Link status via the shared connection vocabulary (S4). Honest
            // liveness: "Connected" only when the auth read actually succeeded
            // over the transport (the real probe) — a remembered-but-stale link
            // (read failing) shows Connecting, not a false Connected.
            add_chip_row(&status, &crate::i18n::t("sysoverview.link", &[]), components::conn_chip(link_state(output)));
            // Share: the tree prefix a paired device browses, plus the real
            // on-disk directory (answers "where do shared files live").
            add_row(
                &status,
                &crate::i18n::t("sysoverview.share_tree", &[]),
                &output.share_prefix,
                Some(&crate::i18n::t("sysoverview.share_tree_hint", &[])),
                None,
            );
            if let Some(path) = &output.share_path {
                add_row(&status, &crate::i18n::t("sysoverview.share_disk", &[]), path, Some(path), None);
            }
            // Rendezvous: whether browsers on this LAN can meet through this
            // desktop. Two browsers need no STUN and no TURN on one LAN
            // (`e2e-webrtc-lan`) — but they do need somewhere to exchange the
            // offer, and this is the only thing that can be it without anyone
            // running server infrastructure.
            render_rendezvous_row(&status, b, ctx);
        }
        None => {
            // Non-content states (S5): still loading vs genuinely absent.
            if output.fetched {
                util::append(&status, &components::empty(&crate::i18n::t("sysoverview.no_backend", &[])));
            } else {
                util::append(&status, &components::loading(&crate::i18n::t("sysoverview.loading_status", &[])));
            }
        }
    }
    util::append(&wrapper, &status);

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
        crate::i18n::t("sysoverview.rendezvous_stop", &[])
    } else {
        crate::i18n::t("sysoverview.rendezvous_start", &[])
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

