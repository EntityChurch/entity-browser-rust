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

mod input;

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::MessageEvent;

use crate::dom::programs::{display_list_driver, sleep_ms, text_driver, with_timeout};
use crate::peers::Peers;
use crate::program_host::bundle::{digest_hex, Bundle, EMBEDDED_PROGRAMS};
use crate::program_host::descriptor::{scene_u64, ProgramDescriptor, SHAPE_DISPLAY_LIST, SHAPE_TEXT};
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

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("app-host: no window"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("app-host: no document"))?;
    let body = document
        .body()
        .ok_or_else(|| JsValue::from_str("app-host: no body"))?;

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
            root.set_text_content(Some(&format!("app-host: unknown program '{other}'")));
            tracing::warn!(program = other, "app-host: unknown program");
        }
    }
    Ok(())
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
        .ok_or_else(|| JsValue::from_str("app-host: program not embedded"))?;
    let bundle = Bundle::parse(embedded.json)
        .map_err(|e| JsValue::from_str(&format!("app-host: bundle parse: {e}")))?;
    // Cross-impl hash gate (same as the oracle/window): refuse a bundle whose
    // entities don't recompute identically under the Rust encoder.
    let entities = bundle
        .verified_entities()
        .map_err(|e| JsValue::from_str(&format!("app-host: bundle verify: {e}")))?;

    // Shared: the tick loop (moved into the 'static future) and the input
    // driver closures (held for the document's lifetime) both hold the peer.
    let peers = Rc::new(Peers::new_direct());
    let peer_id = peers.primary_peer_id().to_string();
    // The IR's internal lookups are origin-qualified, so the program must live
    // under its authoring namespace in the local store (host::qualify).
    let ns = bundle.origin_peer.clone();

    host::materialize_future(&peers, &peer_id, &ns, entities, |_, _| {})
        .await
        .map_err(|e| JsValue::from_str(&format!("app-host: materialize: {e}")))?;
    let desc_ent = peers
        .get_entity(&peer_id, &host::qualify(&ns, &bundle.descriptor_path))
        .ok_or_else(|| JsValue::from_str("app-host: descriptor unreadable after materialize"))?;
    let desc = ProgramDescriptor::decode(&desc_ent)
        .map_err(|e| JsValue::from_str(&format!("app-host: descriptor: {e}")))?;
    // Admit by shape capability — the union of the display and input shapes this
    // host drives (the same fail-closed check the Programs window applies).
    let supported: Vec<&str> = SUPPORTED_DISPLAY_SHAPES
        .iter()
        .chain(input::SUPPORTED_INPUT_SHAPES)
        .copied()
        .collect();
    desc.admit(&supported)
        .map_err(|e| JsValue::from_str(&format!("app-host: admit: {e}")))?;
    host::seed_future(&peers, &peer_id, &ns, &desc)
        .await
        .map_err(|e| JsValue::from_str(&format!("app-host: seed: {e}")))?;

    // The display surface the tick loop re-renders into (the program's text shape).
    root.set_inner_html("");
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| JsValue::from_str("app-host: no document"))?;
    let display = document.create_element("div")?;
    display.set_attribute("data-app-host-display", program_key)?;
    root.append_child(&display)?;

    // Install the keyboard input drivers — one per declared input port, bound by
    // shape (program-blind; the field name comes from the port's SEED, not
    // assumed). The closures are held in `LIVE` for the document's lifetime
    // (D12: no `Closure::forget`). Input is captured here and written to the
    // inner peer — it never crosses ③α (the host stays blind, P1). A port with
    // no readable seed is skipped loudly (seed already validated F-E1, so this
    // is defence in depth).
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
        let closures = input::install(
            peers.clone(),
            peer_id.clone(),
            ns.clone(),
            port,
            field,
            // Stamp the D13 input surface on the payload ROOT (the stable
            // `data-app-host` element) — `display`'s children are replaced every
            // tick, the root persists.
            root.clone(),
        );
        LIVE.with(|v| v.borrow_mut().extend(closures));
    }

    // Announce readiness (host replies `init`, stamping data-host-locale). We
    // ignore the returned saved state for this first test — the program seeds
    // from its initial state, and the host persists what we emit.
    post_to_host("ready-for-init", None);

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
            )
        });
        // Clock-driven rate → ms per tick (guarded against a 0 hint).
        let interval_ms = (1000 / desc.tick.rate_hint.max(1)).clamp(16, 1000) as i32;
        let mut last_hash = String::new();
        let mut ticks: u64 = 0;
        loop {
            if let Err(e) =
                with_timeout(host::tick_future(&peers, &peer_id, &ns, &desc), 4000).await
            {
                tracing::error!(program = program_key, "app-host: tick faulted: {e}");
                display.set_text_content(Some(&format!("app-host: {program_key} faulted — {e}")));
                break;
            }
            ticks += 1;

            // Re-render the program's display shape (program-blind, shape-bound —
            // the identical drivers the Programs window uses). Admission already
            // gated the shape to the supported set, so the fallthrough is
            // defence in depth (a loud marker, never a silent blank).
            if let Some((shape, port_path, bounds)) = &display_port {
                let el = match shape.as_str() {
                    SHAPE_TEXT => text_driver(&peers, &peer_id, &ns, port_path),
                    SHAPE_DISPLAY_LIST => {
                        display_list_driver(&peers, &peer_id, &ns, port_path, *bounds)
                    }
                    other => {
                        let el = crate::dom::util::create_element("div");
                        crate::dom::util::set_text(
                            &el,
                            &format!("app-host: no driver for display shape {other:?}"),
                        );
                        el
                    }
                };
                display.set_inner_html("");
                let _ = display.append_child(&el);
            }

            // Emit the state to the host only when it evolved — so a growing
            // host-side `data-app-state-seq` proves Life actually advanced
            // (dedup by content hash; identical ticks don't spam a write).
            if let Some(state) = peers.get_entity(&peer_id, &state_path) {
                let hash = digest_hex(&state);
                if hash != last_hash {
                    last_hash = hash.clone();
                    let obj = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("program"), &JsValue::from_str(&program_key));
                    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("ticks"), &JsValue::from_f64(ticks as f64));
                    let _ = js_sys::Reflect::set(&obj, &JsValue::from_str("hash"), &JsValue::from_str(&hash));
                    post_to_host("state", Some(&obj));
                }
            }

            sleep_ms(interval_ms).await;
        }
    });
    Ok(())
}

/// The `?app-host=ping` smoke payload: render a marker, arm the ③α client, and on
/// `init` begin emitting an incrementing `state` on a slow interval so the host's
/// debounce+persist path is exercised through the real iframe boundary.
fn run_ping(root: &web_sys::Element) -> Result<(), JsValue> {
    root.set_text_content(Some("app-host: ping — booted, awaiting init…"));

    // A tick counter emitted as state. Shared with the interval callback the
    // `init` handler installs.
    let ticks = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let root_for_init = root.clone();
    let ticks_for_init = ticks.clone();

    // On `init`, flip to "running" and start the emitter.
    let on_init = move || {
        root_for_init.set_text_content(Some("app-host: ping — running"));
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
        root.set_text_content(Some(&format!("app-host: ping — running (tick {n})")));
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
