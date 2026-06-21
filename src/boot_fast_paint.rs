//! Phase-1 fast paint (boot-closure cut 2c).
//!
//! Paint the configured **remote home site** into `#site-layer` over plain
//! HTTP *before* the local peer is ready — so a content-site deployment shows
//! its startup page immediately instead of empty chrome while the boot worker
//! spawns and replays OPFS (the boot long pole). "The startup
//! page is the product; the peer is the engine."
//!
//! **It's Rust, not JS, and that is the safety guarantee.** This reuses the
//! exact [`crate::dom::content_site::render`] path the live overlay uses, fed
//! by the same [`output_from_resolved`](crate::views::content_site::model::output_from_resolved)
//! builder — so the static paint and the live page are *identical*. The peer
//! comes up underneath and re-renders the same page from the same code; there
//! is no flash of a different version.
//!
//! Disciplines:
//! * **D13 (frame integrity):** a `spawn_local` independent of the not-yet-armed
//!   rAF loop; it cannot stall or be stalled by the frame loop.
//! * **D16 (durability honesty):** read-only HTTP, no durability claim, no tree
//!   write. Any fetch / decode failure is a **silent no-op** — the normal boot
//!   proceeds and the live overlay handles the home (or shows its own honest
//!   error). It never blocks boot.
//! * **Read-only:** no live actions are wired, so a click before the peer is up
//!   is inert — the fast-click edge can't break anything.
//! * **D9 (accounting):** the render's listener closures live in a thread-local
//!   held for the app's lifetime — one page's worth, bounded and chosen. The
//!   live overlay clears `#site-layer` on its first active frame, orphaning
//!   them (freed at exit); cut 2a already points the overlay at the same home
//!   on a fresh-tree boot, so the handoff lands on the same page.
//!
//! Configurable kill switch (pre-peer-readable, since the durable tree config
//! isn't available before the peer exists): `?fastpaint=0` URL override or
//! localStorage `entity_fast_paint="0"`. The durable Settings toggle (cut 2c
//! fast-follow) writes the localStorage mirror this reads
//! ([`write_enabled_mirror`]); the per-domain deployment config can also opt
//! out with `fast_paint: false` (honored once fetched, see [`maybe_paint_home`]).

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;

use crate::content_site::http_poll::{resolve_closure_via, FetchBinSource};
use crate::content_site::Location;
use crate::dom::content_site::{render, SiteNavHost};
use crate::dom::util::{self, DomCtx};
use crate::window::{new_closure_vec, ClosureVec, RepaintFn};

/// DomCtx window id for the pre-peer paint — shares the overlay's id (0); it
/// only namespaces the (unused) input drafts and is replaced when the live
/// overlay takes over.
const FAST_PAINT_WINDOW_ID: crate::window::WindowId = 0;

/// Hard off-switch for the pre-peer fast-paint while the site surface is
/// consolidated onto a single owner (the live `SiteOverlay`). See
/// [`maybe_paint_home`] for the rationale. Flip to `false` to restore the
/// pre-peer paint once the overlay owns `#site-layer` exclusively.
const DISABLED_FOR_CONSOLIDATION: bool = true;

thread_local! {
    /// Holds the listener closures of the painted page alive until the live
    /// overlay rebuilds `#site-layer`. One page's worth — bounded, chosen (D9).
    static FAST_PAINT_CLOSURES: RefCell<Option<ClosureVec>> = const { RefCell::new(None) };
}

/// Fire-and-forget entry, called from `start()` **before** awaiting the boot
/// worker. Fetches the per-domain deployment config (cut 2b), and if it (or the
/// build-time `ENTITY_HOME_*` fallback) names a remote home on a boots-into-site
/// deployment, spawns the async paint. No-op otherwise. Never awaits, never
/// blocks boot.
///
/// The deployment-config fetch is what makes this work for a **generic bundle**:
/// a `Full`-built WASM served on a domain whose `/entity-deployment.json` says
/// `strict-site` + a remote home will fast-paint that home — no per-domain
/// rebuild. With no config served, it falls back to the build-time env defaults
/// (cut 2a), so a baked `ENTITY_HOME_*` build still fast-paints.
pub fn maybe_paint_home() {
    // DISABLED for the site-surface consolidation arc. Fast-paint is a SECOND writer of
    // `#site-layer` + the `mode-site` class, out-of-band from the live
    // `SiteOverlay`/`apply_site_mode` state machine — the "two modes fighting"
    // the user reported. While we consolidate on ONE owner of the site surface,
    // the pre-peer paint is off; the live overlay still paints the home one boot
    // beat later (no pre-peer flash-of-connecting). All the kill-switch /
    // deployment-config / Settings-toggle plumbing below is preserved and
    // testable — re-enabling is flipping `DISABLED_FOR_CONSOLIDATION` to false.
    // Do NOT re-enable until the overlay is the sole `#site-layer` owner and
    // fast-paint feeds it (stash the prefetched output) rather than writing the
    // `mode-site` class itself.
    if DISABLED_FOR_CONSOLIDATION {
        tracing::debug!("fast-paint: disabled (site-surface consolidation; HANDOFF-SITE-SURFACE-AUDIT §5)");
        return;
    }
    if !enabled() {
        tracing::debug!("fast-paint: disabled (kill switch)");
        return;
    }
    // The kill-switch is cheap + pre-fetch; everything else needs the config.
    wasm_bindgen_futures::spawn_local(async move {
        let deployment = crate::deployment_config::fetch().await;

        // A deployment config that explicitly sets `fast_paint: false` suppresses
        // the pre-paint from the FIRST visit — `enabled()` (localStorage/URL,
        // read pre-fetch) only sees the durable mirror, which isn't written until
        // boot_load runs, so without this a config's opt-out wouldn't take effect
        // until the second boot. The fetched config is the authority for this
        // domain, so honor it now.
        if deployment.as_ref().and_then(|d| d.fast_paint) == Some(false) {
            tracing::debug!("fast-paint: deployment config opts out (fast_paint=false) — skipping");
            return;
        }

        // Only when the EFFECTIVE deployment posture lands on the site — a
        // chrome-first (Full) deployment shows chrome immediately, so painting
        // the site then flipping to chrome on the first frame would just flash.
        // `resolve_boots_into_site` consults the fetched config's profile, else
        // the build default.
        if !crate::deployment_config::resolve_boots_into_site(deployment.as_ref()) {
            tracing::debug!("fast-paint: deployment does not boot into the site — skipping");
            return;
        }
        let home = crate::deployment_config::resolve_home_site(deployment.as_ref());
        // A *remote* home (real content deployment) carries a hosting peer; a
        // local home (empty peer = system) is seeded instantly, no fast paint.
        if home.peer_id.is_empty() {
            tracing::debug!("fast-paint: home is local — skipping");
            return;
        }
        // Origin: the config's `origins[home_peer]`, else `ENTITY_HOME_ORIGIN`;
        // `""`/`self` expands to this SPA's own origin (same-origin CDN case).
        let origin = match crate::deployment_config::resolve_home_origin(
            deployment.as_ref(),
            &home.peer_id,
        )
        .and_then(|o| crate::deployment_config::expand_origin(&o))
        {
            Some(o) => o,
            None => {
                tracing::debug!("fast-paint: no home origin configured — skipping");
                return;
            }
        };
        let loc = Location {
            peer_id: Some(home.peer_id.clone()),
            site_id: home.id.clone(),
            page: home.loc.clone(),
        };
        tracing::info!(
            origin = %origin,
            site = %home.id,
            page = %home.loc,
            "fast-paint: spawning Phase-1 home paint"
        );
        paint(loc, origin).await;
    });
}

/// Mirror the durable `fast_paint` config to localStorage so the **pre-peer**
/// boot path can honor it (the durable tree config isn't readable before the
/// peer exists). Called by the settings surface on toggle (so the next reload
/// sees it immediately) and by `boot_load` each boot (self-heal). Writes
/// `"1"`/`"0"`; [`enabled`] reads it.
pub fn write_enabled_mirror(enabled: bool) {
    if let Some(Ok(Some(storage))) = web_sys::window().map(|w| w.local_storage()) {
        let _ = storage.set_item("entity_fast_paint", if enabled { "1" } else { "0" });
    }
}

/// The kill switch — readable *before* the peer exists. `?fastpaint=0` /
/// localStorage `entity_fast_paint="0"` disable; default on.
fn enabled() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    // URL override wins (dev / ops immediate switch).
    if let Ok(search) = window.location().search() {
        for pair in search.trim_start_matches('?').split('&') {
            let mut parts = pair.splitn(2, '=');
            if parts.next() == Some("fastpaint") {
                return !matches!(parts.next(), Some("0") | Some("false"));
            }
        }
    }
    // Persisted toggle mirror (a durable Settings control writes this).
    if let Ok(Some(storage)) = window.local_storage() {
        if let Ok(Some(v)) = storage.get_item("entity_fast_paint") {
            return v != "0" && v != "false";
        }
    }
    true
}

/// Fetch the home closure over HTTP-poll and render it read-only into
/// `#site-layer`. Any failure is a logged no-op (safe fallback).
async fn paint(loc: Location, origin: String) {
    let rp = match resolve_closure_via(&FetchBinSource, &origin, &loc).await {
        Ok(rp) => rp,
        Err(e) => {
            tracing::debug!(error = ?e, "fast-paint: home closure fetch failed — leaving boot to handle it");
            return;
        }
    };
    let Some(layer) = util::get_element_by_id("site-layer") else {
        return;
    };
    // Build the SAME output the live overlay would (no sidebar — no peer yet).
    let output =
        crate::views::content_site::model::output_from_resolved(&rp, &loc, false, vec![]);

    // Fresh closure holder for this paint; held in the thread-local so the
    // listeners outlive this future until the overlay rebuilds the layer.
    let closures: ClosureVec = new_closure_vec();
    let noop_repaint: RepaintFn = Rc::new(|| {});
    let ctx = DomCtx {
        window_id: FAST_PAINT_WINDOW_ID,
        // Throwaway sink: clicks before the peer is up push here and are never
        // drained (read-only). The live overlay rewires real handlers.
        actions: Rc::new(RefCell::new(Vec::new())),
        repaint: noop_repaint,
        closures: closures.clone(),
        drafts: Rc::new(RefCell::new(std::collections::HashMap::new())),
    };
    // `render` clears the container first; show the site surface now that we
    // have content (only on success — a failed fetch leaves chrome as-is).
    crate::dom::util::set_container_mode("mode-site");
    // Read-only pre-peer paint: no live actions are wired, so no exit control
    // (the live overlay re-renders with the real `can_exit`). No peer exists
    // yet, so embeds can't resolve from the store — pass a no-op resolver;
    // images fill in when the live overlay takes over.
    let no_assets = |_: &str| -> Option<(String, Vec<u8>)> { None };
    render(&layer, &output, &ctx, SiteNavHost::Overlay { can_exit: false }, &no_assets);
    append_connecting_hint(&layer);
    FAST_PAINT_CLOSURES.with(|c| *c.borrow_mut() = Some(closures));
    tracing::info!(site = %loc.site_id, "fast-paint: painted remote home (read-only, pre-peer)");
}

/// A subtle, no-JS "connecting…" affordance so the read-only paint reads as
/// "live app loading," not a finished static page. Cleared when the live
/// overlay rebuilds `#site-layer`.
fn append_connecting_hint(layer: &web_sys::Element) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let Ok(hint) = doc.create_element("div") else {
        return;
    };
    hint.set_text_content(Some("connecting…"));
    if let Some(el) = hint.dyn_ref::<web_sys::HtmlElement>() {
        let _ = el.set_attribute(
            "style",
            "position:fixed;bottom:10px;right:12px;padding:4px 10px;border-radius:10px;\
             font:12px system-ui,sans-serif;color:#bbb;background:rgba(0,0,0,.55);\
             pointer-events:none;z-index:9",
        );
    }
    let _ = layer.append_child(&hint);
}
