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
use crate::program_host::shapes::TextFrame;
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
        MountStatus::Stopped => t("programs.status_stopped", &[]),
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
            util::append(&row, &event_btn("programs.start", ButtonKind::Primary, START_EVENT));
            util::append(&row, &event_btn("programs.restart", ButtonKind::Small, RESTART_EVENT));
        }
        MountStatus::Running => {
            util::append(&row, &event_btn("programs.stop", ButtonKind::Primary, STOP_EVENT));
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
/// text-frame, render a `<pre>` character grid.
fn text_driver(peers: &Peers, peer_id: &str, ns: &str, port_path: &str) -> Element {
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
