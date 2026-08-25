//! Programs window DOM renderer — program cards + the shape drivers.
//!
//! Phase 0 drives the `text` shape: the display port's
//! `app/shape/text-frame` entity rendered into a `<pre>` (a character
//! grid IS the teletype lineage — no new rendering technology). The
//! renderer is program-blind: it binds by the port's declared shape and
//! never inspects program state.

use web_sys::Element;

use crate::dom::components::{self, ButtonKind};
use crate::dom::theme;
use crate::dom::util::{self, DomCtx};
use crate::i18n::t;
use crate::peers::Peers;
use crate::program_host::descriptor::SHAPE_TEXT;
use crate::program_host::host::qualify;
use crate::program_host::shapes::{DisplayList, TextFrame};
use crate::views::programs::{
    Mount, MountStatus, ProgramsWindow, INSTALL_EVENT, RESTART_EVENT, START_EVENT, STOP_EVENT,
};

pub fn render(container: &Element, window: &ProgramsWindow, peers: &Peers, ctx: &DomCtx) {
    util::clear_children(container);

    let wrapper = util::create_element_with_class("div", "programs");
    wrapper.set_attribute("style", theme::SECTION).ok();

    let header = util::create_element("div");
    header.set_attribute("style", theme::HEADER_ROW).ok();
    let h2 = util::create_element("h2");
    h2.set_attribute("style", theme::TITLE_INLINE).ok();
    util::set_text(&h2, &t("window.programs", &[]));
    util::append(&header, &h2);
    util::append(&wrapper, &header);

    let hint = util::create_element("p");
    hint.set_attribute("style", theme::HINT).ok();
    util::set_text(&hint, &t("programs.subtitle", &[]));
    util::append(&wrapper, &hint);

    for (key, mount) in window.mounts.borrow().iter() {
        util::append(&wrapper, &program_card(key, mount, window, peers, ctx));
    }

    util::append(container, &wrapper);
}

fn program_card(
    key: &str,
    mount: &Mount,
    window: &ProgramsWindow,
    peers: &Peers,
    ctx: &DomCtx,
) -> Element {
    let card = components::card(&title_case(key));
    // Stable per-program hook for the e2e (and any tooling) — text
    // scans across the whole window match the wrong card.
    card.set_attribute("data-program", key).ok();

    // Status line — every state has a surface (D13).
    let status = util::create_element("p");
    status.set_attribute("style", theme::NOTE).ok();
    let status_text = match &mount.status {
        MountStatus::Absent => t("programs.status_absent", &[]),
        MountStatus::Refused(_) => t("programs.status_refused", &[]),
        MountStatus::Materializing { done, total } => t(
            "programs.status_materializing",
            &[("done", &done.to_string()), ("total", &total.to_string())],
        ),
        // Reuse the shared lifecycle vocabulary (peers.start/stop, status.stopped)
        // rather than parallel programs.* keys — a program's start/stop/stopped is
        // the same UI concept, and duplicate keys drift per locale (the i18n
        // consistency gate). Program-specific states (materializing/refused/faulted)
        // have no shared home and keep their own keys.
        MountStatus::Stopped => t("status.stopped", &[]),
        MountStatus::Running => t("programs.status_running", &[]),
        MountStatus::Faulted(_) => t("programs.status_faulted", &[]),
    };
    let rate = mount
        .descriptor
        .as_ref()
        .map(|d| d.tick.rate_hint)
        .unwrap_or(0);
    util::set_text(
        &status,
        &t(
            "programs.status_line",
            &[
                ("status", &status_text),
                ("ticks", &mount.ticks.to_string()),
                ("rate", &rate.to_string()),
            ],
        ),
    );
    util::append(&card, &status);

    // Refusal / fault reasons render through the shared error state.
    if let MountStatus::Refused(reason) | MountStatus::Faulted(reason) = &mount.status {
        util::append(&card, &components::error(reason));
    }

    // Actions — exactly one primary per state (S3).
    let row = util::create_element("div");
    row.set_attribute("style", theme::BTN_ROW).ok();
    let event_btn = |label_key: &str, kind: ButtonKind, event: &str| {
        components::button_action(
            ctx,
            &t(label_key, &[]),
            kind,
            crate::action::Action::WindowEvent {
                window_id: window.window_id,
                event: event.to_string(),
                value: key.to_string(),
            },
        )
    };
    match &mount.status {
        MountStatus::Absent | MountStatus::Faulted(_) => {
            util::append(&row, &event_btn("programs.install", ButtonKind::Primary, INSTALL_EVENT));
        }
        MountStatus::Stopped => {
            util::append(&row, &event_btn("peers.start", ButtonKind::Primary, START_EVENT));
            util::append(&row, &event_btn("programs.restart", ButtonKind::Small, RESTART_EVENT));
        }
        MountStatus::Running => {
            util::append(&row, &event_btn("peers.stop", ButtonKind::Primary, STOP_EVENT));
            util::append(&row, &event_btn("programs.restart", ButtonKind::Small, RESTART_EVENT));
        }
        MountStatus::Refused(_) | MountStatus::Materializing { .. } => {}
    }
    util::append(&card, &row);

    // Display driver — bound by shape, program-blind.
    if matches!(mount.status, MountStatus::Stopped | MountStatus::Running) {
        if let Some(desc) = &mount.descriptor {
            if let Some(port) = desc.display_port() {
                if port.shape == SHAPE_TEXT {
                    util::append(
                        &card,
                        &text_driver(
                            peers,
                            &window.peer_id,
                            &mount.bundle.origin_peer,
                            &port.path,
                        ),
                    );
                }
            }
        }
    }
    card
}

/// The `text` shape driver: read the port entity, decode the
/// text-frame, render a `<pre>` character grid. `pub` so the L5 app-host
/// (`crate::app_host`) renders the same program display inside its iframe —
/// the identical program-blind, shape-bound driver, one code path.
pub fn text_driver(peers: &Peers, peer_id: &str, ns: &str, port_path: &str) -> Element {
    let path = qualify(ns, port_path);
    match peers.get_entity(peer_id, &path) {
        Some(entity) => match TextFrame::decode(&entity) {
            Ok(frame) => {
                let pre = util::create_element("pre");
                pre.set_attribute("style", theme::PRE_OUTPUT).ok();
                pre.set_attribute("data-program-display", "text").ok();
                util::set_text(&pre, &frame.lines().join("\n"));
                pre
            }
            Err(e) => components::error(&e),
        },
        None => components::loading(&t("programs.display_waiting", &[])),
    }
}

/// The SVG namespace — `display-list` renders as inline SVG (a DOM-native
/// vector surface; the repo's DOM-only rule is about the window shell, and SVG
/// *is* DOM). Not a canvas path.
const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// The `display-list` shape driver: read the port entity, decode the closed
/// quads, render an inline `<svg>` of `<polygon>`s in a world-sized viewBox.
/// **Program-blind**, exactly as the workbench `VectorControl`: a `kind` is a
/// colour index, never an object type. `bounds` is the world square
/// (`scene.bounds`); the SVG viewport clips to it. `pub` so the L5 app-host
/// renders the identical driver behind the iframe boundary — one code path.
///
/// `scene.wrap` (torus seam-tiling) is intentionally NOT applied here yet: a
/// seam-crossing actor renders once, not tiled. An honest visual simplification
/// of the first browser display-list driver, not a decode gap (the reference
/// tiles; noted for the follow-up).
pub fn display_list_driver(
    peers: &Peers,
    peer_id: &str,
    ns: &str,
    port_path: &str,
    bounds: u64,
) -> Element {
    let path = qualify(ns, port_path);
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return components::error("no document"); // i18n-ignore — diagnostic fault reason (should never occur), same prose class as the host's Err strings
    };
    match peers.get_entity(peer_id, &path) {
        Some(entity) => match DisplayList::decode(&entity) {
            Ok(dl) => build_display_list_svg(&document, &dl, bounds),
            Err(e) => components::error(&e),
        },
        None => components::loading(&t("programs.display_waiting", &[])),
    }
}

fn build_display_list_svg(document: &web_sys::Document, dl: &DisplayList, bounds: u64) -> Element {
    let make = |name: &str| document.create_element_ns(Some(SVG_NS), name);
    let Ok(svg) = make("svg") else {
        return components::error("display-list: svg unavailable"); // i18n-ignore — diagnostic fault reason (should never occur), same prose class as the host's Err strings
    };
    let b = bounds.max(1);
    svg.set_attribute("viewBox", &format!("0 0 {b} {b}")).ok(); // i18n-ignore — SVG viewBox geometry, not UI prose
    svg.set_attribute("data-program-display", "display-list").ok();
    svg.set_attribute("data-actor-count", &dl.quads.len().to_string()).ok();
    svg.set_attribute(
        "style",
        // Colors as var(--token, #literal) per REFERENCE-THEMING.
        "width:100%;max-width:420px;aspect-ratio:1/1;height:auto;display:block;\
         background:var(--program-display-bg, #0a0c10);border-radius:6px",
    )
    .ok();
    // Kind tags are colour indices (the workbench pen palette, tokenised).
    const PENS: [&str; 4] = [
        "var(--program-kind-0, #8cdcff)",
        "var(--program-kind-1, #c8c8d2)",
        "var(--program-kind-2, #ffd278)",
        "var(--program-kind-3, #ff788c)",
    ];
    for (kind, pts) in &dl.quads {
        let Ok(poly) = make("polygon") else { continue };
        let points = pts
            .iter()
            .map(|(x, y)| format!("{x},{y}"))
            .collect::<Vec<_>>()
            .join(" ");
        poly.set_attribute("points", &points).ok();
        poly.set_attribute("fill", "none").ok();
        poly.set_attribute("stroke", PENS[(*kind as usize) % PENS.len()]).ok();
        // Screen-space stroke — 1 world unit at this zoom would vanish.
        poly.set_attribute("stroke-width", "1.5").ok();
        poly.set_attribute("vector-effect", "non-scaling-stroke").ok();
        svg.append_child(&poly).ok();
    }
    svg
}

/// Program key → display label. The key is the identity; a capitalized
/// key is enough for the POC roster (program display names are not yet
/// part of the descriptor contract).
fn title_case(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// `performance.now()` — the host clock's time source.
pub fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

/// Timer sleep via `setTimeout` (no `Closure` bookkeeping — the promise
/// resolve function keeps itself alive).
pub async fn sleep_ms(ms: i32) {
    use wasm_bindgen::JsCast;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(win) = web_sys::window() {
            let _ = win
                .set_timeout_with_callback_and_timeout_and_arguments_0(resolve.unchecked_ref(), ms);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Race a host future against a timeout. A pipeline stage that never
/// resolves (a lost worker round-trip) becomes a loud fault instead of
/// a frozen-but-green clock.
pub async fn with_timeout(
    fut: crate::program_host::host::HostFuture<()>,
    ms: i32,
) -> Result<(), String> {
    use std::future::Future;
    use std::task::Poll;
    let mut fut = fut;
    let mut timer = Box::pin(sleep_ms(ms));
    std::future::poll_fn(move |cx| match fut.as_mut().poll(cx) {
        Poll::Ready(r) => Poll::Ready(r),
        Poll::Pending => match timer.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(format!(
                "tick timed out after {ms} ms (pipeline stage hung — see log)" // i18n-ignore — diagnostic fault reason, same prose class as the host's Err strings
            ))),
            Poll::Pending => Poll::Pending,
        },
    })
    .await
}
