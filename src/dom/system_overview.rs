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
    health: &crate::views::system_overview::output::HealthView,
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

    // --- Problems (the health checks) ---
    //
    // **Above the `!output.tauri` early return, and that placement is the whole
    // point.** This function returns here in a plain browser, so a section
    // added anywhere below it would exist only in the desktop app — and both
    // incidents this answers happened in a browser, one of them on a phone. It
    // was written below the return first, and looking at the running app is
    // what caught it: the window rendered perfectly and the section simply was
    // not there.
    //
    // Above the topology cards too: if something is wrong with this profile's
    // beliefs, that is why a person opened this window. Under the log tail it
    // would be the "diagnostic you have to remember" trap one scroll down
    // instead of one window over.
    render_health(&wrapper, health, ctx);

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

    // --- Two cards, each answering ONE question ---
    //
    //   A. This device    — what am I, and where do my files go
    //   B. Other devices  — the switches, each with the string it produces
    //
    // This was one "Status" card carrying identity facts, three service
    // switches and the pairing lines as identical `label: value` rows. Every
    // row was individually correct and nothing told you which KIND of row you
    // were reading, so finding "the address to type on the laptop" meant
    // scanning eleven rows and knowing which two to combine.
    //
    // The first fix split it three ways and gave the carry-strings their own
    // card. That was better and still wrong in a way the operator named
    // immediately: turning a switch on and finding out what it produced were
    // two separate reading tasks, in two boxes, with nothing connecting them.
    // The strings live **under the switch that produces them** now, so flipping
    // one visibly yields the thing you carry. Do not re-separate them, and do
    // not add a row here that answers card A's question — that is how this
    // became a pile the first time.
    match &output.backend {
        Some(b) => {
            render_device_card(&wrapper, output, b);
            render_services_card(&wrapper, output, b, ctx);
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
/// The *Problems* card.
///
/// **Quiet when there is nothing wrong** — the operator's framing, and the
/// right one: *if there are no problems you do not go to the doctor*, so a
/// healthy profile gets one line rather than a wall of green ticks that trains
/// people to stop reading it.
///
/// But *quiet* is not *silent*. A section that renders nothing when healthy
/// cannot be told from one that never ran or crashed, so the clear state still
/// says it checked. That is the same distinction the model's `ran` flag exists
/// for, carried through to the screen.
///
/// Every string here comes from `doctor::copy` or from the finding itself; this
/// function contributes none of its own, which is what keeps the whole
/// surface's copy in one file for translation.
fn render_health(
    parent: &Element,
    health: &crate::views::system_overview::output::HealthView,
    ctx: &DomCtx,
) {
    use crate::doctor::{copy, Verdict};

    let card = components::card(&copy::title());

    if !health.ran {
        util::append(&card, &components::loading(&copy::checking()));
        util::append(parent, &card);
        return;
    }

    // Only what `warrants_attention` says — a divergence, or a check that tried
    // and could not tell. NOT `!is_clear()`: on an ordinary healthy profile all
    // three come back "nothing to compare"/"not checked yet", and listing those
    // put three paragraphs of non-problems under a heading that says
    // **Problems**. See the note on `Verdict::warrants_attention`.
    let problems: Vec<&crate::doctor::Finding> =
        health.findings.iter().filter(|f| f.verdict.warrants_attention()).collect();

    if problems.is_empty() {
        let line = util::create_element("p");
        line.set_attribute("style", theme::NOTE).ok();
        util::set_text(&line, &copy::all_clear());
        util::append(&card, &line);

        // …but say what "no problems" was based on. A clear line with no count
        // behind it cannot be told from a section that never ran, and a check
        // that found no source has not cleared anything.
        let quiet = health.findings.iter().filter(|f| !f.verdict.is_clear()).count();
        let when = util::create_element("p");
        when.set_attribute("style", theme::HINT).ok();
        util::set_text(
            &when,
            &if quiet == 0 {
                copy::checked(health.findings.len())
            } else {
                copy::checked_with_gaps(health.findings.len(), quiet)
            },
        );
        util::append(&card, &when);
    } else {
        // Worst first. `Verdict` orders by severity, so this needs no second
        // table to stay in step with a new verdict.
        let mut sorted = problems;
        sorted.sort_by_key(|f| f.verdict);

        // One honest banner when anything could not be established, so the
        // absence of a warning is never read as an all-clear.
        if sorted.iter().any(|f| f.verdict == Verdict::Undetermined) {
            util::append(&card, &components::notice(&copy::some_undetermined()));
        }

        for f in sorted {
            util::append(&card, &health_finding(f, ctx));
        }
    }

    if let Some(msg) = &health.remedy_message {
        util::append(&card, &components::success(msg));
    }

    // **The re-check control says which of its two states it is in.** Pressing
    // it keeps the previous findings on screen — they are still the best answer
    // available — so without this the button changed nothing visible: up to 3 s
    // of nothing on a slow domain (D23's deadline), and forever when the
    // findings come back the same, which is the common case (audit F6).
    let row = util::create_element("div");
    row.set_attribute("style", theme::BTN_ROW).ok();
    if health.checking {
        util::append(&row, &components::loading(&copy::checking()));
    } else {
        util::append(
            &row,
            &components::button(
                ctx,
                &copy::recheck(),
                components::ButtonKind::Small,
                crate::views::system_overview::HEALTH_RECHECK_EVENT,
            ),
        );
    }
    util::append(&card, &row);

    util::append(parent, &card);
}

/// One finding: what this machine believes, what it was checked against, what
/// that means, and — where one exists — the repair, with what it would change
/// stated **beside the button rather than after pressing it**.
fn health_finding(f: &crate::doctor::Finding, ctx: &DomCtx) -> Element {
    let block = util::create_element("div");
    block.set_attribute("style", theme::SECTION_GROUP).ok();

    let head = util::create_element("div");
    head.set_attribute("style", theme::ROW_INLINE).ok();
    let name = util::create_element("strong");
    util::set_text(&name, &f.check.title());
    util::append(&head, &name);
    util::append(&head, &components::health_chip(&f.verdict.chip(), f.verdict.tone()));
    util::append(&block, &head);

    // The audit pair. A finding that says only "something is wrong" is an
    // opinion; belief + source is what makes it checkable by the person
    // reading it.
    for (label, value) in
        [(crate::doctor::copy::belief(), &f.belief), (crate::doctor::copy::source(), &f.source)]
    {
        let row = util::create_element("p");
        row.set_attribute("style", theme::HINT).ok();
        util::set_text(&row, &format!("{label} {value}"));
        util::append(&block, &row);
    }

    let detail = util::create_element("p");
    detail.set_attribute("style", theme::NOTE).ok();
    util::set_text(&detail, &f.detail);
    util::append(&block, &detail);

    if let Some(remedy) = f.remedy {
        let effect = util::create_element("p");
        effect.set_attribute("style", theme::HINT).ok();
        util::set_text(&effect, &remedy.effect());
        util::append(&block, &effect);

        let row = util::create_element("div");
        row.set_attribute("style", theme::BTN_ROW).ok();
        util::append(
            &row,
            &components::button_value(
                ctx,
                &remedy.label(),
                components::ButtonKind::Primary,
                crate::views::system_overview::HEALTH_REMEDY_EVENT,
                remedy.key(),
            ),
        );
        util::append(&block, &row);
    }

    block
}

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
    // **Every address this process holds, in the card whose question is "what
    // am I".** The switches say what they DO and the connect card says what to
    // CARRY; neither answers *which port is this thing on*, and that question
    // is asked constantly — a desktop that has run Tori before does not get
    // 4041, it gets an ephemeral port, and with two of them up "which one am I
    // even talking to" is unanswerable from the UI. It is answerable from here.
    add_row(
        &card,
        &crate::i18n::t("sysoverview.app_addr", &[]),
        output
            .app_server
            .url()
            .unwrap_or(&crate::i18n::t("sysoverview.app_not_serving", &[])),
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

/// **Card B — Other devices.** The three switches, each carrying the string it
/// produces.
///
/// Every row is one [`components::service_row`]: name, state chip, the control
/// that changes it, a sentence saying what that state means for other devices,
/// and underneath it whatever a person is meant to carry to the other machine.
///
/// # Which string belongs to which switch
///
/// The composition — which steps exist, whether the URL claims to be the
/// finished flow, whether a pairing line may be printed at all — stays in
/// [`connect_steps`](crate::views::system_overview::output::connect_steps),
/// where native tests read it; this only *distributes* the result. The mapping
/// is not arbitrary: the URL is produced by the app server, the LAN pairing line
/// by the rendezvous (it is that node's own address), and the internet one by
/// the router that forwarded it.
///
/// # Why the app server is first now
///
/// `connect_steps` puts the URL before the command it replaces, because the URL
/// needs nothing typed on the far end — an operator who reads only the first
/// thing must have read the better one. With the strings distributed to their
/// switches, that ordering decision becomes the ROW order, so the row producing
/// the URL leads. The "turn on Rendezvous" pointer in the unprovisioned state
/// points **down** accordingly.
fn render_services_card(
    parent: &Element,
    output: &SystemOverviewOutput,
    b: &BackendStatusView,
    ctx: &DomCtx,
) {
    use crate::views::system_overview::output::{connect_steps, ConnectStep};

    let card = components::card(&crate::i18n::t("sysoverview.card_services", &[]));
    let steps = connect_steps(b, &output.app_server);
    let pair_line = |want: PairScope| {
        steps.iter().find_map(|s| match s {
            ConnectStep::PasteCommand { scope, command } if *scope == want => Some(command.clone()),
            _ => None,
        })
    };
    let url_step = steps.iter().find_map(|s| match s {
        ConnectStep::OpenUrl { url, provisioned } => Some((url.clone(), *provisioned)),
        _ => None,
    });

    render_app_server_row(&card, b, &output.app_server, url_step, ctx);
    // Rendezvous: whether browsers on this LAN can meet through this desktop.
    // Two browsers need no STUN and no TURN on one LAN (`e2e-webrtc-lan`) — but
    // they do need somewhere to exchange the offer, and this is the only thing
    // that can be it without anyone running server infrastructure.
    render_rendezvous_row(&card, b, pair_line(PairScope::Lan), ctx);
    render_port_mapping_row(&card, b, pair_line(PairScope::Internet), ctx);
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
fn render_rendezvous_row(
    parent: &Element,
    b: &BackendStatusView,
    pair_lan: Option<String>,
    ctx: &DomCtx,
) {
    let (state, text) = if b.signaling_node {
        // The address is the actionable half: "it is on" is not something a
        // user can do anything with, and this is the string the other browser
        // types into `connector add`.
        let addr = b.ws_addr.clone().unwrap_or_default();
        (
            components::ServiceState::On,
            crate::i18n::t("sysoverview.rendezvous_on", &[("addr", addr.as_str())]),
        )
    } else {
        (
            components::ServiceState::Off,
            crate::i18n::t("sysoverview.rendezvous_off", &[]),
        )
    };
    let row = components::service_row(
        &crate::i18n::t("sysoverview.rendezvous", &[]),
        state,
        &text,
        switch(b, b.signaling_node, "sb_set_signaling_node", ctx),
    );
    row.set_attribute("title", &crate::i18n::t("sysoverview.rendezvous_hint", &[])).ok();
    // The LAN pairing line belongs to this switch: the address in it is this
    // node's own, and `pairing_commands` refuses to print the line at all when
    // the rendezvous is off — so it can only ever appear under a row that is on.
    if let Some(command) = pair_lan {
        util::append(
            &row,
            &components::carry_line(
                ctx,
                &crate::i18n::t("sysoverview.pair_lan", &[]),
                &command,
                Some(&crate::i18n::t("sysoverview.pair_hint", &[])),
            ),
        );
    }
    util::append(parent, &row);
}

/// The on/off control for a service row.
///
/// `\x1f`-packed: WHICH backend and WHICH direction. The row is the only place
/// that knows both, and the handler must not have to re-derive the current
/// state to work out what the click meant — a toggle that reads "the opposite
/// of whatever I last painted" flips the wrong way whenever a poll lands
/// between the render and the click.
fn switch(b: &BackendStatusView, on: bool, event: &str, ctx: &DomCtx) -> Element {
    let label = if on {
        crate::i18n::t("label.turn_off", &[])
    } else {
        crate::i18n::t("label.turn_on", &[])
    };
    components::button_value(
        ctx,
        &label,
        components::ButtonKind::Small,
        event,
        &format!("{}\u{1f}{}", b.peer_id, if on { "0" } else { "1" }),
    )
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
fn render_app_server_row(
    parent: &Element,
    b: &BackendStatusView,
    s: &AppServerView,
    url_step: Option<(String, bool)>,
    ctx: &DomCtx,
) {
    // The URL is deliberately NOT interpolated into these sentences: it is
    // rendered below as a copyable code row, because it is going to be typed on
    // a different device and a string inside a sentence cannot be copied.
    let (state, text) = match s {
        AppServerView::Serving { .. } => (
            components::ServiceState::On,
            crate::i18n::t("sysoverview.appserver_on", &[]),
        ),
        // Serving and not useful yet: the visitor gets a working app and no way
        // to reach anybody. `Incomplete` is the state that exists for exactly
        // this, and the sentence points at the row that fixes it.
        AppServerView::ServingUnprovisioned { .. } => (
            components::ServiceState::Incomplete,
            crate::i18n::t("sysoverview.appserver_unprovisioned", &[]),
        ),
        AppServerView::Off => (
            components::ServiceState::Off,
            crate::i18n::t("sysoverview.appserver_off", &[]),
        ),
    };
    let row = components::service_row(
        &crate::i18n::t("sysoverview.appserver", &[]),
        state,
        &text,
        switch(b, s.is_serving(), "sb_set_app_server", ctx),
    );
    row.set_attribute("title", &crate::i18n::t("sysoverview.appserver_hint", &[])).ok();
    // The URL this switch produces, right under it — and it carries the copy
    // button for the pairing line's reason: it is going to be typed on a
    // *different* device, and every character retyped by hand is a chance to
    // produce a failure that reads as connectivity.
    if let Some((url, provisioned)) = url_step {
        let hint = if provisioned {
            crate::i18n::t("sysoverview.connect_url_hint", &[])
        } else {
            crate::i18n::t("sysoverview.connect_url_hint_unprovisioned", &[])
        };
        util::append(
            &row,
            &components::carry_line(
                ctx,
                &crate::i18n::t("sysoverview.connect_url", &[]),
                &url,
                Some(&hint),
            ),
        );
    }
    util::append(parent, &row);
}

// (`render_pairing_row` retired — the pairing lines are steps in
// `render_connect_card`, beside the served URL that replaces them in the common
// case. Its reasoning survives where it belongs: *why a command and not two
// facts* is now `pairing_commands`' own doc, and *why two addresses and never
// silently one* is enforced by that function's tests.)

// (`add_command_row` retired — a pairing line is a `components::carry_line`
// under the switch that produced it, not a `label: value` row in a card of its
// own. The hint it carried survives on that atom: it names the RELOAD as well
// as the address, because the node is read once at boot and a paste that stops
// at "added connector" leaves a correct registry doing nothing.)

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
fn render_port_mapping_row(
    parent: &Element,
    b: &BackendStatusView,
    pair_wan: Option<String>,
    ctx: &DomCtx,
) {
    // The four states get four CHIPS as well as four sentences. That is the
    // point of the vocabulary having `Pending` and `Refused` as separate words:
    // "still asking" and "your router said no" are the same absence of an
    // address and completely different things to do next.
    let (state, text) = match (&b.external_addr, b.port_mapping) {
        // A door is open. The address is the actionable half — it is what
        // someone off this network puts into `connector add`.
        (Some(addr), _) => (
            components::ServiceState::On,
            crate::i18n::t("sysoverview.portmap_on", &[("addr", addr.as_str())]),
        ),
        // Asking, no answer yet. Deliberately NOT the note: the backend sends
        // none while probing, and inventing "checking…" as a failure string
        // would report a refusal that has not happened.
        (None, true) => match &b.port_mapping_note {
            Some(note) => (
                components::ServiceState::Refused,
                crate::i18n::t("sysoverview.portmap_none", &[("why", note.as_str())]),
            ),
            None => (
                components::ServiceState::Pending,
                crate::i18n::t("sysoverview.portmap_asking", &[]),
            ),
        },
        (None, false) => (
            components::ServiceState::Off,
            crate::i18n::t("sysoverview.portmap_off", &[]),
        ),
    };
    // This switch's verbs are its own ("Ask my router" / "Stop asking"), not the
    // shared on/off pair: you do not turn a router on, you ask it for something
    // it may refuse — which is exactly what the `Refused` state above reports.
    let label = if b.port_mapping {
        crate::i18n::t("sysoverview.portmap_stop", &[])
    } else {
        crate::i18n::t("sysoverview.portmap_start", &[])
    };
    let control = components::button_value(
        ctx,
        &label,
        components::ButtonKind::Small,
        "sb_set_port_mapping",
        &format!("{}\u{1f}{}", b.peer_id, if b.port_mapping { "0" } else { "1" }),
    );
    let row = components::service_row(
        &crate::i18n::t("sysoverview.portmap", &[]),
        state,
        &text,
        control,
    );
    row.set_attribute("title", &crate::i18n::t("sysoverview.portmap_hint", &[])).ok();
    // The internet pairing line belongs here: `pairing_commands` emits it only
    // when `external_addr` is `Some`, which is exactly *a door is open* and
    // never *asked* or *refused* — so it cannot appear under a row reporting a
    // refusal.
    if let Some(command) = pair_wan {
        util::append(
            &row,
            &components::carry_line(
                ctx,
                &crate::i18n::t("sysoverview.pair_wan", &[]),
                &command,
                Some(&crate::i18n::t("sysoverview.pair_hint", &[])),
            ),
        );
    }
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

