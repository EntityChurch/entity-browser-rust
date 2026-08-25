//! Program shape drivers + the host clock's timing helpers.
//!
//! The display drivers bind by a port's declared **shape** and are
//! program-blind — they never inspect program state. `text` renders an
//! `app/shape/text-frame` into a `<pre>` character grid (the teletype lineage —
//! no new rendering tech); `display-list` renders closed quads as inline SVG
//! `<polygon>`s (a DOM-native vector surface — the DOM-only rule is about the
//! window shell, and SVG *is* DOM; not a canvas path).
//!
//! These are `pub` so the L5 app-host ([`crate::app_host`]) renders the exact
//! same drivers inside its sandboxed iframe — one code path for both surfaces.
//! The Programs window itself is now a launcher ([`crate::views::programs`])
//! that runs programs behind the L5 boundary, so it no longer renders cards
//! here — only the drivers below survive, shared with the app-host.

use web_sys::Element;

use crate::dom::components;
use crate::dom::theme;
use crate::dom::util;
use crate::i18n::t;
use crate::peers::Peers;
use crate::program_host::host::qualify;
use crate::program_host::shapes::{DisplayList, TextFrame};

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
///
/// `fill` is the program's declared [`scene.render`] intent
/// (RESPONSE-DISPLAY-RENDERING-AND-TEXT-REBIND): `fill` paints solid coloured
/// quads (grids — Life/Snake), the default `stroke` draws coloured wireframe
/// (vector games — Asteroids). The host does not guess it; the manifest declares
/// it (the same "declare presentation, don't infer it" lesson as the input roles).
///
/// [`scene.render`]: crate::program_host::descriptor
pub fn display_list_driver(
    peers: &Peers,
    peer_id: &str,
    ns: &str,
    port_path: &str,
    bounds: u64,
    fill: bool,
) -> Element {
    let path = qualify(ns, port_path);
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return components::error("no document"); // i18n-ignore — diagnostic fault reason (should never occur), same prose class as the host's Err strings
    };
    match peers.get_entity(peer_id, &path) {
        Some(entity) => match DisplayList::decode(&entity) {
            Ok(dl) => build_display_list_svg(&document, &dl, bounds, fill),
            Err(e) => components::error(&e),
        },
        None => components::loading(&t("programs.display_waiting", &[])),
    }
}

fn build_display_list_svg(
    document: &web_sys::Document,
    dl: &DisplayList,
    bounds: u64,
    fill: bool,
) -> Element {
    let make = |name: &str| document.create_element_ns(Some(SVG_NS), name);
    let Ok(svg) = make("svg") else {
        return components::error("display-list: svg unavailable"); // i18n-ignore — diagnostic fault reason (should never occur), same prose class as the host's Err strings
    };
    let b = bounds.max(1);
    svg.set_attribute("viewBox", &format!("0 0 {b} {b}")).ok(); // i18n-ignore — SVG viewBox geometry, not UI prose
    svg.set_attribute("data-program-display", "display-list").ok();
    svg.set_attribute(
        "style",
        // Colors as var(--token, #literal) per REFERENCE-THEMING.
        "width:100%;max-width:420px;aspect-ratio:1/1;height:auto;display:block;\
         background:var(--program-display-bg, #0a0c10);border-radius:6px",
    )
    .ok();
    // Kind tags are colour indices (the workbench pen palette, tokenised).
    // Kind 0 is BACKGROUND — the contract reserves it as "nothing here" and the
    // host MUST NOT draw it. The grid projections are DENSE (a quad per cell,
    // empties carried as kind 0), so skipping it is required, not an optimisation
    // (drawing them paints the whole board). Asteroids is sparse and never emits
    // kind 0, so the rule costs it nothing.
    const PENS: [&str; 4] = [
        "var(--program-kind-0, #8cdcff)",
        "var(--program-kind-1, #c8c8d2)",
        "var(--program-kind-2, #ffd278)",
        "var(--program-kind-3, #ff788c)",
    ];
    let mut drawn = 0usize;
    for (kind, pts) in &dl.quads {
        if *kind == 0 {
            continue; // background — never drawn (display-list presentation contract)
        }
        let Ok(poly) = make("polygon") else { continue };
        let points = pts
            .iter()
            .map(|(x, y)| format!("{x},{y}"))
            .collect::<Vec<_>>()
            .join(" ");
        poly.set_attribute("points", &points).ok();
        let pen = PENS[(*kind as usize) % PENS.len()];
        if fill {
            // Solid coloured cell (grids).
            poly.set_attribute("fill", pen).ok();
            poly.set_attribute("stroke", "none").ok();
        } else {
            // Coloured wireframe (vector games). Screen-space stroke — 1 world
            // unit at this zoom would vanish.
            poly.set_attribute("fill", "none").ok();
            poly.set_attribute("stroke", pen).ok();
            poly.set_attribute("stroke-width", "1.5").ok();
            poly.set_attribute("vector-effect", "non-scaling-stroke").ok();
        }
        svg.append_child(&poly).ok();
        drawn += 1;
    }
    // The count of actually-painted quads (background-skipped) — the honest
    // "what got drawn", which the e2e reads.
    svg.set_attribute("data-actor-count", &drawn.to_string()).ok();
    svg
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
