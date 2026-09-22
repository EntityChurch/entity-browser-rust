// The native binary is a deprecation stub — `EntityApp` is only
// instantiated by the WASM frontend (browser / Tauri WebView). On
// native, every module compiles as dead code, so silence those
// warnings.
#![cfg_attr(
    not(target_arch = "wasm32"),
    allow(dead_code, unused_imports)
)]

mod access_log_store;
mod action;
mod action_event;
mod app;
#[cfg(target_arch = "wasm32")]
mod app_host;
mod app_paths;
mod apps;
mod boot;
// Which build is this — read from the shell that loaded it (C5).
mod boot_diagnostics;
mod build_id;
mod chain_trace_cache;
mod deployment_config;
#[cfg(target_arch = "wasm32")]
mod capabilities;
mod inspect_router;
mod ansi;
mod connections;
mod connect_attempt;
mod connectors;
mod dial_markers;
// Piece A (DESIGN-CONNECTIVITY-UX): the subscribed connection-liveness facet of
// the unified Peer read-model. `allow(dead_code)` while it's staged — the
// consumers (window renders) wire up across Pieces B/C; today only the WASM
// shadow-parity probe uses it. Tighten when the mirror migration lands.
#[allow(dead_code)]
mod peer_liveness;
mod peer_supersession;
mod content_site;
mod peer_auth;
mod backend_auth;
#[cfg(target_arch = "wasm32")]
mod dom;
// Owned, arm-agnostic dispatch handle for spawned multi-step L1 flows (the
// awaited twin of `writer_handle`). Staged with its first consumer.
#[allow(dead_code)]
mod dispatch_handle;
mod event_log_cache;
mod event_log_writer;
// The browser's serving side of a file transfer (`system/content` + an offer
// manifest). Staged: the native proof and the window wiring land together with
// its consumers, so the offer/pull entry points have no caller yet.
#[allow(dead_code)]
mod file_offer;
mod share;
mod format;
#[cfg(feature = "measurement")]
mod frame_counters;
mod i18n;
// The visible outcome of the Offer button — the counterpart of
// `connect_attempt` for the other control that could complete in silence.
mod offer_attempt;
mod ops;
mod peers;
mod program_host;
#[cfg(target_arch = "wasm32")]
mod peers_worker;
mod listener_state;
mod peer_mode;
mod peer_registry;
mod reach_keeper;
// Why a peer could not be reached, from our own ICE agent's gathered candidate
// types. `dial_markers`' shape (local, in-flight, in-memory) — NOT a fourth
// liveness store; the kernel owns connection state and this only advises.
mod reachability;
// Preflight: can this machine reach another machine's peer, and if not which
// half is missing. The pure model + its remedies are native-tested; the
// collector is wasm-only. Read by the Shell's `net` verb.
mod readiness;
mod rendezvous;
mod roster;
mod transport_profiles;
mod window_watch;
mod writer_handle;
mod peer_display;
mod persistence;
mod vault_codec;
mod render_policy;
mod selection;
mod session_config;
mod selection_source;
mod theme_tokens;
mod user_themes;
#[cfg(target_arch = "wasm32")]
mod opfs_cleanup;
#[cfg(target_arch = "wasm32")]
mod idb_cleanup;
#[cfg(target_arch = "wasm32")]
mod storage_durability;
#[cfg(target_arch = "wasm32")]
mod multitab;
// Bounded network reads — the D23 chokepoint for every boot-path fetch.
mod net;
// THE cache-immutability rule (C15) — one expression, four call sites, pinned
// to tools/cache-policy-vectors.txt. A mis-cached mutable file is a deployment
// nobody can correct for a year.
mod cache_policy;
// C9 — retained shells + /builds.json, the publisher half of rollback (row 10).
mod build_slots;
#[cfg(target_arch = "wasm32")]
mod diagnostics;
#[cfg(target_arch = "wasm32")]
mod watchdog;
mod watchdog_policy;
mod tree_click;
#[cfg(target_arch = "wasm32")]
mod tauri_ipc;
#[cfg(target_arch = "wasm32")]
mod boot_fast_paint;
// The pre-frame-loop boot surface — the observability half of boot audit B-1.
#[cfg(target_arch = "wasm32")]
mod boot_progress;
// Entity Doctor — "do my beliefs still match the world?" (heal-path row 11).
// Both halves are arch-independent and native: `refresh_ledger` is the
// recording seam a failed refresh lands in, and `doctor` is the pure verdict
// logic, so the checks are gated by `make test` and not only through a browser.
mod doctor;
mod refresh_ledger;
mod views;
mod window;
mod window_hydration;
mod window_index;
mod window_registry;

// Native binary — prints a deprecation message and exits. There is no
// native UI: the active browser path is `make wasm` / `make serve` and
// the active desktop path is Tauri (`make tauri-run`), both of which
// run the WASM frontend. (The legacy native renderer was removed.)
#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    // There is no native UI, but the native binary IS the headless home for
    // the publish pipeline ([C] — `entity-browser publish [OUT_DIR]`), which
    // needs no browser. Dispatch the verb; bare invocation keeps the
    // deprecation redirect.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("publish") {
        return content_site::publish::run(&args);
    }
    // B16 — the naming half. A registry is a static publisher like any other
    // (EXTENSION-REGISTRY §7.4), so it gets its own verb rather than a flag on
    // `publish`: it emits a different subgraph under a DIFFERENT identity, and
    // conflating the two would let one invocation sign both a content tree and
    // the names that point at it.
    if args.first().map(String::as_str) == Some("registry") {
        return content_site::registry_publish::run(&args);
    }
    // C9 — retain this tree's shell as a rollback target and record it in
    // /builds.json. A separate verb because it acts on an ASSEMBLED tree and is
    // the last step, after both halves of `site-dist` have written it.
    if args.first().map(String::as_str) == Some("builds") {
        return build_slots::run(&args);
    }

    eprintln!("entity-browser: there is no native UI build.");
    eprintln!();
    eprintln!("Native commands:");
    eprintln!("  entity-browser publish [OUT_DIR]  — render the site set to static HTML (make site)");
    eprintln!("      [--ingest=<dir>]              — source sites from a content-team render/ emit (disk→tree)");
    eprintln!("  entity-browser registry OUT_DIR --bind=NAME=PEER_ID@ORIGIN [--bind=...]");
    eprintln!("                                    — emit a static name registry (signed bindings + signed root)");
    eprintln!("      @ORIGIN is REQUIRED           — arch D10: a binding with no transports resolves to a");
    eprintln!("                                      peer-id the consumer has no way to reach");
    eprintln!("      [--ttl-days=N]                — binding lifetime (default 30; a null ttl is not expressible)");
    eprintln!("      [--identity-seed=HEX]         — 32-byte hex; omit to use the durable registry identity");
    eprintln!("      flags take `=`                — `--bind NAME=…` with a space is NOT this flag");
    eprintln!("  entity-browser builds OUT_DIR [--notes=…] [--released-at=…] [--keep=N] [--prune]");
    eprintln!("                                    — retain this tree's shell at /builds/<id>/ and record");
    eprintln!("                                      it in /builds.json, so a bad build has somewhere to");
    eprintln!("                                      fall back TO (row 10 / C9). Run it LAST.");
    eprintln!("      [--min-rollback-index=N]      — advance the anti-rollback floor. Monotone; only for a");
    eprintln!("                                      build that fixes something not safely rollback-able");
    eprintln!("      --prune                       — remove retained shells beyond --keep. NEVER removes an");
    eprintln!("                                      asset a surviving shell names");
    eprintln!("  entity-browser registry OUT_DIR --verify");
    eprintln!("                                    — prove an emitted registry: pointers, signature, walkable closure");
    eprintln!("                                      (use THIS, not `publish --verify` — different durable identity)");
    eprintln!();
    eprintln!("Active build targets:");
    eprintln!("  make wasm       — browser build (DOM)");
    eprintln!("  make tauri-run  — desktop build (DOM in WebView + System backend peer)");
    std::process::ExitCode::FAILURE
}

#[cfg(target_arch = "wasm32")]
fn main() {
    // wasm_bindgen entry handles everything
}

/// Read the configured tracing level for this WASM session.
///
/// Resolution order, first match wins:
/// 1. URL param `?log=<level>` — ephemeral, scoped to this tab.
/// 2. localStorage `entity_log_level` — sticky across reloads;
///    set via DevTools (`localStorage.setItem('entity_log_level','debug')`).
/// 3. Compile-time default: `INFO` for release, `DEBUG` for `cargo build`
///    without `--release` (so live dev still shows debug! lines).
///
/// `tracing_wasm` defaults to `TRACE`, which floods the console with
/// `peers_worker::dispatch_write` trace lines during normal use; this
/// fn imposes the saner default and provides the override knob.
#[cfg(target_arch = "wasm32")]
fn configured_log_level() -> tracing::Level {
    let default = if cfg!(debug_assertions) {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    if let Some(level) = log_level_from_url() {
        return level;
    }
    if let Some(level) = log_level_from_local_storage() {
        return level;
    }
    default
}

#[cfg(target_arch = "wasm32")]
fn parse_log_level(s: &str) -> Option<tracing::Level> {
    match s.to_ascii_lowercase().as_str() {
        "trace" => Some(tracing::Level::TRACE),
        "debug" => Some(tracing::Level::DEBUG),
        "info" => Some(tracing::Level::INFO),
        "warn" | "warning" => Some(tracing::Level::WARN),
        "error" => Some(tracing::Level::ERROR),
        // `off` isn't a tracing::Level, but treat it as max-suppression.
        // We map it to ERROR which is the loudest gate this app exposes.
        "off" => Some(tracing::Level::ERROR),
        _ => None,
    }
}

#[cfg(target_arch = "wasm32")]
fn log_level_from_url() -> Option<tracing::Level> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    let q = search.trim_start_matches('?');
    for pair in q.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some("log") {
            return parse_log_level(parts.next().unwrap_or(""));
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
fn log_level_from_local_storage() -> Option<tracing::Level> {
    let window = web_sys::window()?;
    let storage = window.local_storage().ok().flatten()?;
    let raw = storage.get_item("entity_log_level").ok().flatten()?;
    parse_log_level(&raw)
}

/// Read the `?worker=` URL param as an override of capability-based
/// mode selection. `Some(true)` forces Worker, `Some(false)` forces
/// Direct, `None` defers to capability detection.
///
/// Accepted forms:
/// - `?worker`, `?worker=`, `?worker=1`, `?worker=true` → `Some(true)`
/// - `?worker=0`, `?worker=false` → `Some(false)`
/// - any other value or param absent → `None`
/// True when the URL requests the L1 System Recovery console
/// (`?systemrecovery=1` or bare `?systemrecovery`). The read-only recovery UI
/// is rendered by a first-script in `index.html`; this lets `start()` yield the
/// page to it instead of booting the peer — the BIOS must work even when the
/// app can't. Mirrors the JS detection (`window.__ENTITY_RECOVERY__`).
#[cfg(target_arch = "wasm32")]
fn system_recovery_requested() -> bool {
    let Some(window) = web_sys::window() else { return false };
    let Ok(search) = window.location().search() else { return false };
    for pair in search.trim_start_matches('?').split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some("systemrecovery") {
            return matches!(parts.next(), None | Some("") | Some("1") | Some("true"));
        }
    }
    false
}

/// `?boot=inline` — the PRE-2026-09-02 order: run the deployment-document half
/// of the boot before the frame loop exists.
///
/// **The two-phase order is the default now and there is no product trade left
/// to pick.** The flag it replaced (`?boot=twophase`, default off) existed
/// because deferring phase 2 was believed to cost a visible flicker — the first
/// frames rendering against a not-yet-reconciled config. That cost was an
/// artifact of taking the boot surface down at the rAF arm; it is not inherent
/// to the ordering. The surface now comes down when phase 2 finishes
/// ([`crate::boot_progress`]), so the user sees the same single transition they
/// always did, while the frame loop — and with it the watchdog and the recovery
/// hatch — is live underneath the whole application tier.
///
/// So this flag is not an experiment and nothing is expected to set it. It is
/// kept for two uses: an **escape hatch** if the deferred order is ever
/// implicated in an incident, and the **control** the row-8 gate compares
/// against, which needs the old order reachable on the same build to be able to
/// tell a deferred boot from a fast one.
///
/// Accepts `?boot=inline` only — an unrecognised `?boot=` value is *not* a
/// silent opt-out (a typo must land on the shipped order).
#[cfg(target_arch = "wasm32")]
fn boot_inline_requested() -> bool {
    let Some(window) = web_sys::window() else { return false };
    let Ok(search) = window.location().search() else { return false };
    search
        .trim_start_matches('?')
        .split('&')
        .any(|pair| pair == "boot=inline")
}

/// `?app-host=<program>` — the stripped-startup "L5 app" boot mode: boot minimal
/// inside a sandboxed iframe and run one compute program behind the entity-apps
/// ③α boundary (see [`crate::app_host`]). Returns the requested program id, or
/// `None` for a normal boot. Empty (`?app-host` / `?app-host=`) is treated as
/// absent, not a program named "".
#[cfg(target_arch = "wasm32")]
fn app_host_program() -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    for pair in search.trim_start_matches('?').split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some("app-host") {
            return parts.next().filter(|p| !p.is_empty()).map(str::to_string);
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
fn worker_url_override() -> Option<bool> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    let q = search.trim_start_matches('?');
    for pair in q.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some("worker") {
            return match parts.next() {
                Some("1") | Some("true") | Some("") | None => Some(true),
                Some("0") | Some("false") => Some(false),
                _ => None,
            };
        }
    }
    None
}

/// Frozen-frame watchdog config from `?watchdog=…`. Returns the stall
/// threshold in ms, or `None` when disabled. Default: on at 5000ms (cheap —
/// one heartbeat/sec). `?watchdog=0` disables; `?watchdog=<n>` sets the
/// threshold (used by the e2e to force a detectable stall quickly).
#[cfg(target_arch = "wasm32")]
fn watchdog_threshold_ms() -> Option<u32> {
    const DEFAULT_MS: u32 = 5000;
    let Some(window) = web_sys::window() else {
        return Some(DEFAULT_MS);
    };
    let Ok(search) = window.location().search() else {
        return Some(DEFAULT_MS);
    };
    for pair in search.trim_start_matches('?').split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some("watchdog") {
            return match parts.next() {
                Some("0") | Some("false") => None,
                Some("1") | Some("true") | Some("") | None => Some(DEFAULT_MS),
                Some(n) => n.parse::<u32>().ok().filter(|&n| n >= 500).or(Some(DEFAULT_MS)),
            };
        }
    }
    Some(DEFAULT_MS)
}

/// Whether a detected freeze surfaces the user-facing reload banner. **Off by
/// default** — the freeze is always logged to the diagnostics sink; the banner
/// is an opt-in (`?watchdog-banner=1`) for debugging / the e2e, so normal use
/// never gets an alarming prompt for a recoverable hitch.
#[cfg(target_arch = "wasm32")]
fn watchdog_banner_enabled() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let Ok(search) = window.location().search() else {
        return false;
    };
    for pair in search.trim_start_matches('?').split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() == Some("watchdog-banner") {
            return matches!(parts.next(), Some("1") | Some("true") | Some("") | None);
        }
    }
    false
}

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub async fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let level = configured_log_level();
    let mut builder = tracing_wasm::WASMLayerConfigBuilder::new();
    builder.set_max_level(level);
    tracing_wasm::set_as_global_default_with_config(builder.build());
    let wasm_start_ms = perf_now();
    let html_parsed_to_wasm_start_ms = perf_mark_age("html-parsed").unwrap_or(-1.0);
    tracing::info!(
        boot_phase = "wasm_start",
        wasm_start_ms,
        html_parsed_to_wasm_start_ms,
        "WASM init: panic hook + tracing ready (cold-start timing)"
    );
    tracing::info!(level = %level, "WASM init: tracing level set");

    // C5 — say which build this is, as early as anything can be said.
    //
    // Placement is the point: this sits immediately after the logger and BEFORE
    // the recovery yield, the peer-host selection and `boot_load`, so a session
    // that dies anywhere later still has a line naming the build it died on.
    // Until this existed the application could not report its own version at all
    // (`grep -rn "entity-build" src/` → 0), which meant a bug report could not
    // identify one and "are these two domains running the same code" was only
    // answerable by curling them from outside.
    //
    // Both identities, because they answer different questions: the commit
    // resolves against `git log`, the bundle hash compares across deployments —
    // and two docs-only commits produce byte-identical output, so the commit
    // alone cannot tell you two domains match. Measured: they did.
    {
        let id = crate::build_id::current();
        tracing::info!(
            commit = ?id.commit,
            bundle = ?id.bundle,
            "WASM init: build {}",
            id.describe()
        );
    }

    // L1 System Recovery (?systemrecovery=1): the recovery first-script in
    // index.html has taken over the page with a read-only storage inventory.
    // Yield to it — do NOT boot the peer / rAF loop. Recovery is a JS-only BIOS
    // that must remain usable even when the app can't boot, so this returns
    // before any storage open or peer spawn. (The script renders independently;
    // this just stops the app from fighting it for the DOM.)
    if system_recovery_requested() {
        tracing::warn!(
            "?systemrecovery=1 — skipping WASM boot; System Recovery (L1) owns the page"
        );
        return Ok(());
    }

    // Stripped-startup "L5 app" boot (?app-host=<program>): boot minimal as a
    // compute-program payload behind the entity-apps ③α boundary — NO
    // window-manager, NO peer roster, NO durable storage. Branch before any of
    // that machinery (theme install, fast-paint, peer host-mode selection). The
    // embedding host owns the page chrome; we own only this iframe's document.
    if let Some(program) = app_host_program() {
        tracing::info!(program = %program, "?app-host — stripped app-host boot");
        return app_host::run(&program).await;
    }

    // The loading indicator STAYS UP until boot phase 2 hands the page over
    // (`boot_progress::surface_down()`, below — NOT the rAF arm, which is a
    // separate signal since the two-phase split). It used to be hidden right here —
    // before peer construction and before `boot_load`'s ~14 awaits — which,
    // with `boot_fast_paint` disabled for the site-surface consolidation, left
    // the page **blank** for the whole application-tier boot and took the
    // always-visible "Open System Recovery" hatch down with it. Boot audit §2:
    // *bounded is not the same as observable*. See `boot_progress`.
    boot_progress::step("starting");

    // DOM-only mode — set CSS class.
    dom::util::set_mode_class("mode-dom", "DOM");
    tracing::info!("WASM init: DOM mode set, creating app");

    // Theming: install the `:root` custom-property block BEFORE first paint
    // (fast-paint / DOM render below) so every `var(--token)` resolves from
    // frame one — no flash. `boot_choice()` reads the localStorage mirror
    // (written on every theme change) so a non-default theme survives reload
    // without a dark flash; the durable tree record reconciles once peers boot.
    theme_tokens::install_root(&theme_tokens::boot_choice());
    // Site-overlay appearance (`--site-*`): a SECOND, independent layer. `"site"`
    // (the default) injects nothing — the overlay's own palette renders via the
    // CSS fallbacks; `"system"` / a strict theme override its colors. Installed
    // before fast-paint so a configured override doesn't flash.
    theme_tokens::install_site_root(&theme_tokens::site_appearance_boot_choice());

    // i18n P0: drive `lang`/`dir` from the persisted/detected locale BEFORE
    // fast-paint, the twin of `install_root` above. `boot_choice()` reads the
    // localStorage mirror (written on every language change), else detects from
    // `navigator.language` — so an RTL locale lays out correctly from frame one
    // (no LTR flash) even before any string is translated. Sets `dir` on BOTH
    // the shadow host (`#dom-layer`) and `<html>` (i18n::install_lang_dir).
    i18n::apply(&i18n::boot_choice());

    // Boot-closure cut 2c: Phase-1 fast paint. For a content-site deployment
    // (remote home + boots-into-site), paint the home over HTTP into
    // `#site-layer` NOW, racing the heavy peer boot below — so the startup
    // page shows immediately instead of empty chrome while the boot worker
    // spawns + replays OPFS. Fire-and-forget, read-only, safe-fallback; the
    // live overlay re-renders the same page from the same code once up (cut
    // 2a points it at the same home). No-op unless configured + enabled.
    boot_fast_paint::maybe_paint_home();

    // Peer host-mode selection. The **default is the main-thread IndexedDB
    // system peer** (Direct arm) — it is always available, holds the durable
    // roster + config, and is what makes the roster readable BEFORE any data
    // peer spawns (the Brick-4 boot-flip). Worker+OPFS is now **opt-in** for
    // heavy data peers / explicit testing:
    //   - `?worker=1` (or `=true`, `=`, bare `?worker`) → force Worker.
    //   - `?worker=0` (or `=false`) → force Direct/IDB.
    //   - absent → Direct/IDB (the system-peer default).
    // If Worker is selected but spawn/init fails, log and fall back to Direct
    // so the page doesn't blank-window on unsupported runtimes (e.g. WebKitGTK
    // ≤ 2.52 worker-OPFS gap).
    let caps = capabilities::Capabilities::detect();
    let override_worker = worker_url_override();
    // Tauri's WebView (WebKitGTK ≤ 2.52) lacks WorkerNavigator.storage, so
    // worker-mode OPFS init HARD-FAILS there — it does NOT silently
    // in-memory. `OpfsStore::open` returns `OpfsError::Unavailable`,
    // `build_async()` propagates `Err`, the worker host posts
    // `Response::Init { Some(err) }` (never Ready), `new_wasm_worker()`
    // errors, and the `Err` branch below falls to Direct + an honest banner.
    // We force Direct in Tauri *preemptively* only to skip that wasted
    // failed-worker round-trip and give a consistent in-memory tree. (The
    // Ready handshake's `opfs_active` reports the real backend, but isn't
    // needed for this decision — the failure is already observable as the
    // Init error. Routing Tauri persistence to the native peer over IPC is
    // the real durability answer, deferred.) Browser deployments are
    // unaffected: worker OPFS either comes up durable or errors → banner.
    // See WORKER-MODE-LIVING-DOC §3.6.
    let in_tauri = tauri_ipc::is_tauri();
    // Default = Direct/IDB (the main-thread system peer). Worker only when
    // explicitly requested via `?worker=1`. `caps`/`in_tauri` are retained for
    // the diagnostic log + the worker-capability note, but no longer pick the
    // default — the system-peer-on-IDB posture is uniform across runtimes.
    let try_worker = override_worker.unwrap_or(false);
    let _ = (in_tauri, caps.prefers_worker());
    tracing::info!(
        ?caps,
        ?override_worker,
        in_tauri,
        try_worker,
        "WASM init: peer host mode selection"
    );

    // Multi-tab single-writer guard (sprint #1, TRIAGE §4.1; UNIFIED on the
    // system-seed id, design §9 step 2). Both durable arms contend for a
    // shared per-origin resource — Worker mode for the exclusive OPFS sync
    // handle on `workers/{primary}`, the Direct arm for the IDB database
    // `entity-peer-{system_id}` (which, UNLIKE OPFS, has no exclusivity
    // backstop → silent last-writer-wins corruption). We elect a single
    // leader with ONE Web Lock keyed on the system-seed id, computed (and
    // generated-if-absent) cheaply from localStorage BEFORE any store opens
    // ("elect-then-open"). Using the same key on both arms means a tab elects
    // exactly one leader regardless of which arm it lands on, and even tab 1's
    // FIRST boot holds the lock (system_seed_id generates the seed) — closing
    // the fresh-profile window where two tabs both went durable. A secondary
    // stays ephemeral *intentionally* and says so, instead of letting the
    // second store open fail and silently downgrade (the data-loss papercut).
    let multitab_key = persistence::system_seed_id();
    let multitab_secondary = if try_worker {
        multitab::detect_secondary(&multitab_key).await
    } else {
        false
    };
    let try_worker = try_worker && !multitab_secondary;

    use storage_durability::BootStorageStatus;
    boot_progress::step("opening storage");
    let (app, boot_plan, storage_status) = if try_worker {
        match app::EntityApp::new_wasm_worker().await {
            Ok((app, plan)) => (app, plan, BootStorageStatus::DurableWorker),
            Err(err) => {
                // C5b: the silent Worker→Direct downgrade orphans the durable
                // OPFS tree and looks wiped. Warn loudly and flag the banner.
                tracing::warn!(
                    error = ?err,
                    "worker bootstrap failed; falling back to Direct mode — the \
                     durable OPFS tree (if any) is inaccessible this session"
                );
                // Worker failed: do NOT also open the IDB primary — the
                // orphaned-OPFS interaction + shared-db concerns make the
                // durable IDB path its own follow-up. Keep ephemeral here.
                let (app, _, plan) = app::EntityApp::new_wasm(false).await;
                (app, plan, BootStorageStatus::DowngradedToDirect)
            }
        }
    } else if multitab_secondary {
        // Owned by another tab — Direct (in-memory) on purpose, with a
        // specific banner so it isn't mistaken for storage eviction.
        // Secondary tab must NOT open the shared IDB primary (last-writer-
        // wins race, design §9 multi-tab) — stay ephemeral on purpose.
        let (app, _, plan) = app::EntityApp::new_wasm(false).await;
        (app, plan, BootStorageStatus::SecondaryTabEphemeral)
    } else {
        // The clean Direct boot (`?worker=0`, no worker support, Tauri
        // WebView): the primary is IDB-durable, and IDB has no exclusivity
        // backstop (see the unified-key comment above), so we run the same
        // Web-Lock election here keyed on the same `multitab_key`. Only the
        // try_worker arm ran the election above; this arm runs it now.
        let direct_secondary = multitab::detect_secondary(&multitab_key).await;
        if direct_secondary {
            // Another tab owns the durable IDB peer — stay ephemeral on
            // purpose (do NOT open the shared db) with the specific banner.
            let (app, _, plan) = app::EntityApp::new_wasm(false).await;
            (app, plan, BootStorageStatus::SecondaryTabEphemeral)
        } else {
            // Leader (or single tab): make the primary IDB-durable.
            // `idb_active` picks the honest banner — durable when IDB came up,
            // ephemeral if it didn't.
            let (app, idb_active, plan) = app::EntityApp::new_wasm(true).await;
            let status = if idb_active {
                BootStorageStatus::DurableDirectIdb
            } else {
                BootStorageStatus::EphemeralDirect
            };
            (app, plan, status)
        }
    };
    // D13 honesty: which peer-host arm actually booted (the default is now the
    // main-thread IDB system peer). A grep target for the e2e + ops triage.
    tracing::info!(?storage_status, "boot: peer host arm selected");
    // C5c: honest "not saved" banner for the in-memory modes (no-op for
    // durable Worker mode; suppressed under Tauri — separate persistence story).
    storage_durability::show_storage_banner(storage_status, in_tauri);
    // And the *other* honesty banner, one layer under storage: an origin the
    // browser will not let us do WebRTC or OPFS on. Deliberately after the
    // storage one — when both apply, the origin is the cause and reads better
    // second, next to the symptom it explains. Both are `Some`-id'd and
    // idempotent, so they stack rather than replace.
    readiness::warn_if_insecure_origin();
    // And the one that is total rather than partial: with no `RTCPeerConnection`
    // this session cannot connect to another peer at all, in either direction,
    // while everything up to and including `meet` keeps working. Same banner
    // mechanism, same reason — a precondition that fails on a user's machine
    // owes a surface, not a row in a preflight nobody opens.
    readiness::warn_if_no_webrtc_api();

    // C5a + C5d: ask for persistent (non-evictable) storage for this origin,
    // then make Worker mode honest about the result. persist() is
    // origin-scoped (covers the workers' OPFS + the localStorage keypair) and
    // worth requesting in every mode. But the grant can be DENIED (Firefox
    // prompt → "no"; Chrome/Safari engagement heuristic on a first visit), and
    // without it even the durable Worker+OPFS default is evictable (Safari ITP
    // ~7d / cross-engine LRU). So in DurableWorker mode, if the grant didn't
    // land, surface the soft "saved but evictable" banner (D16 honesty). Other
    // modes already carry their own banner above. Spawned (not blocking boot);
    // FF's prompt appears a beat after first paint, which is fine.
    {
        // Both durable arms — Worker+OPFS and Direct+IDB — are evictable until
        // the origin is granted persistent storage; if the grant didn't land,
        // the soft "saved but evictable" note applies to both (D16 honesty).
        // Suppressed under Tauri (separate persistence story; WebKitGTK IDB
        // durability not yet verified). Other modes carry their own banner above.
        let durable_evictable = matches!(
            storage_status,
            BootStorageStatus::DurableWorker | BootStorageStatus::DurableDirectIdb
        );
        wasm_bindgen_futures::spawn_local(async move {
            let granted = storage_durability::request_persistent_storage().await;
            if durable_evictable && !in_tauri && granted != Some(true) {
                storage_durability::show_evictable_banner();
            }
        });
    }
    tracing::info!("WASM init: app created, starting rAF loop");

    // Sprint #3: frozen-frame watchdog. A tiny off-main-thread worker watches
    // the rAF heartbeat and, if the UI stops rendering past the threshold,
    // records the freeze in the in-app diagnostics sink (#4). Cheap (one
    // beat/sec); `?watchdog=0` disables. The user-facing reload BANNER is
    // opt-in (`?watchdog-banner=1`) — off by default so a recoverable stall
    // doesn't pop an alarming prompt during normal use; the log is the value.
    if let Some(threshold) = watchdog_threshold_ms() {
        watchdog::install(threshold, watchdog_banner_enabled());
    }

    let app = std::rc::Rc::new(std::cell::RefCell::new(app));

    // ---- Boot phase 2: the deployment-document half. -----------------------
    //
    // Heal-path row 8 / `AUDIT-BOOT-PATH-2026-08-27` §4. Phase 1 ran inside the
    // constructor and resolved everything already durable — the session config,
    // the window index, the persisted location. Phase 2 is the part that depends
    // on `/entity-deployment.json`, and it is **deferred by default since
    // 2026-09-02**: it runs behind a live frame loop, so an application-tier step
    // that stalls or panics costs a late config instead of a page that never
    // paints.
    //
    // `?boot=inline` restores the old order as an escape hatch and as the gate's
    // control; see `boot_inline_requested`. Nothing in the product sets it.
    //
    // **The boot surface does not come down here.** It comes down when phase 2
    // finishes, so the user still sees exactly one transition and the deferral
    // costs no flicker — see `boot_progress`, which owns that hand-over and
    // bounds it.
    let inline = boot_inline_requested();
    let mut deferred_plan = None;
    if inline {
        // The network read is awaited holding NO borrow even on the inline path.
        // It costs nothing here (no loop is running yet) and it keeps ONE call
        // shape: the borrow discipline the deferred path depends on is exercised
        // by this branch too, rather than living only down the path the control
        // never runs.
        let read = app::EntityApp::boot_document_read().await;
        app.borrow_mut().boot_phase2(boot_plan, read).await;
    } else {
        deferred_plan = Some(boot_plan);
    }

    // requestAnimationFrame loop.
    let callback: std::rc::Rc<std::cell::RefCell<Option<Closure<dyn FnMut()>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let callback_clone = callback.clone();
    let app_clone = app.clone();

    *callback_clone.borrow_mut() = Some(Closure::wrap(Box::new(move || {
        // C1 (rAF panic-resilience, D13/AP3): schedule the NEXT frame
        // FIRST, before running frame(). A panic in frame() that started
        // this whole arc (Content Site arm-split, 00016bb) froze the app
        // precisely because the reschedule used to come *after* frame() —
        // a panic skipped it and the loop died. Rescheduling first means
        // the loop survives a frame panic regardless of panic strategy.
        if let Some(w) = web_sys::window() {
            if let Some(ref cb) = *callback.borrow() {
                w.request_animation_frame(cb.as_ref().unchecked_ref()).ok();
            }
        }

        // Frozen-frame watchdog heartbeat (#3): posted BEFORE frame() so a
        // frame that then stalls stops the beats and is detected. Throttled
        // to ~1 Hz internally — cheap. No-op when disabled.
        watchdog::beat();

        let frame_start = js_sys::Date::now();
        #[cfg(feature = "measurement")]
        frame_counters::frame_start();
        // Contain a frame() panic so the unwind doesn't escape the rAF
        // callback. Under the release `panic = "unwind"` profile this
        // catches cleanly — the RefCell borrow guard drops (destructors
        // run), so the next frame can borrow again and the app recovers.
        // Under panic=abort (dev) it's a passthrough; the reschedule above
        // is the backstop. Also guards RefCell double-borrow.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match app_clone.try_borrow_mut() {
                Ok(mut app) => app.frame(),
                Err(_) => {
                    tracing::error!("FRAME SKIP — app RefCell already borrowed");
                }
            }
        }));
        if outcome.is_err() {
            // The panic hook already logged the message + backtrace.
            tracing::error!(
                "FRAME PANIC — frame() panicked; rAF loop continues (next frame \
                 already scheduled). See the panic above."
            );
        }

        let frame_elapsed = js_sys::Date::now() - frame_start;
        #[cfg(feature = "measurement")]
        frame_counters::frame_end_and_log(frame_elapsed);
        if frame_elapsed > 50.0 {
            tracing::error!(
                elapsed_ms = frame_elapsed,
                "FRAME STALL — frame() took >50ms"
            );
        }
    }) as Box<dyn FnMut()>));

    // Kick off the first frame.
    if let Some(ref cb) = *callback_clone.borrow() {
        web_sys::window()
            .unwrap()
            .request_animation_frame(cb.as_ref().unchecked_ref())
            .ok();
    }

    // The frame loop is live from here — the app is rendering BEHIND the boot
    // surface. This is `boot_progress`'s ONE structural signal and it is what
    // makes the application tier non-fatal: past this line a phase-2 step that
    // stalls, panics or hangs has a live frame loop, an installed watchdog and a
    // reachable recovery hatch under it. It also arms the hold failsafe, so the
    // surface comes down in bounded time even if phase 2 never reports back.
    //
    // It does NOT take the surface down. The startup surface is decided in phase
    // 2 — behind the deployment document and the supersession adoption, which is
    // a data dependency, not a preference — so painting a guessed surface here
    // and correcting it a moment later is precisely the flicker. See
    // `boot_progress`'s module note.
    boot_progress::frame_loop_live();

    let boot_ms = perf_now() - wasm_start_ms;
    tracing::info!(
        boot_phase = "rafloop_armed",
        boot_ms,
        "Frame loop started (cold-start timing)"
    );

    // The deferred half — the shipped order. Spawned AFTER `frame_loop_live()`,
    // which is the property itself and not a formality: phase 2 runs with a live
    // frame loop behind it, so its worst outcome is a config that never arrives
    // rather than an app that never starts.
    //
    // The document read holds no borrow. The apply that follows takes
    // `borrow_mut()` and holds it across its LOCAL tree awaits — a stated bound,
    // not an oversight: `FRAME SKIP` names it in the log if it ever gets long,
    // and removing it means giving the phase-2 helpers an owned handle instead of
    // `&Peers` (`boot_phase2`'s doc comment says what that costs).
    //
    // **The hand-over is an RAII guard, not a line at the end (AP44).** A
    // `surface_down()` call in the tail is only reached when phase 2 returns
    // normally, and the two cases that most need the page handed over are the
    // ones that do not return normally: an application-tier panic unwinding out
    // of an await, and an early return added later by someone who did not have
    // this rule in their head. `HandOver` drops on every one of those paths and
    // reports which it was. The failsafe in `boot_progress` is still the outer
    // bound; this is what keeps the ordinary and the panicking boot from both
    // waiting it out.
    if let Some(plan) = deferred_plan {
        let app_phase2 = app.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut handover = boot_progress::HandOver::pending();
            tracing::info!(
                "boot phase 2: DEFERRED — the frame loop is live; the deployment \
                 document is being read behind the boot surface"
            );
            let read = app::EntityApp::boot_document_read().await;
            app_phase2.borrow_mut().boot_phase2(plan, read).await;
            handover.completed();
        });
    } else {
        // The `?boot=inline` control: phase 2 already ran, above, before the loop
        // existed. Nothing is pending, so hand the page over now.
        boot_progress::surface_down("phase 2 inline");
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn perf_now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

#[cfg(target_arch = "wasm32")]
fn perf_mark_age(name: &str) -> Option<f64> {
    let perf = web_sys::window()?.performance()?;
    let entries = perf.get_entries_by_name_with_entry_type(name, "mark");
    let entry = entries.iter().next()?;
    let start = js_sys::Reflect::get(&entry, &JsValue::from_str("startTime"))
        .ok()?
        .as_f64()?;
    Some(perf.now() - start)
}
