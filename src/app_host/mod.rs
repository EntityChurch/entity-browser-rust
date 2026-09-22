//! `app_host` — the stripped-startup "L5 app" boot mode.
//!
//! This is `entity-browser-rust` booting in a *different startup mode* (upstream
//! EXPLORATION-L5-APP-HOSTING §3b): when the page is loaded with `?app-host=<program>`
//! (see [`crate::main`]'s early boot branch), we do NOT build the window-manager,
//! the peer roster, or any durable storage. We boot **minimal** — just enough to
//! run one compute program and speak the app side of the entity-apps ③α contract
//! back to whichever host embeds us.
//!
//! The embedding is a sandboxed iframe inside the Apps window
//! (`dom::games::render_player` with `Delivery::Src`), so the compute mount runs
//! *behind the boundary*, never on the system peer (the reframe's §6-step-4
//! correction, review §3). The L5 sandbox is `allow-scripts allow-same-origin`:
//! the payload is OUR trusted stripped runtime and needs same-origin to load its
//! own wasm without opaque-origin CORS/CSP friction; its inner peer is memory-only
//! (no IndexedDB), so nothing durable is exposed. Opaque-origin isolation returns
//! for *untrusted* L5 apps behind the future sub-peer capability model (D21).
//!
//! ## The ③α contract we implement (app side)
//! The host side lives in `dom::games::render_player`. We are its counterpart:
//! - post `{source:"entity-app", type:"ready-for-init"}` once the payload is up;
//! - handle `{source:"entity-host", type:"init", state, locale, dir}` (+ `viewport`);
//! - post `{source:"entity-app", type:"state", state:<obj>}` as the program advances;
//!   the host holds the latest, debounces ~1 s, and persists it verbatim (P1 — the
//!   host is blind to the payload).
//!
//! ## Programs
//! - `ping` — the delivery smoke: boot, render a marker, run the ③α client, emit a
//!   small incrementing `state`. Proves the iframe delivery + handshake in isolation.
//! - any embedded program key (`life`, …) — [`run_program`]: build a lean ephemeral
//!   peer, mount the program through `program_host`, render its display shape, and
//!   drive the tick clock, emitting each evolved state to the host. Life is
//!   pure-builtin (no upstream wasm32 stub). Verified in browser + Tauri.

#![cfg(target_arch = "wasm32")]

mod debug;
mod input;
mod onscreen;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::MessageEvent;

use crate::dom::programs::{display_list_driver, now_ms, sleep_ms, text_driver, with_timeout};
use crate::peers::Peers;
use crate::program_host::bundle::{digest_hex, Bundle, EMBEDDED_PROGRAMS};
use crate::program_host::descriptor::{
    scene_text, scene_u64, ProgramDescriptor, SHAPE_DISPLAY_LIST, SHAPE_TEXT,
};
use crate::program_host::host;
use crate::program_host::shapes;

/// The display shapes this host drives. Combined with
/// [`input::SUPPORTED_INPUT_SHAPES`] to form the admission capability set — a
/// program binding a shape outside the union is refused with the shape named
/// (never mounted-then-mute). `text` (Life/Snake) + `display-list` (Asteroids)
/// display; `direction`/`key-set` input.
const SUPPORTED_DISPLAY_SHAPES: &[&str] = &[SHAPE_TEXT, SHAPE_DISPLAY_LIST];

/// The message envelope's `source` value the host filters on (both directions
/// carry a `source`; ours is always this). Mirrors `dom::games`' host loop.
const APP_SOURCE: &str = "entity-app";
const HOST_SOURCE: &str = "entity-host";

thread_local! {
    /// The `message` listener + any interval closure, held for the payload's
    /// whole lifetime. The iframe document *is* the lifetime — there is no
    /// rebuild here — so owning them here (never `Closure::forget()`, charter
    /// D12 / AP1) is the correct drop path: they die with the document.
    static LIVE: RefCell<Vec<Closure<dyn FnMut(JsValue)>>> =
        const { RefCell::new(Vec::new()) };
}

/// Boot the stripped app-host for the named program and return once the ③α client
/// is armed. Any later work (state emission) is driven by callbacks the client
/// installs. Errors bubble to the caller (`main::start`), which logs them.
pub async fn run(program: &str) -> Result<(), JsValue> {
    tracing::info!(program, "app-host: stripped boot");

    // Localize the payload from FRAME ONE. An L5 app-host runs same-origin
    // (D21 — `allow-same-origin`), so it shares the parent's `localStorage`:
    // `boot_choice()` reads the same persisted locale the outer app uses (else
    // `navigator.language`), and `apply` drives `lang`/`dir` onto this
    // document's `<html>` (RTL flips) + sets the active catalog that `t()`
    // resolves against. Previously the payload dropped the host's `init`
    // locale/dir entirely, so it always rendered `en`/`ltr`
    // (`AUDIT-L5-COMPUTE-HOST-FOUNDATION-2026-08-01` #4). The host still sends
    // `init{locale,dir}` over ③α; honoring an explicit host override on top of
    // the shared-storage default is a noted follow-up.
    crate::i18n::apply(&crate::i18n::boot_choice());

    // Internal `JsValue` errors below bubble to the console, never rendered as
    // UI prose (the "no window/document" impossible-boot class) — hence i18n-ignore.
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("app-host: no window"))?; // i18n-ignore
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("app-host: no document"))?; // i18n-ignore
    let body = document
        .body()
        .ok_or_else(|| JsValue::from_str("app-host: no body"))?; // i18n-ignore

    // Minimal payload UI. Colors reference theme tokens by `var(--token, #lit)`
    // so they resolve from the CSS fallbacks even though this stripped mode never
    // installs the `:root` token block (theme_tokens::install_root).
    body.set_inner_html("");
    let root = document.create_element("div")?;
    root.set_attribute("data-app-host", program)?;
    root.set_attribute(
        "style",
        "box-sizing:border-box;height:100%;margin:0;padding:14px;\
         font-family:system-ui,-apple-system,sans-serif;font-size:14px;\
         color:var(--text, #e2e2ea);background:var(--bg, #101018);",
    )?;
    body.append_child(&root)?;

    match program {
        // The delivery smoke: prove a sandboxed iframe can boot this wasm, run
        // scripts, and round-trip `state` to the outer host.
        "ping" => run_ping(&root)?,
        // A real compute program (Life, …): a lean ephemeral peer runs the
        // generic host behind the boundary and emits its state.
        key if EMBEDDED_PROGRAMS.iter().any(|p| p.key == key) => run_program(&root, key).await?,
        other => {
            root.set_text_content(Some(&crate::i18n::t(
                "apphost.cannot_run",
                &[("program", other), ("reason", "unknown program")], // i18n-ignore — diagnostic {reason} slot
            )));
            tracing::warn!(program = other, "app-host: unknown program");
        }
    }
    Ok(())
}

/// Render a visible boot-failure caption into the payload root, then return the
/// error. A pre-run failure (bundle parse/verify, materialize, descriptor
/// decode, seed) must never leave a **blank** iframe — every failure has a
/// surface (D13, `AUDIT-L5-COMPUTE-HOST-FOUNDATION-2026-08-01` #6). Reachable
/// only before the UI rebuild below, so the caption replaces the boot splash;
/// `reason` is the diagnostic stage detail, the frame is what's translated.
/// Read one `?key=value` query parameter from the document URL (the manual
/// `location().search()` parse the rest of the app uses — `main.rs`'s
/// `log_level_from_url` shape — so no new `web-sys` feature).
fn query_param(key: &str) -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    for pair in search.trim_start_matches('?').split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some(key) {
            return Some(parts.next().unwrap_or("").to_string());
        }
    }
    None
}

fn boot_fail(root: &web_sys::Element, program_key: &str, reason: String) -> JsValue {
    root.set_text_content(Some(&crate::i18n::t(
        "apphost.cannot_run",
        &[("program", program_key), ("reason", &reason)],
    )));
    tracing::error!(program = program_key, "app-host: boot failed — {reason}");
    JsValue::from_str(&reason)
}

/// Boot a compute program as an L5 payload: build a **lean ephemeral peer**
/// (`Peers::new_direct()` — no roster, no durable storage), mount the embedded
/// program through the generic host (materialize → seed), render its display
/// shape into the iframe, and drive the tick clock — emitting each evolved state
/// to the outer host over ③α (P1: the host persists it, blind to what it is).
///
/// This is the reframe's §6-step-4 correction made concrete: the *same* validated
/// generic host that `ProgramsWindow` runs in the primary peer now runs inside the
/// sandboxed inner peer, behind the boundary. Life is pure-builtin, so it dodges
/// both upstream wasm32 stubs (B1/B2) — zero upstream dependency.
async fn run_program(root: &web_sys::Element, program_key: &str) -> Result<(), JsValue> {
    let embedded = EMBEDDED_PROGRAMS
        .iter()
        .find(|p| p.key == program_key)
        .ok_or_else(|| JsValue::from_str("app-host: program not embedded"))?; // i18n-ignore — internal invariant
    let bundle = Bundle::parse(embedded.json)
        .map_err(|e| boot_fail(root, program_key, format!("bundle parse: {e}")))?;
    // Cross-impl hash gate (same as the oracle/window): refuse a bundle whose
    // entities don't recompute identically under the Rust encoder.
    let entities = bundle
        .verified_entities()
        .map_err(|e| boot_fail(root, program_key, format!("bundle verify: {e}")))?;

    // Shared: the tick loop (moved into the 'static future) and the input
    // driver closures (held for the document's lifetime) both hold the peer.
    let peers = Rc::new(Peers::new_direct());
    let peer_id = peers.primary_peer_id().to_string();
    // The IR's internal lookups are origin-qualified, so the program must live
    // under its authoring namespace in the local store (host::qualify).
    let ns = bundle.origin_peer.clone();

    // D13: a long materialize (Asteroids is ~441 entities, ~15 s on the Worker
    // arm) shows a live count instead of a blank pad — the `on_progress`
    // callback the host exposes was previously discarded (`|_, _| {}`). Reuses
    // the Programs window's `status_materializing` string. Replaced by the real
    // UI once materialize completes (the `set_inner_html("")` below).
    let progress_root = root.clone();
    host::materialize_future(&peers, &peer_id, &ns, entities, move |done, total| {
        progress_root.set_text_content(Some(&crate::i18n::t(
            "programs.status_materializing",
            &[("done", &done.to_string()), ("total", &total.to_string())],
        )));
    })
    .await
    .map_err(|e| boot_fail(root, program_key, format!("materialize: {e}")))?;
    let desc_ent = peers
        .get_entity(&peer_id, &host::qualify(&ns, &bundle.descriptor_path))
        .ok_or_else(|| boot_fail(root, program_key, "descriptor unreadable after materialize".to_string()))?; // i18n-ignore — diagnostic {reason} slot
    let desc = ProgramDescriptor::decode(&desc_ent)
        .map_err(|e| boot_fail(root, program_key, format!("descriptor: {e}")))?;
    // Admit by shape capability — the union of the display and input shapes this
    // host drives (the same fail-closed check the Programs window applies).
    let supported: Vec<&str> = SUPPORTED_DISPLAY_SHAPES
        .iter()
        .chain(input::SUPPORTED_INPUT_SHAPES)
        .copied()
        .collect();
    // Fail-closed with a VISIBLE surface: a program binding a shape this host
    // doesn't drive (or declaring capability imports) renders its refusal into
    // the payload rather than bubbling to a blank iframe (D13 — every state has
    // a surface). The Programs launcher shows all built-in programs; the honest
    // "cannot run here" lives here, at the boundary, where admission is enforced.
    if let Err(e) = desc.admit(&supported) {
        // Expected for a program binding a shape this host doesn't drive — a
        // visible refusal, not a blank iframe (D13). `warn`, not `error`: an
        // unsupported program is a legitimate outcome, not a host fault.
        root.set_text_content(Some(&crate::i18n::t(
            "apphost.cannot_run",
            &[("program", program_key), ("reason", &e.to_string())],
        )));
        tracing::warn!(program = program_key, "app-host: admission refused: {e}");
        return Err(JsValue::from_str(&e.to_string()));
    }
    host::seed_future(&peers, &peer_id, &ns, &desc)
        .await
        .map_err(|e| boot_fail(root, program_key, format!("seed: {e}")))?;

    // Rebuild the payload as one centred column: [meta-chrome bar] → [board] →
    // [status caption], with the thumb pad (input only) overlaid on top. The
    // settings-y chrome (reset/pause + 🎮/⇄) no longer floats over the play area.
    root.set_inner_html("");
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsValue::from_str("app-host: no document"))?; // i18n-ignore — internal invariant

    // Inject the shared control stylesheet ONCE. It styles the chrome bar, the
    // board centring, the status caption, AND (when a program has input) the
    // on-screen pad — the chrome exists for EVERY program (even input-less Life),
    // so the injection is unconditional, not gated on an input port.
    let style = document.create_element("style")?;
    style.set_text_content(Some(&format!("{}{}", onscreen::CONTROLS_CSS, debug::DEBUG_CSS)));
    root.append_child(&style)?;

    // The meta-chrome bar (`.ah-chrome`) — the slim normal-flow row ABOVE the
    // board that holds the settings-y controls so they no longer overlap it:
    // host run-state controls (reset/pause) at one end, the input chips (🎮/⇄)
    // at the other (`space-between`). Built first so both clusters can mount into
    // it; the input loop below appends its chips here.
    let chrome = document.create_element("div")?;
    chrome.set_attribute("class", "ah-chrome")?;

    // The generic host run-state controls (reset ↻ / pause ⏸) — program-blind
    // affordances (reseed-to-state₀ + clock-gating need no program knowledge),
    // so every program gets them. The tick loop reads the shared flags. They sit
    // at the leading end of the chrome bar.
    let host_controls = build_host_controls();
    chrome.append_child(&host_controls.bar)?;
    let paused = host_controls.paused.clone();
    let reset_req = host_controls.reset.clone();
    let step_req = host_controls.step.clone();
    LIVE.with(|v| v.borrow_mut().extend(host_controls.closures));
    root.append_child(&chrome)?;

    // The display surface the tick loop re-renders into (the program's display
    // shape). Centred under the chrome bar, sharing its `max-width` so the two
    // line up as one column.
    let display = document.create_element("div")?;
    display.set_attribute("data-app-host-display", program_key)?;
    root.append_child(&display)?;

    // The program-owned status caption (a one-line score/state readout below the
    // board), if the program declares a `status` port. The host relays it blind
    // via the same `text_driver`; it never formats a score
    // (RESPONSE-PROGRAM-CHROME-STATUS-AND-RESET). A program with nothing to
    // report omits the port and gets no caption.
    let status_el = if desc.status_port().is_some() {
        let el = document.create_element("div")?;
        el.set_attribute("class", "ah-status")?;
        el.set_attribute("data-app-host-status", program_key)?;
        root.append_child(&el)?;
        Some(el)
    } else {
        None
    };

    // The 🐞 debug overlay (compute wiring + live values, merged) — a local
    // diagnostic surface, never crossing ③α. The chip joins the meta-chrome
    // bar; the panel sits below the status caption, hidden by default. The
    // whole panel (not just `shown`) moves into the tick-loop future below —
    // `debug::refresh` reads its private row handles directly.
    let (debug_panel, debug_closures) = debug::build(&peers, &peer_id, &desc, &ns);
    chrome.append_child(&debug_panel.chip)?;
    root.append_child(&debug_panel.panel)?;
    LIVE.with(|v| v.borrow_mut().extend(debug_closures));

    // Clock-driven rate → ms per tick (guarded against a 0 hint). Computed here
    // (not just inside the tick loop below) because the input install below
    // needs it too: a `key-set` momentary release is delayed by this same
    // period (`input::install`'s `min_hold_ms`) so a tap shorter than one tick
    // still holds long enough for the program's `step` to observe it.
    let tick_interval_ms = (1000 / desc.tick.rate_hint.max(1)).clamp(16, 1000) as i32;

    // Install the input sources — one target per declared input port, bound by
    // shape (program-blind; the field name comes from the port's SEED, not
    // assumed). Each port gets BOTH a keyboard source and an on-screen pointer
    // source (a D-pad / action buttons) driving ONE shared target. The closures
    // are held in `LIVE` for the document's lifetime (D12: no `Closure::forget`).
    // Input is captured here and written to the inner peer — it never crosses ③α
    // (the host stays blind, P1). A port with no readable seed is skipped loudly
    // (seed already validated F-E1, so this is defence in depth).
    for port in &desc.input_ports {
        let seed_path = host::qualify(&ns, &port.path);
        let Some(seed) = peers.get_entity(&peer_id, &seed_path) else {
            tracing::warn!(port = %port.name, "app-host input: seed unreadable, skipping driver");
            continue;
        };
        let Some((field, _)) = shapes::input_field_name(&seed) else {
            tracing::warn!(port = %port.name, "app-host input: seed is not a single-field numeric entity, skipping driver");
            continue;
        };
        let installed = input::install(
            peers.clone(),
            peer_id.clone(),
            ns.clone(),
            port,
            field,
            // Stamp the D13 input surface on the payload ROOT (the stable
            // `data-app-host` element) — `display`'s children are replaced every
            // tick, the root persists.
            root.clone(),
            tick_interval_ms,
        );
        // The thumb pad overlays the board (mounted at the root so its pointer
        // listeners survive every tick's display churn); the chips join the
        // meta-chrome bar (they drive the pad by a held reference, so their DOM
        // home is independent of the pad's).
        if let Some(pad) = installed.pad {
            // Reserve room below the fixed-position pad (CONTROLS_CSS) so
            // scrolled-to-bottom content (e.g. the debug panel) isn't
            // permanently hidden behind it.
            root.set_attribute("data-has-pad", "")?;
            root.append_child(&pad)?;
        }
        if let Some(chips) = installed.chips {
            chrome.append_child(&chips)?;
        }
        LIVE.with(|v| v.borrow_mut().extend(installed.closures));
    }

    // Announce readiness (host replies `init`, stamping data-host-locale). We
    // ignore the returned saved state for this first test — the program seeds
    // from its initial state, and the host persists what we emit.
    post_to_host("ready-for-init", None);

    // Test seam (query-param gated, e2e only): `&app-host-fault-tick=N` injects
    // a recoverable fault at tick N so the e2e can assert the VISIBLE fault
    // surface under its dev build — where a REAL panic would abort the wasm
    // (`catch_unwind` is release-only). The true panic→Err containment is
    // covered by the native `program_host::host::guarded` unit test. `None` in
    // any normal boot (the param is absent).
    let fault_after: Option<u64> = query_param("app-host-fault-tick")
        .and_then(|v| v.parse::<u64>().ok());
    // Owned handle to the stable payload root, moved into the 'static tick
    // future (the `root` param is a borrow) — the fault D13 surface lands here,
    // the same element the input surface uses (persists across display churn).
    let fault_root = root.clone();

    // Drive the clock off-loop. Each tick: advance the program, re-render the
    // display, and — when the state actually changed — emit it to the host.
    // `program_key` is owned into the 'static future.
    let program_key = program_key.to_string();
    spawn_local(async move {
        let state_path = host::qualify(&ns, &desc.state_path);
        // Capture the display port's (shape, path, world-bounds) once — the
        // per-tick render dispatches on shape (program-blind, shape-bound).
        let display_port = desc.display_port().map(|p| {
            (
                p.shape.clone(),
                p.path.clone(),
                scene_u64(p.scene.as_ref(), "bounds").unwrap_or(0),
                // Declared render intent: `fill` (grids) vs the default `stroke`
                // (vector games). The manifest declares it; the host never guesses.
                scene_text(p.scene.as_ref(), "render") == Some("fill"),
            )
        });
        // The program-owned status readout's path (the caption re-renders from it
        // each frame via `text_driver`, exactly like the display board).
        let status_path = desc.status_port().map(|p| p.path.clone());
        // Computed above (input install needs it too): ms per tick.
        let interval_ms = tick_interval_ms;
        let mut last_hash = String::new();
        let mut ticks: u64 = 0;
        // D13: the clock loop's health has a surface. Rolling mean of the whole
        // per-tick work (eval + render + emit), stamped every 16 ticks — if it
        // ever approaches `interval_ms` the sim is falling behind its rate, and
        // this is where you'd see it (and the e2e/operator can read it).
        let mut work_ms_accum = 0.0f64;
        // Split accumulators for the compute-vs-render breakdown (stamped with
        // the total every 16 ticks).
        let mut compute_ms_accum = 0.0f64;
        let mut render_ms_accum = 0.0f64;
        // The `x-stats` window: busy over span, on a WALL CLOCK rather than a
        // tick count. A tick count is the wrong unit here because `interval_ms`
        // is per-program — 16 ticks is a fifth of a second for Asteroids and
        // sixteen seconds for a 1 Hz program, and the System Monitor's row wants
        // the same cadence from both.
        let mut stats_window_start = now_ms();
        let mut stats_busy_ms = 0.0f64;
        // Emit the current state to the host when it differs from the last
        // emission (dedup by content hash — a climbing host-side seq proves
        // distinct evolution). Shared by the tick path AND the reset path: a
        // reseed is a state change too (back to state₀), and the host should
        // learn of it, so the persisted state stays honest and a reset is
        // observable across ③α.
        let emit_state = |last_hash: &mut String, ticks: u64| {
            if let Some(state) = peers.get_entity(&peer_id, &state_path) {
                let hash = digest_hex(&state);
                if *last_hash != hash {
                    *last_hash = hash.clone();
                    let obj = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("program"), &JsValue::from_str(&program_key));
                    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("ticks"), &JsValue::from_f64(ticks as f64));
                    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("hash"), &JsValue::from_str(&hash));
                    post_to_host("state", Some(&obj));
                }
            }
        };
        // Refresh the debug panel's live wiring values — only when it's
        // actually shown (a `tree_listing` + N `get_entity` reads is cheap on
        // the Direct arm, but there's no reason to pay it every tick for a
        // hidden panel).
        let refresh_debug = || {
            if debug_panel.shown.get() {
                debug::refresh(&debug_panel, &peers, &peer_id, &ns);
            }
        };
        loop {
            // Generic RESET (↻): reseed to state₀ — the `Host.Restart` semantics
            // (stop → reseed → resume), performed here because the loop owns the
            // peer; the button only requests it. Re-render AND re-emit immediately
            // so the reset is visible in the caption and observable to the host
            // (`data-app-state-seq` bumps) even while paused. Program-blind:
            // reseed knows nothing about the program.
            if reset_req.get() {
                reset_req.set(false);
                match host::seed_future(&peers, &peer_id, &ns, &desc).await {
                    Ok(()) => {
                        last_hash.clear(); // state is back to initial → force the reseed emit
                        render_frame(
                            &peers, &peer_id, &ns, &display, &display_port,
                            status_el.as_ref(), status_path.as_deref(),
                        );
                        emit_state(&mut last_hash, ticks);
                        refresh_debug();
                    }
                    Err(e) => tracing::error!(program = %program_key, "app-host: reset reseed: {e}"),
                }
            }

            // PAUSE (⏸): gate the clock without tearing anything down — hand the
            // shared main thread back and re-check next round (reset still works
            // while paused). STEP (⏭) is the one exception: it consumes the flag
            // unconditionally every round (so a stray click while running can't
            // cause a surprise step later after a pause) and, while paused, lets
            // exactly one tick fall through below instead of sleeping.
            // Report our share of the shared main thread — ABOVE the pause
            // branch, deliberately. A paused program is **idle**, and idle is a
            // number (`busy_ms: 0`); going silent instead would render as *not
            // reporting*, which the monitor keeps apart from idle on purpose and
            // which is the one thing a person pausing a program has just
            // disproved. Same rule as every other three-state surface here:
            // "I did no work" and "I have nothing to say" are different facts.
            let stats_now = now_ms();
            let stats_span = stats_now - stats_window_start;
            if stats_span >= STATS_INTERVAL_MS {
                post_stats_to_host(stats_busy_ms, stats_span);
                stats_window_start = stats_now;
                stats_busy_ms = 0.0;
            }

            let stepping = step_req.replace(false);
            if paused.get() && !stepping {
                sleep_ms(interval_ms).await;
                continue;
            }

            // Fixed-RATE scheduling: measure the tick's own work and sleep only
            // the REMAINDER of the interval, not a full interval on top of it.
            // Otherwise the real period is (compute + render + interval), so a
            // heavy tick silently halves the effective rate (Asteroids was
            // running ~7 Hz against a 12 Hz clock before this). The `MIN_YIELD_MS`
            // floor below is load-bearing: the compute evaluator is SYNCHRONOUS
            // and a same-origin iframe shares the PARENT's main thread, so a tick
            // that overruns its budget must still hand the thread back each round
            // or it starves the outer UI (paint + input). We hit the rate when we
            // can and yield a fixed floor when we can't — never spin at 0.
            let tick_start = now_ms();
            // A faulting tick DEGRADES to a visible caption + a stopped clock,
            // never a silent frozen board (D13/AP3, finding #1):
            // - `host::guarded` contains a PANIC in the evaluator (release
            //   profile; the native test covers the catch) → `Err`;
            // - `with_timeout` turns a HUNG tick → `Err`;
            // - the `fault_after` seam injects a recoverable `Err` for the e2e.
            // All three land here, on the one loud-log + visible-surface + break.
            let outcome = if fault_after == Some(ticks + 1) {
                host::guarded(Box::pin(async {
                    Err("injected fault (e2e test seam)".to_string()) // i18n-ignore — test seam
                }))
                .await
            } else {
                host::guarded(Box::pin(with_timeout(
                    host::tick_future(&peers, &peer_id, &ns, &desc),
                    4000,
                )))
                .await
            };
            if let Err(e) = outcome {
                tracing::error!(program = %program_key, "app-host: tick faulted: {e}");
                display.set_text_content(Some(&crate::i18n::t(
                    "apphost.stopped",
                    &[("program", &program_key), ("reason", &e)],
                )));
                // D13 observable for the e2e / operator: the fault reason, on the
                // stable payload element (not the per-tick-replaced display body).
                let _ = fault_root.set_attribute("data-app-host-fault", &e);
                break;
            }
            // Split the per-tick budget: COMPUTE (the synchronous evaluator — the
            // step + every projection port's source) vs RENDER (the DOM rebuild).
            // On a slow device this says where the frame went (and which lever to
            // pull — a faster evaluator vs cheaper rendering). D13, readable live.
            let compute_done = now_ms();
            ticks += 1;

            // Re-render the board AND the program-owned status caption
            // (program-blind, shape-bound — the identical drivers the Programs
            // window uses). Same helper the reset path calls, so a reseeded frame
            // and a ticked frame render identically.
            render_frame(
                &peers, &peer_id, &ns, &display, &display_port,
                status_el.as_ref(), status_path.as_deref(),
            );
            let render_done = now_ms();

            // Emit the evolved state to the host (dedup by content hash — a
            // growing host-side `data-app-state-seq` proves distinct evolution).
            emit_state(&mut last_hash, ticks);
            refresh_debug();

            let elapsed = render_done - tick_start;
            work_ms_accum += elapsed;
            stats_busy_ms += elapsed;
            compute_ms_accum += compute_done - tick_start;
            render_ms_accum += render_done - compute_done;
            if ticks.is_multiple_of(16) {
                let _ = display.set_attribute("data-app-host-tick-ms", &format!("{:.1}", work_ms_accum / 16.0));
                let _ = display.set_attribute("data-app-host-compute-ms", &format!("{:.1}", compute_ms_accum / 16.0));
                let _ = display.set_attribute("data-app-host-render-ms", &format!("{:.1}", render_ms_accum / 16.0));
                work_ms_accum = 0.0;
                compute_ms_accum = 0.0;
                render_ms_accum = 0.0;
            }
            // Fairness floor for the shared main thread (see the loop-head note).
            const MIN_YIELD_MS: f64 = 8.0;
            let remaining = (interval_ms as f64 - elapsed).max(MIN_YIELD_MS) as i32;
            sleep_ms(remaining).await;
        }
    });
    Ok(())
}

/// Render one frame: the display board AND (if present) the program-owned status
/// caption, each via its shape-bound, program-blind driver. Called every tick and
/// after a reset, so a reseeded and a ticked frame are byte-identical. Admission
/// already gated the display shape to the supported set, so the fallthrough is
/// defence in depth (a loud marker, never a silent blank). The status port is
/// always `text` — the same `<pre>` driver Life/Snake used before the display-list
/// rebind, now a one-line readout.
///
/// **Direct-arm assumption (undocumented before AUDIT-…-2026-08-01 #10):** the
/// drivers read the tree SYNCHRONOUSLY (`peers.get_entity`, no subscription).
/// That is correct ONLY because `app_host` always runs `Peers::new_direct()`
/// (an in-process store). If the inner peer is ever Worker-hosted (the module's
/// stated future direction), these unsubscribed reads hit the main-thread cache
/// mirror — seeded only for subscribed prefixes — and return empty, silently
/// blanking the board. A Worker-host move MUST first `WindowWatch`/`observe`
/// the program namespace here (and in `debug::refresh`) or switch to the async
/// round-trip reads.
fn render_frame(
    peers: &Peers,
    peer_id: &str,
    ns: &str,
    display: &web_sys::Element,
    display_port: &Option<(String, String, u64, bool)>,
    status_el: Option<&web_sys::Element>,
    status_path: Option<&str>,
) {
    if let Some((shape, port_path, bounds, fill)) = display_port {
        let el = match shape.as_str() {
            SHAPE_TEXT => text_driver(peers, peer_id, ns, port_path),
            SHAPE_DISPLAY_LIST => display_list_driver(peers, peer_id, ns, port_path, *bounds, *fill),
            other => {
                // Defence in depth — admission (`desc.admit`) already gated the
                // display shape to the supported set, so this is unreachable in
                // practice; a loud diagnostic marker, never a silent blank (D13).
                let el = crate::dom::util::create_element("div");
                crate::dom::util::set_text(
                    &el,
                    // i18n-ignore — diagnostic; unreachable past admission.
                    &format!("app-host: no driver for display shape {other:?}"),
                );
                el
            }
        };
        display.set_inner_html("");
        let _ = display.append_child(&el);
    }
    if let (Some(status_el), Some(status_path)) = (status_el, status_path) {
        let line = text_driver(peers, peer_id, ns, status_path);
        status_el.set_inner_html("");
        let _ = status_el.append_child(&line);
    }
}

/// What [`build_host_controls`] hands back: the control-bar element to mount, the
/// three flags the tick loop polls (`paused`, `reset`, `step`), and the pointer
/// `Closure`s to hold for the document's lifetime (D12 — never `Closure::forget`).
struct HostControls {
    bar: web_sys::Element,
    paused: Rc<Cell<bool>>,
    reset: Rc<Cell<bool>>,
    /// Debug single-step: advance exactly one tick while paused (§debug
    /// overlay). Consumed unconditionally every loop round regardless of
    /// pause state, so a stray click while running can't cause a surprise
    /// step later after the program is paused.
    step: Rc<Cell<bool>>,
    closures: Vec<Closure<dyn FnMut(JsValue)>>,
}

/// Build the generic host run-state control bar: **reset** (↻ — request a reseed
/// to state₀, the `Host.Restart` semantics), **pause/resume** (⏸ ⇄ ▶ — gate
/// the tick clock), and **step** (⏭ — advance one tick while paused, for the
/// debug overlay). These are HOST affordances, not program inputs: reseed,
/// clock-gating, and stepping need zero program knowledge, so every program
/// gets the same three, keeping the chrome internally consistent across
/// programs and (via the bridge) across frontends. The buttons only set
/// shared flags; the tick loop, which owns the peer, does the work.
fn build_host_controls() -> HostControls {
    use crate::dom::util;
    let paused = Rc::new(Cell::new(false));
    let reset = Rc::new(Cell::new(false));
    let step = Rc::new(Cell::new(false));

    let bar = util::create_element("div");
    util::set_attr(&bar, "class", "ah-hostbar");

    // Reset (↻ — controls::standard_action_glyph("restart")).
    let reset_btn = chip("\u{21BB}", "reset", "data-host-reset");
    let reset_cb = {
        let reset = reset.clone();
        Closure::wrap(Box::new(move |e: JsValue| {
            if let Ok(ev) = e.dyn_into::<web_sys::Event>() {
                ev.prevent_default();
            }
            reset.set(true);
        }) as Box<dyn FnMut(JsValue)>)
    };
    let _ = reset_btn.add_event_listener_with_callback("click", reset_cb.as_ref().unchecked_ref());
    util::append(&bar, &reset_btn);

    // Pause/resume (⏸ running → ▶ paused). The button shows the state it will
    // enter, and carries `data-host-paused` for observability.
    let pause_btn = chip("\u{23F8}", "pause", "data-host-pause");
    util::set_attr(&pause_btn, "data-host-paused", "0");
    let pause_cb = {
        let paused = paused.clone();
        let btn = pause_btn.clone();
        Closure::wrap(Box::new(move |e: JsValue| {
            if let Ok(ev) = e.dyn_into::<web_sys::Event>() {
                ev.prevent_default();
            }
            let now = !paused.get();
            paused.set(now);
            util::set_text(&btn, if now { "\u{25B6}" } else { "\u{23F8}" }); // ▶ / ⏸
            let _ = btn.set_attribute("data-host-paused", if now { "1" } else { "0" });
        }) as Box<dyn FnMut(JsValue)>)
    };
    let _ = pause_btn.add_event_listener_with_callback("click", pause_cb.as_ref().unchecked_ref());
    util::append(&bar, &pause_btn);

    // Step (⏭ — debug: advance exactly one tick while paused). Always
    // present (consistent chrome), only meaningful while paused — the tick
    // loop ignores a step request while running.
    let step_btn = chip("\u{23ED}", "step one tick (while paused)", "data-host-step"); // i18n-ignore — aria label (chrome a11y batch)
    let step_cb = {
        let step = step.clone();
        Closure::wrap(Box::new(move |e: JsValue| {
            if let Ok(ev) = e.dyn_into::<web_sys::Event>() {
                ev.prevent_default();
            }
            step.set(true);
        }) as Box<dyn FnMut(JsValue)>)
    };
    let _ = step_btn.add_event_listener_with_callback("click", step_cb.as_ref().unchecked_ref());
    util::append(&bar, &step_btn);

    HostControls {
        bar,
        paused,
        reset,
        step,
        closures: vec![reset_cb, pause_cb, step_cb],
    }
}

/// A host-control chip button (the small pill in the top-corner clusters). Shares
/// the `.ah-chip` styling with the input-source chips so the whole chrome reads as
/// one control language.
fn chip(glyph: &str, aria: &str, data_attr: &str) -> web_sys::Element {
    use crate::dom::util;
    let b = util::create_element("button");
    util::set_attr(&b, "type", "button");
    util::set_attr(&b, "class", "ah-chip");
    util::set_attr(&b, data_attr, "");
    util::set_attr(&b, "aria-label", aria);
    util::set_text(&b, glyph);
    b
}

/// The `?app-host=ping` smoke payload: render a marker, arm the ③α client, and on
/// `init` begin emitting an incrementing `state` on a slow interval so the host's
/// debounce+persist path is exercised through the real iframe boundary.
fn run_ping(root: &web_sys::Element) -> Result<(), JsValue> {
    // i18n-ignore — `?app-host=ping` is a developer delivery-smoke payload, not
    // a shipped program; its markers stay verbatim.
    root.set_text_content(Some("app-host: ping — booted, awaiting init…")); // i18n-ignore

    // A tick counter emitted as state. Shared with the interval callback the
    // `init` handler installs.
    let ticks = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let root_for_init = root.clone();
    let ticks_for_init = ticks.clone();

    // On `init`, flip to "running" and start the emitter.
    let on_init = move || {
        root_for_init.set_text_content(Some("app-host: ping — running")); // i18n-ignore (dev smoke)
        start_emitter(root_for_init.clone(), ticks_for_init.clone());
    };

    install_client(on_init);
    // Announce readiness — the host replies with `init`.
    post_to_host("ready-for-init", None);
    Ok(())
}

/// Install a 500 ms interval that increments the tick counter and posts it as
/// `state`. Stands in for the compute mount's per-tick state emission. The
/// interval closure is held in [`LIVE`] for the document's lifetime.
fn start_emitter(root: web_sys::Element, ticks: std::rc::Rc<std::cell::Cell<u32>>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let cb = Closure::wrap(Box::new(move |_: JsValue| {
        let n = ticks.get().wrapping_add(1);
        ticks.set(n);
        root.set_text_content(Some(&format!("app-host: ping — running (tick {n})"))); // i18n-ignore (dev smoke)
        let state = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&state, &JsValue::from_str("ticks"), &JsValue::from_f64(n as f64));
        post_to_host("state", Some(&state));
    }) as Box<dyn FnMut(JsValue)>);
    let _ = window.set_interval_with_callback_and_timeout_and_arguments_0(
        cb.as_ref().unchecked_ref(),
        500,
    );
    LIVE.with(|v| v.borrow_mut().push(cb));
}

/// Arm the `message` listener that drives the ③α handshake: on the host's `init`
/// message, run `on_init` once. Ignores non-host / non-init traffic. The listener
/// is held in [`LIVE`] for the document's lifetime.
fn install_client<F>(on_init: F)
where
    F: FnMut() + 'static,
{
    let Some(window) = web_sys::window() else {
        return;
    };
    let inited = std::rc::Rc::new(std::cell::Cell::new(false));
    let on_init = std::rc::Rc::new(RefCell::new(on_init));
    let cb = Closure::wrap(Box::new(move |e: JsValue| {
        let Ok(msg) = e.dyn_into::<MessageEvent>() else {
            return;
        };
        let data = msg.data();
        let source = js_sys::Reflect::get(&data, &JsValue::from_str("source"))
            .ok()
            .and_then(|v| v.as_string());
        if source.as_deref() != Some(HOST_SOURCE) {
            return;
        }
        let mtype = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
            .ok()
            .and_then(|v| v.as_string());
        if mtype.as_deref() == Some("init") && !inited.replace(true) {
            (on_init.borrow_mut())();
        }
        // `viewport` and other host messages are accepted but need no action in
        // this slice (the payload sizes itself to the iframe via CSS).
    }) as Box<dyn FnMut(JsValue)>);
    let _ = window.add_event_listener_with_callback("message", cb.as_ref().unchecked_ref());
    LIVE.with(|v| v.borrow_mut().push(cb));
}

/// Post one `{source:"entity-app", type, [state]}` message to the embedding host
/// (`window.parent`), target origin `*` (the same wildcard the host↔app contract
/// uses in both directions). A missing parent (opened top-level, not embedded) no-ops.
fn post_to_host(msg_type: &str, state: Option<&js_sys::Object>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(parent) = window.parent() else { return };
    let Some(parent) = parent else { return };
    let out = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&out, &JsValue::from_str("source"), &JsValue::from_str(APP_SOURCE));
    let _ = js_sys::Reflect::set(&out, &JsValue::from_str("type"), &JsValue::from_str(msg_type));
    if let Some(state) = state {
        let _ = js_sys::Reflect::set(&out, &JsValue::from_str("state"), state);
    }
    let _ = parent.post_message(&out, "*");
}

/// Post `x-stats` — *how much of the shared main thread this payload used, over
/// what span* ([`crate::monitor::MSG_STATS`]).
///
/// # A compute program runs in an IFRAME, so no host-side hook can see its cost
///
/// `src/program_host/` looks in-process and is not: a Programs window delivers
/// `index.html?app-host={key}` through the same `dom::games::render_player` an
/// app uses, so the tick loop is a **separate WASM instance in a separate
/// document**, with its own `monitor::sampler` thread-local that nothing reads.
/// A hook beside `note_sections` therefore cannot account for a tick, however
/// obvious that fix looks from the host side — the numbers have to cross ③α.
///
/// What makes this cheap is that **both ends are ours**: the host half already
/// exists (`dom::games`'s `MSG_STATS` arm → `sampler::note_app_stats`) and is
/// already wired for the Programs window, because Programs reuse the Apps
/// delivery verbatim. `AP-5` — *would entity-apps report `x-stats`* — is about
/// THEIR payloads and does not block this one.
///
/// The fields are top-level, not under `state`: that is the shape the host arm
/// reads, and `post_to_host` can only carry `state`.
fn post_stats_to_host(busy_ms: f64, span_ms: f64) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(Some(parent)) = window.parent() else {
        return;
    };
    let out = js_sys::Object::new();
    let set = |k: &str, v: &JsValue| {
        let _ = js_sys::Reflect::set(&out, &JsValue::from_str(k), v);
    };
    set("source", &JsValue::from_str(APP_SOURCE));
    set("type", &JsValue::from_str(crate::monitor::MSG_STATS));
    set("busy_ms", &JsValue::from_f64(busy_ms));
    set("span_ms", &JsValue::from_f64(span_ms));
    let _ = parent.post_message(&out, "*");
}

/// How often a payload reports its share of the thread. The host clamps `busy_ms`
/// to `span_ms` and refuses a span over 60 s, so this is a floor on resolution
/// rather than a tuning knob: report too rarely and a burst of work is averaged
/// into invisibility.
const STATS_INTERVAL_MS: f64 = 1000.0;
