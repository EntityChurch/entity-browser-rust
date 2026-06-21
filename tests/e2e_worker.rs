// Browser E2E suite — gated behind the `e2e` cargo feature so a default
// `cargo test` / `make test` is bare-box green (no Selenium). Enable with
// `make e2e-worker` (passes `--features e2e`); needs Selenium on :4444.
#![cfg(feature = "e2e")]
//! E2E browser test for worker-mode boot.
//!
//! Drives a Selenium-standalone-firefox container (via Podman) to load
//! the wasm-worker build and capture console output. Asserts that the
//! app reaches "Frame loop started" without any `panicked at` lines.
//!
//! # Prerequisites
//!
//! 1. The dist/ bundle must already be built (Trunk builds BOTH the
//!    `entity-browser` app bundle and the `entity-worker` worker bundle
//!    from index.html, so one `make wasm` covers both):
//!    ```bash
//!    make wasm
//!    ```
//!    (`make e2e-worker` does this for you, then runs this suite with
//!    `--features e2e`.)
//!
//! 2. A Selenium-standalone-firefox container must be running on
//!    localhost:4444. Use Podman (no host-side install required):
//!    ```bash
//!    podman run -d --rm --name e2e-firefox --network=host \
//!        docker.io/selenium/standalone-firefox:149.0.2-geckodriver-0.36.0-20260404
//!    ```
//!
//!    Then to stop: `podman stop e2e-firefox`.
//!
//! 3. This test starts its own `python3 -m http.server` against `dist/`
//!    on a dedicated port (8092, overridable via `E2E_HTTP_PORT`) —
//!    deliberately NOT 8081, so it never collides with a developer's
//!    `make serve` running in parallel.
//!
//! # Running
//!
//! ```bash
//! cargo test --test e2e_worker -- --nocapture
//! ```
//!
//! The `--nocapture` flag lets you see the captured browser console
//! output, which is the most useful signal for diagnosing what failed.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fantoccini::{Client, ClientBuilder};
use tokio::time::sleep;

/// Dedicated test port — NOT 8081 (that's `make serve`, which a dev
/// may be running for phone testing). Override via `E2E_HTTP_PORT`.
fn http_server_port() -> u16 {
    std::env::var("E2E_HTTP_PORT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(8092)
}
const WEBDRIVER_URL: &str = "http://localhost:4444";
const TAURI_BIN: &str = "./src-tauri/target/debug/entity-browser-tauri";

/// Holds a child `python3 -m http.server` process for the duration of a
/// test. `kill()` is called on drop, so panics in the test still clean
/// up the server.
struct DistServer(Child);

impl Drop for DistServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_dist_server() -> Result<DistServer, std::io::Error> {
    let child = Command::new("python3")
        .args([
            "-m",
            "http.server",
            &http_server_port().to_string(),
            "--directory",
            "dist",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(DistServer(child))
}

/// Tauri subprocess running with `ENTITY_BROWSER_AUTOSTART_LISTENER=1`.
/// Boots a native backend peer with a WebSocket listener bound to
/// 127.0.0.1:4041 (or a fallback port if 4041 is taken) and prints a
/// single `ENTITY_BACKEND_LISTENER_READY peer_id=X ws_addr=Y` line to
/// stdout. Phase 14 uses ws_addr as the ConnectPeer target.
///
/// Also opens a WebView. The console bridge in `src-tauri/src/lib.rs`
/// forwards WASM console output to the same stdout we're already
/// scraping, so we can also detect when the WebView's UI booted
/// successfully (the WASM logs `Frame loop started` on its rAF
/// pump). We wait for both signals — autostart and WebView boot
/// are independent paths but production users want both healthy.
/// If the WebView load fails (e.g. OPFS init on WebKitGTK when dist/
/// is accidentally worker-mode WASM, §3.6), `webview_booted` stays
/// false and Phase 14 fails loudly.
struct TauriListener {
    child: Child,
    pub peer_id: String,
    pub ws_addr: String,
    pub webview_booted: bool,
}

impl Drop for TauriListener {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_tauri_listener() -> Result<TauriListener, Box<dyn std::error::Error>> {
    let mut child = Command::new(TAURI_BIN)
        .env("ENTITY_BROWSER_AUTOSTART_LISTENER", "1")
        // Keep the listener on loopback for the test — the production
        // default now binds 0.0.0.0 and reports the LAN IP (phone
        // pairing), but this suite asserts against a same-host connect.
        .env("ENTITY_BROWSER_LOOPBACK_ONLY", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            format!(
                "failed to spawn Tauri binary at {TAURI_BIN}: {e}\n\
                Build it first: cd src-tauri && cargo build"
            )
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or("tauri subprocess has no stdout")?;

    // Background thread forwards each stdout line through a channel.
    // We need this rather than a blocking read_line() loop so we can
    // time out cleanly when autostart fails to produce the READY line.
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    // Wait for both: the autostart READY line (native listener up)
    // AND the WebView's "Frame loop started" message (UI booted).
    // WebView load is independent from autostart but production users
    // want both healthy — if dist/ holds the wrong WASM flavor (e.g.
    // worker-mode on WebKitGTK), the UI fails to boot while autostart
    // still succeeds. We want the test to catch that.
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut peer_id: Option<String> = None;
    let mut ws_addr: Option<String> = None;
    let mut webview_booted = false;
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                if let Some(rest) = line.strip_prefix("ENTITY_BACKEND_LISTENER_READY ") {
                    for tok in rest.trim().split(' ') {
                        if let Some(v) = tok.strip_prefix("peer_id=") {
                            peer_id = Some(v.to_string());
                        } else if let Some(v) = tok.strip_prefix("ws_addr=") {
                            ws_addr = Some(v.to_string());
                        }
                    }
                }
                if line.starts_with("ENTITY_BACKEND_LISTENER_FAILED") {
                    return Err(format!("tauri autostart failed: {line}").into());
                }
                // WASM logs this from src/main.rs:163 once the rAF
                // pump is running. Routed through the console bridge
                // in src-tauri/src/lib.rs which forwards to stdout.
                if line.contains("Frame loop started") {
                    webview_booted = true;
                }
                if peer_id.is_some() && webview_booted {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let peer_id = peer_id.ok_or_else(|| {
        "tauri did not print ENTITY_BACKEND_LISTENER_READY within 20s. \
         Check that DISPLAY is set (Tauri needs a window server) and \
         that the binary was rebuilt after the autostart hook was added."
            .to_string()
    })?;
    let ws_addr = ws_addr.ok_or("READY line was missing ws_addr=...")?;

    Ok(TauriListener {
        child,
        peer_id,
        ws_addr,
        webview_booted,
    })
}

/// Fetch the captured browser console as a flat `Vec<String>`, one
/// line per console entry. Used by every assertion below.
async fn capture_log(client: &Client) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let raw = client
        .execute("return window.__entity_browser_log || [];", vec![])
        .await?;
    let entries = raw.as_array().cloned().unwrap_or_default();
    Ok(entries
        .iter()
        .filter_map(|e| {
            let args = e.get("args")?.as_array()?;
            Some(
                args.iter()
                    .filter_map(|a| a.as_str().map(String::from))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        })
        .collect())
}

/// Read the most recent `system/query count → N` integer from the
/// (shared) event-log `<pre>`s. The request line is
/// `→ system/query count` (no trailing number); only the result line
/// has ` → <digits>`, so the regex is unambiguous. Returns -1 if no
/// result line is present. Used by the Phase 13.5 peer-scoping gate.
async fn read_last_query_count(client: &Client) -> Result<i64, Box<dyn std::error::Error>> {
    let v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            let txt = '';
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.event-log')) continue;
                const pres = sec.querySelectorAll('pre');
                for (const p of pres) txt += p.textContent + '\n';
            }
            const re = /system\/query count → (\d+)/g;
            let m, last = null;
            while ((m = re.exec(txt)) !== null) last = m[1];
            return last === null ? -1 : parseInt(last, 10);
            "#,
            vec![],
        )
        .await?;
    Ok(v.as_i64().unwrap_or(-1))
}

// =========================================================================
// Shell-driven test helpers
// =========================================================================
//
// The Shell window turns "test that a feature works" into "run a verb,
// inspect scrollback." Each helper here collapses a 30–60-line JS blob
// (find shadow-DOM section, set input value, dispatch keydown, sleep,
// read scrollback) into a one-line Rust call.
//
// Usage pattern:
// ```rust
// let sb = shell_submit(&client, "pwd", 200).await?;
// assert!(sb.contains("/"), "pwd should print the working directory");
// ```
//
// Prereqs: a Shell window must already be open. Phase 2 opens every
// registered window type, including Shell; tests that run after Phase 2
// have a Shell ready. Helpers panic with a clear message if not found —
// they're not meant for "is the Shell present" checks (use the shadow-
// DOM probe directly for those).

/// Read the current scrollback `<pre>` text content from the first
/// Shell window. Returns the empty string if no Shell window is open.
async fn shell_scrollback(client: &Client) -> Result<String, Box<dyn std::error::Error>> {
    let v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const title = sec.querySelector('header h3');
                if (title && title.textContent.trim() === 'Shell') {
                    const pre = sec.querySelector("[data-field='shell-scrollback']");
                    return pre ? pre.textContent : '';
                }
            }
            return '';
            "#,
            vec![],
        )
        .await?;
    Ok(v.as_str().unwrap_or("").to_string())
}

/// The shell scrollback is append-only — every command's prompt + output
/// stays in the `<pre>` forever. So a whole-history `contains` check can't
/// test for *absence* (a stale earlier line still matches). This slices the
/// scrollback to just the output of the LAST invocation of `cmd` — the text
/// after the final `> {cmd}` prompt echo — so "after rm, the listing no
/// longer shows X" is a faithful assertion. Returns the whole scrollback if
/// the command echo isn't found (so the caller's assert fails loudly).
fn last_shell_output<'a>(scrollback: &'a str, cmd: &str) -> &'a str {
    let marker = format!("> {cmd}");
    match scrollback.rfind(&marker) {
        Some(i) => &scrollback[i + marker.len()..],
        None => scrollback,
    }
}

/// Type `line` into the first Shell window's input and submit it via
/// keydown Enter. Sleeps `settle_ms` (typical 200–500ms for sync verbs,
/// 800–1500ms for async verbs like `exec` / `count` / `connect`) and
/// returns the post-submit scrollback text.
///
/// Panics if no Shell section / input is found — Phase 2 must have run
/// (or the caller must have opened a Shell window) first.
async fn shell_submit(
    client: &Client,
    line: &str,
    settle_ms: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let v = client
        .execute(
            r#"
            const [line] = arguments;
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            let shellSection = null;
            for (const sec of sections) {
                const title = sec.querySelector('header h3');
                if (title && title.textContent.trim() === 'Shell') {
                    shellSection = sec;
                    break;
                }
            }
            if (!shellSection) return { ok: false, reason: 'no-shell-section' };
            const input = shellSection.querySelector("[data-field='shell-input']");
            if (!input) return { ok: false, reason: 'no-shell-input' };
            input.value = line;
            const evt = new KeyboardEvent('keydown', {
                key: 'Enter',
                code: 'Enter',
                bubbles: true,
                cancelable: true,
            });
            input.dispatchEvent(evt);
            return { ok: true };
            "#,
            vec![serde_json::Value::String(line.to_string())],
        )
        .await?;
    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
    if !ok {
        let reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("");
        panic!(
            "shell_submit({line:?}) failed: {reason}. \
             Did Phase 2 open a Shell window? Is `views::shell` registered?"
        );
    }
    sleep(Duration::from_millis(settle_ms)).await;
    shell_scrollback(client).await
}

/// Peer-scoped variant of `shell_scrollback`. Returns the scrollback
/// `<pre>` text from the Shell window whose section carries
/// `data-peer-id="<peer_id>"`. Returns the empty string if no matching
/// Shell window is open. Used by the cross-Worker e2e flow where
/// multiple Shells are open simultaneously, each bound to a different
/// backend Worker peer.
async fn shell_scrollback_for_peer(
    client: &Client,
    peer_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let v = client
        .execute(
            r#"
            const [pid] = arguments;
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sec = root.querySelector(`section.window[data-peer-id="${pid}"]`);
            if (!sec) return '';
            const title = sec.querySelector('header h3');
            if (!title || title.textContent.trim() !== 'Shell') return '';
            const pre = sec.querySelector("[data-field='shell-scrollback']");
            return pre ? pre.textContent : '';
            "#,
            vec![serde_json::Value::String(peer_id.to_string())],
        )
        .await?;
    Ok(v.as_str().unwrap_or("").to_string())
}

/// Peer-scoped variant of `shell_submit`. Targets the Shell window
/// bound to `peer_id` (via `data-peer-id`). Same `settle_ms` /
/// scrollback-return contract as `shell_submit`.
async fn shell_submit_for_peer(
    client: &Client,
    peer_id: &str,
    line: &str,
    settle_ms: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let v = client
        .execute(
            r#"
            const [pid, line] = arguments;
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sec = root.querySelector(`section.window[data-peer-id="${pid}"]`);
            if (!sec) return { ok: false, reason: 'no-section-for-peer' };
            const title = sec.querySelector('header h3');
            if (!title || title.textContent.trim() !== 'Shell') {
                return { ok: false, reason: 'section-not-shell' };
            }
            const input = sec.querySelector("[data-field='shell-input']");
            if (!input) return { ok: false, reason: 'no-shell-input' };
            input.value = line;
            const evt = new KeyboardEvent('keydown', {
                key: 'Enter', code: 'Enter', bubbles: true, cancelable: true,
            });
            input.dispatchEvent(evt);
            return { ok: true };
            "#,
            vec![
                serde_json::Value::String(peer_id.to_string()),
                serde_json::Value::String(line.to_string()),
            ],
        )
        .await?;
    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
    if !ok {
        let reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("");
        panic!(
            "shell_submit_for_peer({peer_id}, {line:?}) failed: {reason}. \
             Has a Shell been opened on this peer? \
             (`open shell @<peer-id>` in the primary shell)"
        );
    }
    sleep(Duration::from_millis(settle_ms)).await;
    shell_scrollback_for_peer(client, peer_id).await
}

/// Submit `line` and assert each substring in `expects` appears in the
/// resulting scrollback. Returns the scrollback for further inspection.
///
/// On mismatch, prints the full scrollback in the failure message so
/// the diagnostic explains what the shell *did* say, not just that the
/// expectation didn't match.
#[allow(dead_code)]
async fn shell_expect(
    client: &Client,
    line: &str,
    expects: &[&str],
    settle_ms: u64,
) -> Result<String, Box<dyn std::error::Error>> {
    let sb = shell_submit(client, line, settle_ms).await?;
    for needle in expects {
        assert!(
            sb.contains(needle),
            "shell `{line}` scrollback missing {needle:?}.\nscrollback was:\n{sb}"
        );
    }
    Ok(sb)
}

/// Print captured log with indices + level. Used in both pass and fail
/// paths; the console is the primary diagnostic signal.
fn print_log(lines: &[String]) {
    println!("===== Captured browser console ({} entries) =====", lines.len());
    for (i, line) in lines.iter().enumerate() {
        // Strip the noisy tracing-wasm CSS color codes for readability.
        let cleaned = line
            .replace("%cINFO%c", "INFO ")
            .replace("%cWARN%c", "WARN ")
            .replace("%cERROR%c", "ERR  ")
            .replace("%cDEBUG%c", "DBG  ")
            .replace("%cTRACE%c", "TRC  ")
            .replace("%c", "")
            .replace("color: whitesmoke; background: #444 color: gray; font-style: italic color: inherit", "")
            .replace("color: gray; font-style: italic", "")
            .replace("color: inherit", "")
            .replace("color: whitesmoke; background: #444", "");
        println!("  [{i:>3}] {cleaned}");
    }
    println!("===== End =====");
}

fn count_panics(lines: &[String]) -> Vec<&String> {
    lines
        .iter()
        .filter(|l| l.contains("panicked at") || l.contains("Uncaught RuntimeError"))
        .collect()
}

/// Poll the captured browser console until `"Frame loop started"`
/// appears (boot finished) or `timeout_ms` elapses. Used to replace
/// fixed `sleep(Duration::from_secs(6))` after `client.refresh()`.
/// Sleeps 100ms between polls so we don't hammer the WebDriver.
async fn wait_for_boot(
    client: &Client,
    timeout_ms: u64,
) -> Result<u64, Box<dyn std::error::Error>> {
    let start = std::time::Instant::now();
    loop {
        let elapsed = start.elapsed().as_millis() as u64;
        let log = capture_log(client).await?;
        if log.iter().any(|l| l.contains("Frame loop started")) {
            return Ok(elapsed);
        }
        if elapsed > timeout_ms {
            return Err(format!(
                "wait_for_boot: never saw 'Frame loop started' in {timeout_ms}ms"
            )
            .into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// Poll the SDK-count footer until it reaches `expected` or `timeout_ms`
/// elapses. Used to replace fixed sleeps after `+ Backend (...)` clicks
/// while the spawned worker is still initializing on its own thread.
async fn wait_for_sdk_count(
    client: &Client,
    script: &str,
    expected: i64,
    timeout_ms: u64,
) -> Result<i64, Box<dyn std::error::Error>> {
    let start = std::time::Instant::now();
    loop {
        let elapsed = start.elapsed().as_millis() as u64;
        let v = client.execute(script, vec![]).await?;
        let last = v.as_i64().unwrap_or(-1);
        if last == expected {
            return Ok(last);
        }
        if elapsed > timeout_ms {
            return Err(format!(
                "wait_for_sdk_count: expected {expected}, last={last} after {timeout_ms}ms"
            )
            .into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// Probe whether `workers/{peer_id}/` exists in OPFS. Returns "exists",
/// "missing", or "no-workers-dir" (for the case where no Backend(OPFS)
/// peer has ever been created on this origin and the parent dir hasn't
/// been instantiated yet).
async fn check_opfs_workers_subdir(
    client: &Client,
    peer_id: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let script = format!(
        r#"
        const cb = arguments[arguments.length - 1];
        (async () => {{
            try {{
                const root = await navigator.storage.getDirectory();
                let workers;
                try {{
                    workers = await root.getDirectoryHandle('workers', {{ create: false }});
                }} catch (e) {{
                    return 'no-workers-dir';
                }}
                try {{
                    await workers.getDirectoryHandle('{peer_id}', {{ create: false }});
                    return 'exists';
                }} catch (e) {{
                    return 'missing';
                }}
            }} catch (e) {{
                return 'error:' + (e && e.message ? e.message : String(e));
            }}
        }})().then(r => cb(r), e => cb('threw:' + String(e)));
        "#,
    );
    let v = client.execute_async(&script, vec![]).await?;
    Ok(v.as_str().unwrap_or("non-string").to_string())
}

/// Set up a fresh test environment: start the dist server and connect
/// the WebDriver client to the Selenium-firefox container. Returns the
/// client + a guard that kills the server on drop.
async fn setup(
) -> Result<(Client, DistServer), Box<dyn std::error::Error>> {
    // Defensive: Phase 27 drops a `dist/entity-deployment.json` to test the
    // served-config boot path, then removes it. If a prior run crashed mid-phase
    // it could linger and silently change EVERY earlier phase's cold boot (the
    // served config applies when no durable config exists). Remove it before any
    // phase navigates so phases 1–26 always see the default (no-config) path.
    let _ = std::fs::remove_file("dist/entity-deployment.json");

    let server = start_dist_server().map_err(|e| {
        format!(
            "failed to start python3 -m http.server: {e}. \
             Is dist/ built? Run `make wasm` first (or `make e2e-worker`)."
        )
    })?;
    sleep(Duration::from_millis(300)).await;

    let mut caps = serde_json::Map::new();
    caps.insert(
        "moz:firefoxOptions".to_string(),
        serde_json::json!({ "args": ["-headless"] }),
    );
    let client = ClientBuilder::native()
        .capabilities(caps)
        .connect(WEBDRIVER_URL)
        .await
        .map_err(|e| {
            format!(
                "failed to connect to WebDriver at {WEBDRIVER_URL}: {e}\n\
                Is the selenium-firefox container running? Try:\n\
                podman run -d --rm --name e2e-firefox --network=host \\\n\
                    docker.io/selenium/standalone-firefox:149.0.2-geckodriver-0.36.0-20260404"
            )
        })?;
    Ok((client, server))
}

/// Single combined E2E test: bootstrap + open every window type, all in
/// one Firefox session against one http.server. Consolidated because
/// `cargo test` runs tests in parallel by default and both bootstrap +
/// window tests collide on port 8081 / on the single container's
/// session capacity. Splitting into multiple tests would require
/// `--test-threads=1` or independent port allocation — keeping it as
/// one test is simpler and faster (one setup, one teardown).
#[tokio::test(flavor = "current_thread")]
async fn worker_boots_and_opens_all_windows() -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    // `?worker=1` selects Worker mode at runtime (Stage 1A).
    // Without the query param the bundle now boots Direct mode by default.
    client
        // `?log=trace` keeps the `peers_worker dispatch_write: put ok`
        // TRACE line on; without it the new INFO/DEBUG default (see
        // `configured_log_level` in src/main.rs) suppresses the line
        // the Settings phase counts to verify writes happened.
        .goto(&format!("http://localhost:{}/?worker=1&log=trace", http_server_port()))
        .await?;
    // Give the app time to spawn the worker, complete the Init
    // handshake, and run a few frames.
    sleep(Duration::from_secs(5)).await;

    // -- Phase 1: bootstrap assertions ---------------------------------
    {
        let log_lines = capture_log(&client).await?;
        let has_frame_loop = log_lines.iter().any(|l| l.contains("Frame loop started"));
        let has_ready = log_lines
            .iter()
            .any(|l| l.contains("Ready handshake complete"));
        let panics = count_panics(&log_lines);

        if !has_ready || !has_frame_loop || !panics.is_empty() {
            print_log(&log_lines);
        }

        assert!(
            has_ready,
            "worker bootstrap did not complete Ready handshake"
        );
        assert!(
            has_frame_loop,
            "app did not reach rAF frame loop"
        );
        assert!(
            panics.is_empty(),
            "bootstrap panic(s):\n{}",
            panics
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n---\n")
        );
    }

    // -- Phase 1b: Service Worker registered ---------------------------
    //
    // Gap 5 (offline app-shell cache): the SW must register on every
    // page load so a subsequent hard-refresh-while-offline serves the
    // cached static assets. Skipping registration silently re-opens
    // the offline-reload-wipes-app symptom. See
    // `assets/sw.js` and the GAP-5 persistence investigation.
    {
        // Wait briefly for the SW to register. Registration is async
        // and fires after `window.load`; the test bootstrap above has
        // already past Ready handshake so we're well past load, but
        // give the SW activation a small grace period.
        let mut sw_ready = false;
        for _ in 0..20 {
            let v = client
                .execute(
                    r#"
                    if (!('serviceWorker' in navigator)) return 'unsupported';
                    if (!navigator.serviceWorker.controller) return 'no-controller-yet';
                    return navigator.serviceWorker.controller.scriptURL || 'controller-no-url';
                    "#,
                    vec![],
                )
                .await?;
            let s = v.as_str().unwrap_or("non-string");
            if s.ends_with("/sw.js") {
                sw_ready = true;
                break;
            }
            if s == "unsupported" {
                panic!("test runtime lacks ServiceWorker API — fantoccini Firefox should support this");
            }
            sleep(Duration::from_millis(250)).await;
        }
        assert!(
            sw_ready,
            "Service Worker did not register / take control within 5s"
        );
    }

    // -- Phase 2: open every window type, one at a time ----------------
    //
    // Per D10 (feedback_e2e_must_exercise_new_features): discover the
    // spawn list from the DOM instead of hard-coding. Hard-coded
    // arrays let a new window silently skip the loop and the test
    // still report green (caught a case when shell wasn't
    // deployed). Discovery returns labels in palette DOM order, which
    // already preserves System-then-Peer scoping by construction of
    // the palette `<details>` sections.
    //
    // The discovered set is sanity-checked against a known-minimum
    // floor — if the registry collapsed below that count, the test
    // fails loudly rather than passing on an empty iteration.
    let window_types: Vec<String> = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            if (!layer) return [];
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            const labels = [];
            for (const b of btns) {
                const t = b.textContent.trim();
                // Buttons are "+ <Name>". Strip the leading "+ ".
                if (t.startsWith('+ ')) labels.push(t.slice(2));
                else labels.push(t);
            }
            return labels;
            "#,
            vec![],
        )
        .await?
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // Floor sanity check: today we ship 19 window types. The Inspect
    // family (Chain Trace, Path Tap, Wire Recorder, Content Stream)
    // landed; Content Site, then the JS-Apps platform (Games
    // 16th, Apps 17th), the Storage window (18th), and the Site Editor
    // (19th) landed later. If the DOM returns 0 we're parsing
    // wrong; if it returns far fewer than expected we've silently
    // regressed the palette renderer or dropped a window. Update the floor
    // on intentional removals.
    const MIN_DISCOVERED_WINDOW_TYPES: usize = 19;
    assert!(
        window_types.len() >= MIN_DISCOVERED_WINDOW_TYPES,
        "Phase 2: discovered only {} window types ({:?}); expected at least {}. \
         Either the palette renderer regressed or the floor needs lowering for \
         an intentional removal.",
        window_types.len(),
        window_types,
        MIN_DISCOVERED_WINDOW_TYPES
    );

    eprintln!(
        "Phase 2: exercising {} discovered window types: {:?}",
        window_types.len(),
        window_types
    );

    let mut spawn_failures: Vec<String> = Vec::new();
    let mut panic_at: Option<(String, Vec<String>)> = None;

    for name in &window_types {
        // Click the spawn button via JS. Two complications:
        // 1. The DOM renderer mounts its UI inside a Shadow DOM on
        //    `#dom-layer` for style isolation, so `document.querySelector`
        //    can't see the buttons — must go through `.shadowRoot`.
        // 2. The palette is a collapsed `<details>` element, but
        //    `.click()` works on the button regardless of expansion.
        // Buttons are `<button class="spawn-btn">+ <Name></button>`.
        let script = format!(
            r#"
            const layer = document.getElementById('dom-layer');
            if (!layer) return 'no-dom-layer';
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            for (const b of btns) {{
                if (b.textContent.trim() === '+ {name}') {{
                    b.click();
                    return 'clicked';
                }}
            }}
            return `no-match-of-${{btns.length}}-buttons`;
            "#
        );
        let result = client.execute(&script, vec![]).await?;
        let status = result.as_str().unwrap_or("non-string");
        if status != "clicked" {
            spawn_failures.push(format!("'{name}': {status}"));
            continue;
        }

        sleep(Duration::from_millis(800)).await;

        let log_lines = capture_log(&client).await?;
        let panics = count_panics(&log_lines);
        if !panics.is_empty() {
            panic_at = Some((
                name.to_string(),
                panics.into_iter().cloned().collect(),
            ));
            break;
        }
    }

    // -- Phase 2-SE: Site Editor create flow (Commit 2) ---------------
    //
    // Exercise the new Site Editor end to end on the Worker arm (the
    // doctrine-mandated arm for a tree-writing feature): drive the
    // create form, then assert the editor reflects the new site —
    // listed, selected, render-healthy, and its seeded index body in the
    // textarea. This proves the write (seed_write → worker dispatch), the
    // subscription reflect (the sites_prefix watch rebuilds the section),
    // and the health read-back. The cross-window "browser renders it" leg
    // is pinned natively (created_site_renders_through_the_content_site_model);
    // here we keep the e2e to the editor's own observable surface.
    //
    // NB: the create-form inputs are draft-backed (tracked_input), so a
    // bare `.value =` is invisible to the model — we MUST dispatch an
    // `input` event so the draft map captures it before clicking Create.
    //
    // The create form now lives behind a "New site" expander (collapsed by
    // default). Open it first; toggling routes through an Action → rebuild, so
    // give it a frame before the form is in the DOM.
    client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.site-editor')) continue;
                if (sec.querySelector('input[data-field="new_site_id"]')) return { already: true };
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.includes('New site')) { b.click(); return { opened: true }; }
                }
            }
            return { opened: false };
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(800)).await;
    let se_created_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.site-editor')) continue;
                const id = sec.querySelector('input[data-field="new_site_id"]');
                const title = sec.querySelector('input[data-field="new_site_title"]');
                if (!id || !title) return { ok: false, reason: 'no-create-form' };
                id.value = 'e2e-site';
                id.dispatchEvent(new Event('input', { bubbles: true }));
                title.value = 'E2E Site';
                title.dispatchEvent(new Event('input', { bubbles: true }));
                const btns = sec.querySelectorAll('button');
                for (const b of btns) {
                    if (b.textContent.trim() === 'Create site') {
                        b.click();
                        return { ok: true };
                    }
                }
                return { ok: false, reason: 'no-create-btn' };
            }
            return { ok: false, reason: 'no-site-editor-section' };
            "#,
            vec![],
        )
        .await?;
    let se_created = se_created_v.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    println!("  site-editor create dispatched: {se_created}");
    assert!(se_created, "Could not drive Site Editor create flow. Detail: {se_created_v}");
    sleep(Duration::from_millis(1200)).await;

    let se_state_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.site-editor')) continue;
                const chips = Array.from(sec.querySelectorAll('button')).map(b => b.textContent.trim());
                const ta = sec.querySelector('textarea[data-field="body::e2e-site::index"]');
                // Render health is now a ✓/⚠ glyph (with a title tooltip) next to
                // each site in the list — a renderable site exposes a ✓ tip.
                const renders = Array.from(sec.querySelectorAll('span[title]'))
                    .some(s => (s.getAttribute('title') || '').includes('Renders'));
                return {
                    lists_site: chips.some(c => c.includes('e2e-site')),
                    renders,
                    body_seeded: !!ta && ta.value.includes('E2E Site'),
                };
            }
            return { lists_site: false, renders: false, body_seeded: false };
            "#,
            vec![],
        )
        .await?;
    let lists_site = se_state_v.get("lists_site").and_then(|v| v.as_bool()).unwrap_or(false);
    let renders = se_state_v.get("renders").and_then(|v| v.as_bool()).unwrap_or(false);
    let body_seeded = se_state_v.get("body_seeded").and_then(|v| v.as_bool()).unwrap_or(false);
    println!("  site-editor lists/renders/body: {lists_site}/{renders}/{body_seeded}");
    assert!(lists_site, "Site Editor did not list the created site after create. Detail: {se_state_v}");
    assert!(renders, "Site Editor did not report the new site as render-healthy. Detail: {se_state_v}");
    assert!(body_seeded, "Site Editor did not show the seeded index body. Detail: {se_state_v}");

    // Add a page, then delete the whole site — exercising add + delete on the
    // Worker arm. The delete button is confirm-guarded; override window.confirm
    // so the destructive path actually fires under automation.
    let se_added_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.site-editor')) continue;
                const slug = sec.querySelector('input[data-field="new_page_slug"]');
                if (!slug) return { ok: false, reason: 'no-add-input' };
                slug.value = 'about';
                slug.dispatchEvent(new Event('input', { bubbles: true }));
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.trim() === '+ Add page') { b.click(); return { ok: true }; }
                }
                return { ok: false, reason: 'no-add-btn' };
            }
            return { ok: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert!(
        se_added_v.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "Could not drive Site Editor add-page. Detail: {se_added_v}"
    );
    sleep(Duration::from_millis(1000)).await;

    let se_after_add = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.site-editor')) continue;
                const chips = Array.from(sec.querySelectorAll('button')).map(b => b.textContent.trim());
                return { has_about: chips.some(c => c.includes('about')) };
            }
            return { has_about: false };
            "#,
            vec![],
        )
        .await?;
    assert!(
        se_after_add.get("has_about").and_then(|v| v.as_bool()).unwrap_or(false),
        "Site Editor did not show the added page. Detail: {se_after_add}"
    );

    // Rename/move the just-added page (R3): adding it selected it, so the page
    // editor is open with its Title field (R2) + Move row (R3). Drive the Move
    // input → click Move; the page list should then show the new slug. This
    // exercises the new authoring actions through the real DOM on the Worker arm.
    let se_renamed_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.site-editor')) continue;
                const title = sec.querySelector('input[data-field="title::e2e-site::about"]');
                const move = sec.querySelector('input[data-field="rename::e2e-site::about"]');
                if (!title) return { ok: false, reason: 'no-title-field' };
                if (!move) return { ok: false, reason: 'no-move-field' };
                move.value = 'guide/info';
                move.dispatchEvent(new Event('input', { bubbles: true }));
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.trim() === 'Move') { b.click(); return { ok: true }; }
                }
                return { ok: false, reason: 'no-move-btn' };
            }
            return { ok: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert!(
        se_renamed_v.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "Could not drive Site Editor rename/move (R2 title + R3 move). Detail: {se_renamed_v}"
    );
    sleep(Duration::from_millis(1200)).await;

    let se_after_rename = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.site-editor')) continue;
                const chips = Array.from(sec.querySelectorAll('button')).map(b => b.textContent.trim());
                // The 'guide' folder now holds the moved page; 'about' is gone
                // from the root listing.
                return { has_guide: chips.some(c => c.includes('guide')),
                         lost_about: !chips.some(c => c.includes('about')) };
            }
            return { has_guide: false, lost_about: false };
            "#,
            vec![],
        )
        .await?;
    println!("  site-editor after rename/move: {se_after_rename}");
    assert!(
        se_after_rename.get("has_guide").and_then(|v| v.as_bool()).unwrap_or(false)
            && se_after_rename.get("lost_about").and_then(|v| v.as_bool()).unwrap_or(false),
        "Site Editor did not reflect the rename/move. Detail: {se_after_rename}"
    );

    let se_deleted_v = client
        .execute(
            r#"
            window.confirm = () => true;   // auto-accept the destructive confirm
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.site-editor')) continue;
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.trim() === 'Delete site') { b.click(); return { ok: true }; }
                }
                return { ok: false, reason: 'no-delete-btn' };
            }
            return { ok: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert!(
        se_deleted_v.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "Could not drive Site Editor delete-site. Detail: {se_deleted_v}"
    );
    sleep(Duration::from_millis(1200)).await;

    let se_after_delete = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.site-editor')) continue;
                const chips = Array.from(sec.querySelectorAll('button')).map(b => b.textContent.trim());
                return { still_lists: chips.some(c => c.includes('e2e-site')) };
            }
            return { still_lists: false };
            "#,
            vec![],
        )
        .await?;
    println!("  site-editor after delete still-lists: {se_after_delete}");
    assert!(
        !se_after_delete.get("still_lists").and_then(|v| v.as_bool()).unwrap_or(true),
        "Site Editor still lists the deleted site (subgraph delete didn't reflect). Detail: {se_after_delete}"
    );

    // -- Phase 2b: Key Manager renders the real registry roster -------
    //
    // Regression gate for the peer-registry-in-tree migration
    // (Phase 3). Key Manager
    // was a placeholder with hard-coded keys ("Local Identity" /
    // "QmYWZ..."); it is now a Pass-through window over the tree
    // registry. With all windows open the system peer is always in the
    // roster, so its table must show a real row (role "system") and
    // must NOT contain any of the old hard-coded placeholder strings.
    let km_text_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Key Manager') continue;
                const t = sec.querySelector('table');
                return t ? t.textContent : 'no-table';
            }
            return 'no-section';
            "#,
            vec![],
        )
        .await?;
    let km_text = km_text_v.as_str().unwrap_or("?");
    println!("  key-manager table text: {km_text:?}");
    assert!(
        !km_text.contains("QmYWZ")
            && !km_text.contains("QmXKR")
            && !km_text.contains("Local Identity")
            && !km_text.contains("Browser Session"),
        "Key Manager still shows hard-coded placeholder keys — registry \
         migration (Phase 3) did not take effect. Got: {km_text:?}"
    );
    assert!(
        km_text.contains("system"),
        "Key Manager table should show the system peer's role from the \
         registry. Got: {km_text:?}"
    );

    // -- Phase 2c: Shell `help` round-trip ----------------------------
    //
    // Catches the case where Shell isn't in the palette, is in the
    // palette but doesn't render, OR renders but its keydown handler
    // doesn't dispatch ShellSubmit. If the spawn-button click in
    // Phase 2 silently no-op'd (stale build, Shell removed by a
    // refactor), `shell_submit` panics with a clear "no-shell-section"
    // diagnostic.
    let help_sb = shell_submit(&client, "help", 500).await?;
    println!("  shell scrollback after help: {help_sb:?}");
    assert!(
        help_sb.contains("help") && help_sb.contains("pwd"),
        "Shell `help` should print verb table including 'help' and \
         'pwd' rows. Scrollback: {help_sb:?}"
    );

    // -- Phase 2d: Shell `open` round-trip through pending_out queue ---
    //
    // Validates the action-out queue: a verb pushes
    // Action::SpawnWindow into ShellModel.pending_out → render_dom
    // drains it into DomCtx.actions → next frame's process_actions
    // spawns the window. Without the queue plumbing this regresses
    // silently (verb runs, queue holds, nothing spawns).
    let section_count_js = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        return root.querySelectorAll('section.window').length;
        "#;
    let before_open = client
        .execute(section_count_js, vec![])
        .await?
        .as_i64()
        .unwrap_or(0);
    // 800ms covers two-frame propagation: ShellSubmit → drain → SpawnWindow.
    let _ = shell_submit(&client, "open Settings", 800).await?;
    let after_open = client
        .execute(section_count_js, vec![])
        .await?
        .as_i64()
        .unwrap_or(0);
    println!(
        "  window count: {before_open} before `open Settings`, {after_open} after"
    );
    assert!(
        after_open > before_open,
        "`open Settings` from the shell should spawn a new window — \
         pre={before_open} post={after_open}. The pending_out queue may \
         not be draining into DomCtx.actions."
    );

    // -- Phase 2e: Shell read-only verb smoke pass ---------------------
    //
    // Demonstrates the shell-driven test pattern: one line per verb,
    // one assert per expectation, no inline DOM blobs. These verbs are
    // cheap (sync, L0 reads) so they validate the dispatcher + a
    // representative slice of the verb surface for ~300ms total.
    //
    // Add new verbs to this block when they ship — much cheaper than
    // wiring DOM click chains.
    let pwd_sb = shell_submit(&client, "pwd", 200).await?;
    assert!(
        pwd_sb.contains('/'),
        "`pwd` should print a path starting with /. Got: {pwd_sb:?}"
    );

    let info_sb = shell_submit(&client, "info", 200).await?;
    assert!(
        info_sb.contains("bound peer") && info_sb.contains("primary arm"),
        "`info` should show bound peer + primary arm rows. Got: {info_sb:?}"
    );

    let ls_sb = shell_submit(&client, "ls", 300).await?;
    // `ls` against wd should list at least one child (app/ or system/
    // is always present under a peer root). Empty wd would print
    // `(empty: ...)` instead.
    assert!(
        ls_sb.contains("/app") || ls_sb.contains("/system") || ls_sb.contains("(empty"),
        "`ls` should list children or report empty. Got: {ls_sb:?}"
    );

    // -- Phase 2f: Shell async query/count verbs -----------------------
    //
    // `query` and `count` exercise the spawn_local future + dirty-mark
    // plumbing through the worker query handler. Both require a type
    // filter (Worker arm rejects empty expressions as InvalidParams —
    // surfaced by this exact test). We query for a type
    // that always exists in a fresh peer's tree: `system/handler` (the
    // handler-registration entries) — every peer has 10+ of these from
    // boot.
    //
    // Write/read round-trip (set + cat + rm) was tried but hits
    // Worker-arm cache timing: the cache mirror is fed by subscription
    // events and a freshly-written entity at a path no window
    // subscribes to doesn't surface to `cat` synchronously. Covering
    // that needs a window subscribing to the test path's prefix first.
    // Left as a future expansion.
    let query_sb = shell_submit(&client, "query system/handler", 1500).await?;
    assert!(
        query_sb.contains('←') && query_sb.contains("match"),
        "`query system/handler` should report a match-count line. \
         Got: {query_sb:?}"
    );

    let count_sb = shell_submit(&client, "count system/handler", 1500).await?;
    // count's result line is `← <n>` (just the number).
    assert!(
        count_sb.contains('←'),
        "`count system/handler` should report a count. Got: {count_sb:?}"
    );

    // Verify the usage-error path for bare `query` (no args). This is
    // the negative case that motivated the recent fix.
    let bare_query_sb = shell_submit(&client, "query", 200).await?;
    assert!(
        bare_query_sb.contains("usage: query"),
        "Bare `query` should print usage. Got: {bare_query_sb:?}"
    );

    // -- Phase 2f.1: compute verbs work on the Worker arm --------------
    //
    // Regression guard for the compute-in-Worker enable.
    // The primary peer here IS a Worker-arm peer, so before the fix all
    // five `compute` verbs short-circuited with "not supported on
    // Worker-arm peer" (the binding bailed when `peer_context()` returned
    // None). They now route through the generic L1 router: eval/install/
    // uninstall dispatch EXECUTE over the worker wire; `list` runs an L1
    // query for the subgraph metadata; `show` an on-demand Get. This
    // phase proves the routing reaches the worker handler end-to-end —
    // the exact thing that was impossible before.
    //
    // `compute list` on a fresh peer is the positive case: it exercises
    // the full Peers::query → worker query handler → decode → scrollback
    // path and must report the (empty) listing, NOT a "not supported"
    // error. A real positive eval (put a `compute/literal` then eval it)
    // is left as a future expansion — it rides the shell JSON-put
    // tokenizer, orthogonal to the routing this guards.
    let compute_list_sb = shell_submit(&client, "compute list", 1500).await?;
    assert!(
        !compute_list_sb.contains("not supported"),
        "`compute list` must not bail with 'not supported on Worker-arm \
         peer' — the compute-in-Worker reroute regressed. Got: {compute_list_sb:?}"
    );
    assert!(
        compute_list_sb.contains("installed subgraphs"),
        "`compute list` on the Worker primary should render the subgraph \
         listing header via the L1 query path. Got: {compute_list_sb:?}"
    );

    // `compute eval` against a bogus relative path proves the EXECUTE
    // routing reaches the worker compute handler: it returns a dispatch
    // error (no expression entity there), which is the handler talking —
    // crucially NOT the pre-fix "not supported on Worker-arm peer".
    let compute_eval_sb = shell_submit(&client, "compute eval no/such/expr", 1500).await?;
    assert!(
        !compute_eval_sb.contains("not supported"),
        "`compute eval` must reach the worker compute handler, not bail \
         with 'not supported on Worker-arm peer'. Got: {compute_eval_sb:?}"
    );

    // -- Phase 2f.3: Worker-arm DELETE reflects into the mirror --------
    //
    // The foundational tree invariant: a write fires subscriptions, and a
    // DELETE fires them too. On the Worker arm, window renders read the
    // subscription cache MIRROR (peers.tree_listing → proxy.cache_list), so
    // if a removal isn't broadcast/applied, the mirror keeps the ghost and
    // the UI lies. This phase is the regression gate for
    // the backend-peer-delete audit, Finding 2 ("creates reflect,
    // deletes don't"): root-caused to decode_notification reading the
    // deleted entity's OLD hash into `new_hash`, so the host shipped a
    // Change{Some(old_blob)} that re-inserted the entry. Fixed in
    // bindings/sdk (honor `new_hash: None on delete`).
    //
    // Faithful because it asserts the MIRROR, not a fresh dispatched get:
    // the Entity Tree window (opened in Phase 1) subscribes to `/{primary}/`
    // — the whole peer tree — so the path under test is covered. The
    // primary peer here is Worker-hosted, so this is the real failing arm.
    // Before the fix the final `ls` still shows the entity (delete didn't
    // reflect); with the fix it's gone.
    //
    // Relative paths (joined with wd = peer root). put/rm route through
    // dispatch_write/dispatch_remove to the worker; the covering
    // subscription round-trips the Created/deleted back into the mirror, so
    // generous settle times follow each mutation.
    let del_put = shell_submit(
        &client,
        "put app/e2e_deltest/marker marker {\"k\":\"v\"}",
        800,
    )
    .await?;
    println!("  delete-reflect put: {del_put:?}");
    let del_ls_present = shell_submit(&client, "ls app/e2e_deltest", 600).await?;
    let present_out = last_shell_output(&del_ls_present, "ls app/e2e_deltest");
    assert!(
        present_out.contains("marker"),
        "after put, the Worker-arm mirror must show the new entity \
         (creates reflect). last `ls app/e2e_deltest` output: {present_out:?}"
    );

    let del_rm = shell_submit(&client, "rm app/e2e_deltest/marker", 800).await?;
    println!("  delete-reflect rm: {del_rm:?}");
    let del_ls_gone = shell_submit(&client, "ls app/e2e_deltest", 600).await?;
    // Scope to the LAST `ls` output — scrollback is cumulative, so the
    // earlier present-listing + the `put`/`rm` command echoes still mention
    // the path and would false-match a whole-history `contains`.
    let gone_out = last_shell_output(&del_ls_gone, "ls app/e2e_deltest");
    assert!(
        !gone_out.contains("marker"),
        "after rm, the Worker-arm mirror must NOT show the removed entity \
         (deletes MUST reflect — the broken invariant this gate guards). \
         last `ls app/e2e_deltest` output: {gone_out:?}"
    );

    // -- Phase 2f.2: browser-native diagnostics capture ----------------
    //
    // Regression guard for sprint #4 (src/diagnostics.rs). An uncaught
    // browser-level error must be captured and routed to the in-app sink
    // (`note` → tracing target `browser_diagnostics` + the Event Log),
    // not lost to the console users never open. We dispatch a synthetic
    // `error` event on window and confirm the installed listener fired and
    // formatted it (the listener → note path is the whole mechanism; note
    // also writes the Event Log entry via the same call).
    client
        .execute(
            r#"window.dispatchEvent(new ErrorEvent('error', {
                   message: 'E2E_DIAG_PROBE', filename: 'probe.js', lineno: 42 }));
               return true;"#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(400)).await;
    let diag_log = capture_log(&client).await?;
    assert!(
        diag_log
            .iter()
            .any(|l| l.contains("E2E_DIAG_PROBE") && l.contains("uncaught")),
        "the window 'error' listener (src/diagnostics.rs) should capture the \
         dispatched error and route it through `note` (prefixed 'uncaught …'). \
         Not found in captured log."
    );

    // -- Phase 2g: Shell `inspect` verb dispatcher --------------------
    //
    // The inspect verb is the diagnostics backbone. Sub-ops chain /
    // under / errors / help shipped first; entity / dump / find
    // landed at substrate 22d81c3. If any sub-op silently regresses
    // the user-facing observability story collapses.
    //
    // Each line hits the dispatcher + the binding + the substrate
    // read path with a cheap argument. We're proving the verb runs
    // end-to-end and renders into scrollback — empty results ("no
    // entity at X", "no chain-error markers") are valid because the
    // verb did dispatch. Paths use the relative form (joins with
    // wd = peer root) since absolute /system/... doesn't peer-qualify
    // per shell path::resolve.
    let insp_help = shell_submit(&client, "inspect help", 200).await?;
    assert!(
        insp_help.contains("inspect")
            && insp_help.contains("chain")
            && insp_help.contains("under")
            && insp_help.contains("errors")
            && insp_help.contains("entity")
            && insp_help.contains("dump")
            && insp_help.contains("find"),
        "`inspect help` should list all 7 sub-ops. Got: {insp_help:?}"
    );

    let insp_under = shell_submit(&client, "inspect under system/handler", 400).await?;
    assert!(
        insp_under.contains("inspect under") && insp_under.contains("binding"),
        "`inspect under system/handler` should report bindings on \
         a path that every peer has after boot. Got: {insp_under:?}"
    );

    let insp_chain = shell_submit(
        &client,
        "inspect chain nonexistent-chain-id-xyz",
        300,
    )
    .await?;
    assert!(
        insp_chain.contains("inspect chain")
            && insp_chain.contains("nonexistent-chain-id-xyz"),
        "`inspect chain <unknown>` should print a header naming the \
         chain_id (empty body OK). Got: {insp_chain:?}"
    );

    let insp_errors = shell_submit(&client, "inspect errors", 300).await?;
    assert!(
        insp_errors.contains("inspect errors") || insp_errors.contains("no chain-error"),
        "`inspect errors` should print a header or empty-marker \
         row (fresh peer typically has no markers). Got: {insp_errors:?}"
    );

    let insp_entity = shell_submit(&client, "inspect entity app/nothing-here", 400).await?;
    assert!(
        insp_entity.contains("entity") && insp_entity.contains("no entity"),
        "`inspect entity <unbound-path>` should print 'entity ...' \
         header + '(no entity at ...)'. Got: {insp_entity:?}"
    );

    let insp_dump = shell_submit(&client, "inspect dump 0000000000000000", 300).await?;
    assert!(
        insp_dump.contains("dump") && insp_dump.contains("0000000000000000"),
        "`inspect dump <unknown-hash>` should print a 'dump <hash>' \
         header even when nothing matches. Got: {insp_dump:?}"
    );

    let insp_find = shell_submit(&client, "inspect find handler", 800).await?;
    assert!(
        insp_find.contains("inspect find") && insp_find.contains("handler"),
        "`inspect find handler` should print 'inspect find ...' \
         header echoing the substring. Got: {insp_find:?}"
    );

    let insp_bare = shell_submit(&client, "inspect", 200).await?;
    assert!(
        insp_bare.contains("usage: inspect"),
        "Bare `inspect` should print usage. Got: {insp_bare:?}"
    );

    // -- Phase 2h: Chain Trace window renders + accepts input ---------
    //
    // Chain Trace (11th window) is the visual companion to `inspect`.
    // Phase 2 spawned it via the palette loop, but only checked for
    // panics. This phase verifies the window actually rendered its
    // input + empty state, then drives a chain_id submission and
    // confirms the "no continuation" branch surfaces — proving the
    // round trip from DOM input → set_chain_id action → model save
    // → re-render → output.chain_known flag → DOM message.
    //
    // We don't try to produce a real chain-error marker here (would
    // need a failing continuation chain); the empty + unknown-chain
    // paths are the regression gates. A future phase can drive a
    // real failure once we have a verb that reliably emits one.
    let ct_empty = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const ct = sec.querySelector('.chain-trace');
                if (!ct) continue;
                const h2 = ct.querySelector('h2');
                const pre = ct.querySelector('pre');
                return {
                    found: true,
                    h2: h2 ? h2.textContent : null,
                    pre: pre ? pre.textContent : null,
                    has_input: !!ct.querySelector('input'),
                    has_trace_btn: !!Array.from(ct.querySelectorAll('button'))
                        .find(b => b.textContent.trim() === 'Trace'),
                };
            }
            return { found: false };
            "#,
            vec![],
        )
        .await?;
    let ct_found = ct_empty
        .get("found")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        ct_found,
        "Chain Trace window should be rendered after Phase 2 spawn \
         loop. Got: {ct_empty:?}"
    );
    let ct_h2 = ct_empty.get("h2").and_then(|v| v.as_str()).unwrap_or("");
    let ct_pre = ct_empty.get("pre").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(ct_h2, "Chain Trace", "Chain Trace h2 mismatch: {ct_h2:?}");
    assert!(
        ct_pre.contains("enter a chain_id"),
        "Chain Trace empty state should prompt for chain_id. Got pre: {ct_pre:?}"
    );
    assert!(
        ct_empty.get("has_input").and_then(|v| v.as_bool()).unwrap_or(false)
            && ct_empty
                .get("has_trace_btn")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        "Chain Trace should render input + Trace button. Got: {ct_empty:?}"
    );

    // Submit a fabricated chain_id; expect the "no continuation"
    // branch to render after the action round-trips.
    let submit_result = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const ct = sec.querySelector('.chain-trace');
                if (!ct) continue;
                const input = ct.querySelector('input');
                const btn = Array.from(ct.querySelectorAll('button'))
                    .find(b => b.textContent.trim() === 'Trace');
                if (!input || !btn) return 'no-input-or-btn';
                input.value = 'fabricated-chain-id-zzz';
                input.dispatchEvent(new Event('input', { bubbles: true }));
                btn.click();
                return 'submitted';
            }
            return 'no-chain-trace-section';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        submit_result.as_str().unwrap_or(""),
        "submitted",
        "Chain Trace input/button interaction failed: {submit_result:?}"
    );
    sleep(Duration::from_millis(400)).await;

    let ct_after = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const ct = sec.querySelector('.chain-trace');
                if (!ct) continue;
                const pres = ct.querySelectorAll('pre');
                let combined = '';
                for (const p of pres) combined += p.textContent + '\n';
                return combined;
            }
            return '';
            "#,
            vec![],
        )
        .await?;
    let ct_after_text = ct_after.as_str().unwrap_or("");
    assert!(
        ct_after_text.contains("fabricated-chain-id-zzz")
            && ct_after_text.contains("no continuation"),
        "After submitting fabricated chain_id, Chain Trace should \
         render the 'no continuation or chain-error marker bound' \
         message naming the id. Got: {ct_after_text:?}"
    );

    // -- Phase 2h.2: Games + Apps launcher grids render the demo apps --
    //
    // The JS-Apps platform (Games = 16th window, Apps = 17th) shipped
    // with ZERO e2e coverage: Phase 2's spawn loop opened both
    // but only checked for panics. This pins the real user-visible
    // contract. On a plain boot (no origins registered), each window
    // seeds its baked demo token (`ensure_demo_set`, called in the render
    // path) into the local tree and renders the launcher grid — Games
    // bakes "War", Apps bakes "Calculator". We assert the grid shows the
    // app, then launch it and confirm the sandboxed iframe mounts with the
    // bundle `srcdoc` — proving the catalog + bundle round-trip out of the
    // store (the two-hop) and the app actually runs. Guards a regression
    // in the window / launcher / seed / store-read / sandbox paths — the
    // path the user validates apps through (publish → grid → launch).
    //
    // Selector: each window's `section.window` has `<header><h3>{title}`;
    // the grid is one `<button>` card per catalog entry (the app name in a
    // child `<div>`), all in the single `#dom-layer` shadow root.
    let grids = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const out = {};
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3) continue;
                const title = h3.textContent.trim();
                if (title !== 'Games' && title !== 'Apps') continue;
                out[title] = Array.from(sec.querySelectorAll('button'))
                    .map(b => b.textContent.trim());
            }
            return out;
            "#,
            vec![],
        )
        .await?;
    let has_card = |title: &str, name: &str| {
        grids
            .get(title)
            .and_then(|v| v.as_array())
            .map(|cards| {
                cards
                    .iter()
                    .any(|c| c.as_str().map(|s| s.contains(name)).unwrap_or(false))
            })
            .unwrap_or(false)
    };
    assert!(
        has_card("Games", "War"),
        "Games launcher grid should show the baked demo 'War' card after \
         Phase 2 opened it (ensure_demo_set seeds it in the render path). \
         Got: {grids:?}"
    );
    assert!(
        has_card("Apps", "Calculator"),
        "Apps launcher grid should show the baked demo 'Calculator' card. \
         Got: {grids:?}"
    );

    // Launch War: click its card, expect the sandboxed iframe to mount
    // with the full bundle srcdoc (read back from the store, two-hop).
    let launched = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Games') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.includes('War'));
                if (!card) return 'no-war-card';
                card.click();
                return 'clicked';
            }
            return 'no-games-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        launched.as_str().unwrap_or(""),
        "clicked",
        "War card click failed: {launched:?}"
    );
    sleep(Duration::from_millis(800)).await;

    let frame = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Games') continue;
                const fr = sec.querySelector('iframe[sandbox]');
                if (!fr) return { found: false };
                return {
                    found: true,
                    sandbox: fr.getAttribute('sandbox'),
                    srcdoc_len: (fr.getAttribute('srcdoc') || '').length,
                };
            }
            return { found: false };
            "#,
            vec![],
        )
        .await?;
    assert!(
        frame.get("found").and_then(|v| v.as_bool()).unwrap_or(false),
        "Launching War should mount a sandboxed iframe in the Games window. \
         Got: {frame:?}"
    );
    assert_eq!(
        frame.get("sandbox").and_then(|v| v.as_str()).unwrap_or(""),
        "allow-scripts",
        "Game iframe must be sandboxed allow-scripts (opaque origin, no \
         same-origin reach into the page). Got: {frame:?}"
    );
    let srcdoc_len = frame
        .get("srcdoc_len")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    assert!(
        srcdoc_len > 1000,
        "Game iframe srcdoc should carry the full bundle HTML read back \
         from the store, not an empty/placeholder frame. Got {srcdoc_len} bytes"
    );
    eprintln!(
        "Phase 2h.2: Games+Apps grids render the baked demo apps; \
         War launches into a sandboxed iframe (srcdoc {srcdoc_len} bytes)"
    );

    // Launch Calculator in the APPS window and pin the two set-specific player
    // contracts: the back button reads "← Apps" (not the hard-coded "← Games"),
    // and the stage's size vars uncap both axes (`--gm-max-w:none`) so a tool with
    // no per-app `size` hint uses the whole window instead of the games' square
    // cap. Both regressed once; this guards them.
    let apps_launch = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.includes('Calculator'));
                if (!card) return 'no-calculator-card';
                card.click();
                return 'clicked';
            }
            return 'no-apps-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        apps_launch.as_str().unwrap_or(""),
        "clicked",
        "Calculator card click failed: {apps_launch:?}"
    );
    sleep(Duration::from_millis(800)).await;
    let apps_player = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const back = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                const stage = sec.querySelector('.gm-stage');
                return {
                    back: back ? back.textContent.trim() : null,
                    stage_style: stage ? (stage.getAttribute('style') || '') : null,
                    has_iframe: !!sec.querySelector('iframe[sandbox]'),
                };
            }
            return { back: null };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        apps_player.get("back").and_then(|v| v.as_str()).unwrap_or(""),
        "← Apps",
        "Apps-window back button must read '← Apps', not the games label. Got: {apps_player:?}"
    );
    assert!(
        apps_player
            .get("stage_style")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("--gm-max-w:none"))
            .unwrap_or(false),
        "Apps (tool, no size hint) player stage must uncap its width \
         ('--gm-max-w:none') so it uses the whole window, not the games' square \
         cap. Got: {apps_player:?}"
    );
    assert!(
        apps_player.get("has_iframe").and_then(|v| v.as_bool()).unwrap_or(false),
        "Calculator should mount its sandboxed iframe. Got: {apps_player:?}"
    );
    eprintln!("Phase 2h.2: Calculator launches in Apps — back='← Apps', stage gm-fill ✓");

    // -- Phase 2i: Path Tap live dispatch stream ----------------------
    //
    // Path Tap is the second Inspect window (12th overall) — first
    // consumer of the live-event inspect surface (Direct arm via SDK
    // `with_inspect_routing` demuxer hook, Worker arm via
    // wasm-worker-proxy `install_inspect_sink` + Event::Inspect).
    //
    // Phase 2 spawn loop opened it; by now Phases 2e/2f/2g/2h have
    // submitted ~15 shell verbs (pwd/info/ls/query/count/inspect*),
    // each of which fires dispatch hooks on the bound peer. The
    // Worker arm goes through SetInspectEnabled(true) on first sink
    // attach, then Event::Inspect frames flow back to main, then the
    // demultiplexer fans them to our sink, which pushes into the
    // ring buffer.
    //
    // We assert: header renders + at least one dispatch row appears
    // (we don't pin which handler — the ring is FIFO bounded but
    // anything from prior shell verbs is fair game). If the row
    // count is zero with no diagnostic warning, either install
    // failed silently or the Event::Inspect wire path dropped.
    let pt_state = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const pt = sec.querySelector('.path-tap');
                if (!pt) continue;
                const h2 = pt.querySelector('h2');
                const pres = pt.querySelectorAll('pre');
                let text = '';
                for (const p of pres) text += p.textContent + '\n';
                // Crude row count: each row is rendered as a <div>
                // pair (status/handler/op + req-id). Count divs ÷ 2.
                const divs = pt.querySelectorAll('pre div').length;
                const cdiv = pt.querySelector("[data-field='path-tap-counts']");
                return {
                    found: true,
                    h2: h2 ? h2.textContent : null,
                    text,
                    div_count: divs,
                    counts: cdiv ? cdiv.textContent : null,
                };
            }
            return { found: false };
            "#,
            vec![],
        )
        .await?;
    let pt_found = pt_state
        .get("found")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        pt_found,
        "Path Tap window should be rendered after Phase 2 spawn loop \
         (palette discovery + click). Got: {pt_state:?}"
    );
    let pt_h2 = pt_state.get("h2").and_then(|v| v.as_str()).unwrap_or("");
    let pt_text = pt_state.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let pt_div_count = pt_state
        .get("div_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert_eq!(pt_h2, "Path Tap", "Path Tap h2 mismatch: {pt_h2:?}");
    // Routing must have attached cleanly. If it didn't, the renderer
    // surfaces the "Inspect routing failed to attach" warning.
    assert!(
        !pt_text.contains("Inspect routing failed"),
        "Path Tap routing failed at install time. Path Tap pre text: {pt_text:?}"
    );
    // div_count is 2 per row (status line + req-id line) — anything
    // ≥ 2 means at least one fact landed. Earlier shell verbs (Phase
    // 2e/2f/2g all do queries / inspect-under / etc. against
    // system/handler — each fires multiple dispatch facts).
    //
    // Note for the Worker arm: the first SetInspectEnabled(true) is
    // fire-and-forget at install time (PathTap factory in Phase 2
    // spawn loop). By Phase 2i we've awaited many subsequent shell
    // verbs and the worker has had time to: enable marshalling, post
    // Inspect events, the demultiplexer routes them, our sink fires.
    let pt_counts = pt_state
        .get("counts")
        .and_then(|v| v.as_str())
        .unwrap_or("(no counts)");
    eprintln!("Path Tap counts strip: {pt_counts}");
    assert!(
        pt_div_count >= 2,
        "Path Tap should have at least one dispatch row from the \
         shell verbs run in Phases 2e-2h. div_count={pt_div_count}, \
         counts={pt_counts:?}, text={pt_text:?}"
    );

    // -- Phase 2j: Wire Recorder live wire-frame stream ----------------
    //
    // Sibling to Path Tap; consumes `InspectFact::Wire`. By this point
    // no cross-peer traffic has flowed (Phase 2 only spawns windows and
    // runs shell verbs against the local primary), so the row count is
    // expected to be 0. The assertions therefore focus on the routing
    // path itself: header renders, routing-active warning is absent,
    // counters strip is wired.
    //
    // Wire frames DO show up once a remote dial happens (Phase 15.x
    // cross-Worker xworker handshakes). That's covered separately;
    // here we just prove the consumer-side window is alive.
    let wr_state = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const wr = sec.querySelector('.wire-recorder');
                if (!wr) continue;
                const h2 = wr.querySelector('h2');
                const pres = wr.querySelectorAll('pre');
                let text = '';
                for (const p of pres) text += p.textContent + '\n';
                const cdiv = wr.querySelector("[data-field='wire-recorder-counts']");
                return {
                    found: true,
                    h2: h2 ? h2.textContent : null,
                    text,
                    counts: cdiv ? cdiv.textContent : null,
                };
            }
            return { found: false };
            "#,
            vec![],
        )
        .await?;
    assert!(
        wr_state.get("found").and_then(|v| v.as_bool()).unwrap_or(false),
        "Wire Recorder window should be rendered after Phase 2 spawn loop. \
         Got: {wr_state:?}"
    );
    let wr_h2 = wr_state.get("h2").and_then(|v| v.as_str()).unwrap_or("");
    let wr_text = wr_state.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let wr_counts = wr_state
        .get("counts")
        .and_then(|v| v.as_str())
        .unwrap_or("(no counts)");
    assert_eq!(wr_h2, "Wire Recorder", "Wire Recorder h2 mismatch: {wr_h2:?}");
    assert!(
        !wr_text.contains("Inspect routing failed"),
        "Wire Recorder routing failed at install time. Wire Recorder pre text: {wr_text:?}"
    );
    assert!(
        wr_counts.contains("wire="),
        "Wire Recorder counts strip should expose the wire counter. counts={wr_counts:?}"
    );
    eprintln!("Wire Recorder counts strip: {wr_counts}");

    // -- Phase 2k: Content Stream live binding-event stream ------------
    //
    // Third Inspect sibling; consumes `InspectFact::Binding`. By this
    // point Phase 2 has spawned many windows (each writes
    // window-state entities), the shell has executed 15+ verbs (many
    // of which traverse handlers that put intermediate state), and the
    // Path Tap counters strip already showed `binding=20+` at Phase
    // 2i. So we expect ≥1 row in addition to the routing-active +
    // counter assertions.
    let cs_state = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const cs = sec.querySelector('.content-stream');
                if (!cs) continue;
                const h2 = cs.querySelector('h2');
                const pres = cs.querySelectorAll('pre');
                let text = '';
                for (const p of pres) text += p.textContent + '\n';
                const divs = cs.querySelectorAll('pre div').length;
                const cdiv = cs.querySelector("[data-field='content-stream-counts']");
                return {
                    found: true,
                    h2: h2 ? h2.textContent : null,
                    text,
                    div_count: divs,
                    counts: cdiv ? cdiv.textContent : null,
                };
            }
            return { found: false };
            "#,
            vec![],
        )
        .await?;
    assert!(
        cs_state.get("found").and_then(|v| v.as_bool()).unwrap_or(false),
        "Content Stream window should be rendered after Phase 2 spawn loop. \
         Got: {cs_state:?}"
    );
    let cs_h2 = cs_state.get("h2").and_then(|v| v.as_str()).unwrap_or("");
    let cs_text = cs_state.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let cs_div_count = cs_state
        .get("div_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let cs_counts = cs_state
        .get("counts")
        .and_then(|v| v.as_str())
        .unwrap_or("(no counts)");
    assert_eq!(cs_h2, "Content Stream", "Content Stream h2 mismatch: {cs_h2:?}");
    assert!(
        !cs_text.contains("Inspect routing failed"),
        "Content Stream routing failed at install time. Content Stream pre text: {cs_text:?}"
    );
    eprintln!("Content Stream counts strip: {cs_counts}");
    // div_count is 2 per row (kind/path/type line + hash line). ≥ 2
    // means at least one binding fact landed in the ring; the counter
    // already exceeded 20 at Phase 2i.
    assert!(
        cs_div_count >= 2,
        "Content Stream should have at least one binding row by Phase 2k \
         (Path Tap already saw binding=20+ at Phase 2i). div_count={cs_div_count}, \
         counts={cs_counts:?}, text={cs_text:?}"
    );

    // -- Phase 3: interact with Settings to exercise the write path ----
    //
    // With all windows open, Settings is the cleanest target for
    // testing the action→dispatch_write→worker round-trip. It uses:
    //   - select[name^=theme-] for `set_theme` (change event)
    //   - checkbox[name=show_inspector] for `toggle_inspector` (change)
    //   - checkbox[name=auto_connect] for `toggle_autoconnect` (change)
    //
    // Each click should fire exactly one Action::WindowEvent, which
    // routes through the model's set_*/toggle_* methods → dispatch_write
    // → Worker arm → proxy.put → worker tree. The signal we look for
    // is a NEW `dispatch_write: put ok` log line for the settings path.
    let before_log = capture_log(&client).await?;
    let prior_writes = before_log
        .iter()
        .filter(|l| l.contains("dispatch_write: put ok"))
        .count();

    // Select the "light" theme from the dropdown (fires `change`).
    let radio_result = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="theme-"]');
            if (!s) return 'no-select';
            s.value = 'light';
            s.dispatchEvent(new Event('change', { bubbles: true }));
            return 'changed';
            "#,
            vec![],
        )
        .await?;

    sleep(Duration::from_millis(500)).await;

    // Also drive the new "Site appearance" dropdown to "system" (the overlay
    // follows the chrome theme). This exercises the full delivery path:
    // change → Action::WindowEvent → model.set_site_appearance →
    // apply_site_appearance → install_site_root, which injects a
    // `<style id="site-theme-vars">` block aliasing every --site-* token to its
    // chrome counterpart. The element lives in <head> (light DOM, document
    // level — NOT the shadow root).
    let site_appearance_result = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="site-appearance-"]');
            if (!s) return 'no-select';
            s.value = 'system';
            s.dispatchEvent(new Event('change', { bubbles: true }));
            return 'changed';
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(400)).await;
    let site_vars = client
        .execute(
            r#"
            const el = document.getElementById('site-theme-vars');
            return el ? (el.textContent || '') : 'missing';
            "#,
            vec![],
        )
        .await?;
    let site_vars = site_vars.as_str().unwrap_or("").to_string();
    assert!(
        site_vars.contains("--site-bg:var(--overlay-bg)"),
        "Phase 3: 'Match system theme' must install the --site-* alias layer in \
         #site-theme-vars; got {site_vars:?} (select result: {site_appearance_result:?})"
    );

    // Click the inspector + autoconnect checkboxes by NAME — not "all
    // checkboxes." The Settings window also carries a Site & Surface
    // checkbox (show_toggle) plus the startup-surface controls; blindly
    // clicking every checkbox would flip the session config and corrupt the
    // Phase 19/20 site-mode asserts after the Phase 11 reload. Target the
    // two this phase owns.
    let checkbox_count = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            let n = 0;
            for (const sel of ['show_inspector', 'auto_connect']) {
                const cb = root.querySelector(`input[type="checkbox"][name="${sel}"]`);
                if (cb) { cb.click(); n++; }
            }
            return n;
            "#,
            vec![],
        )
        .await?;
    let checkboxes_clicked = checkbox_count.as_i64().unwrap_or(0);

    sleep(Duration::from_millis(800)).await;

    // -- Phase 4: assert Entity Tree actually rendered content ---------
    //
    // Regression gate for the "empty snapshot" / "subscription decode"
    // / "host L1 callback" class of bugs. The cache mirror is only
    // populated by Snapshot + Change events from the worker. If any
    // link in that chain breaks silently, Entity Tree renders zero
    // rows even though everything looks healthy in the console.
    //
    // After all 9 windows are open, the peer's tree contains at least
    // their per-window state entities (one write per window). Entity
    // Tree subscribes to `/{pid}/` so the initial snapshot must
    // include them. Counting `.tree-row` DOM nodes proves the mirror
    // populated and the renderer consumed it.
    let tree_item_count = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            return root.querySelectorAll('.tree-row').length;
            "#,
            vec![],
        )
        .await?;
    let tree_items = tree_item_count.as_i64().unwrap_or(0);
    println!("  entity tree rows:     {tree_items}");

    // -- Panel selection-source: dropdown is wired -------------------
    //
    // Runtime-agnostic structural guard (no `--features measurement`).
    // The consume *logic* (co-orient, no-republish loop guard, type
    // filter, updated_at guard) is covered by model unit tests; this
    // asserts the UI affordance is actually rendered and bound:
    // every Entity Tree panel must show a `.selection-source`
    // <select> offering exactly `none` + `app`, defaulting to `none`
    // (manual — the documented safe default). If the dropdown stops
    // rendering or the option set drifts, this fails.
    let sel_source = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sels = root.querySelectorAll('nav.tree-panel select.selection-source');
            if (sels.length === 0) return { ok: false, reason: 'no-selection-source-select' };
            const s = sels[0];
            const opts = Array.from(s.options).map(o => o.value);
            return { ok: true, count: sels.length, opts, value: s.value };
            "#,
            vec![],
        )
        .await?;
    assert!(
        sel_source.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "Entity Tree is missing the .selection-source dropdown — the \
         panel selection-source UI regressed (consumer can no longer \
         pick a source). Detail: {sel_source}"
    );
    let opts: Vec<String> = sel_source
        .get("opts")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    assert_eq!(
        opts,
        vec!["none".to_string(), "app".to_string()],
        "Selection-source option set drifted from the v1 contract \
         (None + App aggregate). Detail: {sel_source}"
    );
    assert_eq!(
        sel_source.get("value").and_then(|v| v.as_str()),
        Some("none"),
        "Selection source must default to 'none' (manual) — the \
         documented safe default; a non-manual default would silently \
         co-orient panels. Detail: {sel_source}"
    );
    println!("  selection-source dropdown: ok ({} panel(s))",
        sel_source.get("count").and_then(|v| v.as_i64()).unwrap_or(0));

    // -- Phase 5: click a tree row, expect inspector to populate -------
    //
    // After clicking a `.tree-row`, the Entity Tree model writes the
    // new `current_path` to its window-state entity. The render reads
    // current_path and builds the inspector panel from it via
    // cache_get. For the inspector to populate, two things must work:
    //   1. The write fires a subscription notify that flips Entity
    //      Tree's dirty flag → render re-fires.
    //   2. cache_get(current_path) returns the entity.
    //
    // If the L1 layer only delivers to the most-specific matching
    // subscription (and not Entity Tree's broader `/{pid}/` watcher),
    // step 1 fails and the inspector stays empty.
    //
    // The tree now boots COLLAPSED (`AUTO_EXPAND_BELOW = 1` — a
    // fresh peer opens with only its top-level groups visible, the
    // conventional file-tree default). It used to boot fully expanded, so a
    // `.has-entry` leaf was clickable immediately. Drill in: expand every
    // collapsed `▶` toggle, let the action queue + re-render settle, and
    // repeat until an entity-bearing leaf row appears (deep enough to reach the
    // `app/entity-browser/...` bindings ~5–7 levels down).
    for _ in 0..8 {
        let has_leaf = client
            .execute(
                r#"const layer = document.getElementById('dom-layer');
                   const root = layer.shadowRoot || layer;
                   return root.querySelectorAll('.tree-row.has-entry').length > 0;"#,
                vec![],
            )
            .await?
            .as_bool()
            .unwrap_or(false);
        if has_leaf {
            break;
        }
        client
            .execute(
                r#"const layer = document.getElementById('dom-layer');
                   const root = layer.shadowRoot || layer;
                   let n = 0;
                   for (const t of root.querySelectorAll('.tree-toggle')) {
                       if (t.textContent.trim().startsWith('▶')) { t.click(); n++; }
                   }
                   return n;"#,
                vec![],
            )
            .await?;
        sleep(Duration::from_millis(400)).await;
    }
    let inspector_state = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // Filter to rows with .has-entry — intermediate folder
            // rows are also `.tree-row` since Stage A,
            // but only `.has-entry` rows resolve to an inspector
            // body.
            const items = root.querySelectorAll('.tree-row.has-entry');
            if (items.length === 0) return { clicked: false, reason: 'no-tree-rows' };
            items[0].click();
            return { clicked: true, clicked_path: items[0].getAttribute('data-path') };
            "#,
            vec![],
        )
        .await?;
    println!("  clicked tree item:    {}", inspector_state);
    sleep(Duration::from_millis(800)).await;

    let inspector_visible = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // Inspector populates a <dl> with the entity's metadata
            // (or <pre class="entity-content"> for the document panel).
            const dlEntries = root.querySelectorAll('aside.inspector-panel dd, aside.inspector-panel dl dd');
            const docContent = root.querySelector('main.document-panel article');
            return {
                inspector_dd_count: dlEntries.length,
                document_has_article: docContent !== null,
            };
            "#,
            vec![],
        )
        .await?;
    println!("  inspector probe:      {}", inspector_visible);
    let inspector_populated = inspector_visible
        .get("inspector_dd_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        > 0
        || inspector_visible
            .get("document_has_article")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

    // -- Phase 5.1: Stage B selection-slot publish ---------------------
    //
    // Clicking a tree row on Stage B writes two
    // selection entities through publish_selection (controller →
    // dispatch_write → worker → cache mirror → Entity Tree re-render
    // via its own subscription):
    //   - `{pid}/app/entity-browser/workspace/panels/{wid}/selection`
    //   - `{pid}/app/entity-browser/workspace/selection`
    //
    // Both should round-trip back to the Entity Tree window's DOM as
    // `[data-path]` rows (since the Entity Tree subscribes to the
    // whole peer prefix). Verifying this here proves the full
    // write→worker→subscribe→re-render loop for Stage B.
    //
    // The click above WROTE these two selection entities, creating fresh deep
    // `workspace/panels/{wid}/selection` nodes — which, under the collapsed
    // default (`AUTO_EXPAND_BELOW = 1`), insert collapsed and so don't render
    // as visible `[data-path]` rows yet. The round-trip (write→subscribe→
    // re-render) is what this phase proves; expand the freshly-written subtree
    // so the rows become visible to count. (Earlier the auto-expand made
    // them visible immediately.)
    for _ in 0..8 {
        // Break only once the DEEPER `panels/{wid}/selection` leaf is visible —
        // it sits two levels below `workspace/selection`, so checking the
        // shallower one would stop expanding too early (the panel row matters
        // for the `panel_count` assertion below).
        let visible = client
            .execute(
                r#"const layer = document.getElementById('dom-layer');
                   const root = layer.shadowRoot || layer;
                   for (const el of root.querySelectorAll('[data-path]')) {
                       if (/\/app\/entity-browser\/workspace\/panels\/\d+\/selection$/.test(
                               el.getAttribute('data-path') || '')) return true;
                   }
                   return false;"#,
                vec![],
            )
            .await?
            .as_bool()
            .unwrap_or(false);
        if visible {
            break;
        }
        client
            .execute(
                r#"const layer = document.getElementById('dom-layer');
                   const root = layer.shadowRoot || layer;
                   let n = 0;
                   for (const t of root.querySelectorAll('.tree-toggle')) {
                       if (t.textContent.trim().startsWith('▶')) { t.click(); n++; }
                   }
                   return n;"#,
                vec![],
            )
            .await?;
        sleep(Duration::from_millis(400)).await;
    }
    let selection_slots = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // Find any selection-slot rows (per-panel OR app-aggregate).
            // Path suffix is sufficient — we don't have the pid in this
            // closure but the suffix is unique.
            const panelRows = [];
            const appRows = [];
            const allSelection = [];
            const all = root.querySelectorAll('[data-path]');
            for (const el of all) {
                const p = el.getAttribute('data-path') || '';
                if (p.includes('selection')) allSelection.push(p);
                if (/\/app\/entity-browser\/workspace\/panels\/\d+\/selection$/.test(p)) {
                    panelRows.push(p);
                }
                if (/\/app\/entity-browser\/workspace\/selection$/.test(p)) {
                    appRows.push(p);
                }
            }
            // Also dump any data-path containing "panels".
            const allPanels = [];
            for (const el of all) {
                const p = el.getAttribute('data-path') || '';
                if (p.includes('/panels')) allPanels.push(p);
            }
            return { panel_count: panelRows.length, app_count: appRows.length,
                     panel_paths: panelRows, app_paths: appRows,
                     all_selection: allSelection, all_panels_paths: allPanels };
            "#,
            vec![],
        )
        .await?;
    println!("  selection slots:      {}", selection_slots);
    let panel_count = selection_slots
        .get("panel_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let app_count = selection_slots
        .get("app_count")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    assert!(
        panel_count >= 1,
        "Stage B regression: per-panel selection slot missing after click. \
         Expected `workspace/panels/{{wid}}/selection` entity in the tree. \
         Detail: {selection_slots}"
    );
    assert!(
        app_count >= 1,
        "Stage B regression: app-aggregate selection slot missing after click. \
         Expected `workspace/selection` entity in the tree. \
         Detail: {selection_slots}"
    );

    // -- Phase 6: Knowledge Base save → back → click round-trip --------
    //
    // Exercises the real user flow:
    //   1. Locate the Knowledge Base window section.
    //   2. Click "+ New article".
    //   3. Type title + content into the form, click Save.
    //   4. Click "Back to list".
    //   5. Click the just-saved article in the list.
    //   6. Assert the reader populates with the article content
    //      (NOT the "no longer available" warning).
    //
    // All asserts are HARD. A silent skip here previously hid a real
    // regression (subscription pattern bug that broke window-state
    // re-renders); the test must fail loudly when it can't drive the
    // KB UI, not flag and pass.
    //
    // The KB section is found by scanning `section.window` elements
    // for one whose `<h2>` reads "Knowledge Base". All subsequent
    // queries are scoped to that section so other windows (Entity
    // Tree, etc.) can't accidentally satisfy the selector.
    let kb_new_clicked_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // KB's root wrapper has class="knowledge-base", stable across
            // view modes (List/Reader/Editor/New all use the same wrapper).
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.knowledge-base')) continue;
                const btns = sec.querySelectorAll('button');
                for (const b of btns) {
                    if (b.textContent.trim() === '+ New article') {
                        b.click();
                        return { found: true, clicked: true };
                    }
                }
                return { found: true, clicked: false, reason: 'no-new-article-btn' };
            }
            return { found: false, clicked: false, reason: 'no-kb-section' };
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(400)).await;

    let kb_section_found = kb_new_clicked_v
        .get("found")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let kb_new_clicked = kb_new_clicked_v
        .get("clicked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("  kb section found:     {kb_section_found}");
    println!("  kb new btn clicked:   {kb_new_clicked}");
    assert!(
        kb_section_found,
        "Could not find Knowledge Base window section in the DOM. \
         Phase 2 spawned KB, so this is a real UI regression."
    );
    assert!(
        kb_new_clicked,
        "Found KB section but no '+ New article' button. \
         KB list view may not be rendering correctly. \
         Detail: {kb_new_clicked_v}"
    );

    let kb_save_result_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // KB's root wrapper has class="knowledge-base", stable across
            // view modes (List/Reader/Editor/New all use the same wrapper).
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.knowledge-base')) continue;
                const titleInput = sec.querySelector('input[data-field="title"]');
                const contentTextarea = sec.querySelector('textarea[data-field="content"]');
                if (!titleInput || !contentTextarea) {
                    return { ok: false, reason: 'no-editor-form' };
                }
                titleInput.value = 'E2E Test Article';
                contentTextarea.value = 'Some test content body.';
                const btns = sec.querySelectorAll('button');
                for (const b of btns) {
                    if (b.textContent.trim() === 'Save') {
                        b.click();
                        return { ok: true };
                    }
                }
                return { ok: false, reason: 'no-save-btn' };
            }
            return { ok: false, reason: 'no-kb-section-after-new-click' };
            "#,
            vec![],
        )
        .await?;
    let kb_saved = kb_save_result_v
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("  kb save dispatched:   {kb_saved}");
    assert!(
        kb_saved,
        "Could not drive KB save flow. Detail: {kb_save_result_v}"
    );
    sleep(Duration::from_millis(1200)).await;

    let kb_back_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // KB's root wrapper has class="knowledge-base", stable across
            // view modes (List/Reader/Editor/New all use the same wrapper).
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.knowledge-base')) continue;
                // After save, normal Reader view has "← Back" (line 205
                // in dom/knowledge_base.rs). The "← Back to list" string
                // is only used in the warning branch when the article is
                // unreachable — finding THAT text would itself be a
                // regression signal.
                const btns = sec.querySelectorAll('button');
                let saw_warning_back = false;
                for (const b of btns) {
                    const t = b.textContent.trim();
                    if (t === '← Back') {
                        b.click();
                        return { clicked: true };
                    }
                    if (t === '← Back to list') {
                        saw_warning_back = true;
                    }
                }
                return {
                    clicked: false,
                    reason: saw_warning_back
                        ? 'reader-in-warning-state'
                        : 'no-back-btn',
                };
            }
            return { clicked: false, reason: 'no-kb-section' };
            "#,
            vec![],
        )
        .await?;
    let kb_back_clicked = kb_back_v
        .get("clicked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("  kb back clicked:      {kb_back_clicked}");
    assert!(
        kb_back_clicked,
        "KB reader view did not show '← Back to list' button after save. \
         The save→Reader transition may have failed. Detail: {kb_back_v}"
    );
    sleep(Duration::from_millis(400)).await;

    let kb_select_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // KB's root wrapper has class="knowledge-base", stable across
            // view modes (List/Reader/Editor/New all use the same wrapper).
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.knowledge-base')) continue;
                // The List view is now a collapsible docs tree
                // (render_tree_row in dom/knowledge_base.rs). Article
                // leaves are `.kb-tree-row.has-entry`; the label is the
                // path segment, i.e. the slug. "E2E Test Article"
                // slugifies to "e2e-test-article".
                const items = sec.querySelectorAll('.kb-tree-row.has-entry');
                for (const row of items) {
                    if (row.textContent.includes('e2e-test-article')) {
                        row.click();
                        return { clicked: true };
                    }
                }
                return { clicked: false, reason: 'no-article-row' };
            }
            return { clicked: false, reason: 'no-kb-section' };
            "#,
            vec![],
        )
        .await?;
    let kb_select_clicked = kb_select_v
        .get("clicked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("  kb article clicked:   {kb_select_clicked}");
    assert!(
        kb_select_clicked,
        "KB list view did not show the saved article after back-to-list. \
         tree_listing/cache_list may be missing the just-written entity. \
         Detail: {kb_select_v}"
    );
    sleep(Duration::from_millis(600)).await;

    let kb_reader_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // KB's root wrapper has class="knowledge-base", stable across
            // view modes (List/Reader/Editor/New all use the same wrapper).
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.knowledge-base')) continue;
                const text = sec.textContent || '';
                return {
                    has_warning: text.includes('no longer available'),
                    has_content: text.includes('Some test content body.'),
                };
            }
            return { has_warning: false, has_content: false, reason: 'no-kb-section' };
            "#,
            vec![],
        )
        .await?;
    let kb_warning = kb_reader_v
        .get("has_warning")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let kb_content_visible = kb_reader_v
        .get("has_content")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("  kb 'no longer avail': {kb_warning}");
    println!("  kb content visible:   {kb_content_visible}");

    assert!(
        !kb_warning,
        "KB save→back→click showed 'selected article is no longer available'. \
         The just-saved article is not reachable via cache_get on click. \
         Likely a subscription-delivery or cache-population regression."
    );
    assert!(
        kb_content_visible,
        "KB reader didn't render the article content after click. \
         No 'no longer available' warning either, so either render didn't \
         fire or the cache returned a different entity. Detail: {kb_reader_v}"
    );

    // -- Phase 7: Execute Console handler dropdown (Parity-A gate) -----
    //
    // The Execute Console's handler list is populated via
    // `Peers::discover_handlers_async` which branches Direct/Worker.
    // In Worker mode this routes through the proxy and the model
    // caches the result asynchronously. If the proxy round-trip,
    // wire conversion, or async refresh wiring is broken, the
    // dropdown will be empty (0 `<option>` elements) and Execute
    // Console is unusable in worker mode.
    //
    // Phase 2 already opened the Execute Console window. By Phase 7
    // time, multiple seconds have elapsed (plus all of Phase 6's
    // sleeps) — the async refresh has had ample opportunity to land.
    let exec_handler_count = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // Find Execute Console section via stable wrapper class.
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.execute-console')) continue;
                // Guided mode has a <select> with handler options.
                // Each <option> represents one discovered handler.
                const selects = sec.querySelectorAll('select');
                for (const sel of selects) {
                    // The first <select> is peer-selector; the handler
                    // dropdown is the second. Pick whichever has
                    // multiple options (the peer selector has at most
                    // a handful; handler list has dozens).
                    if (sel.options.length > 3) {
                        return { found: true, options: sel.options.length };
                    }
                }
                // Fallback: pick the largest <select>.
                let max = 0;
                for (const sel of selects) {
                    if (sel.options.length > max) max = sel.options.length;
                }
                return { found: true, options: max };
            }
            return { found: false, options: 0 };
            "#,
            vec![],
        )
        .await?;
    let exec_section_found = exec_handler_count
        .get("found")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let exec_options = exec_handler_count
        .get("options")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    println!("  exec section found:   {exec_section_found}");
    println!("  exec handler options: {exec_options}");
    assert!(
        exec_section_found,
        "Could not find Execute Console window section in the DOM."
    );
    assert!(
        exec_options > 0,
        "Execute Console handler dropdown is empty in worker mode. \
         The async `discover_handlers_async` refresh didn't populate \
         the model cache, or render isn't reading from it. Check \
         Peers::discover_handlers_async / WorkerPeerStore::discover_handlers / \
         ExecuteConsoleModel::refresh_handlers."
    );

    // -- Phase 8: Execute Console click → event log round-trip --------
    //
    // Clicking the "Execute" button fires `Action::Execute`, which
    // routes through `Peers::execute` and ends up calling
    // `proxy.execute` in worker mode. The result (success or
    // failure) is appended to the Event Log entity at
    // `/{sys_pid}/app/entity-browser/event-log/v1`. Either outcome
    // proves the full execute pipeline (consumer → Peers → wire →
    // worker → SDK → result → back) is alive — a silent no-op
    // would be the bug.
    let prior_event_log_lines = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.event-log')) continue;
                const pres = sec.querySelectorAll('pre');
                let total = 0;
                for (const p of pres) {
                    total += (p.textContent.match(/\n/g) || []).length;
                }
                return total;
            }
            return 0;
            "#,
            vec![],
        )
        .await?;
    let prior_events = prior_event_log_lines.as_i64().unwrap_or(0);

    let exec_clicked = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.execute-console')) continue;
                const btns = sec.querySelectorAll('button');
                for (const b of btns) {
                    if (b.textContent.trim() === 'Execute') {
                        b.click();
                        return true;
                    }
                }
                return false;
            }
            return false;
            "#,
            vec![],
        )
        .await?;
    println!("  exec btn clicked:     {exec_clicked}");
    sleep(Duration::from_millis(1000)).await;

    let post_event_log_lines = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.event-log')) continue;
                const pres = sec.querySelectorAll('pre');
                let total = 0;
                let has_arrow = false;
                let has_x = false;
                for (const p of pres) {
                    const t = p.textContent;
                    total += (t.match(/\n/g) || []).length;
                    if (t.includes('←')) has_arrow = true;
                    if (t.includes('✗')) has_x = true;
                }
                return { total, has_arrow, has_x };
            }
            return { total: 0, has_arrow: false, has_x: false };
            "#,
            vec![],
        )
        .await?;
    let post_events = post_event_log_lines
        .get("total")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let has_response = post_event_log_lines
        .get("has_arrow")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || post_event_log_lines
            .get("has_x")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
    println!("  event log prior:      {prior_events}");
    println!("  event log post:       {post_events}");
    println!("  exec response logged: {has_response}");

    assert!(
        exec_clicked.as_bool().unwrap_or(false),
        "Could not find / click the Execute button in Execute Console."
    );
    assert!(
        has_response,
        "Execute Console click did not produce a result line (← or ✗) \
         in the event log. Either the worker `execute` pipeline \
         (Peers::execute → WorkerPeerStore::execute → proxy.execute) \
         or the event-log writer's worker arm is broken."
    );

    // -- Phase 9: Query Console count → event log round-trip ----------
    //
    // Click "Count" in Query Console with default (empty) form fields.
    // The default expression matches everything; `count` returns a
    // `u64`. Validates `Peers::count` worker arm + the typed-query
    // path (full fidelity since count returns plain u64).
    let query_count_clicked = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.query-console')) continue;
                const btns = sec.querySelectorAll('button');
                for (const b of btns) {
                    if (b.textContent.trim() === 'Count') {
                        b.click();
                        return true;
                    }
                }
                return false;
            }
            return false;
            "#,
            vec![],
        )
        .await?;
    println!("  query count clicked:  {query_count_clicked}");
    assert!(
        query_count_clicked.as_bool().unwrap_or(false),
        "Could not find / click the Count button in Query Console."
    );
    sleep(Duration::from_millis(800)).await;

    let query_result_seen = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.event-log')) continue;
                const pres = sec.querySelectorAll('pre');
                for (const p of pres) {
                    const t = p.textContent;
                    if (t.includes('system/query count')) return true;
                }
            }
            return false;
            "#,
            vec![],
        )
        .await?;
    println!("  query result logged:  {query_result_seen}");
    assert!(
        query_result_seen.as_bool().unwrap_or(false),
        "Query Console Count click did not produce a 'system/query count' \
         line in the event log. The worker `count` pipeline \
         (Peers::count → WorkerPeerStore::count → proxy.count) may be broken."
    );

    // -- Phase 10: Query Console Find → event log round-trip ----------
    //
    // Click "Find" with default fields. Validates `Peers::query` worker
    // arm. Note: worker-mode `query` returns `WireQueryResults` which
    // lacks `total` / `cursor` / per-match `entity_type` until the wire
    // protocol carries those fields — see §3.5 in the living doc. We
    // assert presence of "system/query find" in the log, not on those
    // lossy fields.
    let query_find_clicked = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.query-console')) continue;
                const btns = sec.querySelectorAll('button');
                for (const b of btns) {
                    if (b.textContent.trim() === 'Find') {
                        b.click();
                        return true;
                    }
                }
                return false;
            }
            return false;
            "#,
            vec![],
        )
        .await?;
    println!("  query find clicked:   {query_find_clicked}");
    assert!(
        query_find_clicked.as_bool().unwrap_or(false),
        "Could not find / click the Find button in Query Console."
    );
    sleep(Duration::from_millis(800)).await;

    let query_find_logged = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.event-log')) continue;
                const pres = sec.querySelectorAll('pre');
                for (const p of pres) {
                    if (p.textContent.includes('system/query find')) return true;
                }
            }
            return false;
            "#,
            vec![],
        )
        .await?;
    println!("  query find logged:    {query_find_logged}");
    assert!(
        query_find_logged.as_bool().unwrap_or(false),
        "Query Console Find click did not produce a 'system/query find' \
         line in the event log. The worker `query` pipeline may be broken \
         OR `WireQueryResults` decode failed."
    );

    let after_log = capture_log(&client).await?;
    let post_writes = after_log
        .iter()
        .filter(|l| l.contains("dispatch_write: put ok"))
        .count();
    let new_writes = post_writes.saturating_sub(prior_writes);
    let post_panics = count_panics(&after_log);

    println!("\n--- Settings interaction ---");
    println!("  theme radio click:    {:?}", radio_result.as_str());
    println!("  checkboxes clicked:   {checkboxes_clicked}");
    println!("  new dispatch_write:   {new_writes}");
    println!("  new panics:           {}", post_panics.len());

    // -- Phase 12: Parity-B — create peer in worker mode round-trips -
    //
    // Click "New Peer" in the Peers management window. The worker
    // host generates a fresh keypair (browser getrandom), persists it
    // inside the worker SDK, and returns the seed inline (PROTOCOL_VERSION=4).
    // Consumer-side `Peers::create_new_peer_worker` future:
    //   - persists the seed to localStorage,
    //   - appends to the WorkerPeerStore peer mirror (RefCell),
    //   - the end-of-frame peer_registry reconcile writes the new
    //     peer's registry entity so palette + Peers window re-render.
    //
    // We assert: the Peers window's table grows by 1 row, AND
    // localStorage `entity_peers` grows by one new line. If wire
    // roundtripping is broken or the seed isn't persisted, one or
    // both of these would stay flat.
    let pre_create_peers = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let pre_create_lines = pre_create_peers
        .as_str()
        .unwrap_or("")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();

    let pre_create_rows_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                const rows = sec.querySelectorAll('tbody tr');
                return rows.length;
            }
            return -1;
            "#,
            vec![],
        )
        .await?;
    let pre_create_rows = pre_create_rows_v.as_i64().unwrap_or(-1);
    println!("  pre-create peer rows:  {pre_create_rows}");
    println!("  pre-create ls lines:   {pre_create_lines}");
    assert!(
        pre_create_rows >= 1,
        "Peers window had {pre_create_rows} rows pre-create — primary peer should be present. \
         Window selector or rendering may have broken."
    );

    // "+ Frontend" is the Stage 2B-renamed entry-point for what was
    // previously "New Peer"; in Worker boot it falls through to the
    // same `create_new_peer_worker` path the original test exercised.
    let new_peer_clicked = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                const btns = sec.querySelectorAll('button');
                for (const b of btns) {
                    if (b.textContent.trim() === '+ Frontend') {
                        b.click();
                        return 'clicked';
                    }
                }
                return 'no-frontend-btn';
            }
            return 'no-peers-section';
            "#,
            vec![],
        )
        .await?;
    println!(
        "  + Frontend button click: {:?}",
        new_peer_clicked.as_str().unwrap_or("non-string")
    );
    assert_eq!(
        new_peer_clicked.as_str(),
        Some("clicked"),
        "Could not click '+ Frontend' button in Peers window."
    );
    // Worker round-trip: protocol send + handle_create_peer (Keypair gen
    // + sdk.create_peer + set_metadata) + response decode + main-thread
    // mirror append + signal bump + render. ~1.5s gives ample margin.
    sleep(Duration::from_millis(1500)).await;

    let post_create_peers = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let post_create_lines = post_create_peers
        .as_str()
        .unwrap_or("")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    let post_create_rows_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                const rows = sec.querySelectorAll('tbody tr');
                return rows.length;
            }
            return -1;
            "#,
            vec![],
        )
        .await?;
    let post_create_rows = post_create_rows_v.as_i64().unwrap_or(-1);
    println!("  post-create peer rows: {post_create_rows}");
    println!("  post-create ls lines:  {post_create_lines}");
    assert_eq!(
        post_create_rows,
        pre_create_rows + 1,
        "Peers window row count did not grow by 1 after 'New Peer' click. \
         Pre={pre_create_rows} Post={post_create_rows}. Either the worker \
         create_peer round-trip failed, the mirror update didn't fire, or \
         the registry-signal-driven re-render didn't pick it up."
    );
    assert_eq!(
        post_create_lines,
        pre_create_lines + 1,
        "localStorage 'entity_peers' did not gain a new line after 'New Peer' click. \
         Pre={pre_create_lines} Post={post_create_lines}. The seed return path \
         (worker → consumer → persistence::save_peer) is broken."
    );

    // -- Phase 13: non-primary peer subscribe round-trip --------------
    //
    // Regression gate for the v6 subscribe peer-scoping bug.
    // That class of bug hides as long as every test only ever drives
    // the primary peer — Subscribe defaulting to primary still routes
    // correctly when the caller IS the primary. Same shape as Phase 5
    // (click tree row → inspector populates), but bound to the
    // non-primary peer created in Phase 12.
    //
    // The click→Navigate flow exercises:
    //   1. L1 write to /{non_primary_pid}/app/entity-browser/.../state
    //   2. Tree notify on that path
    //   3. Subscribe-driven WindowWatch callback flips the per-window
    //      dirty flag
    //   4. Render refires and inspector populates
    //
    // If Subscribe ignores peer_id and binds to the primary, step 3
    // never fires for the non-primary window, render stays cold, and
    // the inspector stays empty.
    let non_primary_select = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // Palette peer-selector is the <select> rendered directly into
            // the command palette (`append_peer_selector`) — NOT inside a
            // menu-group <details> (those carry spawn buttons since the menu
            // grouping redesign). Options are "{glyph} {name} ({role})" with
            // role = system / frontend / backend (memory) / backend (opfs).
            // The just-created peer was made via "+ Frontend" and is the only
            // "(frontend)" option (the primary/system peer is "(system)").
            const select = root.querySelector('.command-palette select');
            if (!select) return { ok: false, reason: 'no-palette-select' };
            let target = null;
            const seen = [];
            for (const opt of select.options) {
                seen.push(opt.text);
                const lower = opt.text.toLowerCase();
                if (lower.includes('(frontend)') && !lower.includes('(system)')) {
                    target = opt.value;
                    break;
                }
            }
            if (!target) return { ok: false, reason: 'no-frontend-option', seen };
            select.value = target;
            select.dispatchEvent(new Event('change', { bubbles: true }));
            return { ok: true, pid: target };
            "#,
            vec![],
        )
        .await?;
    let non_primary_ok = non_primary_select
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let non_primary_pid = non_primary_select
        .get("pid")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    println!("  non-primary pid:       {non_primary_pid}");
    assert!(
        non_primary_ok && !non_primary_pid.is_empty(),
        "Could not select non-primary peer in palette dropdown. \
         Phase 12 created a peer but it didn't appear as a (frontend) \
         option, or the palette select isn't where we expect. \
         Detail: {non_primary_select}"
    );
    // Let the palette re-render with the new selection latched.
    sleep(Duration::from_millis(200)).await;

    // Spawn Entity Tree bound to the non-primary peer. Peer-scoped
    // spawn buttons read the palette's `selected_peer` at click time
    // (src/dom/mod.rs render_palette), so this binds to the peer we
    // just selected.
    let np_spawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            for (const b of btns) {
                if (b.textContent.trim() === '+ Entity Tree') {
                    b.click();
                    return 'clicked';
                }
            }
            return `no-match-of-${btns.length}-buttons`;
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        np_spawn.as_str(),
        Some("clicked"),
        "Could not click '+ Entity Tree' spawn button for non-primary peer."
    );
    // Spawn writes initial window-state entity + registers subscriptions
    // + first render. 1s covers all three on slow CI.
    sleep(Duration::from_millis(1000)).await;

    // The header badge now renders "{role_glyph} {display_name}"
    // (display_name = peer label if set, else short_pid). The test
    // peer is created without an alias, so display_name == short_pid;
    // we match on the badge *containing* short_pid (robust to the
    // glyph prefix and to display_name's label-or-pid fallback).
    // Still disambiguates the two Entity Tree windows since they are
    // bound to different peers (distinct short_pids).
    let badge_short = if non_primary_pid.len() > 16 {
        format!(
            "{}...{}",
            &non_primary_pid[..8],
            &non_primary_pid[non_primary_pid.len() - 6..]
        )
    } else {
        non_primary_pid.clone()
    };
    println!("  badge to match:        {badge_short}");

    // Find the non-primary Entity Tree section by walking
    // section.window elements and matching on header h3 + badge text.
    // Once located, count its tree items (initial snapshot signal).
    let np_tree_v = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const sections = root.querySelectorAll('section.window');
                let n = 0;
                for (const sec of sections) {{
                    const h3 = sec.querySelector('header h3');
                    if (!h3 || h3.textContent.trim() !== 'Entity Tree') continue;
                    const badges = sec.querySelectorAll('header span');
                    let badge_text = '';
                    for (const sp of badges) badge_text = sp.textContent.trim();
                    if (!badge_text.includes('{badge_short}')) continue;
                    const items = sec.querySelectorAll('.tree-row');
                    return {{ found: true, items: items.length, instance: sec.getAttribute('data-instance') }};
                }}
                return {{ found: false, sections: sections.length }};
                "#
            ),
            vec![],
        )
        .await?;
    let np_section_found = np_tree_v
        .get("found")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let np_tree_items = np_tree_v.get("items").and_then(|v| v.as_i64()).unwrap_or(0);
    let np_instance = np_tree_v
        .get("instance")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    println!("  np entity tree found:  {np_section_found}");
    println!("  np tree items:         {np_tree_items}");
    assert!(
        np_section_found,
        "Could not find Entity Tree section bound to non-primary peer. \
         Spawn may not have read the updated selected_peer, or the badge \
         rendering changed. Detail: {np_tree_v}"
    );
    assert!(
        np_tree_items >= 1,
        "Non-primary Entity Tree rendered zero `.tree-row` rows. \
         The non-primary peer's tree mirror is empty even though spawn \
         just wrote its own per-window state. Likely a subscription \
         / snapshot-delivery regression on non-primary peers."
    );

    // The non-primary tree also boots collapsed (`AUTO_EXPAND_BELOW = 1`).
    // Expand within THIS section until an entity-bearing leaf is
    // clickable, same drill-down as the primary tree above.
    for _ in 0..8 {
        let has_leaf = client
            .execute(
                &format!(
                    r#"const layer = document.getElementById('dom-layer');
                       const root = layer.shadowRoot || layer;
                       const sec = root.querySelector('section.window[data-instance="{np_instance}"]');
                       if (!sec) return false;
                       return sec.querySelectorAll('.tree-row.has-entry').length > 0;"#
                ),
                vec![],
            )
            .await?
            .as_bool()
            .unwrap_or(false);
        if has_leaf {
            break;
        }
        client
            .execute(
                &format!(
                    r#"const layer = document.getElementById('dom-layer');
                       const root = layer.shadowRoot || layer;
                       const sec = root.querySelector('section.window[data-instance="{np_instance}"]');
                       if (!sec) return 0;
                       let n = 0;
                       for (const t of sec.querySelectorAll('.tree-toggle')) {{
                           if (t.textContent.trim().startsWith('▶')) {{ t.click(); n++; }}
                       }}
                       return n;"#
                ),
                vec![],
            )
            .await?;
        sleep(Duration::from_millis(400)).await;
    }

    // Click the first tree item in the non-primary section and assert
    // its inspector populates. This is the v6 regression gate —
    // primary subscriptions deliver, non-primary silently doesn't.
    let np_click_v = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const sec = root.querySelector('section.window[data-instance="{np_instance}"]');
                if (!sec) return {{ clicked: false, reason: 'section-gone' }};
                // Filter to .has-entry — intermediate folder rows
                // are also `.tree-row` but only entity-bound rows
                // resolve in the inspector.
                const items = sec.querySelectorAll('.tree-row.has-entry');
                if (items.length === 0) return {{ clicked: false, reason: 'no-items' }};
                items[0].click();
                return {{ clicked: true, path: items[0].getAttribute('data-path') }};
                "#
            ),
            vec![],
        )
        .await?;
    let np_clicked = np_click_v
        .get("clicked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        np_clicked,
        "Could not click a tree item in the non-primary Entity Tree. \
         Detail: {np_click_v}"
    );
    sleep(Duration::from_millis(800)).await;

    let np_inspector_v = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const sec = root.querySelector('section.window[data-instance="{np_instance}"]');
                if (!sec) return {{ dd: 0, doc: false, reason: 'section-gone' }};
                const dd = sec.querySelectorAll('aside.inspector-panel dd').length;
                const doc = sec.querySelector('main.document-panel article') !== null;
                return {{ dd, doc }};
                "#
            ),
            vec![],
        )
        .await?;
    let np_dd = np_inspector_v
        .get("dd")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let np_doc = np_inspector_v
        .get("doc")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("  np inspector dd:       {np_dd}");
    println!("  np inspector doc:      {np_doc}");
    let np_inspector_populated = np_dd > 0 || np_doc;
    assert!(
        np_inspector_populated,
        "Non-primary Entity Tree inspector did not populate after click. \
         Almost certainly a regression of the v6 subscribe peer-scoping \
         bug: Subscribe is defaulting to the primary peer, so the \
         non-primary window's WindowWatch never sees the Navigate write \
         and the dirty flag never flips (subscribe peer-scoping regression)."
    );

    // -- Phase 13.5: non-primary Query Console scopes to its peer -----
    //
    // Regression gate for the §4.3 defect (system review,
    // P0): `handle_query`/`handle_count` used to hard-code
    // `primary_peer_id()`, so a Query Console palette-bound to a
    // non-primary peer silently ran against the *primary's* tree —
    // wrong results, no error. Same defect class as the closed
    // delete/subscribe bugs; reachable because the palette lists
    // local non-primary peers and Query Console has no in-window peer
    // selector.
    //
    // Proof shape: the non-primary peer was created in Phase 12 and
    // holds only a handful of entities (its bootstrap + the two
    // windows we bind to it); the primary's tree is heavily populated
    // by Phases 2–10. A `count` with empty fields = that peer's total
    // entity count. Pre-fix the non-primary-bound Count == the
    // primary-bound Count (both hit primary). Post-fix the
    // non-primary count is strictly smaller. Compile + unit tests
    // structurally cannot catch this — only the worker e2e exercises
    // the bound-peer routing end-to-end.
    //
    // The non-primary peer is still the palette selection from
    // Phase 13; re-select defensively (idempotent) so the spawn is
    // self-contained regardless of any palette re-render.
    let _ = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const details = root.querySelector('details');
            const select = details ? details.querySelector('select') : null;
            if (!select) return false;
            for (const opt of select.options) {
                const lower = opt.text.toLowerCase();
                if (lower.includes('(frontend)') && !lower.includes('(system)')) {
                    select.value = opt.value;
                    select.dispatchEvent(new Event('change', { bubbles: true }));
                    return true;
                }
            }
            return false;
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(200)).await;

    let npq_spawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            for (const b of btns) {
                if (b.textContent.trim() === '+ Query Console') {
                    b.click();
                    return 'clicked';
                }
            }
            return `no-match-of-${btns.length}-buttons`;
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        npq_spawn.as_str(),
        Some("clicked"),
        "Could not click '+ Query Console' spawn button for non-primary peer."
    );
    sleep(Duration::from_millis(1000)).await;

    // Locate the non-primary Query Console by badge (same disambiguation
    // Phase 13 uses for the two Entity Trees: the np window's header
    // badge contains the non-primary peer's short_pid; the Phase-2
    // primary-bound one does not).
    let npq_v = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const sections = root.querySelectorAll('section.window');
                for (const sec of sections) {{
                    if (!sec.querySelector('.query-console')) continue;
                    const badges = sec.querySelectorAll('header span');
                    let badge_text = '';
                    for (const sp of badges) badge_text = sp.textContent.trim();
                    if (!badge_text.includes('{badge_short}')) continue;
                    return {{ found: true, instance: sec.getAttribute('data-instance') }};
                }}
                return {{ found: false, sections: sections.length }};
                "#
            ),
            vec![],
        )
        .await?;
    let npq_found = npq_v.get("found").and_then(|v| v.as_bool()).unwrap_or(false);
    let npq_instance = npq_v
        .get("instance")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    println!("  np query console found: {npq_found}");
    assert!(
        npq_found,
        "Could not find Query Console bound to the non-primary peer. \
         Spawn may not have read the palette selection. Detail: {npq_v}"
    );

    // Click Count in the non-primary Query Console.
    let npq_count_click = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const sec = root.querySelector('section.window[data-instance="{npq_instance}"]');
                if (!sec) return false;
                const btns = sec.querySelectorAll('button');
                for (const b of btns) {{
                    if (b.textContent.trim() === 'Count') {{ b.click(); return true; }}
                }}
                return false;
                "#
            ),
            vec![],
        )
        .await?;
    assert!(
        npq_count_click.as_bool().unwrap_or(false),
        "Could not click Count in the non-primary Query Console."
    );
    sleep(Duration::from_millis(900)).await;
    let np_count = read_last_query_count(&client).await?;
    println!("  np count:              {np_count}");

    // Click Count in the primary-bound Query Console (the Phase-2 one;
    // its badge does NOT contain the non-primary short_pid).
    let pq_count_click = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const sections = root.querySelectorAll('section.window');
                for (const sec of sections) {{
                    if (!sec.querySelector('.query-console')) continue;
                    const badges = sec.querySelectorAll('header span');
                    let badge_text = '';
                    for (const sp of badges) badge_text = sp.textContent.trim();
                    if (badge_text.includes('{badge_short}')) continue;
                    const btns = sec.querySelectorAll('button');
                    for (const b of btns) {{
                        if (b.textContent.trim() === 'Count') {{ b.click(); return true; }}
                    }}
                }}
                return false;
                "#
            ),
            vec![],
        )
        .await?;
    assert!(
        pq_count_click.as_bool().unwrap_or(false),
        "Could not click Count in the primary-bound Query Console."
    );
    sleep(Duration::from_millis(900)).await;
    let primary_count = read_last_query_count(&client).await?;
    println!("  primary count:         {primary_count}");

    assert!(
        np_count >= 0 && primary_count >= 0,
        "Could not parse a `system/query count → N` result line for one \
         or both Query Consoles (np={np_count}, primary={primary_count}). \
         The worker count pipeline may be broken."
    );
    assert!(
        np_count < primary_count,
        "Non-primary Query Console count ({np_count}) is not smaller than \
         the primary-bound count ({primary_count}). They should differ \
         sharply — the non-primary peer was just created and holds only \
         a few entities, the primary's tree is heavily populated. Equal \
         counts mean the non-primary-bound query silently ran against \
         the PRIMARY's tree: a regression of the §4.3 \
         hard-coded-primary_peer_id defect (P0)."
    );

    // -- Phase 14: ConnectPeer end-to-end against Tauri-side listener -
    //
    // Validates the full Parity-D-narrow flow: browser-side primary
    // peer (worker mode) connects to a native WebSocket listener
    // outside the browser process and successfully establishes the
    // peer-to-peer connection.
    //
    // Listener: a separate Tauri binary spawned with the
    // ENTITY_BROWSER_AUTOSTART_LISTENER=1 env var, which short-circuits
    // the normal "click Start in the WebView" flow and brings up a
    // native backend peer immediately. The Tauri binary prints a
    // single parseable READY line carrying peer_id + ws_addr; we
    // scrape it here.
    //
    // Browser side: the existing Peer Connections window (opened in
    // Phase 2, bound to the primary peer) receives the ws_addr via
    // its address input + Connect button — same path a user would
    // exercise manually with `make tauri-run` + browser.
    //
    // Success signal: the Tauri peer's short_pid appearing in the
    // window's "Connected Peers" list after the click. That requires
    // the WS handshake to complete, the entity-protocol handshake to
    // succeed, and the consumer's `handle_connect_peer` worker arm
    // to insert into the connection pool — i.e. the whole
    // Parity-D-narrow surface end-to-end.
    let tauri = start_tauri_listener()?;
    println!("  tauri peer_id:         {}", tauri.peer_id);
    println!("  tauri ws_addr:         {}", tauri.ws_addr);
    println!("  tauri webview booted:  {}", tauri.webview_booted);
    // "Frame loop started" only logs from src/main.rs:163, which runs
    // strictly AFTER `EntityApp::new_wasm[_worker]` returns Ok. So this
    // is a universal "WebView UI booted" signal independent of Direct
    // vs Worker mode. If the autostart hook ever broke the WebView
    // load (or anything else regresses on the Tauri WebKitGTK path),
    // this assertion catches it before users see a blank window.
    assert!(
        tauri.webview_booted,
        "Tauri WebView never logged 'Frame loop started' within the \
         20s startup budget. Autostart printed the READY line so the \
         native backend is fine, but the WebView UI failed to boot. \
         A user running `make tauri-run` would see a blank window. \
         Check the captured Tauri stdout for [UNCAUGHT] errors, OPFS \
         init failures, or other WASM init failures."
    );

    let tauri_short = if tauri.peer_id.len() > 16 {
        format!(
            "{}...{}",
            &tauri.peer_id[..8],
            &tauri.peer_id[tauri.peer_id.len() - 6..]
        )
    } else {
        tauri.peer_id.clone()
    };
    println!("  tauri short_pid:       {tauri_short}");

    let connect_click = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const sections = root.querySelectorAll('section.window');
                for (const sec of sections) {{
                    if (!sec.querySelector('.peer-connections')) continue;
                    const input = sec.querySelector('input[data-field="address"]');
                    if (!input) return {{ ok: false, reason: 'no-address-input' }};
                    input.value = '{ws_addr}';
                    const btns = sec.querySelectorAll('button');
                    for (const b of btns) {{
                        if (b.textContent.trim() === 'Connect') {{
                            b.click();
                            return {{ ok: true }};
                        }}
                    }}
                    return {{ ok: false, reason: 'no-connect-btn' }};
                }}
                return {{ ok: false, reason: 'no-peer-connections-section' }};
                "#,
                ws_addr = tauri.ws_addr,
            ),
            vec![],
        )
        .await?;
    let connect_clicked = connect_click
        .get("ok")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        connect_clicked,
        "Could not drive Peer Connections → Connect for ConnectPeer flow. \
         Detail: {connect_click}"
    );
    // Connect involves: WS handshake to Tauri listener + entity protocol
    // handshake + connection-pool insert + post-connect type fetch. 2.5s
    // is a comfortable budget on a dev box; CI may need more.
    sleep(Duration::from_millis(2500)).await;

    let connected_v = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const sections = root.querySelectorAll('section.window');
                for (const sec of sections) {{
                    if (!sec.querySelector('.peer-connections')) continue;
                    // After ConnectPeer success the renderer adds a
                    // "Connected Peers" block with <code>{{short_pid}}</code>
                    // rows. We grep for the Tauri peer's short_pid.
                    const codes = sec.querySelectorAll('code');
                    const seen = [];
                    for (const c of codes) {{
                        const t = c.textContent.trim();
                        seen.push(t);
                        if (t === '{tauri_short}') {{
                            return {{ connected: true, found: t }};
                        }}
                    }}
                    return {{ connected: false, codes: seen }};
                }}
                return {{ connected: false, reason: 'no-section' }};
                "#
            ),
            vec![],
        )
        .await?;
    let connected = connected_v
        .get("connected")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("  connected to tauri:    {connected}");
    assert!(
        connected,
        "Browser primary peer did not list Tauri peer's short_pid in \
         Connected Peers after click. ConnectPeer flow broken end-to-end. \
         Could be: WS handshake failed, entity-protocol handshake failed, \
         worker arm of handle_connect_peer didn't pool the connection, or \
         the post-connect refresh isn't writing the connections list back \
         into the tree the window subscribes to. Detail: {connected_v}"
    );

    // -- Stage F regression: QR generation is lazy ------------------
    //
    // The QR pairing SVG (Reed-Solomon encode + per-module SVG build +
    // set_inner_html parse) is the dominant render cost for this
    // window. Stage F deferred it to the <details> toggle so collapsed
    // renders pay nothing. This guard is runtime-agnostic (no
    // `--features measurement` needed): assert the QR content carries
    // no <svg> while collapsed, then that opening it generates one.
    // If someone moves generation back into the eager render path the
    // pre-open assertion fails.
    let qr_pre = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.peer-connections')) continue;
                let qr = null;
                for (const d of sec.querySelectorAll('details')) {
                    const s = d.querySelector('summary');
                    if (s && s.textContent.trim() === 'QR Pairing') { qr = d; break; }
                }
                if (!qr) return { ok: false, reason: 'no-qr-details' };
                const before = qr.querySelectorAll('svg').length;
                qr.open = true;
                return { ok: true, before };
            }
            return { ok: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert!(
        qr_pre.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
        "Could not locate the QR Pairing <details> in Peer Connections. \
         Detail: {qr_pre}"
    );
    assert_eq!(
        qr_pre.get("before").and_then(|v| v.as_u64()),
        Some(0),
        "QR SVG was present while the <details> was still collapsed — \
         Stage F lazy-generation regressed; generation is back in the \
         eager per-render path and the window pays the QR encode cost \
         every frame. Detail: {qr_pre}"
    );
    // `details.open = true` queues the `toggle` event; let it fire +
    // run the lazy generator before asserting.
    sleep(Duration::from_millis(200)).await;
    let qr_post = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.peer-connections')) continue;
                for (const d of sec.querySelectorAll('details')) {
                    const s = d.querySelector('summary');
                    if (s && s.textContent.trim() === 'QR Pairing') {
                        return { after: d.querySelectorAll('svg').length };
                    }
                }
            }
            return { after: -1 };
            "#,
            vec![],
        )
        .await?;
    assert!(
        qr_post.get("after").and_then(|v| v.as_i64()).unwrap_or(0) >= 1,
        "QR SVG did not appear after opening the <details>. The lazy \
         toggle handler didn't generate the code — Stage F lazy path \
         is broken (users would see an empty QR Pairing panel). \
         Detail: {qr_post}"
    );

    // -- Measurement checkpoint -------------------------------------
    //
    // Capture per-window render numbers from Phases 1–10's interactions
    // before the Phase 11 reload wipes the page-local log buffer. Only
    // fires under `--features measurement`; otherwise the filter is a no-op
    // because the render-counter logs aren't emitted.
    {
        let pre_reload_log = capture_log(&client).await?;
        let render_lines: Vec<&String> = pre_reload_log
            .iter()
            .filter(|l| l.contains("window render"))
            .collect();
        if !render_lines.is_empty() {
            println!(
                "===== Per-window render measurement ({} samples) =====",
                render_lines.len()
            );
            for (i, line) in render_lines.iter().enumerate() {
                let cleaned = line
                    .replace("%cINFO%c", "INFO ")
                    .replace("%c", "")
                    .replace("color: whitesmoke; background: #444", "")
                    .replace("color: gray; font-style: italic", "")
                    .replace("color: inherit", "")
                    .replace('\n', " | ");
                println!("  [{i:>3}] {cleaned}");
            }
            println!("===== End measurement =====");
        }
    }

    // -- Phase 11: reload page, OPFS-backed state must persist --------
    //
    // The persistence acceptance test. With `enable_opfs: true` in
    // InitParams (src/app.rs new_wasm_worker), the worker host builds
    // its SDK against `OpfsStore` instead of the in-memory default.
    // After a page reload the same primary peer keypair is loaded
    // from localStorage, the worker re-attaches to the same OPFS root,
    // and the entity tree (including the KB article saved in Phase 6)
    // must still be there.
    //
    // If OPFS wiring is broken — wrong store factory, opfs() not
    // awaited, build_async() not called, host using sync build()
    // path — the tree comes up empty on reload and the article is
    // gone. That's the single failure mode this phase catches.
    let pre_reload_peers = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let pre_reload_peers_str = pre_reload_peers.as_str().unwrap_or("");
    println!(
        "  pre-reload entity_peers bytes: {}",
        pre_reload_peers_str.len()
    );
    assert!(
        !pre_reload_peers_str.is_empty(),
        "localStorage 'entity_peers' is empty pre-reload — bootstrap \
         never persisted the primary keypair, can't run the reload test."
    );

    client.refresh().await?;
    // Bootstrap is slower than first run on some systems because
    // OpfsStore::open() must reattach to existing journal files; allow
    // up to 8s but return as soon as the "Frame loop started" sentinel
    // lands. Saves a few seconds vs. a fixed 6s wait in practice.
    let phase11_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 11 reload boot: {phase11_boot_ms}ms");

    // Sanity: the primary peer keypair must round-trip across reload.
    // If localStorage was cleared, OPFS would key off a different
    // peer_id and we'd be testing an empty tree — making the article
    // assertion below misleading.
    let post_reload_peers = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let post_reload_peers_str = post_reload_peers.as_str().unwrap_or("");
    assert_eq!(
        pre_reload_peers_str, post_reload_peers_str,
        "localStorage 'entity_peers' changed across reload. \
         Primary peer identity must persist for the OPFS tree to be \
         keyed correctly. Without this invariant the article-still-present \
         assertion is meaningless."
    );

    // Confirm post-reload bootstrap actually completed before driving
    // any UI. The boot log line is stable across Direct and Worker modes.
    let post_reload_log = capture_log(&client).await?;
    let booted = post_reload_log
        .iter()
        .any(|l| l.contains("Frame loop started"));
    assert!(
        booted,
        "Post-reload app never logged 'Frame loop started' within 6s. \
         Worker spawn / Init handshake / SDK build_async may be hung."
    );
    let reload_panics = count_panics(&post_reload_log);
    assert!(
        reload_panics.is_empty(),
        "Post-reload bootstrap triggered panic(s):\n{}",
        reload_panics
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n")
    );

    // Spawn the KB window via the palette. After reload, no windows
    // are auto-restored — we click the same spawn button Phase 2 uses.
    let kb_respawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            if (!layer) return 'no-dom-layer';
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            for (const b of btns) {
                if (b.textContent.trim() === '+ Knowledge Base') {
                    b.click();
                    return 'clicked';
                }
            }
            return `no-match-of-${btns.length}-buttons`;
            "#,
            vec![],
        )
        .await?;
    let kb_respawn_status = kb_respawn.as_str().unwrap_or("non-string");
    println!("  post-reload kb spawn:  {kb_respawn_status}");
    assert_eq!(
        kb_respawn_status, "clicked",
        "Could not respawn Knowledge Base window after reload. Palette \
         may not have rendered yet, or the spawn button label changed."
    );
    sleep(Duration::from_millis(1200)).await;

    // The persistence assertion: the article saved in Phase 6 must
    // appear in the KB list view rendered from the freshly-hydrated
    // OPFS tree.
    let kb_article_persisted_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                if (!sec.querySelector('.knowledge-base')) continue;
                // List view is the collapsible docs tree; article
                // leaves are `.kb-tree-row.has-entry` labelled by slug.
                // "E2E Test Article" → "e2e-test-article".
                const items = sec.querySelectorAll('.kb-tree-row.has-entry');
                const titles = [];
                for (const row of items) {
                    titles.push(row.textContent.trim().slice(0, 80));
                    if (row.textContent.includes('e2e-test-article')) {
                        return { found: true, count: items.length };
                    }
                }
                return { found: false, count: items.length, titles };
            }
            return { found: false, reason: 'no-kb-section' };
            "#,
            vec![],
        )
        .await?;
    let kb_article_persisted = kb_article_persisted_v
        .get("found")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!("  kb article persisted:  {kb_article_persisted}");
    assert!(
        kb_article_persisted,
        "Phase 6 saved 'E2E Test Article' but it's gone after page reload. \
         OPFS persistence is not end-to-end: either enable_opfs didn't \
         take effect, the host built against MemoryStore anyway, or \
         OpfsStore failed to rehydrate on reattach. Detail: {kb_article_persisted_v}"
    );

    // -- Phase 15: Stage 2B — multi-SDK backend peer creation --------
    //
    // Clicks the "+ Backend (Memory)" and "+ Backend (OPFS)" buttons.
    // Each spawns a *new* `Sdk::Worker` in `Peers.sdks` (lazy), so the
    // peer-management footer should reflect the growing SDK count.
    //
    // Pre-state: the boot worker is SDK #0 (sdk_count == 1), plus
    // however many additional SDKs prior phases may have attached
    // (currently none — Phases 12-14 don't add SDKs). We assert
    // `final_sdk_count == initial + 2` rather than exact 3 so prior
    // setup can shift without breaking this phase.
    // The Phase 11 reload closed all windows. Respawn the Peers
    // window from the palette before we can read its footer or click
    // its mode buttons.
    let peers_respawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            if (!layer) return 'no-dom-layer';
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            for (const b of btns) {
                if (b.textContent.trim() === '+ Peers') {
                    b.click();
                    return 'clicked';
                }
            }
            return 'no-peers-spawn-btn';
            "#,
            vec![],
        )
        .await?;
    println!(
        "  respawn Peers window: {:?}",
        peers_respawn.as_str().unwrap_or("non-string")
    );
    sleep(Duration::from_millis(600)).await;

    // Footer wording: `N peer(s)` when sdk_count == 1, or
    // `N peer(s) — 1 boot + M dedicated worker(s)` when sdk_count > 1.
    // Map back to sdk_count = 1 + dedicated for assertion compatibility
    // with the previous "across N SDK(s)" shape.
    let read_sdk_count_script = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        const sections = root.querySelectorAll('section.window');
        for (const sec of sections) {
            const h2 = sec.querySelector('h2');
            if (!h2 || h2.textContent.trim() !== 'Peers') continue;
            const text = sec.textContent || '';
            const dedicated_re = /1 boot \+ (\d+) dedicated worker/;
            const m = text.match(dedicated_re);
            if (m) return 1 + parseInt(m[1], 10);
            // Footer with no dedicated workers: just `N peer(s)`.
            // sdk_count is 1 (only the boot SDK exists).
            if (/\d+ peer\(s\)/.test(text)) return 1;
            return -1;
        }
        return -2;
    "#;

    let initial_sdk_count = client
        .execute(read_sdk_count_script, vec![])
        .await?
        .as_i64()
        .unwrap_or(-3);
    println!("  initial sdk_count:    {initial_sdk_count}");
    assert!(
        initial_sdk_count >= 1,
        "Couldn't parse SDK count from Peers footer — got {initial_sdk_count}. \
         Footer regex or render may have changed."
    );

    // Helper builds the JS for "click '+ <Mode>' in Peers window".
    fn click_mode_btn_js(btn_text: &str) -> String {
        format!(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {{
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                const btns = sec.querySelectorAll('button');
                for (const b of btns) {{
                    if (b.textContent.trim() === '{btn_text}') {{
                        b.click();
                        return 'clicked';
                    }}
                }}
                return 'no-btn';
            }}
            return 'no-peers-section';
            "#
        )
    }

    let bm_status_v = client
        .execute(&click_mode_btn_js("+ Backend (Memory)"), vec![])
        .await?;
    let bm_status = bm_status_v.as_str().unwrap_or("non-string");
    println!("  + Backend (Memory):   {bm_status}");
    assert_eq!(bm_status, "clicked", "Couldn't click '+ Backend (Memory)' button");
    // Poll for the SDK count to grow rather than burning a fixed 2s.
    // Worker spawn + Ready handshake + drain happens within ~200-400ms
    // typically; give a generous 4s timeout.
    let after_memory_sdk_count =
        wait_for_sdk_count(&client, read_sdk_count_script, initial_sdk_count + 1, 4000)
            .await
            .unwrap_or_else(|e| {
                println!("  wait_for_sdk_count(memory) failed: {e}");
                -1
            });
    println!("  after-memory sdk_count: {after_memory_sdk_count}");
    // Diagnostic: print the actual footer text so we can tell whether
    // the DOM rebuilt at all.
    let footer_text_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                // Find the footer div — it's the last direct child div.
                const divs = sec.querySelectorAll(':scope > div');
                if (divs.length > 0) {
                    return divs[divs.length - 1].textContent;
                }
                return 'no-divs';
            }
            return 'no-section';
            "#,
            vec![],
        )
        .await?;
    println!("  peer-mgmt footer text: {:?}", footer_text_v.as_str().unwrap_or("?"));
    if after_memory_sdk_count != initial_sdk_count + 1 {
        let diag_log = capture_log(&client).await?;
        println!("--- diagnostic: Backend (Memory) click trail ---");
        for line in diag_log.iter().rev().take(40).rev() {
            if line.contains("worker")
                || line.contains("Worker")
                || line.contains("spawn")
                || line.contains("attach")
                || line.contains("CreatePeerWithMode")
                || line.contains("backend")
                || line.contains("error")
                || line.contains("WARN")
            {
                println!("  | {}", line);
            }
        }
    }
    assert_eq!(
        after_memory_sdk_count,
        initial_sdk_count + 1,
        "SDK count didn't grow after Backend (Memory) click. \
         Worker spawn or pending-attachment drain may be broken. \
         Initial: {initial_sdk_count}, After: {after_memory_sdk_count}"
    );

    let bo_status_v = client
        .execute(&click_mode_btn_js("+ Backend (OPFS)"), vec![])
        .await?;
    let bo_status = bo_status_v.as_str().unwrap_or("non-string");
    println!("  + Backend (OPFS):     {bo_status}");
    assert_eq!(bo_status, "clicked", "Couldn't click '+ Backend (OPFS)' button");
    // OPFS workers take longer than memory — give 6s.
    let final_sdk_count =
        wait_for_sdk_count(&client, read_sdk_count_script, initial_sdk_count + 2, 6000)
            .await
            .unwrap_or_else(|e| {
                println!("  wait_for_sdk_count(opfs) failed: {e}");
                -1
            });
    println!("  final sdk_count:      {final_sdk_count}");
    // U7 resolved by upstream PROTOCOL_VERSION=7 (opfs_root): the boot
    // worker now uses `workers/{primary_peer_id}` and each
    // backend-OPFS peer's worker uses `workers/{its_peer_id}`, so the
    // `createSyncAccessHandle` exclusivity no longer blocks
    // coexistence. The Backend (OPFS) click is now expected to grow
    // the SDK count just like Backend (Memory).
    assert_eq!(
        final_sdk_count,
        initial_sdk_count + 2,
        "Backend (OPFS) should have added an SDK after U7 fix. \
         Initial: {initial_sdk_count}, Final: {final_sdk_count}. \
         If this regresses, check that opfs_root is unique per worker."
    );
    let opfs_grew = true;

    // Verify peer rows reflect the Memory peer at minimum.
    let final_rows_v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                return sec.querySelectorAll('tbody tr').length;
            }
            return -1;
            "#,
            vec![],
        )
        .await?;
    let final_rows = final_rows_v.as_i64().unwrap_or(-1);
    println!("  final peer rows:      {final_rows}");
    // Primary + Phase 12 frontend + Memory backend = 3 minimum.
    // Plus OPFS peer (4) only if the OPFS spawn succeeded.
    let expected_rows = if opfs_grew { 4 } else { 3 };
    assert!(
        final_rows >= expected_rows,
        "Expected at least {expected_rows} peer rows, got {final_rows}"
    );

    // -- Phase 15.5: Cross-Worker xworker:// handshake -----------------
    //
    // Phase 15 created two backend Worker peers (Memory + OPFS), each
    // in its own dedicated Web Worker. Phase 15.5 verifies the
    // cross-Worker MessagePort transport landed:
    //
    //   1. Read both backend peer-ids from localStorage (where the
    //      app persists them after creation).
    //   2. Open a Shell window on the backend-memory peer via the
    //      primary shell: `open shell @<bm-pid>`.
    //   3. From that backend-memory shell, submit
    //      `connect xworker://<bo-pid>`. The connect verb dispatches
    //      from the shell's bound peer (the backend-memory Worker),
    //      whose `MessagePortConnector` sends an `OpenChannel` to
    //      the main-thread `MessagePortBroker`, which transfers a
    //      fresh MessagePort pair between the two Workers.
    //   4. Assert the success line (`← connected to <prefix>`)
    //      appears in the backend-memory shell's scrollback.
    //
    // If this phase regresses, the failure is one of:
    //   - Broker not wired into EntityApp boot (no `register_peer`).
    //   - Spawn helper not using `with_control_port` for backends.
    //   - Shell verb_connect still using primary instead of self.peer_id.
    //   - Upstream MessagePortConnector/Listener regression.
    println!("--- Phase 15.5: Cross-Worker xworker handshake ---");
    // Open a primary-bound Shell first — the Phase 2 Shell didn't
    // survive the Phase 11 reload, and we need a Shell to drive
    // `open shell @<backend-pid>` for the cross-Worker step.
    let open_primary_shell = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        const btns = root.querySelectorAll('button.spawn-btn');
        for (const b of btns) {
            if (b.textContent.trim() === '+ Shell') {
                b.click();
                return 'clicked';
            }
        }
        return `no-shell-btn-of-${btns.length}`;
    "#;
    let primary_shell_open = client.execute(open_primary_shell, vec![]).await?;
    assert_eq!(
        primary_shell_open.as_str().unwrap_or(""),
        "clicked",
        "couldn't click '+ Shell' palette button to reopen primary shell"
    );
    sleep(Duration::from_millis(500)).await;

    let peers_ls_v = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let peers_ls_str = peers_ls_v.as_str().unwrap_or("");
    let mut bm_pid: Option<String> = None;
    let mut bo_pid: Option<String> = None;
    for line in peers_ls_str.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 4 {
            continue;
        }
        let pid = parts[0];
        let mode = parts[parts.len() - 1];
        match mode {
            "backend-memory" if bm_pid.is_none() => bm_pid = Some(pid.to_string()),
            "backend-opfs" if bo_pid.is_none() => bo_pid = Some(pid.to_string()),
            _ => {}
        }
    }
    let bm_pid = bm_pid.expect("expected backend-memory pid in localStorage after Phase 15");
    let bo_pid = bo_pid.expect("expected backend-opfs pid in localStorage after Phase 15");
    println!("  backend-memory pid: {bm_pid}");
    println!("  backend-opfs   pid: {bo_pid}");

    // Open a Shell bound to the backend-memory Worker peer via the
    // primary shell. The `open shell @<pid>` form is the standard way
    // to spawn a window bound to a non-default peer.
    println!("  opening shell @backend-memory via primary shell");
    let _ = shell_submit(&client, &format!("open shell @{bm_pid}"), 600).await?;

    // Poll for the new Shell section to appear with our peer-id.
    let new_shell_deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut new_shell_seen = false;
    while std::time::Instant::now() < new_shell_deadline {
        let v = client
            .execute(
                r#"
                const [pid] = arguments;
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const sec = root.querySelector(
                    `section.window[data-peer-id="${pid}"]`
                );
                if (!sec) return false;
                const title = sec.querySelector('header h3');
                return title && title.textContent.trim() === 'Shell';
                "#,
                vec![serde_json::Value::String(bm_pid.clone())],
            )
            .await?;
        if v.as_bool().unwrap_or(false) {
            new_shell_seen = true;
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert!(
        new_shell_seen,
        "Shell window bound to backend-memory pid {bm_pid} never rendered after \
         `open shell @<pid>`. data-peer-id attribute missing, or peer-scoped \
         window spawn broken."
    );
    println!("  ✓ backend-memory shell rendered");

    // Submit the xworker connect from the backend-memory shell.
    let connect_line = format!("connect xworker://{bo_pid}");
    println!("  submitting: {connect_line}");
    let _ = shell_submit_for_peer(&client, &bm_pid, &connect_line, 200).await?;

    // Poll for the success line. Worker → broker → other Worker is a
    // round-trip of postMessage hops plus the entity-protocol
    // handshake — generously 3s.
    //
    // The verb prints `connected to <short_pid>` where short_pid is
    // `first-8...last-6` (per `views::short_pid`), not a raw prefix.
    let short_bo = if bo_pid.len() > 16 {
        format!("{}...{}", &bo_pid[..8], &bo_pid[bo_pid.len() - 6..])
    } else {
        bo_pid.clone()
    };
    let success_needle = format!("connected to {short_bo}");
    let connect_deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut connect_seen = false;
    let mut last_sb = String::new();
    while std::time::Instant::now() < connect_deadline {
        last_sb = shell_scrollback_for_peer(&client, &bm_pid).await?;
        if last_sb.contains(&success_needle) {
            connect_seen = true;
            break;
        }
        // Also break early on a clear failure line so we surface the
        // error instead of timing out.
        if last_sb.contains("✗ connect") {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    if !connect_seen {
        let diag = capture_log(&client).await?;
        println!("--- diagnostic: xworker connect log tail ---");
        for line in diag.iter().rev().take(40).rev() {
            if line.contains("xworker")
                || line.contains("MessagePort")
                || line.contains("broker")
                || line.contains("ControlPort")
                || line.contains("connect_peer")
                || line.contains("WARN")
                || line.contains("ERROR")
            {
                println!("  | {}", line);
            }
        }
        panic!(
            "xworker connect from backend-memory to backend-opfs did not produce \
             success line within 3s.\nLast scrollback:\n{last_sb}"
        );
    }
    println!("  ✓ xworker handshake completed (success line in scrollback)");

    // -- Phase 15.6: MultiConnector composition — same backend, both schemes --
    //
    // Phase 15.5 proved `xworker://` works from the backend-memory
    // Worker. Phase 15.6 verifies the upstream MultiConnector
    // (landed in `bindings/wasm-worker-host/src/lib.rs:354-391`)
    // composes ws/wss alongside xworker on the same Worker. Before
    // this landing, a Worker with a control port had ONLY
    // `MessagePortConnector` — `connect ws://...` from a backend
    // shell errored with "expected xworker:// scheme" (wrong scheme
    // handler). After it, the same Worker dispatches both:
    //   - xworker://<pid>  → MessagePortConnector → broker
    //   - ws://<host:port> → BrowserWebSocketConnector → external relay
    //
    // The Tauri listener from Phase 14 is still alive (TauriListener
    // drop happens at fn-scope end). We reuse its ws_addr as a real
    // target — the backend-memory Worker connects to it via ws, and
    // we assert the success line appears in the backend-memory shell.
    println!("--- Phase 15.6: MultiConnector ws scheme from backend Worker ---");
    let ws_connect_line = format!("connect {}", tauri.ws_addr);
    println!("  submitting from backend-memory shell: {ws_connect_line}");
    let _ = shell_submit_for_peer(&client, &bm_pid, &ws_connect_line, 200).await?;

    // ws connect is fast on loopback — 2s is plenty. Use the same
    // short_pid format the verb prints (first-8...last-6).
    let short_tauri = if tauri.peer_id.len() > 16 {
        format!(
            "{}...{}",
            &tauri.peer_id[..8],
            &tauri.peer_id[tauri.peer_id.len() - 6..]
        )
    } else {
        tauri.peer_id.clone()
    };
    let ws_success_needle = format!("connected to {short_tauri}");
    let ws_deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut ws_connect_seen = false;
    let mut ws_last_sb = String::new();
    while std::time::Instant::now() < ws_deadline {
        ws_last_sb = shell_scrollback_for_peer(&client, &bm_pid).await?;
        if ws_last_sb.contains(&ws_success_needle) {
            ws_connect_seen = true;
            break;
        }
        if ws_last_sb.contains("✗ connect ws://") {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    if !ws_connect_seen {
        // Loud diagnostic — if the MultiConnector composition
        // regresses, the failure here will be "expected xworker://
        // scheme" or similar (the wrong-scheme-handler signature).
        // That's the regression we're guarding against.
        let diag = capture_log(&client).await?;
        println!("--- diagnostic: ws connect from backend log tail ---");
        for line in diag.iter().rev().take(40).rev() {
            if line.contains("connect")
                || line.contains("ws://")
                || line.contains("MultiConnector")
                || line.contains("MessagePort")
                || line.contains("Browser")
                || line.contains("WARN")
                || line.contains("ERROR")
            {
                println!("  | {}", line);
            }
        }
        panic!(
            "ws connect from backend-memory to {} did not produce \
             success line within 3s. If the scrollback shows \
             \"expected xworker:// scheme\", the host's MultiConnector \
             composition regressed (kernel-side).\n\
             Last scrollback:\n{}",
            tauri.ws_addr, ws_last_sb
        );
    }
    println!(
        "  ✓ ws connect from backend Worker completed (MultiConnector composes both schemes)"
    );

    // -- Phase 15.7: backend → boot-worker primary via xworker:// -----
    //
    // Proves the boot-worker control-port wiring (consumer-side, this
    // session) plus the upstream multi-peer reachability fix
    // actually composes to make the boot worker's primary
    // peer reachable as an `xworker://` target from sibling Workers.
    //
    // Before this wiring landed:
    //   - The boot worker spawned without a control port → its host
    //     bound only `BrowserWebSocketConnector`, no listener.
    //   - Even after wiring, if only the primary listener were bound
    //     (earlier upstream), additional boot peers would be
    //     silent. Today every boot peer gets a listener and the
    //     consumer registers every boot peer-id against the broker.
    //
    // Phase 15.7 verifies: backend-memory Worker connects via
    // `xworker://<system-primary-pid>`, the broker routes to the
    // boot-worker control port, the boot worker's ControlPortClient
    // dispatches by `to_peer` to the primary's MessagePortListener,
    // handshake completes, success line lands in the backend shell.
    println!("--- Phase 15.7: backend → boot-worker primary via xworker:// ---");
    // The boot worker hosts every `frontend`-mode peer in localStorage,
    // primary first per `partition_entries` + `persisted.remove(0)`.
    let mut system_pid: Option<String> = None;
    for line in peers_ls_str.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 4 { continue; }
        if parts[parts.len() - 1] == "frontend" {
            system_pid = Some(parts[0].to_string());
            break;
        }
    }
    let system_pid = system_pid
        .expect("expected at least one frontend-mode entry in localStorage (boot primary)");
    println!("  boot system primary pid: {system_pid}");

    let boot_connect_line = format!("connect xworker://{system_pid}");
    println!("  submitting from backend-memory shell: {boot_connect_line}");
    let _ = shell_submit_for_peer(&client, &bm_pid, &boot_connect_line, 200).await?;

    let short_system = if system_pid.len() > 16 {
        format!(
            "{}...{}",
            &system_pid[..8],
            &system_pid[system_pid.len() - 6..]
        )
    } else {
        system_pid.clone()
    };
    let boot_success_needle = format!("connected to {short_system}");
    let boot_deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut boot_connect_seen = false;
    let mut boot_last_sb = String::new();
    while std::time::Instant::now() < boot_deadline {
        boot_last_sb = shell_scrollback_for_peer(&client, &bm_pid).await?;
        if boot_last_sb.contains(&boot_success_needle) {
            boot_connect_seen = true;
            break;
        }
        if boot_last_sb.contains("✗ connect xworker://") {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    if !boot_connect_seen {
        let diag = capture_log(&client).await?;
        println!("--- diagnostic: backend → boot-primary xworker log tail ---");
        for line in diag.iter().rev().take(40).rev() {
            if line.contains("xworker")
                || line.contains("MessagePort")
                || line.contains("ControlPort")
                || line.contains("broker")
                || line.contains("boot-worker peers")
                || line.contains("WARN")
                || line.contains("ERROR")
            {
                println!("  | {}", line);
            }
        }
        panic!(
            "xworker connect from backend-memory to boot-primary {} did \
             not produce success line within 3s.\nLast scrollback:\n{}",
            system_pid, boot_last_sb
        );
    }
    println!(
        "  ✓ boot-worker primary reachable via xworker:// from a sibling Worker"
    );

    // -- Phase 15.8: runtime-added Frontend reachable via xworker:// ---
    //
    // Boot-time registration (Phase 15.7) covers peers known when
    // `build_wasm_app` runs. Frontends created AT RUNTIME (via
    // `+ Frontend` click or `peer create frontend`) hit a separate
    // code path: `Peers::create_new_peer` round-trips
    // `Request::CreatePeer` to the boot worker, the boot worker's
    // host binds a `MessagePortListener` for the new peer
    // (kernel-side), and the consumer's
    // `create_frontend_peer` success path now calls
    // `broker.register_peer(new_pid, boot_port.clone())` so the
    // broker knows about it (consumer-side — Gap A fix).
    //
    // Before the consumer-side fix, the new Frontend worked locally
    // but was silently unreachable cross-Worker — "feature half
    // works" quiet bug. This phase is the gate against that
    // regression.
    println!("--- Phase 15.8: runtime-added Frontend reachable via xworker:// ---");

    // Snapshot pre-create localStorage so we can identify which pid
    // is the newly-added one. Existing Frontends from Phase 12 are
    // already in the list.
    let pre_ls_v = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let pre_ls = pre_ls_v.as_str().unwrap_or("").to_string();
    let pre_pids: std::collections::HashSet<String> = pre_ls
        .lines()
        .filter_map(|l| l.split('|').next().map(|s| s.to_string()))
        .collect();
    println!("  pre-create entity_peers lines: {}", pre_pids.len());

    // Click `+ Frontend` in the Peers window — same button Phase 12
    // exercised pre-reload. The click dispatches
    // `Action::CreateFrontendPeer` → `create_frontend_peer`.
    let frontend_status = client
        .execute(&click_mode_btn_js("+ Frontend"), vec![])
        .await?;
    assert_eq!(
        frontend_status.as_str().unwrap_or(""),
        "clicked",
        "couldn't click '+ Frontend' in Peers window"
    );

    // Wait for localStorage to grow — that's our signal that the
    // CreatePeer round-trip completed and `create_frontend_peer`
    // persisted the new entry. 3s budget for the worker round-trip
    // on a dev box.
    let create_deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut new_fe_pid: Option<String> = None;
    while std::time::Instant::now() < create_deadline {
        let cur_ls_v = client
            .execute(
                r#"return window.localStorage.getItem('entity_peers') || '';"#,
                vec![],
            )
            .await?;
        let cur_ls = cur_ls_v.as_str().unwrap_or("").to_string();
        let new_pid = cur_ls.lines().find_map(|l| {
            let pid = l.split('|').next()?;
            if pre_pids.contains(pid) {
                return None;
            }
            // Filter to frontend mode only (defensive — there
            // shouldn't be any other mode landing in this window).
            let parts: Vec<&str> = l.split('|').collect();
            if parts.last().copied() == Some("frontend") {
                Some(pid.to_string())
            } else {
                None
            }
        });
        if let Some(pid) = new_pid {
            new_fe_pid = Some(pid);
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    let new_fe_pid = new_fe_pid.expect(
        "runtime-added Frontend never landed in localStorage within 3s — \
         `+ Frontend` click → CreatePeer → persistence path is broken",
    );
    println!("  runtime-added frontend pid: {new_fe_pid}");

    // Connect from the still-open backend-memory shell.
    let runtime_connect_line = format!("connect xworker://{new_fe_pid}");
    println!("  submitting from backend-memory shell: {runtime_connect_line}");
    let _ = shell_submit_for_peer(&client, &bm_pid, &runtime_connect_line, 200).await?;

    let short_new_fe = if new_fe_pid.len() > 16 {
        format!(
            "{}...{}",
            &new_fe_pid[..8],
            &new_fe_pid[new_fe_pid.len() - 6..]
        )
    } else {
        new_fe_pid.clone()
    };
    let runtime_success_needle = format!("connected to {short_new_fe}");
    let runtime_deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut runtime_connect_seen = false;
    let mut runtime_last_sb = String::new();
    while std::time::Instant::now() < runtime_deadline {
        runtime_last_sb = shell_scrollback_for_peer(&client, &bm_pid).await?;
        if runtime_last_sb.contains(&runtime_success_needle) {
            runtime_connect_seen = true;
            break;
        }
        if runtime_last_sb.contains("✗ connect xworker://") {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    if !runtime_connect_seen {
        let diag = capture_log(&client).await?;
        println!("--- diagnostic: runtime-Frontend xworker log tail ---");
        for line in diag.iter().rev().take(40).rev() {
            if line.contains("registered runtime-added Frontend")
                || line.contains("xworker")
                || line.contains("ControlPort")
                || line.contains("broker")
                || line.contains("ChannelDenied")
                || line.contains("WARN")
                || line.contains("ERROR")
            {
                println!("  | {}", line);
            }
        }
        panic!(
            "xworker connect to runtime-added Frontend {} did not produce \
             success line within 3s. If the diagnostic shows \"no such peer\" \
             from the broker, the consumer-side register_peer in \
             `create_frontend_peer` (Gap A fix) regressed.\nLast scrollback:\n{}",
            new_fe_pid, runtime_last_sb
        );
    }
    println!(
        "  ✓ runtime-added Frontend reachable via xworker:// (Gap A regression gate)"
    );

    // -- Phase 16: Stage 2C — PeerConfig persistence across reload ----
    //
    // After Phase 15: boot worker SDK + 2 backend SDKs = 3 SDKs total.
    // Phase 16 reloads the page. With Stage 2C, the Backend(Memory)
    // and Backend(OPFS) peers persisted in localStorage with their
    // `mode` field; on reload `new_wasm_worker` should partition by
    // mode, send only the Frontend peers into the boot worker, and
    // respawn each Backend* peer into its own worker SDK with a
    // stable opfs_root.
    //
    // The pass condition is straightforward: sdk_count after reload
    // matches sdk_count before reload (3 SDKs each).
    println!("--- Phase 16: Stage 2C reload — persisted modes ---");
    let pre_phase16_sdk_count = final_sdk_count;
    let pre_phase16_peers_ls = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let pre_phase16_peers_str = pre_phase16_peers_ls.as_str().unwrap_or("");
    println!(
        "  pre-reload entity_peers bytes: {}",
        pre_phase16_peers_str.len()
    );
    // Sanity-check that the persisted format actually carries `mode`.
    // (Pre-2C format was 3 fields — `peer_id|seed|label`; post-2C is 4.)
    let has_backend_mode = pre_phase16_peers_str
        .lines()
        .any(|l| l.ends_with("|backend-memory") || l.ends_with("|backend-opfs"));
    assert!(
        has_backend_mode,
        "localStorage should contain at least one backend-mode peer after \
         Phase 15 created two of them. Got:\n{pre_phase16_peers_str}"
    );

    client.refresh().await?;
    // Boot worker + 2 backend-worker respawns; OPFS replay adds latency.
    // Poll for "Frame loop started" rather than burning a fixed 6s.
    let phase16_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 16 reload boot: {phase16_boot_ms}ms");

    // Respawn the Peers window to read its footer.
    let _peers_respawn2 = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            if (!layer) return 'no-dom-layer';
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            for (const b of btns) {
                if (b.textContent.trim() === '+ Peers') {
                    b.click();
                    return 'clicked';
                }
            }
            return 'no-peers-spawn-btn';
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(800)).await;

    let post_reload_sdk_count = client
        .execute(read_sdk_count_script, vec![])
        .await?
        .as_i64()
        .unwrap_or(-3);
    println!("  post-reload sdk_count: {post_reload_sdk_count}");
    println!("  pre-reload  sdk_count: {pre_phase16_sdk_count}");
    let post_reload_log = capture_log(&client).await?;
    let phase16_panics = count_panics(&post_reload_log);
    assert!(
        phase16_panics.is_empty(),
        "Phase 16 reload triggered panic(s):\n{}",
        phase16_panics
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
    assert_eq!(
        post_reload_sdk_count,
        pre_phase16_sdk_count,
        "Stage 2C: SDK count should match across reload (backend peers \
         should re-spawn into their own SDKs). Pre: {pre_phase16_sdk_count}, \
         Post: {post_reload_sdk_count}. \
         If post == 1, mode-based partitioning didn't fire — all peers \
         landed in the boot worker. If post == 2, only one backend \
         respawned — check the partition logic."
    );

    // -- Phase 17: OPFS cleanup on Backend(OPFS) delete ---------------
    //
    // Verifies: when a Backend(OPFS) peer is deleted, its localStorage
    // entry is removed AND a tombstone is recorded. At next boot, the
    // `workers/{peer_id}/` OPFS subdir is removed and the tombstone is
    // cleared. No worker-terminate API exists upstream, so runtime
    // OPFS cleanup deferred to boot-time GC.
    println!("--- Phase 17: OPFS cleanup on delete + reload ---");

    // Find the OPFS peer's id from localStorage (line ending with
    // |backend-opfs). Failure mode: Phase 15 didn't actually persist
    // an OPFS peer; abort with a clear error.
    let opfs_peer_id_v = client
        .execute(
            r#"
            const data = window.localStorage.getItem('entity_peers') || '';
            for (const line of data.split('\n')) {
                if (line.endsWith('|backend-opfs')) {
                    return line.split('|')[0];
                }
            }
            return '';
            "#,
            vec![],
        )
        .await?;
    let opfs_peer_id = opfs_peer_id_v.as_str().unwrap_or("").to_string();
    println!("  opfs peer id:         {opfs_peer_id}");
    assert!(
        !opfs_peer_id.is_empty(),
        "Couldn't find a backend-opfs peer in localStorage — Phase 15 \
         must have created one. Check that the mode persisted correctly."
    );

    // Confirm the OPFS subdir actually exists before delete.
    let opfs_subdir_pre = check_opfs_workers_subdir(&client, &opfs_peer_id).await?;
    println!("  opfs subdir pre:      {opfs_subdir_pre}");
    assert_eq!(
        opfs_subdir_pre, "exists",
        "OPFS subdir `workers/{opfs_peer_id}/` should exist before delete; \
         got '{opfs_subdir_pre}'. Without an existing subdir the cleanup \
         assertion below is meaningless."
    );

    // Click Delete on the row whose ID cell contains the OPFS peer's
    // id prefix (first 8 chars of the short_pid). Match by `includes`,
    // not `startsWith`: since the peer-identity-UX work the ID cell
    // renders "{role_glyph} {short_pid}" (e.g. "◆⛁ 2KX3rahy...xxxxxx"),
    // so the row text no longer starts with the raw pid.
    let opfs_prefix = &opfs_peer_id[..8.min(opfs_peer_id.len())];

    // BUG #1 regression baseline (backend-peer-delete audit):
    // a backend peer is the sole primary of its own dedicated Worker
    // SDK, so deleting it must DROP the whole SDK and remove the Peers
    // row. Before the fix, `delete_peer` routed into the worker, which
    // refused to delete its own primary → `Ok(false)` → the row stuck
    // forever (the "24 stuck peers" pain). Capture sdk_count + the row's
    // presence NOW so the post-delete assertions below can prove both
    // the SDK and the row are gone — the assertion whose absence let the
    // bug ship green.
    let pre_delete_sdk_count = client
        .execute(read_sdk_count_script, vec![])
        .await?
        .as_i64()
        .unwrap_or(-3);
    println!("  pre-delete sdk_count: {pre_delete_sdk_count}");
    assert!(
        pre_delete_sdk_count >= 2,
        "Phase 17: expected the backend-OPFS peer to be its own dedicated \
         Worker SDK (sdk_count >= 2) before delete; got {pre_delete_sdk_count}. \
         Phase 15 must have grown the SDK count."
    );

    // JS that counts Peers-window rows whose ID cell contains the OPFS
    // peer prefix. Used to assert the row is GONE after delete.
    let count_opfs_rows_js = format!(
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        const sections = root.querySelectorAll('section.window');
        for (const sec of sections) {{
            const h2 = sec.querySelector('h2');
            if (!h2 || h2.textContent.trim() !== 'Peers') continue;
            let n = 0;
            for (const row of sec.querySelectorAll('tbody tr')) {{
                if ((row.textContent || '').includes('{opfs_prefix}')) n++;
            }}
            return n;
        }}
        return -1;
        "#,
    );
    let opfs_rows_pre = client
        .execute(&count_opfs_rows_js, vec![])
        .await?
        .as_i64()
        .unwrap_or(-1);
    println!("  opfs rows pre-delete: {opfs_rows_pre}");
    assert!(
        opfs_rows_pre >= 1,
        "Phase 17: the backend-OPFS peer's row should be present before \
         delete; got {opfs_rows_pre} matching rows."
    );

    let click_delete_js = format!(
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        const sections = root.querySelectorAll('section.window');
        for (const sec of sections) {{
            const h2 = sec.querySelector('h2');
            if (!h2 || h2.textContent.trim() !== 'Peers') continue;
            const rows = sec.querySelectorAll('tbody tr');
            for (const row of rows) {{
                const txt = row.textContent || '';
                if (!txt.includes('{opfs_prefix}')) continue;
                const buttons = row.querySelectorAll('button');
                for (const b of buttons) {{
                    if (b.textContent.trim() === 'Delete') {{
                        b.click();
                        return 'clicked';
                    }}
                }}
                return 'no-delete-btn';
            }}
            return 'no-row-match';
        }}
        return 'no-peers-section';
        "#,
    );
    let delete_status = client.execute(&click_delete_js, vec![]).await?;
    println!("  delete click:         {:?}", delete_status.as_str().unwrap_or("non-string"));
    assert_eq!(
        delete_status.as_str(),
        Some("clicked"),
        "Couldn't click Delete on the OPFS peer row"
    );
    sleep(Duration::from_secs(1)).await;

    // BUG #1 regression gate: the dedicated Worker SDK must be torn down
    // and the Peers row must vanish. Without the fix, `delete_peer`
    // returns `Ok(false)` (the worker refuses to delete its own
    // primary), sdk_count is unchanged, and this row never goes — the
    // exact failure the user hit ("can't delete backend peers"). This
    // assertion is the one whose absence let the bug ship green: Phase
    // 17 previously checked only the tombstone / localStorage / OPFS
    // subdir (all cleaned BEFORE `delete_peer` runs), never the row.
    let post_delete_sdk_count = wait_for_sdk_count(
        &client,
        read_sdk_count_script,
        pre_delete_sdk_count - 1,
        4000,
    )
    .await
    .map_err(|e| {
        format!(
            "Phase 17 BUG #1 regression: deleting the backend-OPFS peer did \
             NOT drop its dedicated Worker SDK. {e}. This is the backend-peer-\
             delete bug — `Peers::delete_peer` must tear down the whole SDK \
             for an is_backend_hosted peer."
        )
    })?;
    println!("  post-delete sdk_count: {post_delete_sdk_count}");

    let opfs_rows_post = client
        .execute(&count_opfs_rows_js, vec![])
        .await?
        .as_i64()
        .unwrap_or(-1);
    println!("  opfs rows post-delete: {opfs_rows_post}");
    assert_eq!(
        opfs_rows_post, 0,
        "Phase 17 BUG #1 regression: the backend-OPFS peer's Peers-window \
         row should be GONE after delete, but {opfs_rows_post} matching \
         row(s) remain. The dedicated Worker SDK was not torn down."
    );

    // Tombstone should be set now.
    let tombstones_post_delete = client
        .execute(
            r#"return window.localStorage.getItem('entity_opfs_tombstones') || '';"#,
            vec![],
        )
        .await?;
    let tombstones_str = tombstones_post_delete.as_str().unwrap_or("");
    println!("  tombstones post-del:  {tombstones_str}");
    assert!(
        tombstones_str.lines().any(|l| l == opfs_peer_id),
        "Expected '{opfs_peer_id}' in entity_opfs_tombstones after delete; \
         got '{tombstones_str}'"
    );

    // localStorage entry for the peer should be gone.
    let post_delete_peers = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let post_delete_str = post_delete_peers.as_str().unwrap_or("");
    assert!(
        !post_delete_str.lines().any(|l| l.starts_with(&opfs_peer_id)),
        "OPFS peer's localStorage entry should be gone post-delete, but \
         found a line starting with {opfs_peer_id} in:\n{post_delete_str}"
    );

    // OPFS subdir should still exist pre-reload (worker holds handles).
    // Re-checked just for the diagnostic; no assertion either way.
    let opfs_subdir_after_delete =
        check_opfs_workers_subdir(&client, &opfs_peer_id).await?;
    println!("  opfs subdir post-del: {opfs_subdir_after_delete}");

    // Reload — boot-time cleanup runs before any worker spawn.
    client.refresh().await?;
    let phase17_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 17 reload boot: {phase17_boot_ms}ms");

    let tombstones_post_reload = client
        .execute(
            r#"return window.localStorage.getItem('entity_opfs_tombstones') || '';"#,
            vec![],
        )
        .await?;
    let tombstones_post_str = tombstones_post_reload.as_str().unwrap_or("");
    println!("  tombstones post-reload: '{tombstones_post_str}'");
    assert!(
        tombstones_post_str.is_empty(),
        "Boot-time OPFS cleanup should have drained the tombstone list; \
         still got '{tombstones_post_str}'"
    );

    let opfs_subdir_post_reload =
        check_opfs_workers_subdir(&client, &opfs_peer_id).await?;
    println!("  opfs subdir post-reload: {opfs_subdir_post_reload}");
    assert_eq!(
        opfs_subdir_post_reload, "missing",
        "OPFS subdir `workers/{opfs_peer_id}/` should be removed by \
         boot-time cleanup; status='{opfs_subdir_post_reload}'"
    );

    // ====================================================================
    // Phase 18: Direct-browser mode (C4 — closes the largest §5 coverage
    // hole). Every phase above ran Worker mode (?worker=1). Direct mode —
    // the auto-fallback, in-memory-only arm — had ZERO app-level e2e, and
    // that is exactly the hole the original freeze fell through. Here we
    // re-navigate with ?worker=0 and drive the Direct spine:
    //   boot → C5 banner → window factory → reactive write → reload.
    // It also proves C5 (persist() + ephemeral banner) in the live build.
    // ====================================================================
    println!("--- Phase 18: Direct-browser mode (?worker=0) ---");

    // The keypair persisted by the worker-mode phases must survive the
    // mode switch (localStorage is origin-scoped).
    let pre_direct_peers = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let pre_direct_peers_str = pre_direct_peers.as_str().unwrap_or("").to_string();
    assert!(
        !pre_direct_peers_str.is_empty(),
        "Phase 18: localStorage 'entity_peers' empty before the Direct flow — \
         can't verify identity survival."
    );

    client
        .goto(&format!(
            "http://localhost:{}/?worker=0&log=trace",
            http_server_port()
        ))
        .await?;
    let phase18_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 18 direct boot: {phase18_boot_ms}ms");

    let direct_log = capture_log(&client).await?;
    let direct_panics = count_panics(&direct_log);
    assert!(
        direct_panics.is_empty(),
        "Phase 18: Direct-mode boot panicked:\n{}",
        direct_panics
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n")
    );

    // 18a — Direct mode is now DURABLE via a main-thread IndexedDB primary
    // (the IDB system-seed primary). So `?worker=0` must (1) write
    // the primary tree into an `entity-peer-*` IndexedDB database, and (2) NOT
    // show the old "memory only" ephemeral banner — `DurableDirectIdb`
    // suppresses it, like Worker mode. This is the committed, end-to-end analog
    // of the isolated engine proof: durable Direct arm in the real app.
    //
    // The IDB writes ride the ~250ms write-behind debounce, so poll for the
    // store to fill rather than snapshot immediately after boot.
    let mut idb_entities = 0u64;
    let mut idb_locations = 0u64;
    let idb_deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < idb_deadline {
        let idb = client
            .execute_async(
                r#"
                const cb = arguments[arguments.length - 1];
                (async () => {
                    const dbs = (await indexedDB.databases()).map(d => d.name).filter(Boolean);
                    const ours = dbs.filter(n => n.startsWith('entity-peer-'));
                    if (!ours.length) { cb({entities:0, locations:0}); return; }
                    const db = await new Promise((res, rej) => {
                        const r = indexedDB.open(ours[0]); r.onsuccess=()=>res(r.result); r.onerror=()=>rej(r.error);
                    });
                    const count = s => new Promise((res, rej) => {
                        const rq = db.transaction(s,'readonly').objectStore(s).count();
                        rq.onsuccess=()=>res(rq.result); rq.onerror=()=>rej(rq.error);
                    });
                    cb({entities: await count('entities'), locations: await count('locations')});
                })().catch(e => cb({entities:0, locations:0, error:String(e)}));
                "#,
                vec![],
            )
            .await?;
        idb_entities = idb.get("entities").and_then(|v| v.as_u64()).unwrap_or(0);
        idb_locations = idb.get("locations").and_then(|v| v.as_u64()).unwrap_or(0);
        if idb_entities > 0 && idb_locations > 0 {
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }
    assert!(
        idb_entities > 0 && idb_locations > 0,
        "Phase 18: Direct mode must persist its primary tree into IndexedDB \
         (durable Direct arm); got entities={idb_entities} locations={idb_locations}."
    );
    println!("  Direct IDB primary durable: {idb_entities} entities / {idb_locations} locations");

    // The durable Direct arm must NOT show the old "memory only" ephemeral
    // banner (it's suppressed for DurableDirectIdb, as in Worker mode).
    let banner_boot_text = banner_text(&client).await?;
    assert!(
        !banner_boot_text.contains("memory only"),
        "Phase 18: durable Direct (IDB) must NOT show the 'memory only' ephemeral \
         banner; got: {banner_boot_text:?}"
    );
    println!("  Direct durable: no 'memory only' banner (text={banner_boot_text:?})");

    // 18b — C5a: the persist() request runs at boot. It's spawn_local'd
    // and awaits navigator.storage promises, so its log line lands shortly
    // AFTER "Frame loop started" — poll for it rather than snapshot-check.
    let mut persist_logged = false;
    let persist_deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < persist_deadline {
        let l = capture_log(&client).await?;
        if l.iter().any(|x| x.contains("storage durability:")) {
            persist_logged = true;
            break;
        }
        sleep(Duration::from_millis(150)).await;
    }
    assert!(
        persist_logged,
        "Phase 18: expected a 'storage durability:' log line (C5a persist() \
         request) within 3s of the Direct boot."
    );

    // 18c — window factory in Direct mode (§5 'U' hole — window spawn was
    // never driven in Direct). Discover the palette the same way Phase 2
    // does; the floor must match Worker mode.
    let direct_window_types: Vec<String> = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            if (!layer) return [];
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            const labels = [];
            for (const b of btns) {
                const t = b.textContent.trim();
                if (t.startsWith('+ ')) labels.push(t.slice(2));
                else labels.push(t);
            }
            return labels;
            "#,
            vec![],
        )
        .await?
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        direct_window_types.len() >= 14,
        "Phase 18: Direct mode discovered only {} window types ({:?}); the \
         palette/factory should match Worker mode (>=14).",
        direct_window_types.len(),
        direct_window_types
    );
    println!(
        "  direct palette: {} window types",
        direct_window_types.len()
    );

    // Spawn the Shell window — proves the Direct-arm window factory builds
    // and renders an interactive window.
    let shell_spawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            if (!layer) return 'no-dom-layer';
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            for (const b of btns) {
                if (b.textContent.trim() === '+ Shell') { b.click(); return 'clicked'; }
            }
            return `no-shell-btn-of-${btns.length}`;
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        shell_spawn.as_str(),
        Some("clicked"),
        "Phase 18: couldn't spawn the Shell window in Direct mode: {shell_spawn:?}"
    );
    sleep(Duration::from_millis(600)).await;

    // 18d — reactive write in Direct mode (§5 'A' hole). A shell verb is a
    // full DOM-event → Action → tree-write → subscription → dirty → render
    // round trip; the scrollback is entity-backed. If the Direct reactive
    // loop is broken the scrollback won't update.
    let scrollback_before = shell_scrollback(&client).await.unwrap_or_default();
    let scrollback_after = shell_submit(&client, "help", 800).await?;
    assert!(
        scrollback_after.len() > scrollback_before.len()
            && scrollback_after.to_lowercase().contains("help"),
        "Phase 18: Direct-mode shell reactive write didn't update the scrollback \
         (before={} bytes, after={} bytes). The DOM→Action→tree→render loop may \
         be broken on the Direct arm.",
        scrollback_before.len(),
        scrollback_after.len()
    );
    println!(
        "  direct reactive write ok ({} → {} bytes)",
        scrollback_before.len(),
        scrollback_after.len()
    );

    // 18e — Direct reload semantics. Identity (keypair) must round-trip
    // across the Direct boot AND a Direct reload; the in-memory tree being
    // lost is BY DESIGN (that's what the banner warns about), so we do NOT
    // assert tree survival here — only identity + clean re-boot.
    let direct_peers = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let direct_peers_str = direct_peers.as_str().unwrap_or("").to_string();
    assert_eq!(
        pre_direct_peers_str, direct_peers_str,
        "Phase 18: primary keypair changed across the Worker→Direct mode switch."
    );

    client.refresh().await?; // URL still carries ?worker=0
    let phase18_reload_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 18 direct reload boot: {phase18_reload_ms}ms");

    let post_direct_log = capture_log(&client).await?;
    let post_direct_panics = count_panics(&post_direct_log);
    assert!(
        post_direct_panics.is_empty(),
        "Phase 18: Direct-mode RELOAD panicked:\n{}",
        post_direct_panics
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
    let post_direct_peers = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    assert_eq!(
        direct_peers_str,
        post_direct_peers.as_str().unwrap_or("").to_string(),
        "Phase 18: primary keypair changed across the Direct reload — identity \
         must persist."
    );

    // ★ Durable Direct: the IDB-backed primary tree must SURVIVE the reload,
    // and the system seed (the durable identity) must be stable. This is the
    // committed reload-survival proof for the Direct arm — the app-level
    // analog of the isolated engine round-trip test.
    let after = client
        .execute_async(
            r#"
            const cb = arguments[arguments.length - 1];
            (async () => {
                const seed = localStorage.getItem('entity_system_seed');
                const dbs = (await indexedDB.databases()).map(d => d.name).filter(Boolean);
                const ours = dbs.filter(n => n.startsWith('entity-peer-'));
                if (!ours.length) { cb({seed, entities:0, locations:0}); return; }
                const db = await new Promise((res, rej) => {
                    const r = indexedDB.open(ours[0]); r.onsuccess=()=>res(r.result); r.onerror=()=>rej(r.error);
                });
                const count = s => new Promise((res, rej) => {
                    const rq = db.transaction(s,'readonly').objectStore(s).count();
                    rq.onsuccess=()=>res(rq.result); rq.onerror=()=>rej(rq.error);
                });
                cb({seed, entities: await count('entities'), locations: await count('locations')});
            })().catch(e => cb({entities:0, locations:0, error:String(e)}));
            "#,
            vec![],
        )
        .await?;
    let after_seed = after.get("seed").and_then(|v| v.as_str()).unwrap_or("");
    let after_entities = after.get("entities").and_then(|v| v.as_u64()).unwrap_or(0);
    let after_locations = after.get("locations").and_then(|v| v.as_u64()).unwrap_or(0);
    assert!(
        !after_seed.is_empty(),
        "Phase 18: the durable system seed (entity_system_seed) must persist across reload."
    );
    assert!(
        after_entities > 0 && after_locations > 0,
        "Phase 18: the IDB primary tree must SURVIVE the Direct reload; got \
         entities={after_entities} locations={after_locations}."
    );
    println!(
        "  Direct IDB primary SURVIVED reload: {after_entities} entities / {after_locations} locations (seed stable)"
    );

    // Durable Direct must NOT show the "memory only" ephemeral banner after
    // reload either (DurableDirectIdb, like Worker mode).
    let banner_after_text = banner_text(&client).await?;
    assert!(
        !banner_after_text.contains("memory only"),
        "Phase 18: durable Direct (IDB) must NOT show the 'memory only' banner \
         after reload; got: {banner_after_text:?}"
    );
    println!("  Phase 18 OK — durable Direct boot, IDB primary, factory, reactive write, reload-survival");

    // ====================================================================
    // Phase 19: Site Mode overlay (P2 — the content-site overlay surface).
    // Runs in the current Direct context (?worker=0), where the demo site
    // seeds synchronously so the overlay resolves deterministically. We
    // DRIVE the feature, not just check it exists (the hard-coded
    // window_types lesson): toggle into the overlay → assert the live demo
    // site rendered into #site-layer → navigate a nav link → toggle back to
    // the entity-browser chrome. A panic in the overlay render would freeze
    // the rAF loop (D13/AP3 — the bug that started this arc), so we assert
    // clean panics throughout.
    // ====================================================================
    println!("--- Phase 19: Site Mode overlay (toggle → render → navigate) ---");

    // 19a — the always-on status-bar toggle is present + visible (light
    // DOM, outside the shadow root, like #storage-banner). Boot lands in
    // chrome mode: #app-container.mode-dom, #site-layer hidden.
    let pre_toggle = client
        .execute(
            r#"
            const c = document.getElementById('app-container');
            const t = document.getElementById('site-toggle');
            const sl = document.getElementById('site-layer');
            return {
                container_class: c ? c.className : null,
                toggle_present: !!t,
                toggle_display: t ? getComputedStyle(t).display : null,
                site_visible: sl ? getComputedStyle(sl).display !== 'none' : null,
            };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        pre_toggle.get("toggle_present").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 19: status-bar #site-toggle missing"
    );
    assert_ne!(
        pre_toggle.get("toggle_display").and_then(|v| v.as_str()),
        Some("none"),
        "Phase 19: #site-toggle should be visible (show_toggle default true): {pre_toggle:?}"
    );
    assert_eq!(
        pre_toggle.get("container_class").and_then(|v| v.as_str()),
        Some("mode-dom"),
        "Phase 19: should boot in chrome mode (mode-dom): {pre_toggle:?}"
    );
    assert_eq!(
        pre_toggle.get("site_visible").and_then(|v| v.as_bool()),
        Some(false),
        "Phase 19: #site-layer should be hidden before toggling: {pre_toggle:?}"
    );

    // 19b — toggle into Site Mode. The light-DOM click drains next frame →
    // toggle_active → apply_site_mode flips the class and
    // render_site_overlay paints the demo site.
    client
        .execute(
            r#"document.getElementById('site-toggle').click(); return true;"#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(500)).await;

    let in_site = client
        .execute(
            r#"
            const c = document.getElementById('app-container');
            const sl = document.getElementById('site-layer');
            const sb = document.getElementById('status-bar');
            return {
                container_class: c ? c.className : null,
                site_visible: sl ? getComputedStyle(sl).display !== 'none' : null,
                status_bar_visible: sb ? getComputedStyle(sb).display !== 'none' : null,
                site_text: sl ? (sl.textContent || '').trim() : '',
                nav_count: sl ? sl.querySelectorAll('a').length : 0,
                has_exit: sl ? Array.from(sl.querySelectorAll('button'))
                    .some(b => (b.textContent || '').trim().startsWith('Enter Peer')) : false,
            };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        in_site.get("container_class").and_then(|v| v.as_str()),
        Some("mode-site"),
        "Phase 19: toggle should switch the container to mode-site: {in_site:?}"
    );
    assert_eq!(
        in_site.get("site_visible").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 19: #site-layer should be visible after toggling: {in_site:?}"
    );
    // The overlay fills the whole page — status bar hidden, exit control
    // moved into the site's own nav bar.
    assert_eq!(
        in_site.get("status_bar_visible").and_then(|v| v.as_bool()),
        Some(false),
        "Phase 19: status bar should hide in Site Mode (overlay fills page): {in_site:?}"
    );
    assert_eq!(
        in_site.get("has_exit").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 19: the site nav bar should carry an 'Enter Peer' control: {in_site:?}"
    );
    let site_text = in_site.get("site_text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        site_text.contains("Entity Demo Site"),
        "Phase 19: overlay didn't render the demo site title; got: {site_text:?}"
    );
    assert!(
        site_text.contains("Welcome"),
        "Phase 19: overlay didn't render the demo index page body; got: {site_text:?}"
    );
    let nav_count = in_site.get("nav_count").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        nav_count >= 3,
        "Phase 19: expected the demo's nav links (>=3) in the overlay, got {nav_count}"
    );
    println!("  overlay rendered the demo site ({nav_count} links)");

    // 19-img: the embed/asset arc — the index seeds an `::embed{ref=assets/
    // figures/demo.svg}`, lowered to a sanitized <img> and resolved DOM-side
    // (rewrite_images) to a `data:` URL from the store. This is the WASM DOM
    // read path native tests cannot reach (the F6 "verify through the real
    // delivery path" gap). Asset resolution runs after mount, so give it a
    // beat. The src MUST be a self-contained data: URL — never a verbatim
    // off-site ref (that would be the XSS/404 regression this arc closed).
    sleep(Duration::from_millis(400)).await;
    let img = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const imgs = sl ? Array.from(sl.querySelectorAll('img')) : [];
            const data_imgs = imgs.filter(i => (i.getAttribute('src') || '')
                .startsWith('data:image/svg+xml'));
            return {
                img_count: imgs.length,
                data_img_count: data_imgs.length,
                first_alt: imgs.length ? (imgs[0].getAttribute('alt') || '') : '',
                // any <img> still pointing at a non-resolved (off-store) src is a
                // leak — resolution either yields data: or strips the src entirely.
                leaked_src: imgs.some(i => {
                    const s = i.getAttribute('src') || '';
                    return s && !s.startsWith('data:');
                }),
            };
            "#,
            vec![],
        )
        .await?;
    assert!(
        img.get("img_count").and_then(|v| v.as_i64()).unwrap_or(0) >= 1,
        "Phase 19: the demo index ::embed didn't lower to an <img>: {img:?}"
    );
    assert!(
        img.get("data_img_count").and_then(|v| v.as_i64()).unwrap_or(0) >= 1,
        "Phase 19: the demo figure <img> never resolved to a data: URL from the \
         store (embed/asset live render broken): {img:?}"
    );
    assert_eq!(
        img.get("leaked_src").and_then(|v| v.as_bool()),
        Some(false),
        "Phase 19: an <img> kept a non-data: src — site-local asset gate leaked: {img:?}"
    );
    println!(
        "  demo figure rendered as a data: URL ({} img, alt {:?})",
        img.get("data_img_count").and_then(|v| v.as_i64()).unwrap_or(0),
        img.get("first_alt").and_then(|v| v.as_str()).unwrap_or("")
    );

    // A frame panic from the overlay render would freeze the loop (D13).
    let site_log = capture_log(&client).await?;
    let site_panics = count_panics(&site_log);
    assert!(
        site_panics.is_empty(),
        "Phase 19: rendering the Site Mode overlay panicked:\n{}",
        site_panics
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n")
    );

    // 19b.5 — the Share control (the static→live round-trip's live→link reverse
    // half). The overlay nav bar carries a "Share link" button copying the live
    // `?site=` deep link; clicking flips the label to "Copied" (clipboard may be
    // denied headless, but the flip is unconditional feedback — AND a dropped
    // rejected clipboard promise must NOT reload the app, the bug this guards).
    let share_flip = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const btns = sl ? Array.from(sl.querySelectorAll('button')) : [];
            const share = btns.find(b => (b.textContent || '').trim().startsWith('Share link'));
            if (!share) return 'no-share';
            share.click();
            return (share.textContent || '').trim();
            "#,
            vec![],
        )
        .await?;
    assert!(
        share_flip.as_str().map(|s| s.starts_with("Copied")).unwrap_or(false),
        "Phase 19: clicking 'Share link' should flip the label to 'Copied'; got {share_flip:?}"
    );
    println!("  Share control present; Share link copied (label flipped)");
    // Let the overlay settle before the next navigation (Worker-arm cache
    // mirror timing — same reason the other overlay steps sleep).
    sleep(Duration::from_millis(300)).await;

    // 19c — navigate within the site. Click the "About" nav link; the
    // in-app nav (SiteOverlayNavigate) persists the location and the
    // overlay re-renders the About page (no full reload).
    let nav_click = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const links = sl ? sl.querySelectorAll('a') : [];
            for (const a of links) {
                if ((a.textContent || '').trim() === 'About') { a.click(); return 'clicked'; }
            }
            return 'no-about-link';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        nav_click.as_str(),
        Some("clicked"),
        "Phase 19: couldn't find the 'About' nav link in the overlay"
    );
    sleep(Duration::from_millis(500)).await;

    // After navigating, the overlay must STILL be in site mode showing About.
    // Regression guard for the bug where a stray clipboard rejection from the
    // Share control surfaced as an `unhandledrejection` → index.html reloaded
    // the whole app (resetting to chrome with an empty #site-layer). Assert the
    // container MODE too, not just the text, so a silent reload-to-chrome fails.
    let about = client
        .execute(
            r#"
            const c = document.getElementById('app-container');
            const sl = document.getElementById('site-layer');
            return {
                container_class: c ? c.className : null,
                site_text: sl ? (sl.textContent || '').trim() : '',
            };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        about.get("container_class").and_then(|v| v.as_str()),
        Some("mode-site"),
        "Phase 19: navigating in the overlay must NOT drop out of site mode (app reload?): {about:?}"
    );
    let about_text = about.get("site_text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        about_text.contains("About") && about_text.contains("showcase"),
        "Phase 19: overlay didn't navigate to the About page; got: {about_text:?}"
    );
    println!("  overlay navigated to the About page (still in site mode)");

    // 19d — exit via the site's own nav-bar "Enter Peer" control (the
    // status bar is hidden in Site Mode, so the bridge back lives in the
    // site chrome). It survives the About navigation (re-rendered each
    // frame with the Overlay host).
    let exit_click = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const btns = sl ? sl.querySelectorAll('button') : [];
            for (const b of btns) {
                if ((b.textContent || '').trim().startsWith('Enter Peer')) { b.click(); return 'clicked'; }
            }
            return 'no-exit-btn';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        exit_click.as_str(),
        Some("clicked"),
        "Phase 19: couldn't find the 'Enter Peer' control in the overlay nav bar"
    );
    sleep(Duration::from_millis(400)).await;
    let back = client
        .execute(
            r#"
            const c = document.getElementById('app-container');
            const sl = document.getElementById('site-layer');
            const sb = document.getElementById('status-bar');
            return { class: c ? c.className : null,
                     site_visible: sl ? getComputedStyle(sl).display !== 'none' : null,
                     status_bar_visible: sb ? getComputedStyle(sb).display !== 'none' : null };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        back.get("class").and_then(|v| v.as_str()),
        Some("mode-dom"),
        "Phase 19: exiting should return to chrome (mode-dom); got {back:?}"
    );
    assert_eq!(
        back.get("site_visible").and_then(|v| v.as_bool()),
        Some(false),
        "Phase 19: #site-layer should hide again after exiting: {back:?}"
    );
    assert_eq!(
        back.get("status_bar_visible").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 19: the status bar should reappear back in chrome mode: {back:?}"
    );

    let phase19_log = capture_log(&client).await?;
    let phase19_panics = count_panics(&phase19_log);
    assert!(
        phase19_panics.is_empty(),
        "Phase 19: Site Mode interaction panicked:\n{}",
        phase19_panics
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
    println!("  Phase 19 OK — overlay toggle, live render, in-site nav, toggle back");

    // ====================================================================
    // Phase 20: Site Mode in WORKER mode (the browser default) — regression
    // guard for the cache-mirror bug. Phase 19 ran Direct (?worker=0) where
    // reads hit the real store synchronously, so it could NOT catch the bug
    // where the overlay polled an UNSUBSCRIBED path: in Worker mode
    // get_entity reads a cache mirror fed only for subscribed prefixes
    // (peers_worker::cache_get), so without the overlay's own observes the
    // toggle silently did nothing until another window flushed the cache.
    // We re-boot a CLEAN worker session (no default window opens on the
    // Worker arm), toggle immediately, and assert the overlay both flips
    // AND renders live content — which only holds if the overlay subscribes
    // to its own mode + content paths.
    // ====================================================================
    println!("--- Phase 20: Site Mode in Worker mode (cache-mirror regression) ---");
    client
        .goto(&format!(
            "http://localhost:{}/?worker=1&log=trace",
            http_server_port()
        ))
        .await?;
    let phase20_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 20 worker boot: {phase20_boot_ms}ms");

    // Nothing else is open in a fresh Worker boot (the default Entity Tree
    // spawns only on the Direct arm), so no other subscription feeds the
    // cache — the overlay must stand on its own observes.
    sleep(Duration::from_millis(400)).await;
    client
        .execute(
            r#"document.getElementById('site-toggle').click(); return true;"#,
            vec![],
        )
        .await?;
    // Worker mode needs a couple of round-trips (toggle write → worker →
    // Change → cache; demo seed → worker → Change → cache), so give it a
    // beat longer than the Direct path.
    sleep(Duration::from_millis(1500)).await;

    let worker_site = client
        .execute(
            r#"
            const c = document.getElementById('app-container');
            const sl = document.getElementById('site-layer');
            return {
                container_class: c ? c.className : null,
                site_visible: sl ? getComputedStyle(sl).display !== 'none' : null,
                site_text: sl ? (sl.textContent || '').trim() : '',
            };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        worker_site.get("container_class").and_then(|v| v.as_str()),
        Some("mode-site"),
        "Phase 20: toggle didn't switch to Site Mode in WORKER mode — the mode \
         read polls an unsubscribed cache path (the reported bug): {worker_site:?}"
    );
    let wtext = worker_site.get("site_text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        wtext.contains("Entity Demo Site") && wtext.contains("Welcome"),
        "Phase 20: overlay didn't render live content in WORKER mode — the content \
         resolve reads an unsubscribed cache path: {wtext:?}"
    );

    // 20b — deep navigation + active-trail in the LIVE Worker-mode overlay
    // (the deep-site cycle). The demo site is now genuinely
    // nested (a Guide section with 2- and 3-level pages). Drive into it and
    // assert (1) deep pages resolve+render from the tree by path, and (2)
    // the "Guide" section nav item stays highlighted across the whole
    // subtree — the renderer bolds active nav links, so a bold Guide anchor
    // on a child page is active-trail working end to end in the browser.
    //
    // Helper JS: click a NAV-BAR anchor by exact label (the nav bar is the
    // wrapper's first child div, so we avoid matching in-page body links of
    // the same text), and read whether the "Guide" nav anchor is bold.
    let click_nav = |label: &str| {
        let label = label.to_string();
        format!(
            r#"
            const bar = document.querySelector('#site-layer > div > div');
            const links = bar ? bar.querySelectorAll('a') : [];
            for (const a of links) {{
                if ((a.textContent || '').trim() === '{label}') {{ a.click(); return 'clicked'; }}
            }}
            return 'not-found';
            "#
        )
    };
    // Read deep state: the rendered text + whether the Guide nav anchor is bold.
    let read_deep = r#"
        const sl = document.getElementById('site-layer');
        const bar = document.querySelector('#site-layer > div > div');
        let guide_bold = false;
        if (bar) for (const a of bar.querySelectorAll('a')) {
            if ((a.textContent || '').trim() === 'Guide') {
                const w = getComputedStyle(a).fontWeight;
                guide_bold = (w === 'bold' || parseInt(w, 10) >= 600);
            }
        }
        return { text: sl ? (sl.textContent || '').trim() : '', guide_bold };
    "#;

    // Into the Guide section (guide/intro, 2-level).
    let g1 = client.execute(&click_nav("Guide"), vec![]).await?;
    assert_eq!(g1.as_str(), Some("clicked"), "Phase 20b: no 'Guide' nav link in the overlay");
    sleep(Duration::from_millis(900)).await;
    let intro = client.execute(read_deep, vec![]).await?;
    let intro_text = intro.get("text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        intro_text.contains("Guide: Intro"),
        "Phase 20b: Guide intro (guide/intro) didn't render in Worker mode: {intro_text:?}"
    );
    assert_eq!(
        intro.get("guide_bold").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 20b: 'Guide' nav should be active on guide/intro: {intro:?}"
    );

    // Deeper: an in-page link to the 3-level page guide/advanced/internals.
    let deep_click = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            for (const a of (sl ? sl.querySelectorAll('a') : [])) {
                if ((a.textContent || '').trim() === 'Internals') { a.click(); return 'clicked'; }
            }
            return 'not-found';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(deep_click.as_str(), Some("clicked"), "Phase 20b: no 'Internals' in-page link");
    sleep(Duration::from_millis(900)).await;
    let deep = client.execute(read_deep, vec![]).await?;
    let deep_text = deep.get("text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        deep_text.contains("Guide: Internals"),
        "Phase 20b: 3-level page (guide/advanced/internals) didn't resolve in Worker mode: {deep_text:?}"
    );
    assert_eq!(
        deep.get("guide_bold").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 20b: active-trail — 'Guide' must stay active 3 levels deep: {deep:?}"
    );
    println!("  Phase 20b OK — deep nav (3 levels) + active-trail live in Worker mode");

    // 20c — the tree-driven SIDEBAR in the LIVE Worker-mode overlay. This is
    // the cache-mirror proof for `.list`: the sidebar is built from
    // `Peers::tree_listing`, which on the Worker arm is `cache_list` over the
    // mirror — fed only for the subscribed site prefix. If the pages subtree
    // weren't in the mirror, the sidebar would be SILENTLY EMPTY (the classic
    // Direct-passes / Worker-empty trap). We're deep in the Guide section, so
    // the sidebar must list the top-level sections AND the expanded Guide
    // children (proving both the first and second-level cache_list).
    let read_sidebar = r#"
        const side = document.querySelector('#site-layer > div > div:nth-child(2) > nav');
        if (!side) return { present: false, labels: [] };
        const labels = Array.from(side.querySelectorAll('a')).map(a => (a.textContent||'').trim());
        return { present: true, labels };
    "#;
    let side = client.execute(read_sidebar, vec![]).await?;
    assert_eq!(
        side.get("present").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 20c: the section sidebar did not render in WORKER mode — `.list` \
         (cache_list) likely returned empty (pages subtree not in the cache \
         mirror): {side:?}"
    );
    let side_labels: Vec<String> = side
        .get("labels")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    assert!(
        side_labels.iter().any(|l| l == "Guide"),
        "Phase 20c: sidebar should list the Guide section from the tree: {side_labels:?}"
    );
    assert!(
        side_labels.iter().any(|l| l == "Intro") && side_labels.iter().any(|l| l == "Install"),
        "Phase 20c: the active Guide section should expand to its child pages \
         (Intro/Install) — proves second-level cache_list too: {side_labels:?}"
    );
    println!("  Phase 20c OK — tree-driven sidebar (.list/cache_list) live in Worker mode");

    // 20d — section-index: a section path with no page of its own renders a
    // generated listing (not a 404). Click the sidebar "Advanced" section
    // (guide/advanced has no page, only guide/advanced/internals) → its
    // generated index must list the child.
    let adv = client
        .execute(
            r#"
            const side = document.querySelector('#site-layer > div > div:nth-child(2) > nav');
            for (const a of (side ? side.querySelectorAll('a') : [])) {
                if ((a.textContent||'').trim() === 'Advanced') { a.click(); return 'clicked'; }
            }
            return 'not-found';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(adv.as_str(), Some("clicked"), "Phase 20d: no 'Advanced' section in the sidebar");
    sleep(Duration::from_millis(900)).await;
    let adv_text = client
        .execute(
            r#"const sl = document.getElementById('site-layer'); return sl ? (sl.textContent || '').trim() : '';"#,
            vec![],
        )
        .await?;
    assert!(
        adv_text.as_str().unwrap_or("").contains("Internals"),
        "Phase 20d: the 'Advanced' section path should render a generated index \
         listing its child (Internals), not a not-found: {:?}",
        adv_text.as_str()
    );
    println!("  Phase 20d OK — section-index listing live in Worker mode");

    let phase20_log = capture_log(&client).await?;
    let phase20_panics = count_panics(&phase20_log);
    assert!(
        phase20_panics.is_empty(),
        "Phase 20: Worker-mode Site Mode panicked:\n{}",
        phase20_panics
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
    println!("  Phase 20 OK — Worker-mode toggle + live content render");

    // ====================================================================
    // Phase 21: CROSS-PEER HTTP-poll — a REMOTE site fetched over static
    // HTTP and rendered in the overlay (the multi-peer milestone). Boots
    // with `?remote_fixture`, which registers a same-origin fixture origin
    // for `bills-labs-peer` and points the overlay at its `labs` site,
    // served from `dist/remote-fixture/` as static Amendment-5 artifacts
    // (`.bin` system/hash pointers + `content/<hash>` bodies). The app
    // `fetch()`es them, follows the two-hop, hash-verifies, and renders —
    // the LIVE path native tests can't reach (web_sys fetch + the
    // Pending→repaint→Ready cache cycle). Exercises the feature, not just a
    // window spawn (AP10 / the hard-coded window_types lesson).
    // ====================================================================
    println!("--- Phase 21: cross-peer HTTP-poll remote site render ---");

    // Emit the remote fixture into dist/remote-fixture/ (kept in lockstep
    // with the encoder; the running dist server serves it same-origin).
    // Best-effort: if the bin-test build is cold this compiles it once.
    match Command::new(env!("CARGO"))
        .args(["test", "--bin", "entity-browser", "emit_e2e_fixture", "--", "--ignored"])
        .output()
    {
        Ok(o) if o.status.success() => {}
        Ok(o) => panic!(
            "fixture emit failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => panic!("fixture emit failed to spawn: {e}"),
    }
    // The true precondition: the manifest pointer must exist on disk for
    // the dist server to serve it.
    // Must match `app::REMOTE_FIXTURE_PEER` (the bin crate has no lib target to
    // import from). A REAL peer-id so the write-through cache can durably land
    // the foreign site (tree paths validate the peer-segment) — Phase 21b.
    let manifest_bin =
        "dist/remote-fixture/2KFAQwKL6XzdwLkoHkxZ9WE7kvBtS59piFA2AkdBBiQUt5/sites/labs/manifest.bin";
    assert!(
        std::path::Path::new(manifest_bin).exists(),
        "fixture not emitted: {manifest_bin} missing after `cargo test emit_e2e_fixture`"
    );
    println!("  fixture present: {manifest_bin}");

    client
        .goto(&format!(
            "http://localhost:{}/?worker=1&remote_fixture=1&log=trace",
            http_server_port()
        ))
        .await?;
    let phase21_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 21 worker boot: {phase21_boot_ms}ms");

    // Let the origin-registry dispatch land + the overlay's origin-prefix
    // subscription flush the Worker cache mirror, then toggle into Site
    // Mode (the overlay is already pointed at the remote site by the boot
    // hook, so this kicks off the HTTP-poll fetch).
    sleep(Duration::from_millis(700)).await;
    client
        .execute(r#"document.getElementById('site-toggle').click(); return true;"#, vec![])
        .await?;

    let read_site = r#"
        const sl = document.getElementById('site-layer');
        return sl ? (sl.textContent || '').trim() : '';
    "#;
    // Poll up to ~6s for the remote content to fetch + render (fetch + two
    // two-hops + verify + repaint).
    let mut remote_text = String::new();
    for _ in 0..20 {
        sleep(Duration::from_millis(300)).await;
        remote_text =
            client.execute(read_site, vec![]).await?.as_str().unwrap_or("").to_string();
        if remote_text.contains("Bill's Labs") {
            break;
        }
    }
    if !remote_text.contains("Bill's Labs") {
        // Diagnostic: dump the console (set_origin / multi-resolver traces)
        // before the assertion, since the final print_log runs after.
        println!("  Phase 21 DIAG — console log before assert:");
        print_log(&capture_log(&client).await?);
    }
    assert!(
        remote_text.contains("Bill's Labs"),
        "Phase 21: the REMOTE site (fetched over HTTP-poll from dist/remote-fixture/) did not \
         render. Did the fixture emit? got: {remote_text:?}"
    );
    assert!(
        remote_text.contains("fetched over HTTP-poll"),
        "Phase 21: remote page body didn't render: {remote_text:?}"
    );
    println!("  Phase 21: remote site title + body rendered over HTTP-poll");

    // Nested remote page (./guide/intro) — multi-page remote closure, not
    // just the root.
    let g = client
        .execute(
            r#"
            const bar = document.querySelector('#site-layer > div > div');
            for (const a of (bar ? bar.querySelectorAll('a') : [])) {
                if ((a.textContent || '').trim() === 'Guide') { a.click(); return 'clicked'; }
            }
            return 'not-found';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(g.as_str(), Some("clicked"), "Phase 21: no 'Guide' nav in the remote site");
    let mut nested = String::new();
    for _ in 0..15 {
        sleep(Duration::from_millis(300)).await;
        nested = client.execute(read_site, vec![]).await?.as_str().unwrap_or("").to_string();
        if nested.contains("nested remote page") {
            break;
        }
    }
    if !nested.contains("nested remote page") {
        // Diagnostic: dump the console (http_poll / resolver traces) before
        // the assertion, since the final print_log runs after.
        println!("  Phase 21 NESTED DIAG — console log before assert:");
        print_log(&capture_log(&client).await?);
    }
    assert!(
        nested.contains("nested remote page"),
        "Phase 21: nested remote page (guide/intro) didn't resolve over HTTP-poll: {nested:?}"
    );

    let phase21_log = capture_log(&client).await?;
    let phase21_panics = count_panics(&phase21_log);
    assert!(
        phase21_panics.is_empty(),
        "Phase 21: HTTP-poll remote render panicked:\n{}",
        phase21_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );
    println!("  Phase 21 OK — cross-peer HTTP-poll remote site rendered live");

    // -- Phase 21b: manifest-pinned site survives reload (O3) ---------------
    //
    // The peer-general site-cache headline under the O3 manifest-pinned
    // default (DESIGN-PEER-GENERAL-SITE-CACHE §3/§5): browsing the remote site
    // in Phase 21 wrote its **manifest** THROUGH to MY OPFS tree at its natural
    // `/{bills-labs-peer}/sites/labs/manifest` path (always — the manifest is
    // the enumerable anchor), while the **page body stayed ephemeral** (not
    // "kept offline"). Now we **delete the fixture** so the origin 404s,
    // **reload**, and assert:
    //   - the site CHROME still renders ("Bill's Labs" — the title from the
    //     durable manifest, via the manifest-pinned SHELL), which it can only
    //     do from the cache: it proves the manifest write-through landed, the
    //     foreign-prefix subscription fed the Worker cache mirror, and the
    //     read-before-route + shell path hit;
    //   - the page BODY ("fetched over HTTP-poll", asserted live in Phase 21)
    //     is GONE — the page was ephemeral, so offline it can't render (the
    //     shell shows a notice instead). That's the manifest-pinned default
    //     proven, not an accidental full-cache pass.
    // (The KEPT-offline full-page path is covered natively:
    //  resolver::kept_offline_site_writes_page_through_and_serves_fully_on_reload.)
    // This is also the cure for the "exit site ⇒ can't get back" trap: the
    // site stays enumerable + its shell navigable across the reload.
    println!("--- Phase 21b: manifest-pinned site survives reload (O3 shell) ---");
    // Pull the rug: the origin can no longer serve anything.
    let _ = std::fs::remove_dir_all("dist/remote-fixture");
    assert!(
        !std::path::Path::new(manifest_bin).exists(),
        "Phase 21b: fixture should be deleted so only the cache can serve"
    );
    // Reload. `remote_fixture=1` re-registers the durable origin (so the
    // foreign-prefix subscription is set on the Worker arm) but the FILES are
    // gone — the network is dead; only OPFS can answer.
    client
        .goto(&format!(
            "http://localhost:{}/?worker=1&remote_fixture=1&log=trace",
            http_server_port()
        ))
        .await?;
    let phase21b_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 21b reload boot: {phase21b_boot_ms}ms");
    sleep(Duration::from_millis(700)).await;
    // Ensure we're in Site Mode showing the cached site. Poll generously; if
    // the layer is still empty partway through, toggle Site Mode on (the boot
    // posture may differ across the reload).
    let mut cached_text = String::new();
    let mut toggled = false;
    for i in 0..30 {
        sleep(Duration::from_millis(300)).await;
        cached_text =
            client.execute(read_site, vec![]).await?.as_str().unwrap_or("").to_string();
        if cached_text.contains("Bill's Labs") {
            break;
        }
        if i == 5 && !toggled {
            client
                .execute(
                    r#"const t=document.getElementById('site-toggle'); if(t){t.click();} return true;"#,
                    vec![],
                )
                .await?;
            toggled = true;
        }
    }
    if !cached_text.contains("Bill's Labs") {
        println!("  Phase 21b DIAG — console log before assert:");
        print_log(&capture_log(&client).await?);
    }
    assert!(
        cached_text.contains("Bill's Labs"),
        "Phase 21b: the foreign site CHROME did NOT render from the durable manifest cache after \
         the origin went 404 (reload). The manifest write-through / foreign-prefix subscription / \
         cache-read + shell path didn't carry on the Worker arm. got: {cached_text:?}"
    );
    // Manifest-pinned PROOF: the page body (live-only in Phase 21) is GONE —
    // the page was ephemeral, so offline the shell shows a notice, not the
    // body. (If this body text reappeared, the page had been wrongly cached.)
    assert!(
        !cached_text.contains("fetched over HTTP-poll"),
        "Phase 21b: the ephemeral page body rendered offline — manifest-pinned means the page is \
         NOT cached by default; only the manifest + shell should survive. got: {cached_text:?}"
    );
    let phase21b_log = capture_log(&client).await?;
    let phase21b_panics = count_panics(&phase21b_log);
    assert!(
        phase21b_panics.is_empty(),
        "Phase 21b: cache-served reload panicked:\n{}",
        phase21b_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );
    println!(
        "  Phase 21b OK — manifest-pinned shell rendered from the durable OPFS manifest (origin \
         deleted → reload → chrome survives, ephemeral page gone); exit-site trap dissolved"
    );

    // -- Phase 22: Settings "Site & Surface" drives the session config -----
    //
    // Step 4 of the boot/config reframe: the system-settings surface. Proves
    // the FULL user-facing round trip on the WORKER arm — a settings control
    // → Action::WindowEvent → model → session_config::write (seed_write /
    // dispatch_write) → subscription → apply_site_mode reflects it in the DOM.
    // Unit tests cover the config logic; this is the surface
    // (feedback_verify_user_facing_surfaces + e2e_must_exercise_new_features).
    println!("--- Phase 22: Settings → session config (Site & Surface) ---");

    // 22a — exit the overlay (Phase 21 left it active; the chrome palette /
    // Settings window live under the hidden #dom-layer). Tolerant: if we're
    // already in chrome there's no Enter Peer button.
    client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const btns = sl ? sl.querySelectorAll('button') : [];
            for (const b of btns) {
                if ((b.textContent || '').trim().startsWith('Enter Peer')) { b.click(); break; }
            }
            return true;
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(400)).await;

    // 22b — spawn Settings (the Phase 11 reload closed all windows; later
    // phases respawn only some, so don't assume it's open).
    let settings_spawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const btns = root.querySelectorAll('button.spawn-btn');
            for (const b of btns) {
                if (b.textContent.trim() === '+ Settings') { b.click(); return 'clicked'; }
            }
            return 'no-settings-btn';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        settings_spawn.as_str(),
        Some("clicked"),
        "Phase 22: couldn't find the '+ Settings' spawn button"
    );
    sleep(Duration::from_millis(700)).await;

    // 22c — the Site & Surface section rendered with the startup-surface
    // (peer, kind, target) controls: the profile <select>, the boot-kind radios
    // (default "chrome"), the peer <select>, the target <select>, and the
    // show_toggle checkbox. The old single default-site text field is gone —
    // "which site" is now the peer-qualified Target dropdown.
    let section = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sel = root.querySelector('select');
            const profileOpts = sel ? Array.from(sel.options).map(o => o.value) : [];
            const kindChecked = (() => {
                const r = root.querySelector('input[data-kind][checked], input[data-kind]:checked');
                return r ? r.getAttribute('data-kind') : null;
            })();
            return {
                has_select: !!sel,
                profile_opts: profileOpts,
                kinds: Array.from(root.querySelectorAll('input[data-kind]')).map(r => r.getAttribute('data-kind')),
                kind_checked: kindChecked,
                has_peer_select: !!root.querySelector('select[name="boot_peer"]'),
                has_target_select: !!root.querySelector('select[name="boot_target"]'),
                show_toggle_cb: !!root.querySelector('input[type="checkbox"][name="show_toggle"]'),
                fast_paint_cb: !!root.querySelector('input[type="checkbox"][name="fast_paint"]'),
                singleton_cb: !!root.querySelector('input[type="checkbox"][name="singleton_windows"]'),
            };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        section.get("has_select").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 22: profile <select> missing from Site & Surface: {section:?}"
    );
    let kinds = section.get("kinds").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    assert_eq!(kinds.len(), 3, "Phase 22: expected 3 boot-kind radios (chrome/site/window): {section:?}");
    assert_eq!(
        section.get("kind_checked").and_then(|v| v.as_str()),
        Some("chrome"),
        "Phase 22: default boot kind should be 'chrome': {section:?}"
    );
    assert_eq!(
        section.get("has_peer_select").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 22: boot peer <select> missing: {section:?}"
    );
    assert_eq!(
        section.get("has_target_select").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 22: boot target <select> missing: {section:?}"
    );
    assert_eq!(
        section.get("show_toggle_cb").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 22: 'show toggle' checkbox missing (named hook): {section:?}"
    );
    // Fast-paint is a HELD SEAM: the feature is gated off
    // (`boot_fast_paint::DISABLED_FOR_CONSOLIDATION`), so its Settings checkbox
    // is intentionally NOT rendered — a toggle with no observable effect is
    // dishonest UI (D13). The config field / model toggle / boot reader stay
    // wired, so this guards "the lever isn't shown while it does nothing,"
    // not "the feature was deleted."
    assert_eq!(
        section.get("fast_paint_cb").and_then(|v| v.as_bool()),
        Some(false),
        "Phase 22: 'fast paint' checkbox should be hidden (held seam, gated off): {section:?}"
    );
    // The single-instance ("immutable") windows toggle (Windows section).
    assert_eq!(
        section.get("singleton_cb").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 22: 'singleton windows' checkbox missing from Windows section: {section:?}"
    );
    println!("  Phase 22b/c OK — Site & Surface renders the (peer, kind, target) startup controls");

    // 22d — flip "Show the site toggle" OFF via Settings and assert the
    // status-bar #site-toggle hides. This is the load-bearing proof: the
    // write goes through the Worker arm and apply_site_mode (per-frame read
    // of the session config) reflects it. Then restore (so no state leaks).
    let toggle_visible_before = site_toggle_visible(&client).await?;
    assert!(
        toggle_visible_before,
        "Phase 22: #site-toggle should be visible before unchecking show_toggle"
    );
    client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const cb = root.querySelector('input[type="checkbox"][name="show_toggle"]');
            if (cb) cb.click();
            return true;
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(700)).await;
    let toggle_visible_after = site_toggle_visible(&client).await?;
    assert!(
        !toggle_visible_after,
        "Phase 22: unchecking 'show toggle' in Settings did not hide #site-toggle — the \
         session-config write → subscription → apply_site_mode round trip is broken on the \
         Worker arm."
    );

    // Restore show_toggle ON (re-check) so we leave config at its default.
    client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const cb = root.querySelector('input[type="checkbox"][name="show_toggle"]');
            if (cb) cb.click();
            return true;
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(700)).await;
    let toggle_restored = site_toggle_visible(&client).await?;
    assert!(
        toggle_restored,
        "Phase 22: re-checking 'show toggle' did not restore #site-toggle visibility"
    );

    let phase22_log = capture_log(&client).await?;
    let phase22_panics = count_panics(&phase22_log);
    assert!(
        phase22_panics.is_empty(),
        "Phase 22: Settings session-config interaction panicked:\n{}",
        phase22_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );
    println!("  Phase 22 OK — Settings drives the session config live (Worker arm)");

    // -- Phase 22.5: every window root FILLS its panel horizontally ---------
    //
    // Each window renders one root wrapper into `.window-content`; the panel
    // is a flex row, so without a horizontal grow the root collapses to its
    // content width and "sits there" on a wide screen (the reported bug —
    // Settings/Shell/Site-Editor were trimmed while Entity Tree, which sets an
    // explicit width:100%, filled). The `.window-content > *` grow rule fixes
    // it generically. Assert a non-Entity-Tree window root spans (near) the
    // full panel width — Settings is open and chrome here from Phase 22.
    let fill = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            // A windowed (non-maximized) section's content panel + its single
            // root child. Pick the widest panel to avoid a stray narrow one.
            const panels = [...root.querySelectorAll('section.window:not(.maximized) .window-content')];
            let best = null;
            for (const p of panels) {
                const child = p.firstElementChild;
                if (!child) continue;
                const pw = p.getBoundingClientRect().width;
                if (pw < 400) continue; // need a genuinely wide panel to be meaningful
                if (!best || pw > best.pw) {
                    best = { pw, cw: child.getBoundingClientRect().width };
                }
            }
            return best ? { pw: Math.round(best.pw), cw: Math.round(best.cw) } : { pw: 0, cw: 0 };
            "#,
            vec![],
        )
        .await?;
    let panel_w = fill.get("pw").and_then(|v| v.as_i64()).unwrap_or(0);
    let child_w = fill.get("cw").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        panel_w >= 400,
        "Phase 22.5: no wide (>=400px) windowed panel found to measure fill: {fill:?}"
    );
    assert!(
        child_w as f64 >= panel_w as f64 - 2.0,
        "Phase 22.5: window root does not fill its panel horizontally — \
         root width {child_w}px vs panel {panel_w}px (the trimmed-window bug): {fill:?}"
    );
    println!("  Phase 22.5 OK — window root fills its panel width ({child_w}px / {panel_w}px)");

    // -- Phase 23: maximize a window to the full-screen surface ------------
    //
    // Step 5 of the reframe (§4-B Surfaces). A window's maximize control
    // promotes its own section.window to the full-screen surface via the
    // `.maximized` class (one-deep). Restore removes it. We're in chrome mode
    // here (Phase 22 left us there with Settings open), so a maximize control
    // is reachable. Worker arm.
    println!("--- Phase 23: maximize → full-screen surface → restore ---");

    // 23a — a maximize control exists in some window header.
    let maximize_click = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const btn = root.querySelector('button.winctl[title="Maximize window"]');
            if (!btn) return 'no-maximize-btn';
            btn.click();
            return 'clicked';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        maximize_click.as_str(),
        Some("clicked"),
        "Phase 23: no maximize control (button.winctl) found in any window header"
    );
    sleep(Duration::from_millis(500)).await;

    // 23b — exactly one section is promoted to the maximized surface, and it
    // is a TRUE full-viewport overlay: position:fixed, top:0, full height —
    // i.e. it covers the status bar (the bug we're fixing: maximize must use
    // the full-screen surface, not grow inside the bordered DOM panel).
    let maxed = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const maxed = root.querySelectorAll('section.window.maximized');
            const one = maxed.length === 1 ? maxed[0] : null;
            const r = one ? one.getBoundingClientRect() : null;
            // Does the maximized window cover where the status bar sits?
            const sb = document.getElementById('status-bar');
            const sbr = sb ? sb.getBoundingClientRect() : null;
            return {
                count: maxed.length,
                position: one ? getComputedStyle(one).position : null,
                top: r ? Math.round(r.top) : null,
                covers_height: r ? (r.height >= window.innerHeight - 2) : false,
                covers_statusbar: (r && sbr) ? (r.top <= sbr.top && r.bottom >= sbr.bottom) : false,
                has_restore: !!root.querySelector('button.winctl[title="Restore window"]'),
            };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        maxed.get("count").and_then(|v| v.as_i64()),
        Some(1),
        "Phase 23: expected exactly one .maximized section (one-deep): {maxed:?}"
    );
    assert_eq!(
        maxed.get("position").and_then(|v| v.as_str()),
        Some("fixed"),
        "Phase 23: the maximized section must be position:fixed (full-viewport surface): {maxed:?}"
    );
    assert_eq!(
        maxed.get("top").and_then(|v| v.as_i64()),
        Some(0),
        "Phase 23: the maximized surface must start at the viewport top (over the status bar): {maxed:?}"
    );
    assert_eq!(
        maxed.get("covers_statusbar").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 23: the maximized surface must cover the status bar — the whole point of full screen: {maxed:?}"
    );
    assert_eq!(
        maxed.get("has_restore").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 23: the maximize control didn't flip to a Restore control: {maxed:?}"
    );
    println!("  Phase 23b OK — window maximized to a true full-viewport surface (covers status bar)");

    // 23c — restore: click the Restore control; the .maximized surface is gone.
    let restore_click = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const btn = root.querySelector('button.winctl[title="Restore window"]');
            if (!btn) return 'no-restore-btn';
            btn.click();
            return 'clicked';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(restore_click.as_str(), Some("clicked"), "Phase 23: no Restore control");
    sleep(Duration::from_millis(500)).await;
    let after_restore = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            return root.querySelectorAll('section.window.maximized').length;
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        after_restore.as_i64(),
        Some(0),
        "Phase 23: restoring didn't pop the window off the surface (.maximized still present)"
    );

    let phase23_log = capture_log(&client).await?;
    let phase23_panics = count_panics(&phase23_log);
    assert!(
        phase23_panics.is_empty(),
        "Phase 23: maximize/restore panicked:\n{}",
        phase23_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );
    println!("  Phase 23 OK — maximize → surface → restore (one-deep, Worker arm)");

    // -- Phase 24: boot directly into a maximized window surface -----------
    //
    // §4-B Surfaces, C1a: the `BootSurface::Window` seam, activated. A
    // `?boot_window=<type>` reload (e2e/showcase switch; never production)
    // drives boot_load's effective-surface path — spawn the window + promote
    // it to the full-viewport surface via the SAME maximize path step 5
    // proved at runtime (Phase 23). This is the architecture probe: does a
    // window generalize to a *base* surface at boot? Pass = exactly one
    // `.maximized` section, position:fixed, anchored at the viewport top —
    // with zero new render path. The override drives the SPAWN only; it does
    // not persist, so it can't clobber the durable boot_surface.
    println!("--- Phase 24: boot into maximized window surface (BootSurface::Window) ---");
    client
        .goto(&format!(
            "http://localhost:{}/?worker=1&boot_window=Settings&log=trace",
            http_server_port()
        ))
        .await?;
    let phase24_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 24 boot: {phase24_boot_ms}ms");
    // Let the first frames paint the boot-maximized window into the DOM.
    sleep(Duration::from_millis(600)).await;

    let phase24_log = capture_log(&client).await?;
    let phase24_panics = count_panics(&phase24_log);
    assert!(
        phase24_panics.is_empty(),
        "Phase 24: booting into a maximized window triggered panic(s):\n{}",
        phase24_panics
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
    assert!(
        phase24_log
            .iter()
            .any(|l| l.contains("booted into maximized window surface")),
        "Phase 24: boot_load never logged the BootSurface::Window spawn — the \
         ?boot_window override didn't drive the effective-surface path."
    );

    // The maximized window IS the base surface: exactly one `.maximized`
    // section, position:fixed, anchored at the viewport top (covers the
    // status bar). Same shape Phase 23 asserts for a runtime maximize —
    // proving the boot path reuses the render path with no parallel host.
    let phase24_surface = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const maxed = root.querySelectorAll('section.window.maximized');
            if (maxed.length !== 1) return { count: maxed.length };
            const el = maxed[0];
            const cs = getComputedStyle(el);
            const r = el.getBoundingClientRect();
            return { count: 1, position: cs.position, top: r.top };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        phase24_surface.get("count").and_then(|v| v.as_i64()),
        Some(1),
        "Phase 24: expected exactly one .maximized section at boot \
         (the window-as-base-surface): {phase24_surface:?}"
    );
    assert_eq!(
        phase24_surface.get("position").and_then(|v| v.as_str()),
        Some("fixed"),
        "Phase 24: the booted window surface must be position:fixed \
         (full-viewport): {phase24_surface:?}"
    );
    assert!(
        phase24_surface
            .get("top")
            .and_then(|v| v.as_f64())
            .map(|t| t <= 1.0)
            .unwrap_or(false),
        "Phase 24: the booted window surface must start at the viewport top \
         (over the status bar): {phase24_surface:?}"
    );
    println!("  Phase 24 OK — booted directly into a maximized window surface (Worker arm)");

    // -- Phase 25: PERSISTED-config boot into a peer-scoped window surface --
    //
    // Phase 24 proved the `?boot_window=` override (spawn-only, never
    // persisted). This closes the real loop: drive the new startup-surface
    // controls in Settings → persist `boot_surface = Window{peer, type}` →
    // reload with NO override → assert boot lands in that maximized window
    // FROM THE DURABLE CONFIG. This is the (peer, target) pair threaded end to
    // end (handoff §6.5) on the Worker arm.
    //
    // We're sitting in the maximized Settings window from Phase 24. Set the
    // boot kind to Window and the target to Shell using the named hooks.
    println!("--- Phase 25: persisted boot_surface=Window drives boot (no URL override) ---");
    let set_window_kind = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const radio = root.querySelector('input[data-kind="window"]');
            if (!radio) return 'no-window-radio';
            radio.click();
            return 'clicked';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        set_window_kind.as_str(),
        Some("clicked"),
        "Phase 25: couldn't find the Window boot-kind radio in Settings"
    );
    sleep(Duration::from_millis(700)).await;
    // Now the target dropdown lists window types; pick Shell (Peer-scoped,
    // valid on the system peer) and fire change.
    let set_target = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sel = root.querySelector('select[name="boot_target"]');
            if (!sel) return 'no-target-select';
            const has = Array.from(sel.options).some(o => o.value === 'Shell');
            if (!has) return 'no-shell-option';
            sel.value = 'Shell';
            sel.dispatchEvent(new Event('change', { bubbles: true }));
            return 'set';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        set_target.as_str(),
        Some("set"),
        "Phase 25: couldn't select 'Shell' in the boot target dropdown: {set_target:?}"
    );
    sleep(Duration::from_millis(800)).await;

    // Reload with NO ?boot_window — the only way a maximized window can appear
    // now is the PERSISTED boot_surface.
    client
        .goto(&format!("http://localhost:{}/?worker=1&log=trace", http_server_port()))
        .await?;
    let phase25_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 25 boot: {phase25_boot_ms}ms");
    sleep(Duration::from_millis(700)).await;

    let phase25_log = capture_log(&client).await?;
    let phase25_panics = count_panics(&phase25_log);
    assert!(
        phase25_panics.is_empty(),
        "Phase 25: persisted-config Window boot triggered panic(s):\n{}",
        phase25_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );
    // Exactly one maximized surface, and it's the Shell window — proving the
    // durable (peer, type) drove boot, not a URL override.
    let phase25_surface = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const maxed = root.querySelectorAll('section.window.maximized');
            if (maxed.length !== 1) return { count: maxed.length };
            const el = maxed[0];
            const h = el.querySelector('h3');
            return {
                count: 1,
                position: getComputedStyle(el).position,
                title: h ? h.textContent.trim() : null,
                peer: el.getAttribute('data-peer-id'),
            };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        phase25_surface.get("count").and_then(|v| v.as_i64()),
        Some(1),
        "Phase 25: expected exactly one .maximized section from the persisted config: {phase25_surface:?}"
    );
    assert_eq!(
        phase25_surface.get("position").and_then(|v| v.as_str()),
        Some("fixed"),
        "Phase 25: the persisted-config window surface must be position:fixed: {phase25_surface:?}"
    );
    assert_eq!(
        phase25_surface.get("title").and_then(|v| v.as_str()),
        Some("Shell"),
        "Phase 25: the booted surface must be the Shell window we persisted: {phase25_surface:?}"
    );
    assert!(
        phase25_surface.get("peer").and_then(|v| v.as_str()).map(|p| !p.is_empty()).unwrap_or(false),
        "Phase 25: the maximized Shell section must carry its (system) peer id: {phase25_surface:?}"
    );
    assert!(
        phase25_log.iter().any(|l| l.contains("booted into maximized window surface")),
        "Phase 25: boot_load never logged the persisted Window spawn"
    );
    println!("  Phase 25 OK — persisted boot_surface=Window booted into the maximized Shell (Worker arm)");

    // -- Phase 26: static→live deep-link round-trip (?site=) ---------------
    //
    // [F3]: a static page's "open in live peer" banner ([F2]) links to
    // `{origin}/?site=self/{site}/{page}`. Booting that URL must drop straight
    // into the site overlay AT THAT PAGE — the close of the static↔live loop.
    // `self` resolves to the system peer (where the demo site is seeded), so a
    // same-origin deep link resolves locally regardless of the publish peer id.
    // Pass = mode-site + #site-layer shows the DEEP-LINKED page (guide/intro),
    // not the index — proving the param drove navigation, not just "show a site".
    println!("--- Phase 26: ?site= deep-link round-trip (static→live) ---");
    client
        .goto(&format!(
            "http://localhost:{}/?worker=1&site=self/demo/guide/intro&log=trace",
            http_server_port()
        ))
        .await?;
    let phase26_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 26 boot: {phase26_boot_ms}ms");
    // Let boot_load's navigate + the first overlay frames resolve the page
    // (Worker arm: the resolve hits the cache mirror, filled on subscription).
    sleep(Duration::from_millis(800)).await;

    let phase26_log = capture_log(&client).await?;
    let phase26_panics = count_panics(&phase26_log);
    assert!(
        phase26_panics.is_empty(),
        "Phase 26: ?site deep-link boot triggered panic(s):\n{}",
        phase26_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );
    assert!(
        phase26_log.iter().any(|l| l.contains("?site deep-link")),
        "Phase 26: boot_load never logged the ?site deep-link override — the param \
         didn't drive the site-overlay path."
    );

    let phase26 = client
        .execute(
            r#"
            const c = document.getElementById('app-container');
            const sl = document.getElementById('site-layer');
            return {
                container_class: c ? c.className : null,
                site_visible: sl ? getComputedStyle(sl).display !== 'none' : null,
                site_text: sl ? (sl.textContent || '').trim() : '',
            };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        phase26.get("container_class").and_then(|v| v.as_str()),
        Some("mode-site"),
        "Phase 26: ?site= should boot straight into the site overlay (mode-site): {phase26:?}"
    );
    assert_eq!(
        phase26.get("site_visible").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 26: #site-layer should be visible after a ?site= boot: {phase26:?}"
    );
    let p26_text = phase26.get("site_text").and_then(|v| v.as_str()).unwrap_or("");
    // The DEEP-LINKED page, not the index — `guide/intro` renders "Guide: Intro".
    assert!(
        p26_text.contains("Guide: Intro"),
        "Phase 26: overlay didn't land on the deep-linked guide/intro page; got: {p26_text:?}"
    );
    println!("  Phase 26 OK — ?site=self/demo/guide/intro booted straight into the deep-linked page (Worker arm)");

    // 26b — Enter Peer after a `?site=` boot MUST exit (regression guard).
    // The deep-link forces the overlay on for the session; if that override is
    // never released, "Enter Peer" can't exit (it was ORed back on every frame).
    // Click the overlay's Enter Peer control and assert we return to chrome.
    let exit_after_deeplink = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const btns = sl ? Array.from(sl.querySelectorAll('button')) : [];
            const exit = btns.find(b => (b.textContent || '').trim().startsWith('Enter Peer'));
            if (!exit) return 'no-exit';
            exit.click();
            return 'clicked';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        exit_after_deeplink.as_str(),
        Some("clicked"),
        "Phase 26: no 'Enter Peer' control in the deep-linked overlay"
    );
    sleep(Duration::from_millis(400)).await;
    let post_exit = client
        .execute(
            r#"
            const c = document.getElementById('app-container');
            const sl = document.getElementById('site-layer');
            return {
                container_class: c ? c.className : null,
                site_visible: sl ? getComputedStyle(sl).display !== 'none' : null,
            };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        post_exit.get("container_class").and_then(|v| v.as_str()),
        Some("mode-dom"),
        "Phase 26: Enter Peer after a ?site= boot must return to chrome (mode-dom): {post_exit:?}"
    );
    assert_eq!(
        post_exit.get("site_visible").and_then(|v| v.as_bool()),
        Some(false),
        "Phase 26: #site-layer must hide after Enter Peer post-deep-link: {post_exit:?}"
    );
    println!("  Phase 26b OK — Enter Peer released the ?site= override and returned to chrome");

    // ====================================================================
    // Phase 27: per-domain deployment config served at /entity-deployment.json
    // (boot-closure cut 2b). The harness builds a GENERIC `Full` bundle; this
    // phase publishes a strict-site, same-origin config + the demo content into
    // dist/ and boots the SAME bundle against it — proving the generic-WASM-
    // per-domain model: one build adopts a domain's posture + home from a
    // fetched file, NO rebuild. Boots Direct (`?worker=0`, ephemeral tree) so
    // there's never a durable session config — the deployment fetch runs this
    // boot (a persisted config would otherwise win the precedence). Asserts:
    // config fetched+applied, fast-paint painted the home for the Full bundle
    // (THE crux — the build is chrome-first; the served config flips it to
    // boots-into-site), mode-site, and the published home rendered same-origin.
    // ====================================================================
    println!("--- Phase 27: per-domain /entity-deployment.json (cut 2b) ---");

    // Publish the demo + a strict-site same-origin config into dist/ (the
    // served origin). Same nested-cargo fixture pattern as Phase 21.
    match Command::new(env!("CARGO"))
        .args([
            "test",
            "--bin",
            "entity-browser",
            "emit_deployment_config_fixture",
            "--",
            "--ignored",
        ])
        .output()
    {
        Ok(o) if o.status.success() => {}
        Ok(o) => panic!(
            "emit_deployment_config_fixture failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => panic!("could not run emit_deployment_config_fixture: {e}"),
    }
    let cfg_file = "dist/entity-deployment.json";
    assert!(
        std::path::Path::new(cfg_file).exists(),
        "fixture not emitted: {cfg_file} missing"
    );
    println!("  config + content published into dist/");

    // Boot the GENERIC bundle in Direct mode. After the IDB-default flip,
    // `?worker=0` is DURABLE — so a prior phase's persisted config would win the
    // precedence (persisted > fetched > build) and boot_load would SKIP the
    // deployment fetch. Land + WIPE first for a genuinely fresh deployment, then
    // there is no durable config and the served one is fetched + applied on this
    // boot — the path that matters now that fast-paint (which used to fetch
    // unconditionally and so masked this) is disabled for the site-surface
    // consolidation.
    let url27 = format!("http://localhost:{}/?worker=0&log=trace", http_server_port());
    client.goto(&url27).await?;
    wipe_all_storage(&client).await?;
    client.goto(&url27).await?;
    let phase27_boot_ms = wait_for_boot(&client, 8000).await?;
    println!("  phase 27 boot: {phase27_boot_ms}ms");

    // Poll for the published home to render. Fast-paint's pre-peer paint is
    // DISABLED for the consolidation, so the LIVE overlay is the sole
    // `#site-layer` owner: it paints the config's home post-peer over
    // same-origin HTTP once boot_load applies the served strict-site config.
    let read_site = r#"
        const sl = document.getElementById('site-layer');
        return sl ? (sl.textContent || '').trim() : '';
    "#;
    let mut home_text = String::new();
    for _ in 0..20 {
        sleep(Duration::from_millis(300)).await;
        home_text = client.execute(read_site, vec![]).await?.as_str().unwrap_or("").to_string();
        if home_text.contains("Welcome to the Entity Demo Site") {
            break;
        }
    }

    let phase27_log = capture_log(&client).await?;
    if !home_text.contains("Welcome to the Entity Demo Site") {
        println!("  Phase 27 DIAG — console before assert:");
        print_log(&phase27_log);
    }

    // (a) The served config was fetched + applied (NOT the 404 fallback the
    // default phases hit).
    assert!(
        phase27_log.iter().any(|l| l.contains("deployment-config: applied")),
        "Phase 27: the served /entity-deployment.json was not fetched+applied"
    );
    // (b) Fast-paint is DISABLED for the site-surface consolidation
    // (HANDOFF-SITE-SURFACE-AUDIT §5), so its pre-peer "fast-paint: painted
    // remote home" must NOT fire. The generic-bundle crux of cut 2b (a Full
    // build adopting a SERVED strict-site config + rendering its home) is still
    // verified end-to-end below — by (c) mode-site and (d) the home rendering
    // via the LIVE overlay — just post-peer instead of pre-peer.
    assert!(
        !phase27_log.iter().any(|l| l.contains("fast-paint: painted remote home")),
        "Phase 27: fast-paint is disabled for the consolidation — it must not paint pre-peer"
    );
    // (c) Posture applied: strict-site config boots the generic bundle into the
    // site overlay even though the BUILD is Full (chrome-first).
    let container_class = client
        .execute(
            r#"const c = document.getElementById('app-container'); return c ? c.className : '';"#,
            vec![],
        )
        .await?;
    assert_eq!(
        container_class.as_str(),
        Some("mode-site"),
        "Phase 27: strict-site config should boot the generic bundle into the site overlay"
    );
    // (d) End-to-end: the published home rendered, fetched same-origin.
    assert!(
        home_text.contains("Welcome to the Entity Demo Site"),
        "Phase 27: the published home didn't render same-origin; got: {home_text:?}"
    );
    let phase27_panics = count_panics(&phase27_log);
    assert!(
        phase27_panics.is_empty(),
        "Phase 27: served-config boot panicked:\n{}",
        phase27_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );

    // Remove the served config so it can't change any later phase / re-run's
    // default boot path (setup() also clears it defensively).
    let _ = std::fs::remove_file(cfg_file);
    println!("  Phase 27 OK — generic bundle adopted the served strict-site config + home (cut 2b)");

    // Final print regardless of pass/fail — captured console is the
    // primary diagnostic signal under --nocapture.
    let final_log = capture_log(&client).await?;
    print_log(&final_log);

    client.close().await.ok();

    // Settings assertions.
    assert!(
        post_panics.is_empty(),
        "Settings interaction triggered panic(s):\n{}",
        post_panics
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n")
    );
    assert!(
        new_writes >= 1,
        "Settings clicks didn't produce any dispatch_write — controls may not be wired correctly. \
        Got {new_writes} new writes (prior {prior_writes}, post {post_writes})."
    );
    assert!(
        tree_items >= 1,
        "Entity Tree rendered zero `.tree-row` rows. The cache mirror is empty — likely a \
        regression in worker-side snapshot construction, subscription delivery, or the \
        host's L1 callback (empty-snapshot bug)."
    );
    assert!(
        inspector_populated,
        "Entity Tree inspector did not populate after clicking a tree row. \
        Either the window's dirty flag is not flipping after Navigate (action \
        handler should call self.watch.mark_dirty()), or cache_get is failing \
        for the clicked path."
    );

    if let Some((name, panics)) = panic_at {
        panic!(
            "opening window '{name}' triggered panic(s):\n{}",
            panics.join("\n---\n")
        );
    }
    if !spawn_failures.is_empty() {
        panic!(
            "could not find spawn button(s) — palette UI may have changed:\n{}",
            spawn_failures.join("\n")
        );
    }

    Ok(())
}

/// Whether the light-DOM status-bar `#site-toggle` is currently visible
/// (driven by `apply_site_mode` from the session config's `show_toggle`).
async fn site_toggle_visible(client: &Client) -> Result<bool, Box<dyn std::error::Error>> {
    let v = client
        .execute(
            r#"const t = document.getElementById('site-toggle');
               return t ? getComputedStyle(t).display !== 'none' : false;"#,
            vec![],
        )
        .await?;
    Ok(v.as_bool().unwrap_or(false))
}

/// Read the boot storage-banner text (empty string when no banner present).
async fn banner_text(client: &Client) -> Result<String, Box<dyn std::error::Error>> {
    let v = client
        .execute(
            r#"const b = document.getElementById('storage-banner');
               return b ? (b.textContent || "") : "";"#,
            vec![],
        )
        .await?;
    Ok(v.as_str().unwrap_or("").to_string())
}

/// Multi-tab single-writer guard (stabilization sprint #1).
///
/// The handoff claimed multi-tab "cannot be e2e'd (single-session)". It can:
/// two WebDriver windows in ONE session share the browser profile — same
/// `localStorage`, same OPFS, same Web Lock namespace — which is exactly the
/// real multi-tab scenario. We drive it directly.
///
/// `src/multitab.rs` is a Web Locks leader election keyed on the primary
/// peer_id, and it only has a peer_id to key on once one is *persisted* —
/// i.e. for a returning user (the case that matters for "did it save my
/// work"). So we reproduce that: boot tab 1, reload it (now it's a returning
/// tab → it acquires + holds the lock), then open a second tab in the same
/// profile. Tab 2 reads the same primary, finds the lock held, and renders
/// the specific "already open in another tab" banner instead of silently
/// failing to save (TRIAGE §4.1; charter D16 / D9).
///
/// Separate test for an isolated fresh profile; `make e2e-worker` runs
/// `--test-threads=1` so it doesn't collide with the main test on the http
/// port or the single Selenium session.
#[tokio::test(flavor = "current_thread")]
async fn second_tab_detects_secondary_and_warns() -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());

    // Tab 1, first boot on a fresh profile: generates + persists the primary
    // Frontend peer. (No lock yet — there was no persisted peer_id to key on
    // at the mode decision; this is the fresh-profile edge documented in
    // src/multitab.rs.)
    client.goto(&url).await?;
    sleep(Duration::from_millis(3000)).await;

    // Reload: now the primary IS persisted, so this "returning" boot acquires
    // and holds the Web Lock for the session — tab 1 is the durable owner.
    client.goto(&url).await?;
    sleep(Duration::from_millis(3000)).await;
    let tab1 = client.window().await?;
    let tab1_banner = banner_text(&client).await?;
    assert!(
        !tab1_banner.contains("already open in another tab"),
        "Tab 1 is the owner and must NOT show the secondary banner. Got: {tab1_banner:?}"
    );

    // Tab 2: a second window in the SAME browser profile (shared localStorage
    // + OPFS + Web Locks) — the real multi-tab scenario.
    let new_win = client.new_window(true).await?;
    client.switch_to_window(new_win.handle).await?;
    client.goto(&url).await?;
    sleep(Duration::from_millis(3000)).await;

    let tab2_banner = banner_text(&client).await?;
    assert!(
        tab2_banner.contains("already open in another tab"),
        "Tab 2 shares the profile with the owner tab, so the multi-tab guard \
         (Web Locks leader election) should render the specific 'already open \
         in another tab' banner — not a silent ephemeral downgrade. Got: \
         {tab2_banner:?}"
    );

    // Cleanup: close tab 2, return to tab 1, end the session.
    client.close_window().await.ok();
    client.switch_to_window(tab1).await.ok();
    client.close().await.ok();
    Ok(())
}

/// Multi-tab single-writer guard on the **Direct / IndexedDB arm**.
///
/// Companion to `second_tab_detects_secondary_and_warns` (the Worker/OPFS arm).
/// This is the arm the hardening pass added election to: IndexedDB — UNLIKE
/// OPFS sync access handles (exclusive per file) — has NO cross-connection
/// exclusivity, so without this election two `?worker=0` tabs would both open
/// the same `entity-peer-{system_id}` db and race (last-writer-wins, silent
/// corruption). The fix runs the SAME Web-Lock leader election on the Direct
/// path, keyed on the system-seed id (the contended IDB db identity).
///
/// The lock key (`persistence::system_seed_id`) GENERATES the system seed on a
/// fresh profile, so tab 1 acquires + holds the lock on its FIRST boot (closing
/// the fresh-first-session race the Worker arm leaves to its OPFS backstop —
/// which the IDB arm doesn't have). We still reload tab 1 here to also prove
/// leadership survives a reload (lock release on the old page + reacquire on the
/// new), then open tab 2 in the same profile: it finds the lock held and renders
/// the specific "already open in another tab" banner — NOT a silent shared-db
/// write race (charter D13 / D16).
///
/// Separate test (fresh profile, `--test-threads=1`) so it doesn't collide on
/// the http port / single Selenium session.
#[tokio::test(flavor = "current_thread")]
async fn second_tab_detects_secondary_on_direct_idb() -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=0&log=trace", http_server_port());

    // Land a document, then wipe storage for a deterministic fresh profile
    // (IDB persists across sessions otherwise).
    client.goto(&url).await?;
    wipe_all_storage(&client).await?;

    // Tab 1, first boot on the now-fresh profile: `system_seed_id()` generates
    // + persists the system seed, the election acquires the Web Lock, and the
    // primary comes up durable (DurableDirectIdb). Tab 1 holds the lock from
    // this first boot.
    client.goto(&url).await?;
    sleep(Duration::from_millis(3000)).await;

    // Reload: the old page released the lock on unload; this "returning" boot
    // reacquires + holds it for the session — proving leadership survives a
    // reload. Tab 1 remains the durable IDB owner.
    client.goto(&url).await?;
    sleep(Duration::from_millis(3000)).await;
    let tab1 = client.window().await?;
    let tab1_banner = banner_text(&client).await?;
    assert!(
        !tab1_banner.contains("already open in another tab"),
        "Tab 1 is the durable IDB owner and must NOT show the secondary banner. \
         Got: {tab1_banner:?}"
    );
    assert!(
        !tab1_banner.contains("memory only"),
        "Tab 1 is the durable IDB leader (DurableDirectIdb), so it must NOT show \
         the ephemeral 'memory only' banner. Got: {tab1_banner:?}"
    );

    // Tab 2: a second window in the SAME browser profile (shared localStorage +
    // IndexedDB + Web Locks) — the real multi-tab scenario. It must detect the
    // held lock and stay ephemeral with the specific banner, NOT open the
    // shared db and race tab 1.
    let new_win = client.new_window(true).await?;
    client.switch_to_window(new_win.handle).await?;
    client.goto(&url).await?;
    sleep(Duration::from_millis(3000)).await;

    let tab2_banner = banner_text(&client).await?;
    assert!(
        tab2_banner.contains("already open in another tab"),
        "Tab 2 shares the profile with the durable IDB owner, so the Direct-arm \
         multi-tab guard (Web Locks leader election keyed on the system-seed id) \
         must render the 'already open in another tab' banner — not silently open \
         the shared IndexedDB db and race. Got: {tab2_banner:?}"
    );

    // --- 1a durability gate (MAP §10): peer creation is REFUSED in this
    // ephemeral secondary tab — closes S-1 (shared-vault multi-writer race) and
    // L-2 (silent loss). Tab 2 is the active window. Open the Peers window,
    // snapshot the shared `entity_peers` vault, click `+ Frontend`, and assert
    // (a) the vault is UNCHANGED (no peer written) and (b) the honest
    // create-refused banner appeared (D13 — the refusal is never silent).
    let peers_spawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            if (!layer) return 'no-dom-layer';
            const root = layer.shadowRoot || layer;
            for (const b of root.querySelectorAll('button.spawn-btn')) {
                if (b.textContent.trim() === '+ Peers') { b.click(); return 'clicked'; }
            }
            return 'no-peers-spawn-btn';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        peers_spawn.as_str().unwrap_or(""),
        "clicked",
        "couldn't open the Peers window in tab 2: {peers_spawn:?}"
    );
    sleep(Duration::from_millis(600)).await;

    let vault_before = client
        .execute(r#"return window.localStorage.getItem('entity_peers') || '';"#, vec![])
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();

    // The create panel still RENDERS here: this is the full profile, so the 1b
    // capability is intact — only this tab's durability (1a) fails. The action
    // guard, not a hidden button, is what blocks it.
    let click_status = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.trim() === '+ Frontend') { b.click(); return 'clicked'; }
                }
                return 'no-btn';
            }
            return 'no-peers-section';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        click_status.as_str().unwrap_or(""),
        "clicked",
        "the '+ Frontend' button should still render in a secondary tab \
         (full profile keeps the capability) — got {click_status:?}"
    );

    // Generous budget for any (wrongful) create round-trip to land.
    sleep(Duration::from_millis(1500)).await;

    let vault_after = client
        .execute(r#"return window.localStorage.getItem('entity_peers') || '';"#, vec![])
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    assert_eq!(
        vault_before, vault_after,
        "creating a peer in an ephemeral secondary tab MUST NOT touch the shared \
         localStorage vault (S-1 / L-2 gate). before={vault_before:?} after={vault_after:?}"
    );

    // The banner is boot-level light DOM (outside the shadow root).
    let refused = client
        .execute(
            r#"return document.getElementById('create-refused-banner') ? true : false;"#,
            vec![],
        )
        .await?;
    assert_eq!(
        refused.as_bool(),
        Some(true),
        "a refused create must surface the honest 'create-refused-banner' (D13) \
         rather than failing silently"
    );

    // Cleanup: close tab 2, return to tab 1, end the session.
    client.close_window().await.ok();
    client.switch_to_window(tab1).await.ok();
    client.close().await.ok();
    Ok(())
}

/// Frozen-frame watchdog (stabilization sprint #3).
///
/// The watchdog (`src/watchdog.rs`) runs a tiny off-main-thread worker that
/// watches the rAF heartbeat; if the UI stops rendering past a threshold it
/// reports the freeze into the in-app diagnostics sink (#4) + offers a clean
/// reload. We prove it does something real: boot with a low threshold
/// (`?watchdog=1500`), then **block the main thread synchronously** for
/// several seconds — a genuine frame stall — and confirm the watchdog
/// detected it (the diagnostic landed and the reload banner appeared) once
/// the main thread recovered.
///
/// Separate test (its own `?watchdog=1500` boot — a low threshold would
/// false-trigger during the heavy main test's slow phases); `--test-threads=1`
/// keeps it from colliding on the http port / Selenium session.
#[tokio::test(flavor = "current_thread")]
async fn watchdog_detects_a_stalled_frame() -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    client
        .goto(&format!(
            // `watchdog-banner=1` opts into the user-facing reload banner — it's
            // off by default now (a recoverable stall self-recovers; the freeze is
            // always logged regardless), so the banner assertion below needs it.
            "http://localhost:{}/?worker=1&watchdog=1500&watchdog-banner=1&log=trace",
            http_server_port()
        ))
        .await?;
    sleep(Duration::from_millis(3000)).await;

    // No freeze yet → no watchdog banner.
    let pre = client
        .execute(
            r#"return document.getElementById('watchdog-banner') ? true : false;"#,
            vec![],
        )
        .await?;
    assert_eq!(
        pre.as_bool(),
        Some(false),
        "watchdog banner should be absent before any stall"
    );

    // Block the main thread for ~4s — a real frame stall. The off-thread
    // watcher (1500ms threshold) sees the heartbeat go silent and reports;
    // the report is processed once this busy-loop returns.
    client
        .execute(
            r#"const end = Date.now() + 4000; while (Date.now() < end) {} return true;"#,
            vec![],
        )
        .await?;
    // Let the main thread process the queued freeze report + paint a frame.
    sleep(Duration::from_millis(1500)).await;

    let banner = client
        .execute(
            r#"return document.getElementById('watchdog-banner') ? true : false;"#,
            vec![],
        )
        .await?;
    assert_eq!(
        banner.as_bool(),
        Some(true),
        "after a ~4s main-thread stall the watchdog should surface the reload \
         banner (src/watchdog.rs). It did not appear."
    );

    let log = capture_log(&client).await?;
    assert!(
        log.iter()
            .any(|l| l.contains("frozen-frame watchdog") && l.contains("stopped rendering")),
        "the stall should be recorded in the in-app diagnostics sink \
         (`note` → 'frozen-frame watchdog … stopped rendering'). Not found."
    );

    client.close().await.ok();
    Ok(())
}

/// L1 System Recovery console (`?systemrecovery=1`). The read-only "BIOS"
/// inventory must render — and the WASM app must NOT boot (recovery owns the
/// page; `main.rs::start()` bails on the param). This is the gate against the
/// console silently regressing back to "inert query param" (where it lived
/// earlier — referenced only in docs, never wired).
#[tokio::test(flavor = "current_thread")]
async fn system_recovery_renders_readonly_inventory_without_booting(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!(
        "http://localhost:{}/?systemrecovery=1&log=trace",
        http_server_port()
    );
    client.goto(&url).await?;
    sleep(Duration::from_millis(2500)).await;

    // The BIOS screen rendered.
    let present = client
        .execute(
            r#"return document.getElementById('entity-recovery') ? true : false;"#,
            vec![],
        )
        .await?;
    assert_eq!(
        present.as_bool(),
        Some(true),
        "?systemrecovery=1 must render the #entity-recovery console"
    );

    // It shows the read-only inventory sections.
    let text_v = client
        .execute(
            r#"const r = document.getElementById('entity-recovery'); return r ? r.textContent : '';"#,
            vec![],
        )
        .await?;
    let text = text_v.as_str().unwrap_or("");
    for needle in ["Storage Inventory", "IndexedDB", "localStorage", "no writes and no deletes"] {
        assert!(
            text.contains(needle),
            "recovery console missing the {needle:?} section. Got: {text:?}"
        );
    }

    // The app must NOT have booted — recovery yields the page (no windows).
    let windows = client
        .execute(
            r#"const layer = document.getElementById('dom-layer');
               if (!layer) return 0;
               const root = layer.shadowRoot || layer;
               return root.querySelectorAll('section.window').length;"#,
            vec![],
        )
        .await?;
    assert_eq!(
        windows.as_i64(),
        Some(0),
        "System Recovery must NOT boot the app (no windows should spawn)"
    );

    client.close().await.ok();
    Ok(())
}

/// Read the raw `entity_peers` localStorage blob and parse it into
/// `(peer_id, mode)` pairs. Mirrors the persistence format
/// (`peer_id|seed_hex|label|mode`, mode is the LAST field).
async fn read_persisted_peers(
    client: &Client,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let v = client
        .execute(
            r#"return window.localStorage.getItem('entity_peers') || '';"#,
            vec![],
        )
        .await?;
    let raw = v.as_str().unwrap_or("");
    let mut out = Vec::new();
    for line in raw.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() < 2 {
            continue;
        }
        let pid = parts[0].to_string();
        // Pre-Stage-2C entries have 3 fields (no mode) → treat as frontend.
        let mode = if parts.len() >= 4 {
            parts[parts.len() - 1].to_string()
        } else {
            "frontend".to_string()
        };
        out.push((pid, mode));
    }
    Ok(out)
}

/// Click a top-level palette spawn button by its exact label.
async fn click_spawn_btn(
    client: &Client,
    label: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let script = format!(
        r#"
        const layer = document.getElementById('dom-layer');
        if (!layer) return 'no-dom-layer';
        const root = layer.shadowRoot || layer;
        const btns = root.querySelectorAll('button.spawn-btn');
        for (const b of btns) {{
            if (b.textContent.trim() === '{label}') {{ b.click(); return 'clicked'; }}
        }}
        return 'no-btn';
        "#
    );
    let v = client.execute(&script, vec![]).await?;
    Ok(v.as_str().unwrap_or("non-string").to_string())
}

/// Click a `+ <Mode>` button inside the Peers window.
async fn click_peers_mode_btn(
    client: &Client,
    btn_text: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let script = format!(
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        const sections = root.querySelectorAll('section.window');
        for (const sec of sections) {{
            const h2 = sec.querySelector('h2');
            if (!h2 || h2.textContent.trim() !== 'Peers') continue;
            const btns = sec.querySelectorAll('button');
            for (const b of btns) {{
                if (b.textContent.trim() === '{btn_text}') {{ b.click(); return 'clicked'; }}
            }}
            return 'no-btn';
        }}
        return 'no-peers-section';
        "#
    );
    let v = client.execute(&script, vec![]).await?;
    Ok(v.as_str().unwrap_or("non-string").to_string())
}

/// Click every "Delete" button in the Peers window in a single pass —
/// a rapid batch delete (mirrors the user deleting many rows). Returns
/// the number of Delete buttons clicked.
async fn delete_all_deletable_peers(
    client: &Client,
) -> Result<i64, Box<dyn std::error::Error>> {
    let v = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                const btns = sec.querySelectorAll('tbody button');
                let n = 0;
                for (const b of btns) {
                    if (b.textContent.trim() === 'Delete') { b.click(); n++; }
                }
                return n;
            }
            return -1;
            "#,
            vec![],
        )
        .await?;
    Ok(v.as_i64().unwrap_or(-1))
}

/// BUG-A regression gate: **a deleted backend peer must stay
/// deleted across a page reload.**
///
/// The user's #1 pain: in a Worker-mode profile with several backend
/// (Memory/OPFS) peers, deleting them makes the rows vanish at runtime —
/// then a refresh brings every peer back. `decode_notification`'s
/// delete-reflection bug (BUG-B) was one half; this gate guards the OTHER
/// half: the *durable* removal across reload. A delete isn't done until it
/// survives a reload with N peers present (review §2 "candidate discipline":
/// a lifecycle gate must assert the terminal state across a reload in a
/// multi-entity configuration — "the row vanished" ≠ "it's deleted").
///
/// Three peers is enough to exercise the process (one frontend primary +
/// one Memory backend + one OPFS backend, each in its own dedicated
/// Worker). Separate fresh-profile test; `--test-threads=1` keeps it off
/// the main test's http port / Selenium session.
#[tokio::test(flavor = "current_thread")]
async fn deleted_backend_peers_stay_deleted_across_reload(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());

    // -- Boot a fresh profile: generates + persists the primary Frontend.
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(800)).await;

    let boot_peers = read_persisted_peers(&client).await?;
    println!("  post-boot persisted peers: {boot_peers:?}");
    assert_eq!(
        boot_peers.len(),
        1,
        "fresh profile should persist exactly the primary Frontend; got {boot_peers:?}"
    );

    // -- Create two backend peers (Memory + OPFS) via the Peers window.
    assert_eq!(click_spawn_btn(&client, "+ Peers").await?, "clicked",
        "couldn't open the Peers window");
    sleep(Duration::from_millis(600)).await;

    assert_eq!(click_peers_mode_btn(&client, "+ Backend (Memory)").await?, "clicked",
        "couldn't click '+ Backend (Memory)'");
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(click_peers_mode_btn(&client, "+ Backend (OPFS)").await?, "clicked",
        "couldn't click '+ Backend (OPFS)'");
    sleep(Duration::from_millis(2500)).await;

    let after_create = read_persisted_peers(&client).await?;
    println!("  after-create persisted peers: {after_create:?}");
    let bm: Vec<_> = after_create.iter().filter(|(_, m)| m == "backend-memory").collect();
    let bo: Vec<_> = after_create.iter().filter(|(_, m)| m == "backend-opfs").collect();
    assert_eq!(bm.len(), 1, "expected one backend-memory peer persisted; got {after_create:?}");
    assert_eq!(bo.len(), 1, "expected one backend-opfs peer persisted; got {after_create:?}");
    let bm_pid = bm[0].0.clone();
    let bo_pid = bo[0].0.clone();
    println!("  backend-memory pid: {bm_pid}");
    println!("  backend-opfs   pid: {bo_pid}");

    // -- Reload: backend peers must SURVIVE (create durability — the
    //    baseline the delete gate is measured against).
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1500)).await;
    let after_reload1 = read_persisted_peers(&client).await?;
    println!("  after reload #1 (pre-delete): {after_reload1:?}");
    assert!(
        after_reload1.iter().any(|(p, _)| p == &bm_pid)
            && after_reload1.iter().any(|(p, _)| p == &bo_pid),
        "backend peers should SURVIVE a reload before deletion (sanity); got {after_reload1:?}"
    );

    // -- Delete every deletable peer (rapid batch — both backends).
    assert_eq!(click_spawn_btn(&client, "+ Peers").await?, "clicked",
        "couldn't re-open the Peers window after reload #1");
    sleep(Duration::from_millis(700)).await;
    let deleted_n = delete_all_deletable_peers(&client).await?;
    println!("  Delete buttons clicked: {deleted_n}");
    assert!(deleted_n >= 2, "expected to click Delete on at least the 2 backend peers; got {deleted_n}");
    // Let the synchronous localStorage cleanup + async worker teardown run.
    sleep(Duration::from_millis(2000)).await;

    let after_delete = read_persisted_peers(&client).await?;
    println!("  after-delete persisted peers (runtime): {after_delete:?}");
    assert!(
        !after_delete.iter().any(|(p, _)| p == &bm_pid),
        "backend-memory peer still in localStorage immediately after Delete \
         (synchronous cleanup at app.rs delete path failed): {after_delete:?}"
    );
    assert!(
        !after_delete.iter().any(|(p, _)| p == &bo_pid),
        "backend-opfs peer still in localStorage immediately after Delete: {after_delete:?}"
    );

    // -- Reload #2: THE GATE. Deleted backend peers must STAY gone.
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1500)).await;
    let after_reload2 = read_persisted_peers(&client).await?;
    println!("  after reload #2 (THE GATE): {after_reload2:?}");

    let panics = capture_log(&client).await?;
    let panic_lines = count_panics(&panics);
    assert!(panic_lines.is_empty(), "panics during the BUG-A delete/reload cycle:\n{panic_lines:#?}");

    assert!(
        !after_reload2.iter().any(|(p, _)| p == &bm_pid),
        "BUG-A: deleted backend-memory peer RESURRECTED after reload — \
         {bm_pid} is back in localStorage: {after_reload2:?}"
    );
    assert!(
        !after_reload2.iter().any(|(p, _)| p == &bo_pid),
        "BUG-A: deleted backend-opfs peer RESURRECTED after reload — \
         {bo_pid} is back in localStorage: {after_reload2:?}"
    );

    // Belt-and-suspenders: the Peers window must not render the deleted
    // backends as rows either (re-open it post-reload).
    assert_eq!(click_spawn_btn(&client, "+ Peers").await?, "clicked",
        "couldn't re-open the Peers window after reload #2");
    sleep(Duration::from_millis(700)).await;
    let rows_text = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                return sec.querySelector('tbody') ? sec.querySelector('tbody').textContent : '';
            }
            return '';
            "#,
            vec![],
        )
        .await?;
    let rows = rows_text.as_str().unwrap_or("");
    let bm_short = &bm_pid[..12.min(bm_pid.len())];
    let bo_short = &bo_pid[..12.min(bo_pid.len())];
    assert!(
        !rows.contains(bm_short) && !rows.contains(bo_short),
        "BUG-A: deleted backend peer rows reappeared in the Peers window after reload. \
         Looking for {bm_short}/{bo_short} in: {rows:?}"
    );

    println!("  deleted_backend_peers_stay_deleted_across_reload OK");
    client.close().await.ok();
    Ok(())
}

/// Assert the current page's boot logged a CLEAN roster reconcile and no
/// DRIFT. `window.__entity_browser_log` is per-page, so after a reload this
/// reflects exactly that boot's reconcile (app.rs `boot_load` step 0b).
async fn assert_roster_reconcile_clean(
    client: &Client,
    phase: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let log = capture_log(client).await?;
    let drift: Vec<&String> = log
        .iter()
        .filter(|l| l.contains("roster reconcile: DRIFT"))
        .collect();
    assert!(drift.is_empty(), "[{phase}] roster DRIFT (roster ≠ set A):\n{drift:#?}");
    let clean = log.iter().any(|l| l.contains("roster reconcile: CLEAN"));
    assert!(
        clean,
        "[{phase}] no 'roster reconcile: CLEAN' boot log — reconcile didn't run \
         or the roster drifted from set A"
    );
    Ok(())
}

/// Brick 3 roster gate: the authoritative roster
/// (`system/roster/`) must shadow-match the durable spawn-list (set A,
/// `entity_peers`) across the peer lifecycle on the **Worker arm**. The
/// boot-time reconcile (`boot_load` step 0b) reads the roster over the L1
/// `List` path (the sync mirror is Worker-blind for the unwatched prefix) and
/// logs CLEAN / DRIFT; this drives create + delete + reload and asserts the
/// boot stays CLEAN — proving the create/delete dual-write and the one-shot
/// backfill keep the two authorities in agreement BEFORE Brick 4 makes the
/// roster load-bearing for spawn decisions. Fresh-profile, `--test-threads=1`.
#[tokio::test(flavor = "current_thread")]
async fn roster_shadow_matches_spawn_list_across_lifecycle(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());

    // Fresh profile: the backfill shadows the primary into the roster. The
    // reconcile does NOT run on this boot (it's the backfill boot — the puts
    // are in flight), so no assertion here.
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1000)).await;

    // Create two backend peers (each dual-writes its roster entry).
    assert_eq!(click_spawn_btn(&client, "+ Peers").await?, "clicked", "open Peers");
    sleep(Duration::from_millis(600)).await;
    assert_eq!(click_peers_mode_btn(&client, "+ Backend (Memory)").await?, "clicked");
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(click_peers_mode_btn(&client, "+ Backend (OPFS)").await?, "clicked");
    sleep(Duration::from_millis(2500)).await;

    // Reload #1: the roster is replayed durably (primary + 2 backends) and the
    // reconcile runs — it MUST agree with set A.
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1500)).await;
    let after_create = read_persisted_peers(&client).await?;
    assert_eq!(after_create.len(), 3, "primary + 2 backends in set A; got {after_create:?}");
    assert_roster_reconcile_clean(&client, "after create + reload").await?;

    // Delete the backends (each dual-removes its roster entry), reload, still CLEAN.
    assert_eq!(click_spawn_btn(&client, "+ Peers").await?, "clicked", "re-open Peers");
    sleep(Duration::from_millis(700)).await;
    let n = delete_all_deletable_peers(&client).await?;
    assert!(n >= 2, "expected >=2 Delete clicks; got {n}");
    sleep(Duration::from_millis(2000)).await;
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1500)).await;
    let after_delete = read_persisted_peers(&client).await?;
    assert_eq!(after_delete.len(), 1, "only the primary remains in set A; got {after_delete:?}");
    assert_roster_reconcile_clean(&client, "after delete + reload").await?;

    let panics = capture_log(&client).await?;
    let panic_lines = count_panics(&panics);
    assert!(panic_lines.is_empty(), "panics during roster lifecycle:\n{panic_lines:#?}");

    println!("  roster_shadow_matches_spawn_list_across_lifecycle OK");
    client.close().await.ok();
    Ok(())
}

/// Brick 4: the **default boot** (no `?worker`) is now the
/// main-thread IndexedDB **system peer**, and boot spawns data peers from its
/// ROSTER. This gate drives the bare default URL — proving (1) the default arm
/// is IDB (not Worker), (2) a created data peer SURVIVES a reload sourced from
/// the roster on the always-available main-thread system peer, and (3) the boot
/// reconcile stays CLEAN on the IDB arm. This is the "boot spawns from the
/// system peer's roster" flip, verified through the real delivery path.
#[tokio::test(flavor = "current_thread")]
async fn default_boot_is_idb_and_roster_drives_spawn(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    // DEFAULT url — NO ?worker → must boot the main-thread IDB system peer.
    let url = format!("http://localhost:{}/?log=trace", http_server_port());

    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1000)).await;

    // (1) Confirm the DEFAULT arm is IDB, not Worker.
    let boot_log = capture_log(&client).await?;
    let is_idb = boot_log
        .iter()
        .any(|l| l.contains("DurableDirectIdb"));
    let is_worker = boot_log.iter().any(|l| l.contains("DurableWorker"));
    assert!(
        is_idb && !is_worker,
        "default boot must select the main-thread IDB system peer (DurableDirectIdb), \
         not Worker. saw is_idb={is_idb} is_worker={is_worker}"
    );

    // (2) Create a backend peer (spawns its own OPFS worker even on the IDB
    //     default arm — heavy data stays on OPFS by design). It dual-writes a
    //     roster entry on the IDB system peer.
    assert_eq!(click_spawn_btn(&client, "+ Peers").await?, "clicked", "open Peers");
    sleep(Duration::from_millis(600)).await;
    assert_eq!(click_peers_mode_btn(&client, "+ Backend (Memory)").await?, "clicked");
    sleep(Duration::from_millis(2000)).await;
    let after_create = read_persisted_peers(&client).await?;
    let backend: Vec<_> = after_create.iter().filter(|(_, m)| m == "backend-memory").collect();
    assert_eq!(backend.len(), 1, "expected one backend-memory peer; got {after_create:?}");
    let backend_pid = backend[0].0.clone();

    // (3) Reload the DEFAULT url → the IDB system peer replays its roster and
    //     boot spawns the backend FROM THE ROSTER (joined to its vault key).
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1500)).await;

    let after_reload = read_persisted_peers(&client).await?;
    assert!(
        after_reload.iter().any(|(p, _)| p == &backend_pid),
        "roster-driven spawn: the backend peer must SURVIVE a default-arm reload; \
         got {after_reload:?}"
    );
    // The backend must actually be hosted (rendered as a row), i.e. it was
    // spawned from the roster, not merely present in localStorage.
    assert_eq!(click_spawn_btn(&client, "+ Peers").await?, "clicked", "re-open Peers");
    sleep(Duration::from_millis(800)).await;
    let rows = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h2 = sec.querySelector('h2');
                if (h2 && h2.textContent.trim() === 'Peers') {
                    return sec.querySelector('tbody') ? sec.querySelector('tbody').textContent : '';
                }
            }
            return '';
            "#,
            vec![],
        )
        .await?;
    let rows_s = rows.as_str().unwrap_or("");
    // The Peers window abbreviates ids as "<8>...<tail>", so match the 8-char prefix.
    let short = &backend_pid[..8.min(backend_pid.len())];
    assert!(
        rows_s.contains(short),
        "roster-driven spawn: backend {short} must render as a hosted row after reload; \
         rows={rows_s:?}"
    );

    // (4) Reconcile CLEAN on the IDB arm.
    assert_roster_reconcile_clean(&client, "idb default after create + reload").await?;

    let panics = capture_log(&client).await?;
    let panic_lines = count_panics(&panics);
    assert!(panic_lines.is_empty(), "panics on the IDB-default arm:\n{panic_lines:#?}");

    println!("  default_boot_is_idb_and_roster_drives_spawn OK");
    client.close().await.ok();
    Ok(())
}

/// Cross-arm regression (review MEDIUM-1): the roster lives on the
/// SYSTEM peer, whose id DIFFERS by arm today (Direct/IDB = the
/// `entity_system_seed` id; Worker = the set-A primary id), so each arm has its
/// own roster on its own prefix. The roster-migrated gate was once a single
/// GLOBAL localStorage flag — so a default (IDB) boot that set it would suppress
/// the Worker arm's never-run backfill, leaving the Worker roster
/// empty-but-"migrated" → a PERMANENT false `reconcile DRIFT` (every set-A peer
/// reported missing, never converging). Fixed by keying the flag per
/// system-peer-id. This drives default(IDB) → worker → worker and asserts the
/// Worker arm runs its OWN backfill and reconciles CLEAN, not perpetual DRIFT.
/// Fresh-profile, `--test-threads=1`.
#[tokio::test(flavor = "current_thread")]
async fn cross_arm_roster_migration_flag_is_per_system_peer(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let idb_url = format!("http://localhost:{}/?log=trace", http_server_port());
    let worker_url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());

    // (1) Default boot = IDB system peer. Its backfill marks the roster-migrated
    //     flag for the IDB system id (under the OLD global-key bug this would
    //     poison every other arm). Create a backend so set A is non-empty.
    client.goto(&idb_url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1000)).await;
    assert_eq!(click_spawn_btn(&client, "+ Peers").await?, "clicked", "open Peers");
    sleep(Duration::from_millis(600)).await;
    assert_eq!(click_peers_mode_btn(&client, "+ Backend (Memory)").await?, "clicked");
    sleep(Duration::from_millis(2000)).await;

    // (2) Switch to the Worker arm. Its system peer (set-A primary) has a
    //     DIFFERENT roster prefix → it must run its OWN backfill, not be
    //     suppressed by the IDB arm's flag. (First worker boot = backfill;
    //     reconcile is skipped on the backfill boot by design.)
    client.goto(&worker_url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1500)).await;
    let worker_boot1 = capture_log(&client).await?;
    assert!(
        worker_boot1
            .iter()
            .any(|l| l.contains("roster backfill: shadowed set A")),
        "Worker arm must run its OWN roster backfill (per-arm migrated flag), not \
         inherit the IDB arm's flag — else its roster stays empty-but-migrated and \
         reconciles a permanent false DRIFT"
    );

    // (3) Second Worker boot: the worker roster is now durably replayed and the
    //     reconcile runs — it MUST be CLEAN, never the perpetual cross-arm DRIFT.
    client.goto(&worker_url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1500)).await;
    assert_roster_reconcile_clean(&client, "worker arm after a prior IDB-default boot").await?;

    let panics = capture_log(&client).await?;
    let panic_lines = count_panics(&panics);
    assert!(panic_lines.is_empty(), "panics in cross-arm flag test:\n{panic_lines:#?}");

    println!("  cross_arm_roster_migration_flag_is_per_system_peer OK");
    client.close().await.ok();
    Ok(())
}

/// Regression (live trace, the "published papers stuck on connecting"
/// bug): a per-domain deployment config whose `home_site` lives on ANOTHER
/// (foreign) peer must boot the **DEFAULT IDB arm** straight into that remote
/// home — not strand the overlay on the bundled `demo` default. The `boot_load`
/// overlay re-point was gated on `boot_class.tree_is_durable()`, but on the IDB
/// arm the multi-tab election persists the system seed BEFORE `new_wasm`
/// measures `was_persisted`, so a FRESH boot misclassifies as WarmDurable → the
/// re-point was SKIPPED and the deployment showed "No site manifest at 'demo'".
/// Fixed by gating on config-presence (`durable.is_none()`). Phase 27 only
/// covered the ephemeral `?worker=0` arm (where boot_class is correctly Cold),
/// so it missed this — the IDB-default arm's foreign-home path was untested.
/// Reuses Phase 27's `emit_deployment_config_fixture`. Fresh-profile,
/// `--test-threads=1`.
#[tokio::test(flavor = "current_thread")]
async fn default_idb_boots_into_remote_deployment_home(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;

    // Publish the demo under a foreign publish-peer + a strict-site same-origin
    // entity-deployment.json into dist/ (identical to Phase 27's fixture).
    let out = Command::new(env!("CARGO"))
        .args([
            "test",
            "--bin",
            "entity-browser",
            "emit_deployment_config_fixture",
            "--",
            "--ignored",
        ])
        .output()
        .expect("run emit_deployment_config_fixture");
    assert!(
        out.status.success(),
        "fixture emit failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let cfg_file = "dist/entity-deployment.json";
    assert!(std::path::Path::new(cfg_file).exists(), "fixture not emitted: {cfg_file}");

    // Boot the DEFAULT url (NO ?worker → main-thread IDB system peer).
    let r = async {
        client
            .goto(&format!("http://localhost:{}/?log=trace", http_server_port()))
            .await?;
        wait_for_boot(&client, 30_000).await?;

        let read_site =
            r#"const sl=document.getElementById('site-layer');return sl?(sl.textContent||'').trim():'';"#;
        let mut home_text = String::new();
        for _ in 0..25 {
            sleep(Duration::from_millis(300)).await;
            home_text = client.execute(read_site, vec![]).await?.as_str().unwrap_or("").to_string();
            if home_text.contains("Welcome to the Entity Demo Site") {
                break;
            }
        }
        let log = capture_log(&client).await?;
        if !home_text.contains("Welcome to the Entity Demo Site") {
            print_log(&log);
        }

        // (1) The DEFAULT arm is the IDB system peer (not Worker) — the arm the
        //     bug needs (Worker boot_class is unaffected).
        assert!(
            log.iter().any(|l| l.contains("DurableDirectIdb")),
            "default boot must select the main-thread IDB system peer"
        );
        // (2) The served deployment config was fetched + applied.
        assert!(
            log.iter().any(|l| l.contains("deployment-config: applied")),
            "served /entity-deployment.json was not applied"
        );
        // (3) THE FIX: the overlay was re-pointed at the remote home ON THE IDB
        //     arm (gated on config-presence, not the corrupted boot_class).
        assert!(
            log.iter().any(|l| l.contains("pointed overlay at remote home")),
            "overlay was NOT re-pointed to the deployment home on the IDB arm — \
             boot_class misclassified the fresh boot as warm-durable (the bug)"
        );
        // (4) End-to-end: the FOREIGN home actually rendered, and the overlay was
        //     NOT stranded on the bundled 'demo' default.
        assert!(
            home_text.contains("Welcome to the Entity Demo Site"),
            "remote deployment home did not render on the IDB arm; got: {home_text:?}"
        );
        assert!(
            !home_text.contains("No site manifest"),
            "overlay stranded on the bundled demo default (the bug); got: {home_text:?}"
        );
        let panics = count_panics(&log);
        assert!(panics.is_empty(), "panics on the IDB remote-home boot:\n{panics:#?}");
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    // Always remove the served config so it can't leak into other tests / re-runs.
    let _ = std::fs::remove_file(cfg_file);
    r?;

    println!("  default_idb_boots_into_remote_deployment_home OK");
    client.close().await.ok();
    Ok(())
}

/// BUG-A multi-tab reproduction: single-tab delete is durable
/// (proven by `deleted_backend_peers_stay_deleted_across_reload`), so the
/// user's "delete → refresh → every peer is back" must be a **multi-tab**
/// interaction — they run many tabs sharing one `entity_peers` key. This
/// test holds a second tab alive (with the peers loaded) while the first
/// tab deletes them, then exercises the second tab and reloads the first,
/// reading `localStorage` at every step so the trace pins the exact
/// resurrection vector. (Same shared-profile two-window technique as
/// `second_tab_detects_secondary_and_warns`.)
#[tokio::test(flavor = "current_thread")]
async fn deleted_backend_peers_stay_deleted_with_second_tab_open(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());

    // -- Tab 1: fresh boot + create two backend peers.
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(800)).await;
    let tab1 = client.window().await?;

    assert_eq!(click_spawn_btn(&client, "+ Peers").await?, "clicked");
    sleep(Duration::from_millis(600)).await;
    assert_eq!(click_peers_mode_btn(&client, "+ Backend (Memory)").await?, "clicked");
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(click_peers_mode_btn(&client, "+ Backend (OPFS)").await?, "clicked");
    sleep(Duration::from_millis(2500)).await;

    let created = read_persisted_peers(&client).await?;
    println!("  [tab1] after-create: {created:?}");
    let bm_pid = created.iter().find(|(_, m)| m == "backend-memory").map(|(p, _)| p.clone())
        .expect("backend-memory peer should be persisted");
    let bo_pid = created.iter().find(|(_, m)| m == "backend-opfs").map(|(p, _)| p.clone())
        .expect("backend-opfs peer should be persisted");

    // -- Tab 2: a second window in the SAME profile (shared localStorage).
    //    It boots and reads the same 3 peers, keeping their workers alive
    //    in this tab while tab 1 deletes them.
    let tab2 = client.new_window(true).await?;
    client.switch_to_window(tab2.handle.clone()).await?;
    client.goto(&url).await?;
    sleep(Duration::from_millis(4000)).await;
    let tab2_view = read_persisted_peers(&client).await?;
    println!("  [tab2] after-boot localStorage: {tab2_view:?}");

    // -- Tab 1: delete both backend peers while tab 2 holds them.
    client.switch_to_window(tab1.clone()).await?;
    sleep(Duration::from_millis(300)).await;
    let deleted_n = delete_all_deletable_peers(&client).await?;
    println!("  [tab1] Delete buttons clicked: {deleted_n}");
    assert!(deleted_n >= 2, "expected to delete the 2 backends; got {deleted_n}");
    sleep(Duration::from_millis(2000)).await;
    let after_delete = read_persisted_peers(&client).await?;
    println!("  [tab1] after-delete localStorage: {after_delete:?}");

    // -- Tab 2: still alive with the peers in memory. Force it to write
    //    entity_peers by creating a NEW peer — a read-modify-write of the
    //    shared key. If tab 2's view of the roster is stale (or our save
    //    path rewrites from memory), the just-deleted backends ride back in.
    client.switch_to_window(tab2.handle.clone()).await?;
    sleep(Duration::from_millis(300)).await;
    let t2_open = click_spawn_btn(&client, "+ Peers").await?;
    println!("  [tab2] open Peers: {t2_open}");
    sleep(Duration::from_millis(600)).await;
    let t2_create = click_peers_mode_btn(&client, "+ Backend (Memory)").await?;
    println!("  [tab2] create backend-memory: {t2_create}");
    sleep(Duration::from_millis(2500)).await;
    let after_tab2_write = read_persisted_peers(&client).await?;
    println!("  [tab2] localStorage after tab2 create: {after_tab2_write:?}");

    // -- Tab 1: reload. THE GATE — deleted backends must not have ridden
    //    back in via tab 2's write.
    client.switch_to_window(tab1.clone()).await?;
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(1500)).await;
    let final_state = read_persisted_peers(&client).await?;
    println!("  [tab1] FINAL localStorage after reload: {final_state:?}");

    // Cleanup tab 2 before asserting (so the session is clean even on panic).
    client.switch_to_window(tab2.handle.clone()).await.ok();
    client.close_window().await.ok();
    client.switch_to_window(tab1.clone()).await.ok();

    assert!(
        !final_state.iter().any(|(p, _)| p == &bm_pid),
        "BUG-A (multi-tab): deleted backend-memory peer {bm_pid} RESURRECTED — \
         final localStorage: {final_state:?}"
    );
    assert!(
        !final_state.iter().any(|(p, _)| p == &bo_pid),
        "BUG-A (multi-tab): deleted backend-opfs peer {bo_pid} RESURRECTED — \
         final localStorage: {final_state:?}"
    );

    println!("  deleted_backend_peers_stay_deleted_with_second_tab_open OK");
    client.close().await.ok();
    Ok(())
}

/// A 23-peer `entity_peers` roster (6 frontend + 17 backend Memory/OPFS)
/// derived from the user's real capture. The capture came back
/// with its columns mis-aligned (the pid FIELD no longer matches the SEED
/// in the next column), which — happily — makes this a faithful fixture
/// for the IDENTITY-DRIFT case: every entry's stored `peer_id` field
/// disagrees with `Keypair::from_seed(seed).peer_id()`.
///
/// That drift is the exact condition that breaks delete: the app hosts /
/// renders / hands Delete the seed-derived id, but the OLD `delete_peer`
/// matched the stored field — so Delete logged success, the row vanished
/// via the registry self-heal, and the peer RESURRECTED on reload (it was
/// never removed from localStorage). The fix matches the derived id; this
/// gate proves a drifted peer is now durably deletable.
///
/// Stored as one blob with `mode<newline>pid` separators already inserted.
const DRIFTED_IDENTITY_ROSTER: &str = "\
2KY8jxnVPTGejff8E8gYcSYqePscwwJ8zihFDaRSHHTUD2|3db0e969f92f79c5875fcf8193ccf1f2df89431b978e57cdc48e61f7ffa95c2b||frontend
2KTCiM1AqgVNAZZti6YgP62gLzt8j3H4pEwQ3druokc8eC|9bf0ea84c8d159e24dc28b8ffcb9da393c59bc39cfdaf12f5a0c65ee35572608||backend-opfs
2KRSH8z6HbhjHQFB3CiaNhPkvZGfWomr1SUQamq6Sviijn|6a54bda17f7e8ff15ee8089a8eee0f86e90d25ddc8fc8701cdea1f706bbc7383||backend-memory
2KXuX1G1eyGmo5iMUi6Jxx3os3gBR5YxtE7hYbYe6Pfnf7|3da5b748ceacfc1b3c699eb430c1c3ce5a8eb6c7362a0df908f8997c7190399f||frontend
2KaFPorSjjSQwvfcYnNB43cwvHc3Y5RX8MBHqdAjxqpW6V|b56002ab26324a02efb68d748844c565a1ac279559153d3a9c991fc27fa35877||backend-opfs
2KVnFGH6A2Dv98SibFdBE4VkhesNQAymKiFLMUL2pJwUUA|2a99a64e1fa1bfda8ec6ba2d8e4c760d796735020bff689fed0a798931453dc9||backend-memory
2KYDwPnTWeuHPKUKBzki7bJUHix7FctjNgvWrYPRHd3dbg|08a5871c70a5e1954b4f24448cb62b8b24b68a890f84e6d60b86b09db75f06d6||backend-opfs
2KWJP5bzPur9eNGvJ9ksJpDanu7ptZJcbT3GCDvWf3PDcg|3b4da4117980d7491712e0495119edb3236401ab6a42056fa56fbb532ed15868||backend-memory
2KPSAJnxzVfDe5MdPtcijEyWrTTqCkisKvu4oV8efMuuoh|c2a880ed8e5b916e88177a8ab3de903ce41337759e0256ab2b2f3634fdbe0657||frontend
2KMt9diceByppqGp39UYiwJQsAR9LTfTPNVJVCA9Vt1MWR|2e76d4b32b10e76a5ebd017637760fd7c2702f5cced9e0ed41391a8f57626611||backend-opfs
2KbmpfdhKvEqqbKGiqyN4tb3WyFehf6mC52yL4NtvWxV9M|57e0ed3e3e81fae4dd6721f7e79e91f834a292a8b79009a3c989e60b664710df||backend-opfs
2KaqrMt22gD7gyKnanjiv36rujaxg5weE9tFXrT3cdUxvR|81de33c98771de1f88fb9e9f96da26d4a7983f72e3ca1df7feb02686db7dd22b||frontend
2KPD23AudkxG6p5bfNH19P3aF6M77oSjG4iJEDeznvSqkk|085090b3d12e4834d4bd22e84a40c4e2a867231e51234ec25e53226ecfd48444||backend-memory
2KZAhd98yALHJnLbFF2c35yDUzmuYHY9xPH5VUnLeAXUkg|bcbfd1b30dd8a0801ea5e869af84cbb5f8c1a32072d15a07fa983057fd4f5f2a||backend-opfs
2KNwssuYbVJkyF43PrYsCxjSzX4XrgMeycq4G9DLi9gX7z|d293ebbbc68330fc0ff9ca6229219a1aafd7d137361f038672a43f26deca4276||backend-opfs
2KZvwZ2jXEE1AQSs4mbZWaLKSYCxXPrVwwZt4Fu2xqkoX7|874ae19e26faaca79cc9133c041784814def69bb6a39affe27a0e15787d6718e||backend-memory
2KYXR9GLkWnA1qQ5qLDCBSRpMZwZLsibMT1SeB1ottXENz|40b1937a37247bd9abef21d40a44ea2ed2bf9f55e0f3c075a96d2d2b066a5331||frontend
2KcJyWmWCmc9bBaeNedcFZPYm7Bd6vKebxxtvT8ikP4EpV|3538aa352504d66e14d689f463cecaf08b0bcf298ceb7e275235ad33cf886d7c||backend-opfs
2KRgBcB7TRpdBwhuf6ftyCwMKGuuYLTBFFykq5B3NrchoS|842c3f0414293953106a5b9692ec657a63aa9c4a2a956d46247a247a2601cdaa|opfs|backend-opfs
2KZ5bfZBmxMo8utDBAcscopShRHt1NYXhRjbnT6iMpw5Yh|201949bf121b58c787b41876e67632c5730406397b3c281fd4fc393682101d9d||backend-opfs
2KRHpgT2LFV3bGSQzaMv5LbrK2vpJKfgzd13zvaFAH9CHe|705a1ae568f3a069b82893de8610942013a6ab06d546f05a6573b782576701a2||backend-memory
2KZw813FTbFCtZaYDCfFoUcG8jrMFTnxKxFUuGPZKTquSc|f74311fa64fb66de507b2c65f34d8023d40781687a4803f6f775c91536e36924||frontend
2Kc9hXhp41GyUed8iRunw5AU7udXRCX4aqYNpr85CG495q|cdfed9b0f97361f5e89bd961b9f27097e66bf37bf7f1c73af7f2e2341c3f9fbe||backend-opfs";

/// Best-effort wipe of ALL browser storage for this origin: localStorage,
/// every IndexedDB database, and the entire OPFS tree. Used to start a
/// repro from a TRULY clean profile — the Selenium container persists its
/// profile across tests in a session, so prior runs' peers/OPFS journals
/// leak in and confound peer-count assertions. (Dirs whose sync handles a
/// live worker still holds may resist removal; that's fine — the next boot
/// re-derives a fresh primary and orphans them.)
async fn wipe_all_storage(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
    let script = r#"
        const cb = arguments[arguments.length - 1];
        (async () => {
            try { window.localStorage.clear(); } catch (e) {}
            try { window.sessionStorage.clear(); } catch (e) {}
            try {
                if (indexedDB.databases) {
                    const dbs = await indexedDB.databases();
                    for (const d of dbs) { if (d.name) indexedDB.deleteDatabase(d.name); }
                }
            } catch (e) {}
            try {
                const root = await navigator.storage.getDirectory();
                for await (const [name, handle] of root.entries()) {
                    try { await root.removeEntry(name, { recursive: true }); } catch (e) {}
                }
            } catch (e) {}
            cb('wiped');
        })();
    "#;
    let _ = client.execute_async(script, vec![]).await?;
    Ok(())
}

/// Whether the rAF render loop is still alive (heartbeat advancing).
/// Reads `Frame N` log markers twice with a gap and checks the count grew.
async fn frame_loop_alive(client: &Client) -> Result<bool, Box<dyn std::error::Error>> {
    let count = |lines: &[String]| -> usize {
        lines.iter().filter(|l| l.contains("Frame ") || l.contains("frame")).count()
    };
    let a = count(&capture_log(client).await?);
    sleep(Duration::from_millis(1200)).await;
    let b = count(&capture_log(client).await?);
    // If the log doesn't carry per-frame markers, fall back to a liveness
    // probe: can the page still execute a trivial script promptly?
    if a == b {
        let probe = client
            .execute(r#"return 1+1;"#, vec![])
            .await
            .ok()
            .and_then(|v| v.as_i64());
        return Ok(probe == Some(2));
    }
    Ok(b > a)
}

/// BUG-A identity-drift gate: peers whose persisted `peer_id`
/// FIELD disagrees with their seed-derived id must still be durably
/// deletable. Seed a 23-peer roster with that drift (see
/// `DRIFTED_IDENTITY_ROSTER`), boot it, mass-delete every backend, reload,
/// and assert zero backends survive. Before the `delete_peer` fix this
/// FAILS — `localStorage AFTER mass-delete` stays at 23 (field-only match
/// removes nothing) and all 17 backends resurrect. Separate fresh-profile
/// test; `--test-threads=1`.
#[tokio::test(flavor = "current_thread")]
async fn drifted_identity_peers_are_durably_deletable(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());

    // Boot once so a page exists, WIPE all prior-run storage (the Selenium
    // profile persists across tests and leaks peers/OPFS journals), then
    // seed the user's real roster and reload into it.
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(500)).await;
    wipe_all_storage(&client).await?;
    sleep(Duration::from_millis(500)).await;
    client
        .execute(
            "window.localStorage.setItem('entity_peers', arguments[0]); return 'seeded';",
            vec![serde_json::Value::String(DRIFTED_IDENTITY_ROSTER.to_string())],
        )
        .await?;

    let seeded = read_persisted_peers(&client).await?;
    println!("  seeded roster size: {} ({} frontend, {} backend)",
        seeded.len(),
        seeded.iter().filter(|(_, m)| m == "frontend").count(),
        seeded.iter().filter(|(_, m)| m.starts_with("backend")).count());
    assert_eq!(seeded.len(), 23, "expected 23 seeded peers; got {}", seeded.len());

    // Reload into the seeded roster — the user's cold boot.
    client.goto(&url).await?;
    let boot_res = wait_for_boot(&client, 60_000).await;
    println!("  boot with 23 peers: {boot_res:?}");
    // Give the backend-worker spawn storm time to settle.
    sleep(Duration::from_millis(8000)).await;

    let boot_log = capture_log(&client).await?;
    let boot_panics = count_panics(&boot_log);
    println!("  panics during 23-peer boot: {}", boot_panics.len());
    for p in &boot_panics { println!("    PANIC: {p}"); }

    let alive_after_boot = frame_loop_alive(&client).await.unwrap_or(false);
    println!("  frame loop alive after 23-peer boot: {alive_after_boot}");

    // Open Peers + read how many rows actually render vs how many seeded.
    let opened = click_spawn_btn(&client, "+ Peers").await.unwrap_or_else(|_| "err".into());
    println!("  open Peers after boot: {opened}");
    sleep(Duration::from_millis(1500)).await;
    let row_count = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            if (!layer) return -2;
            const root = layer.shadowRoot || layer;
            const sections = root.querySelectorAll('section.window');
            for (const sec of sections) {
                const h2 = sec.querySelector('h2');
                if (!h2 || h2.textContent.trim() !== 'Peers') continue;
                return sec.querySelectorAll('tbody tr').length;
            }
            return -1;
            "#,
            vec![],
        )
        .await?
        .as_i64()
        .unwrap_or(-3);
    println!("  Peers window rows rendered: {row_count} (of 23 seeded)");

    // Mass-delete every deletable row.
    let deleted_n = delete_all_deletable_peers(&client).await.unwrap_or(-1);
    println!("  Delete buttons clicked (mass delete): {deleted_n}");
    // Let synchronous LS cleanup + async teardown churn run.
    sleep(Duration::from_millis(5000)).await;

    let after_delete = read_persisted_peers(&client).await?;
    println!("  localStorage AFTER mass-delete: {} peers", after_delete.len());
    let alive_after_delete = frame_loop_alive(&client).await.unwrap_or(false);
    println!("  frame loop alive after mass-delete: {alive_after_delete}");
    let del_log = capture_log(&client).await?;
    let del_panics = count_panics(&del_log);
    println!("  total panics after mass-delete: {}", del_panics.len());
    // The fix's loud no-op warning must NOT fire — every Delete matched.
    let noop_deletes = del_log.iter()
        .filter(|l| l.contains("NO localStorage entry matched"))
        .count();
    println!("  no-op delete warnings: {noop_deletes}");
    assert_eq!(
        noop_deletes, 0,
        "delete_peer logged {noop_deletes} no-op deletes — a Delete removed nothing \
         (identity-drift match failed); the peer will resurrect."
    );

    // Reload — THE GATE: how many peers actually persisted as deleted?
    client.goto(&url).await?;
    let _ = wait_for_boot(&client, 60_000).await;
    sleep(Duration::from_millis(3000)).await;
    let final_state = read_persisted_peers(&client).await?;
    let final_backends = final_state.iter().filter(|(_, m)| m.starts_with("backend")).count();
    println!("  localStorage AFTER reload (THE GATE): {} peers ({} backend)",
        final_state.len(), final_backends);

    // The gate: after deleting every deletable (drifted) peer, NO backend
    // may survive the reload. (The frontend primary has no Delete button.)
    assert_eq!(
        final_backends, 0,
        "identity-drift BUG-A: {final_backends} backend peers RESURRECTED after \
         mass-delete + reload. frame-alive after boot={alive_after_boot}, \
         after delete={alive_after_delete}, boot panics={}, rows rendered={row_count}, \
         deleted clicks={deleted_n}. Final: {final_state:?}",
        boot_panics.len()
    );

    println!("  drifted_identity_peers_are_durably_deletable OK");
    client.close().await.ok();
    Ok(())
}
