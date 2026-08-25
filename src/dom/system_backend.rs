//! System Backend DOM renderer — pure consumer of
//! [`SystemBackendOutput`](crate::views::system_backend::output::SystemBackendOutput).

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::dom::components::{self, AuthState, ConnState};
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::views::system_backend::output::{AuthorizationsView, AuthRow, SystemBackendOutput};
use crate::views::system_overview::output::SystemOverviewOutput;

use web_sys::Element;

/// Render the merged System Overview window: the system peer(s) + posture
/// (`overview`) up top, then the native peer's live detail (`output` — status,
/// device authorizations, logs, share). One window, one job (S2): the old
/// standalone "System Peer (Native)" window and its drill-in are retired.
pub fn render(
    container: &Element,
    output: &SystemBackendOutput,
    overview: &SystemOverviewOutput,
    ctx: &DomCtx,
) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "system-backend");
    wrapper.set_attribute("style", theme::SECTION).ok();

    // --- Header (title + log controls) ---
    let header = util::create_element("div");
    header.set_attribute("style", theme::HEADER_ROW).ok();
    let h2 = util::create_element("h2");
    h2.set_attribute("style", "margin:0").ok();
    util::set_text(&h2, "System Overview");
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
        util::set_text(&lvl_label, "Level");
        util::append(&controls, &lvl_label);

        let select = util::create_element("select");
        select.set_attribute("style", theme::SELECT).ok();
        for level in crate::views::system_backend::model::LEVELS {
            let opt = util::create_element("option");
            opt.set_attribute("value", level).ok();
            if *level == output.log_level {
                opt.set_attribute("selected", "").ok();
            }
            util::set_text(&opt, level);
            util::append(&select, &opt);
        }
        ctx.on_select_change(&select, "sb_set_level");
        util::append(&controls, &select);

        let clear_btn = util::create_element("button");
        util::set_text(&clear_btn, "Clear logs");
        clear_btn.set_attribute("style", theme::BTN_SMALL).ok();
        ctx.on_window_event(&clear_btn, "click", "sb_clear_logs", "");
        util::append(&controls, &clear_btn);

        util::append(&header, &controls);
    }
    util::append(&wrapper, &header);

    // System peers + posture (always — this is the browser view too). The
    // native peer's identity card here is terse; its live detail follows below.
    crate::dom::system_overview::render_system_peers(&wrapper, overview);

    if !output.tauri {
        // Not a dead end — the backend simply isn't reachable from a plain
        // browser session today (its logs come over a desktop-local IPC).
        // Exposing them to an authorized remote peer is a planned direction.
        let note = util::create_element("p");
        note.set_attribute("style", "color:var(--text-dim, #888);margin:4px 0")
            .ok();
        util::set_text(
            &note,
            "The native system peer runs in the desktop app. Its live logs aren't \
             streamed over this browser session yet — exposing them to an \
             authorized peer is planned.",
        );
        util::append(&wrapper, &note);
        util::append(container, &wrapper);
        return;
    }

    // --- Status card ---
    let status = components::card("Status");
    match &output.backend {
        Some(b) => {
            add_row(&status, "Native peer", &b.short_id, Some(&b.peer_id), None);
            let status_color = status_color_for(&b.status);
            add_row(&status, "Status", &b.status, None, Some(status_color));
            add_row(
                &status,
                "Listen",
                b.ws_addr.as_deref().unwrap_or("(not listening)"),
                None,
                None,
            );
            // Link status via the shared connection vocabulary (S4). Honest
            // liveness: "Connected" only when the auth read actually succeeded
            // over the transport (the real probe) — a remembered-but-stale link
            // (read failing) shows Connecting, not a false Connected.
            add_chip_row(&status, "Link (S↔B)", components::conn_chip(link_state(output)));
            // Share: the tree prefix a paired device browses, plus the real
            // on-disk directory (answers "where do shared files live").
            add_row(
                &status,
                "Share (tree)",
                &output.share_prefix,
                Some("Files a connected device can browse and pull, exposed under this peer's tree"),
                None,
            );
            if let Some(path) = &output.share_path {
                add_row(&status, "Share (disk)", path, Some(path), None);
            }
        }
        None => {
            // Non-content states (S5): still loading vs genuinely absent.
            if output.fetched {
                util::append(&status, &components::empty("No native system peer provisioned."));
            } else {
                util::append(&status, &components::loading("Loading status…"));
            }
        }
    }
    util::append(&wrapper, &status);

    // --- Device authorizations card (inbound; Direction A home) ---
    render_authorizations(&wrapper, output, ctx);

    // --- Live log card ---
    let logs = components::card("Logs");
    let pre = util::create_element("pre");
    pre.set_attribute("style", theme::PRE_OUTPUT).ok();
    if output.log_lines.is_empty() {
        pre.set_inner_html(
            "<span style='color:var(--text-dim, #888)'>(no logs yet)</span>",
        );
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

/// The honest S↔B link state: Connected only when the auth read actually
/// succeeded over the transport (the real liveness probe); a remembered-but-
/// stale link (connected proxy true, read failing/unconfirmed) is Connecting.
fn link_state(output: &SystemBackendOutput) -> ConnState {
    if !output.connected {
        // Distinguish an armed, actively-dialing auto-connect (progress) from a
        // genuine down link. The boot provision→dial→handshake gap now reads as
        // "Connecting…" instead of a broken-looking "Offline" that snaps green.
        return if output.dialing {
            ConnState::Connecting
        } else {
            ConnState::Offline
        };
    }
    match &output.authorizations {
        Some(a) if a.checked && a.error.is_none() => ConnState::Connected,
        _ => ConnState::Connecting,
    }
}

/// Inbound-device authorizations for the canonical backend — the authority
/// surface (moved here from Peer Connections, Direction A). A table (S7) of
/// connected devices with the shared status chip (S4) and a per-device
/// Authorize control. Refreshes **reactively** (auto-fired on a throttle by the
/// window; see `mod.rs`) — no manual refresh button, per the subscription-first
/// design.
fn render_authorizations(parent: &Element, output: &SystemBackendOutput, ctx: &DomCtx) {
    let Some(auth) = &output.authorizations else {
        return;
    };
    let card = components::card("Device authorizations");

    if !output.connected {
        util::append(
            &card,
            &components::empty("Waiting for the backend link — devices appear once connected."),
        );
    } else if !auth.checked {
        util::append(&card, &components::loading("Checking connected devices…"));
    } else if let Some(err) = &auth.error {
        // Clean, human presentation (operator ask): a plain headline + the raw
        // reason kept as dim detail rather than dumped as the whole message.
        util::append(
            &card,
            &components::error("Couldn't read the backend's devices — the manager link may not be ready yet."),
        );
        let detail = util::create_element("p");
        detail
            .set_attribute("style", "color:var(--text-dim, #888);font-size:11px;margin:2px 0")
            .ok();
        util::set_text(&detail, err);
        util::append(&card, &detail);
    } else if auth.pending.is_empty() && auth.authorized.is_empty() {
        util::append(&card, &components::empty("No devices connected to this backend."));
    } else {
        let (tbl, body) = components::table(&["Device", "Status", "Grant", ""]);
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
    let select = util::create_element("select");
    select.set_attribute("style", "font-size:12px;padding:2px 4px").ok();
    for (token, label) in [("file-transfer", "Pull only"), ("file-transfer-rw", "Two-way")] {
        let opt = util::create_element("option");
        opt.set_attribute("value", token).ok();
        util::set_text(&opt, label);
        util::append(&select, &opt);
    }

    let authorize = util::create_element("button");
    util::set_text(&authorize, "Authorize");
    authorize.set_attribute("style", theme::BTN_PRIMARY).ok();
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
            util::set_text(&span, p.label());
            span.set_attribute("title", p.scope_summary()).ok();
        }
        None => {
            util::set_text(&span, "granted");
            span.set_attribute(
                "title",
                "Authorized on the backend; the specific scope isn't recorded locally.",
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

