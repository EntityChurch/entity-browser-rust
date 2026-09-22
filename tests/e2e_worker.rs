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
use std::sync::{mpsc, Arc, Mutex};
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
/// Where the WebDriver lives. Overridable because the **multi-host federation
/// gate** drives a Firefox that sits on a podman bridge (so it can reach a
/// publisher container by IP) rather than the host-network one every other test
/// uses; that container publishes its control port, so the URL differs while
/// nothing else does.
fn webdriver_url() -> String {
    std::env::var("E2E_WEBDRIVER_URL")
        .unwrap_or_else(|_| "http://localhost:4444".to_string())
}

/// The origin the **app** is served from, as the browser must address it.
///
/// `localhost` is right whenever the browser shares a network namespace with the
/// dist server. It is wrong for the multi-host gate: there the browser is in a
/// container, so `localhost` is its *own* loopback and the app is on the host,
/// reachable at the bridge gateway. `E2E_APP_ORIGIN` carries that address.
fn app_base() -> String {
    std::env::var("E2E_APP_ORIGIN")
        .unwrap_or_else(|_| format!("http://localhost:{}", http_server_port()))
}
const TAURI_BIN: &str = "./src-tauri/target/debug/entity-browser-tauri";

/// Upper bound for a poll loop waiting on an **async round-trip** — a connect
/// handshake, a worker `CreatePeer`, a spawned listener answering, a
/// `spawn_local`'d boot task logging its result.
///
/// It is a bound, not a wait: every loop using it returns the moment its
/// condition holds, and the ones watching a connect also break out immediately
/// on the explicit `✗ connect …` line, so a real failure is still reported fast.
/// The budget is only ever spent when *nothing* arrives — which is a hang, and
/// deserves to be reported rather than raced.
///
/// This constant exists because these budgets used to be a scattering of
/// hand-written `Duration::from_secs(3)`s, each justified by a guess about how
/// long its round-trip takes on an unloaded box ("ws connect is fast on
/// loopback — 2s is plenty"). On a loaded one the guess is wrong and the phase
/// fails for a reason unrelated to what it tests: two of them were measured
/// doing exactly that (Phase 15.6, and Phase 14's `sleep(2500)`). Prefer this
/// over a fresh literal, and prefer `poll_json` over a bare sleep.
///
/// Well below the 240s stall watchdog, so a loop that does spend it still gets
/// to report its own diagnosis rather than being killed from outside.
const ASYNC_ROUND_TRIP_BUDGET: Duration = Duration::from_secs(30);

/// Upper bound on "the app booted far enough to start its frame loop".
///
/// **This replaced fourteen bare `8000`s, and the reason is the one this file
/// already records one constant up: a budget that is generous and still fails
/// is not tight, it is LOAD-SENSITIVE, and a bigger guess does not fix that
/// shape.** Measured across a healthy unfiltered run, boot is **108–711 ms** —
/// so 8 s was already an 11x margin over the slowest healthy case, and three
/// consecutive runs on a box with other seats' browser containers up still
/// failed on it, at a DIFFERENT phase each time (21, then 20, then 19), always
/// as `never saw 'Frame loop started'`. That reads as an app that will not
/// boot, at a phase with nothing to do with what changed.
///
/// It is an upper bound, not a wait: `wait_for_boot` returns the moment the
/// marker appears, so a healthy run pays none of it. Well under the 240 s stall
/// watchdog, so a genuinely wedged boot still reports its own diagnosis.
const BOOT_BUDGET_MS: u64 = 30_000;

/// Above this, `wait_for_boot` says so on SUCCESS.
///
/// Raising a budget silently trades a false failure for a lost signal — a tight
/// budget was accidentally reporting "this got slower". This keeps the signal
/// without the failures: 4 s is ~6x the slowest measured healthy boot, so it
/// cannot fire on a normal run, and when it does fire the number is in the log
/// instead of being absorbed.
const BOOT_SLOW_NOTICE_MS: u64 = 4_000;

// ── Phase filter (E2E_UNTIL) ──────────────────────────────────────────────
//
// `worker_boots_and_opens_all_windows` is one long stateful chain and it
// dominates the suite's wall-clock. When you are iterating on a phase in the
// middle of it, `E2E_UNTIL` lets you stop as soon as that phase is done
// instead of paying for every phase after it.

/// Every phase of `worker_boots_and_opens_all_windows`, in EXECUTION order.
///
/// The filter compares *positions in this list*, never the labels — the labels
/// are not orderable text ("2h.2" precedes "2i", "13.5" precedes "14", "21b"
/// precedes "22"), and Phase 11 deliberately runs late, right after Phase 14.
/// Adding a phase to the test means adding its label here, in the slot where
/// it actually runs; `phase_gate!` panics on a label that is missing, so the
/// list cannot silently drift out of sync with the test.
const PHASE_ORDER: &[&str] = &[
    "1", "1b", "2", "2-SE", "2b", "2c", "2d", "2e", "2-net", "2f", "2f.1", "2f.3", "2f.2", "2g", "2h",
    "2h.2", "2h.2s", "2h.2b", "2h.2c", "2h.2d", "2h.2e", "2h.3", "2h.4", "2i", "2i.5", "2j", "2k", "3", "3-i18n", "4", "5", "5.1", "6", "7", "8", "9",
    "10",
    "12", "13", "13.5", "14", "14.2", "14.3", "14.5", "14.6", "14.7", "11", "15", "15.5", "15.7", "15.8", "16", "17", "18", "19", "19-doc", "20",
    "21", "21b", "22", "22.5", "23", "24", "25", "26", "26.8", "26.9", "27",
];

/// Resolve `E2E_UNTIL` to a position in `PHASE_ORDER`; `None` = run everything.
///
/// A **prefix** is the only cut on offer — there is deliberately no `E2E_FROM`.
/// Phases 1–17 share a single browser session, and every later phase
/// re-navigates into state its predecessors built (spawned windows, created
/// peers, persisted session config, published sites). Jumping into the middle
/// would fail on absent prerequisites and read exactly like a real regression,
/// which is worse than slow. Stopping early can only run *fewer* assertions —
/// it can never turn a red green, and the stop line says where it stopped.
///
/// To iterate on ONE surface without paying for the chain at all, write it as
/// its own `#[tokio::test]` (there are a dozen already, each doing its own
/// `setup()`) and select it with `make e2e-worker T=<name>`.
fn phase_until_index() -> Option<usize> {
    let want = std::env::var("E2E_UNTIL").ok()?;
    let want = want.trim().trim_start_matches("Phase ").to_string();
    if want.is_empty() {
        return None;
    }
    match PHASE_ORDER.iter().position(|p| *p == want) {
        Some(i) => Some(i),
        None => panic!(
            "E2E_UNTIL={want:?} is not a phase label. Known phases, in run order:\n  {}",
            PHASE_ORDER.join(" ")
        ),
    }
}

/// Position of `label` in `PHASE_ORDER`, panicking if the roster is stale.
fn phase_index(label: &str) -> usize {
    PHASE_ORDER
        .iter()
        .position(|p| *p == label)
        .unwrap_or_else(|| {
            panic!("phase_gate!({label:?}): label is missing from PHASE_ORDER — add it")
        })
}

// ── Stall watchdog ────────────────────────────────────────────────────────
//
// Everything in this suite that waits has a deadline EXCEPT the WebDriver
// round-trips themselves, and those are the ones that can wedge: a hung
// renderer, a browser that stops painting, a Selenium container that dies
// mid-command. Then `client.execute(...).await` never returns, and `cargo
// test` sits there producing nothing — no output, no failure, no diagnosis, in
// CI or an agent loop, until something external kills it. A suite that hangs
// is worse than one that fails: a failure names a phase, a hang names nothing.
//
// So: a process-wide watchdog. Every phase reports progress; if none is
// reported for `E2E_STALL_SECS`, we print WHERE we were stuck and kill the
// process. It cannot rescue the run — it makes the run *tell you what
// happened*, which is the whole difference.
//
// The threshold is a stall bound, not a runtime budget: the full suite is
// ~285s across ~57 phases (~5s each; the slowest, Phase 14, waits up to 60s on
// a spawned Tauri). 240s of total silence is therefore far outside any healthy
// phase while still failing fast.

/// Millis since the process epoch at the last reported progress, and the label
/// we were on. Written by [`note_progress`], read by the watchdog thread.
static PROGRESS: std::sync::Mutex<Option<(std::time::Instant, String)>> =
    std::sync::Mutex::new(None);

fn stall_budget() -> Duration {
    std::env::var("E2E_STALL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(240))
}

/// Report that the suite is still moving. Cheap; called per phase and per test.
fn note_progress(label: &str) {
    if let Ok(mut p) = PROGRESS.lock() {
        *p = Some((Instant::now(), label.to_string()));
    }
}

/// Arm the watchdog once per process. Idempotent.
fn arm_stall_watchdog() {
    static ARMED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ARMED.get_or_init(|| {
        let budget = stall_budget();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let Ok(p) = PROGRESS.lock() else { continue };
            let Some((at, ref label)) = *p else { continue };
            let stuck = at.elapsed();
            if stuck < budget {
                continue;
            }
            let label = label.clone();
            drop(p);
            eprintln!(
                "\n\n=== E2E STALL WATCHDOG ===\n\
                 No progress for {}s (limit {}s). Last phase entered: {label}\n\
                 Something below the assertions wedged — a hung renderer, a\n\
                 browser that stopped painting, or a dead Selenium container.\n\
                 WebDriver round-trips are the only unbounded waits here, so\n\
                 that is where to look; `target/e2e-tauri-stderr.log` covers the\n\
                 Tauri phases. Raise with E2E_STALL_SECS=<n> if a phase legitimately\n\
                 needs longer.\n\
                 Killing the run so it fails loudly instead of hanging forever.\n\
                 ==========================\n",
                stuck.as_secs(),
                budget.as_secs()
            );
            // The test thread is wedged in a syscall we cannot unwind, so a
            // panic would not reach it — exiting the process is the only way
            // out that still prints the diagnosis above.
            std::process::exit(101);
        });
    });
}

/// Placed at the top of every phase. Ends the run green when `E2E_UNTIL` names
/// an earlier phase; otherwise a no-op. See [`phase_until_index`].
///
/// Takes the client explicitly so the early exit can hand the browser session
/// back — bailing out without closing would leave the Selenium standalone's one
/// slot occupied and stall the next run (see [`reap_stale_sessions`]).
macro_rules! phase_gate {
    ($client:expr, $label:literal) => {
        // Entering a phase IS the suite's unit of progress — so the watchdog
        // rides the gate every phase already goes through, and can never be
        // forgotten on a newly-added phase.
        note_progress($label);
        if let Some(until) = phase_until_index() {
            let here = phase_index($label);
            if here > until {
                println!(
                    "=== E2E_UNTIL={} reached — stopping before Phase {}; \
                     {}/{} phases ran ===",
                    PHASE_ORDER[until],
                    $label,
                    here,
                    PHASE_ORDER.len()
                );
                $client.close().await.ok();
                return Ok(());
            }
        }
    };
}

/// Delete any WebDriver session a previous run left behind, before asking for
/// a new one.
///
/// Every test closes its session on the success path — but an assertion
/// failure (or an `E2E_UNTIL` early exit, or a Ctrl-C) unwinds straight past
/// the `close()`. The Selenium standalone image serves **one session at a
/// time** and queues further requests for its `--session-timeout` (300 s), so
/// the next run blocks for five minutes and then dies with `New session
/// request timed out` — infra noise that reads exactly like a real failure and
/// makes iterating on a narrowed run impossible. Reaping on the way *in* is
/// the only cleanup a panicking run cannot skip.
///
/// Driven through `python3`, which this suite already requires for its own
/// `http.server` — not a new dependency.
fn reap_stale_sessions() {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            r#"
import json, urllib.request
BASE = "http://localhost:4444"
try:
    st = json.load(urllib.request.urlopen(BASE + "/status", timeout=5))
except Exception:
    raise SystemExit(0)          # no grid yet — connect() reports it properly
ids = []
def walk(o):
    if isinstance(o, dict):
        s = o.get("session")
        if isinstance(s, dict) and s.get("sessionId"):
            ids.append(s["sessionId"])
        for v in o.values():
            walk(v)
    elif isinstance(o, list):
        for v in o:
            walk(v)
walk(st)
for sid in ids:
    req = urllib.request.Request(BASE + "/session/" + sid, method="DELETE")
    try:
        urllib.request.urlopen(req, timeout=10)
        print(sid)
    except Exception:
        pass

# A DELETE returns before the node has actually freed the slot, and this grid
# serves ONE session at a time — so asking for the next one too early does not
# fail fast, it QUEUES for --session-timeout (300 s). That is where a 650 s
# run and `InvalidSessionId: Tried to run command without establishing a
# connection` both come from. Wait for the grid to report itself empty.
import time
if ids:
    deadline = time.time() + 30
    while time.time() < deadline:
        try:
            st = json.load(urllib.request.urlopen(BASE + "/status", timeout=5))
        except Exception:
            break
        left = []
        walk_left = [st]
        while walk_left:
            o = walk_left.pop()
            if isinstance(o, dict):
                s = o.get("session")
                if isinstance(s, dict) and s.get("sessionId"):
                    left.append(s["sessionId"])
                walk_left.extend(o.values())
            elif isinstance(o, list):
                walk_left.extend(o)
        if not left:
            break
        time.sleep(0.1)
    else:
        print("WARN-slot-still-busy")
"#,
        )
        .output();
    if let Ok(out) = out {
        let reaped = String::from_utf8_lossy(&out.stdout);
        let mut reaped: Vec<&str> = reaped.split_whitespace().collect();
        // The slot never came free. Say so LOUDLY: the next `connect()` will
        // queue for the grid's 300 s session-timeout, and the resulting failure
        // names neither the queue nor the reap.
        let stuck = reaped.iter().position(|s| *s == "WARN-slot-still-busy");
        if let Some(i) = stuck {
            reaped.remove(i);
            eprintln!(
                "  WARNING: reaped {} session(s) but the grid still reports one busy after 30s. \
                 The next connect() will QUEUE (session-timeout 300s), and its error will not \
                 mention any of this. Something outside the suite is probably driving :4444.",
                reaped.len()
            );
        }
        if !reaped.is_empty() {
            println!(
                "  reaped {} stale WebDriver session(s) from a previous run: {}",
                reaped.len(),
                reaped.join(", ")
            );
        }
    }
}

/// Holds a child `python3 -m http.server` process for the duration of a
/// test. `kill()` is called on drop, so panics in the test still clean
/// up the server.
///
/// It also **drains the server's request log for the child's whole life**, into
/// a bounded buffer. Two reasons, and the second is why the buffer exists at
/// all rather than a bare counter:
///
/// 1. `python3 -m http.server` logs one line per request to stderr, which makes
///    the server the only place in this rig that can answer *"how many bytes
///    actually crossed the wire"*. A browser cannot: Firefox zeroes
///    `transferSize` **and** `encodedBodySize` for any response a service worker
///    supplied, whether the SW went to the network or served from its cache —
///    measured, and it is why the obvious in-page assertion is vacuous here.
/// 2. Before this, stderr was piped and read **only** on the immediate-exit
///    path, so a server that died mid-run discarded its own explanation — the
///    same closed-pipe shape that cost a session on the Tauri listener.
struct DistServer {
    child: Child,
    log: Arc<Mutex<Vec<String>>>,
}

impl DistServer {
    /// How many requests the server was asked to serve whose log line contains
    /// `needle`. Measured at the wire, not reported by the thing under test.
    fn request_count(&self, needle: &str) -> usize {
        match self.log.lock() {
            Ok(lines) => lines.iter().filter(|l| l.contains(needle)).count(),
            Err(_) => 0,
        }
    }
}

impl Drop for DistServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Cap on retained request lines. The monolith makes a few thousand requests;
/// this is a diagnostic buffer, not a transcript, and an unbounded one in a
/// 15-minute test is its own bug.
const DIST_LOG_CAP: usize = 20_000;

/// Start the dist server and **prove it is accepting connections** before
/// returning.
///
/// **This used to spawn, `Stdio::null()` the stderr, and sleep a fixed 300 ms.**
/// Both halves are anti-patterns this repo has already written down, and
/// together they make the most common suite failure undiagnosable:
///
/// - **Nulling a helper server's stderr** is the exact thing AGENTS.md forbids
///   ("a bind failure is precisely the error that makes the rig lie"). If the
///   port is still held, python prints `Address already in use` and exits — into
///   `/dev/null` — and the harness carries on as though it had a server.
/// - **A fixed sleep then navigate** is a guess about someone else's startup.
///   The browser's report when the guess is wrong is `connectionFailure` at
///   `localhost:8092`, which reads as a network or app fault and is neither.
///
/// Measured before changing it, so the sleep is not being blamed for more than
/// it did: bind latency here is **22–32 ms idle and 21–25 ms under load**, so
/// 300 ms was a 10× margin and is *not* the cause of the observed failures. The
/// point of polling is not that 300 ms was too short — it is that a fixed wait
/// cannot tell "not yet" from "never", and the whole failure class is invisible
/// while the diagnostic goes to `/dev/null`.
fn start_dist_server() -> Result<DistServer, std::io::Error> {
    // Plain static server: the L5 iframe is same-origin (`allow-same-origin`), so
    // it fetches its wasm same-origin — no CORS header needed. (An *untrusted* L5
    // app would run opaque-origin and then need a CORS-adding server; that arrives
    // with the sub-peer capability model — D21.)
    let port = http_server_port();
    let child = Command::new("python3")
        .args(["-m", "http.server", &port.to_string(), "--directory", "dist"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    await_server_ready(child, port, "the dist server")
}

/// Port for the **black-hole** origin (G1). Distinct from the dist server's so a
/// stalled origin can never be mistaken for, or collide with, the healthy one —
/// and so the two can be up at once. Override via `E2E_BLACKHOLE_PORT`.
#[allow(dead_code)]
fn blackhole_server_port() -> u16 {
    std::env::var("E2E_BLACKHOLE_PORT")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(8093)
}

/// Start `tools/e2e/blackhole-serve.py` over `dist/`: a static server that
/// serves everything normally **except** the paths in `stall`, which it accepts
/// and never answers (see that file for why "accepts and never answers" is a
/// different bug from "refuses").
///
/// Readiness is proven exactly as the dist server's is — an HTTP round-trip for
/// `/index.html`, not a bare connect. That check matters more here than
/// anywhere else in the suite: a bare TCP connect cannot distinguish this
/// server from the failure it exists to simulate.
#[allow(dead_code)]
fn start_blackhole_server(stall: &[&str]) -> Result<DistServer, std::io::Error> {
    let port = blackhole_server_port();
    let mut args: Vec<String> = vec![
        "tools/e2e/blackhole-serve.py".to_string(),
        port.to_string(),
        "--directory".to_string(),
        "dist".to_string(),
    ];
    for p in stall {
        args.push("--stall".to_string());
        args.push((*p).to_string());
    }
    let child = Command::new("python3")
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    await_server_ready(child, port, "the black-hole server")
}

/// Turn the black hole on or off at runtime. `stall` replaces the whole set;
/// an empty slice clears it.
///
/// Needed because the service-worker half of G1 cannot start black-holed: the
/// shell has to be fetched and cached normally first, or there is nothing to
/// fall back to and a passing test would prove nothing.
#[allow(dead_code)]
fn set_blackhole(stall: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    let port = blackhole_server_port();
    let mut sock = std::net::TcpStream::connect(("127.0.0.1", port))?;
    sock.set_read_timeout(Some(Duration::from_secs(5)))?;
    let req = format!(
        "GET /__blackhole?stall={} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n",
        stall.join(",")
    );
    sock.write_all(req.as_bytes())?;
    let mut body = String::new();
    sock.read_to_string(&mut body)?;
    if !body.contains(" 200 ") {
        return Err(format!("black-hole control returned:\n{body}").into());
    }
    Ok(body)
}

/// Turn on the **headers-then-stall** mode: the origin answers `200` with a
/// truthful `Content-Length` and then never sends the body.
///
/// A different bug from `set_blackhole`, and the reason it needs its own control:
/// `fetch` resolves on **headers**, so a deadline that disarms at that moment is
/// already disarmed when the body fails to arrive. Whatever reads the body next
/// is the thing that hangs. A timeout written only against the never-answers
/// case cannot see this.
#[allow(dead_code)]
fn set_blackhole_body(stall: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    let port = blackhole_server_port();
    let mut sock = std::net::TcpStream::connect(("127.0.0.1", port))?;
    sock.set_read_timeout(Some(Duration::from_secs(5)))?;
    let req = format!(
        "GET /__blackhole?body={} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n",
        stall.join(",")
    );
    sock.write_all(req.as_bytes())?;
    let mut body = String::new();
    sock.read_to_string(&mut body)?;
    if !body.contains(" 200 ") {
        return Err(format!("black-hole body control returned:\n{body}").into());
    }
    Ok(body)
}

/// Shared readiness probe + stderr drain for a helper HTTP server.
///
/// Extracted so the black-hole server inherits every hard-won property of the
/// dist server's startup rather than re-deriving them: the child-alive check
/// **before** the connect probe, an HTTP round-trip rather than a bare connect,
/// and stderr that stays readable for the child's whole life.
fn await_server_ready(
    mut child: Child,
    port: u16,
    what: &str,
) -> Result<DistServer, std::io::Error> {
    // Poll rather than guessing. **"Is our child alive" is checked BEFORE "is
    // something listening", and that order is load-bearing** — found by
    // mutation, holding :8092 with a socket that accepts and never answers.
    // With the connect probe first, it saw the *blocker* listening, returned
    // Ok over an already-dead child, and the failure surfaced 60 s later as a
    // WebDriver navigation timeout: the cheap check shadowing the real one, in
    // the fix for a bug whose whole shape was a missing diagnostic.
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        // If it already exited, say WHY — this is the branch that used to be
        // silent, and "Address already in use" is what it usually says.
        if let Ok(Some(status)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                use std::io::Read;
                let _ = e.read_to_string(&mut err);
            }
            return Err(std::io::Error::other(format!(
                "{what} on :{port} exited immediately ({status}). Its stderr:\n{}\n\
                 If that says 'Address already in use', something still holds :{port} — a \
                 previous test's server, a stray `make serve`, or another suite run. This \
                 used to present as the BROWSER reporting connectionFailure, which looks \
                 like an app fault and is not one.",
                if err.trim().is_empty() { "(empty)" } else { err.trim() }
            )));
        }
        // An HTTP round-trip, not a bare TCP connect. A connect only proves
        // *something* holds the port — including a socket that accepts and
        // never answers, which is what made the first version of this loop
        // return Ok over a dead child. Requiring a response line proves it is a
        // server, and requiring 200 for `/index.html` proves it is serving
        // `dist/` rather than someone else's directory.
        if let Ok(mut sock) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            use std::io::{Read, Write};
            let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
            let req = format!(
                "GET /index.html HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
            );
            if sock.write_all(req.as_bytes()).is_ok() {
                let mut head = [0u8; 64];
                if let Ok(n) = sock.read(&mut head) {
                    if String::from_utf8_lossy(&head[..n]).contains(" 200 ") {
                        // Only now take stderr: until this point the loop above
                        // owns it, because a child that exits has to be able to
                        // hand back its own reason.
                        let log = Arc::new(Mutex::new(Vec::new()));
                        if let Some(err) = child.stderr.take() {
                            let sink = Arc::clone(&log);
                            std::thread::spawn(move || {
                                for line in BufReader::new(err).lines().map_while(Result::ok) {
                                    if let Ok(mut buf) = sink.lock() {
                                        if buf.len() < DIST_LOG_CAP {
                                            buf.push(line);
                                        }
                                    }
                                }
                            });
                        }
                        return Ok(DistServer { child, log });
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(std::io::Error::other(format!(
        "{what} never served a 200 for /index.html on :{port} within 20s. \
         Either `dist/` has no index.html (run `make wasm`), or something else is \
         holding :{port} and answering — check for a stray `make serve` or another \
         suite run before reading this as an app fault."
    )))
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
    /// How long the native listener took to print its READY line, and how long
    /// the WebView took to reach `Frame loop started` — both from spawn.
    ///
    /// **They exist because a 60s budget that prints nothing on success cannot
    /// tell a loaded box from a broken WebView, and that is precisely the
    /// question the display-gated phase's known red raises.** The failure reads
    /// *"the WebView never booted in 60s"*, which is a statement about the
    /// budget as much as about the app — and until now nobody could say whether
    /// the healthy path takes 3s or 55s. Same rule this suite already learned
    /// twice (`ASYNC_ROUND_TRIP_BUDGET`, `BOOT_SLOW_NOTICE_MS`): print the
    /// margin on success, or raising a budget silently trades a false failure
    /// for a lost signal.
    pub ready_ms: u128,
    pub webview_ms: Option<u128>,
    /// Which of [`WEBVIEW_BOOT_MILESTONES`] the child actually logged during
    /// startup, in arrival order, with the elapsed ms from spawn. The whole
    /// point of the phase's failure message: *how far did it get.*
    pub boot_milestones: Vec<(&'static str, u128)>,
    /// Rolling tail of the child's stdout, filled by the reader thread for
    /// as long as the child lives.
    ///
    /// This exists for two reasons, and the first one is not diagnostics:
    ///
    /// 1. **Something must keep draining the pipe.** This used to be a bare
    ///    `mpsc` whose receiver was a local in `start_tauri_listener` — so it
    ///    was dropped the moment startup succeeded, the reader thread's next
    ///    `send` failed, the thread broke out of its loop, and the read end of
    ///    the child's stdout closed. From the READY line onward every log line
    ///    in the Tauri process hit `EPIPE` and its logger dumped a multi-line
    ///    "Error performing logging / attempted to log / record: …" block to
    ///    stderr instead. Measured on a real run: the broken-pipe flood starts
    ///    at the same second as the READY line and accounts for most of the
    ///    32 KB stderr capture. The child now keeps a live reader for its whole
    ///    life, so its stdout writes stay cheap and its stderr stays readable.
    /// 2. Having drained it, we may as well keep it: the Tauri phases run
    ///    minutes after startup, and when one fails, the child's recent stdout
    ///    is the evidence. Before this it was discarded, so a Phase 14 / 15.6
    ///    failure could only be guessed at.
    stdout_tail: Arc<Mutex<std::collections::VecDeque<String>>>,
}

/// How many stdout lines to retain from the Tauri child. Bounded so a long
/// suite can't grow it without limit; large enough that a failing phase sees
/// the run-up to the failure, not just its last gasp.
const TAURI_STDOUT_TAIL_LINES: usize = 200;

/// The WebView's boot milestones, **in the order the WASM module logs them**,
/// from `src/main.rs`. Every one goes through `tracing_wasm` → the console →
/// the console bridge in `src-tauri/src/lib.rs` → the same stdout this harness
/// is already scraping, so their presence is observable here for free.
///
/// **Why this exists.** On 2026-08-23 the display-gated phase went red 3/3 and
/// the only thing the failure could say was *"never saw `Frame loop started` in
/// 60s"* — which is one bit, and it does not distinguish *the module never
/// started* from *the module started and app boot hung*. Those have completely
/// different causes and send you to different files. Recovering the answer cost
/// a session of reading `src/main.rs` by hand to work out what SHOULD have been
/// logged, and then reasoning from the absence.
///
/// The absence was only readable because the bridge was demonstrably alive
/// (other `[webview]` lines were coming through). **That is the load-bearing
/// part: an absent log line is evidence only once you have shown the channel
/// works** — so the report below always states how many milestones arrived,
/// never just which one is missing.
///
/// Reported on **success too**, deliberately. A failure-only diagnostic has no
/// baseline to compare against: this suite could not answer "does a healthy
/// boot also hit the `instantiateStreaming` fallback?" because the child's
/// stdout was dumped on failure and nowhere else. Same rule the suite already
/// learned for budgets — print the margin on success, or a green run carries no
/// evidence.
const WEBVIEW_BOOT_MILESTONES: &[&str] = &[
    "WASM init: tracing level set",
    "WASM init: DOM mode set, creating app",
    "boot: peer host arm selected",
    "WASM init: app created, starting rAF loop",
    "Frame loop started",
];

/// Render the boot-milestone trace for a message. `reached` is `(label, ms)`
/// in arrival order.
fn render_boot_milestones(reached: &[(&'static str, u128)]) -> String {
    if reached.is_empty() {
        return "  (none — the WASM module never reached its first log statement, \
                which is BEFORE any app code runs; look at module instantiation, \
                not at app boot)"
            .to_string();
    }
    let mut out: Vec<String> = reached
        .iter()
        .map(|(label, ms)| format!("  ✓ {label} @ {ms} ms"))
        .collect();
    for label in WEBVIEW_BOOT_MILESTONES {
        if !reached.iter().any(|(l, _)| l == label) {
            out.push(format!("  ✗ {label} — never seen"));
        }
    }
    out.join("\n")
}

/// The two branches of [`render_boot_milestones`] that only appear on a FAILING
/// run — so they are pinned here rather than first exercised on the day
/// somebody needs them to be right.
///
/// The healthy branch has its own control and it is a real one: a passing
/// Phase 14 prints `tauri boot milestones: 5/5`, which is what proves the
/// milestone strings still match what the console bridge emits. That control is
/// the load-bearing half — if the app ever reworded one of these lines, the
/// count drops on a GREEN run and says so, instead of silently degrading the
/// next failure message back to the single bit it used to carry.
///
/// Needs no browser, no Selenium and no display: it is a pure function, and
/// keeping it that way is why the diagnosis is testable at all.
#[test]
fn the_boot_milestone_trace_distinguishes_never_started_from_hung_midway() {
    // Nothing arrived: the message must send the reader at INSTANTIATION, and
    // must not name a step as "the one that hung" — there wasn't one.
    let none = render_boot_milestones(&[]);
    assert!(
        none.contains("never reached its first log statement"),
        "empty trace must say the module never began executing, got: {none}"
    );
    assert!(
        !none.contains('✓'),
        "empty trace must not claim any milestone was reached, got: {none}"
    );

    // Got partway: every reached step is shown WITH its timing, and every
    // missing one is named. A trace that only listed what was reached would
    // leave the reader counting against a list they have to go find.
    let partial = render_boot_milestones(&[
        (WEBVIEW_BOOT_MILESTONES[0], 12),
        (WEBVIEW_BOOT_MILESTONES[1], 40),
    ]);
    assert!(partial.contains("✓ WASM init: tracing level set @ 12 ms"), "{partial}");
    assert!(partial.contains("✓ WASM init: DOM mode set, creating app @ 40 ms"), "{partial}");
    assert!(partial.contains("✗ boot: peer host arm selected — never seen"), "{partial}");
    assert!(partial.contains("✗ Frame loop started — never seen"), "{partial}");
    assert_eq!(
        partial.lines().count(),
        WEBVIEW_BOOT_MILESTONES.len(),
        "every milestone must appear exactly once, reached or not: {partial}"
    );
}

/// Render a stdout ring buffer for a failure message, newest last. Never
/// panics on a poisoned lock — a diagnostic that can itself fail is worse than
/// no diagnostic. Shared by the startup-timeout path (which has no
/// `TauriListener` yet) and [`TauriListener::stdout_tail`].
fn render_stdout_tail(tail: &Arc<Mutex<std::collections::VecDeque<String>>>) -> String {
    match tail.lock() {
        Ok(t) if t.is_empty() => "  (child produced no stdout)".to_string(),
        Ok(t) => t
            .iter()
            .map(|l| format!("  | {l}"))
            .collect::<Vec<_>>()
            .join("\n"),
        Err(_) => "  (stdout tail unavailable: lock poisoned)".to_string(),
    }
}

impl TauriListener {
    /// The child's most recent stdout lines, ready to paste into a failure
    /// message.
    fn stdout_tail(&self) -> String {
        render_stdout_tail(&self.stdout_tail)
    }
}

impl Drop for TauriListener {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn the desktop binary for the Tauri phases, or `Ok(None)` when the
/// environment has no display — GTK cannot initialize without a window
/// server, so in a headless container the child dies at spawn (proven:
/// `Failed to initialize GTK` on stderr). The phases that need it SKIP
/// loudly rather than failing the whole suite red forever in headless
/// runs — a permanent known-red masks every new regression. To exercise
/// these phases, run where the e2e container gets a display (see the
/// Makefile's display passthrough / AGENTS §e2e).
fn start_tauri_listener() -> Result<Option<TauriListener>, Box<dyn std::error::Error>> {
    let has_display = std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("DISPLAY").is_ok();
    if !has_display {
        return Ok(None);
    }
    // Capture the child's stderr to a file instead of discarding it — when
    // this phase times out, the stderr tail is the diagnosis (a panic, a GTK/
    // display failure, a store error print there, not on stdout).
    let stderr_path = "target/e2e-tauri-stderr.log";
    let stderr_file = std::fs::File::create(stderr_path)?;
    let mut child = Command::new(TAURI_BIN)
        .env("ENTITY_BROWSER_AUTOSTART_LISTENER", "1")
        // Keep the listener on loopback for the test — the production
        // default now binds 0.0.0.0 and reports the LAN IP (phone
        // pairing), but this suite asserts against a same-host connect.
        .env("ENTITY_BROWSER_LOOPBACK_ONLY", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr_file))
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
    //
    // The thread reads until the child's stdout actually ends — it does NOT
    // stop when the receiver goes away. Closing the read end early is what
    // used to give the child EPIPE on every subsequent log write; see
    // `TauriListener::stdout_tail`. So `send` is deliberately best-effort
    // (`let _ =`): once the startup loop drops `rx`, sends fail, nothing is
    // buffered, and the ring buffer below carries on being the sink.
    let stdout_tail: Arc<Mutex<std::collections::VecDeque<String>>> =
        Arc::new(Mutex::new(std::collections::VecDeque::new()));
    let (tx, rx) = mpsc::channel::<String>();
    let reader_tail = Arc::clone(&stdout_tail);
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(mut tail) = reader_tail.lock() {
                if tail.len() >= TAURI_STDOUT_TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line.clone());
            }
            let _ = tx.send(line);
        }
    });

    // Wait for both: the autostart READY line (native listener up)
    // AND the WebView's "Frame loop started" message (UI booted).
    // WebView load is independent from autostart but production users
    // want both healthy — if dist/ holds the wrong WASM flavor (e.g.
    // worker-mode on WebKitGTK), the UI fails to boot while autostart
    // still succeeds. We want the test to catch that.
    //
    // 60s: generous because this runs ~200s into the suite on a loaded
    // box (Firefox + serve + cargo); the loop exits the moment both
    // signals arrive, so a healthy boot doesn't pay for the headroom.
    // On timeout we report the child's recent stdout (ring buffer) +
    // point at the stderr capture — the phase must carry its own
    // diagnosis, not guess at causes in a static message.
    let started = Instant::now();
    let deadline = started + Duration::from_secs(60);
    let mut peer_id: Option<String> = None;
    let mut ws_addr: Option<String> = None;
    let mut webview_booted = false;
    let mut ready_at: Option<Instant> = None;
    let mut webview_at: Option<Instant> = None;
    let mut boot_milestones: Vec<(&'static str, u128)> = Vec::new();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                if let Some(rest) = line.strip_prefix("ENTITY_BACKEND_LISTENER_READY ") {
                    ready_at.get_or_insert_with(Instant::now);
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
                // Record every boot milestone the module logs, not just the
                // last one — a failure's whole question is HOW FAR it got, and
                // that is unanswerable from a single boolean. Matched with
                // `contains` because each line arrives wrapped in the console
                // bridge's `[webview]` prefix and tracing's own formatting.
                for label in WEBVIEW_BOOT_MILESTONES {
                    if line.contains(label)
                        && !boot_milestones.iter().any(|(l, _)| l == label)
                    {
                        boot_milestones
                            .push((label, (Instant::now() - started).as_millis()));
                    }
                }
                // WASM logs this from src/main.rs:163 once the rAF
                // pump is running. Routed through the console bridge
                // in src-tauri/src/lib.rs which forwards to stdout.
                if line.contains("Frame loop started") {
                    webview_booted = true;
                    webview_at.get_or_insert_with(Instant::now);
                }
                if peer_id.is_some() && webview_booted {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let startup_tail = render_stdout_tail(&stdout_tail);
    let peer_id = peer_id.ok_or_else(|| {
        format!(
            "tauri did not print ENTITY_BACKEND_LISTENER_READY within 60s.\n\
             Recent stdout from the child:\n{startup_tail}\n\
             Child stderr captured at {stderr_path} — read its tail for the cause\n\
             (a panic / GTK display failure / store error prints there, not on stdout).\n\
             If stdout is empty, the process likely died at spawn: check the binary\n\
             exists and was rebuilt (cd src-tauri && cargo build).",
        )
    })?;
    let ws_addr = ws_addr.ok_or("READY line was missing ws_addr=...")?;

    Ok(Some(TauriListener {
        child,
        peer_id,
        ws_addr,
        webview_booted,
        ready_ms: ready_at.map(|t| (t - started).as_millis()).unwrap_or(0),
        webview_ms: webview_at.map(|t| (t - started).as_millis()),
        boot_milestones,
        stdout_tail,
    }))
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

/// The `entity-peer-*` IndexedDB database names currently present in the
/// origin (each durable peer — the primary system peer and any `frontend-idb`
/// peer — has its own `entity-peer-{id}` database). Used to detect a newly
/// created durable this-tab peer and to assert its store survives a reload.
async fn list_entity_peer_dbs(
    client: &Client,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let v = client
        .execute_async(
            r#"
            const cb = arguments[arguments.length - 1];
            (async () => {
                const dbs = (await indexedDB.databases()).map(d => d.name).filter(Boolean);
                cb(dbs.filter(n => n.startsWith('entity-peer-')));
            })().catch(() => cb([]));
            "#,
            vec![],
        )
        .await?;
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

/// Open a Shell window bound to `peer_id`: select the peer in the command
/// palette's peer-selector (by option value, robust to display text), then
/// click the peer-scoped `+ Shell` spawn (spawn buttons read the palette
/// selection at click time). Fails loudly if the peer isn't a selectable
/// palette option — which, after a reload, is itself the proof that the peer
/// was NOT rehydrated.
async fn open_peer_shell(
    client: &Client,
    peer_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let sel = client
        .execute(
            r#"
            const [pid] = arguments;
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const select = root.querySelector('.command-palette select');
            if (!select) return 'no-palette-select';
            let found = false;
            for (const opt of select.options) { if (opt.value === pid) { found = true; break; } }
            if (!found) return 'no-peer-option';
            select.value = pid;
            select.dispatchEvent(new Event('change', { bubbles: true }));
            return 'ok';
            "#,
            vec![serde_json::Value::String(peer_id.to_string())],
        )
        .await?;
    assert_eq!(
        sel.as_str(),
        Some("ok"),
        "open_peer_shell: couldn't select peer {peer_id} in the palette (rehydrated?): {sel:?}"
    );
    sleep(Duration::from_millis(300)).await;
    let spawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
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
        spawn.as_str(),
        Some("clicked"),
        "open_peer_shell: couldn't spawn a Shell for peer {peer_id}: {spawn:?}"
    );
    sleep(Duration::from_millis(800)).await;
    Ok(())
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
            if elapsed > BOOT_SLOW_NOTICE_MS {
                eprintln!(
                    "  NOTE: boot took {elapsed}ms (healthy is 108-711ms). Not a \
                     failure — but if this is climbing, it is the signal the old \
                     fixed 8000ms budget used to deliver as a false failure."
                );
            }
            return Ok(elapsed);
        }
        if elapsed > timeout_ms {
            // **Dump what the page DID say.** Without this the failure is
            // "never saw 'Frame loop started'" and nothing else — which reads
            // as a dead app and is indistinguishable from a wedged worker, a
            // multi-tab lock, an OPFS refusal, or a panic during boot. Every
            // one of those has a distinct signature in this log, and the
            // failure was thrown away before anyone could see it. The suite's
            // own rule about `Stdio::null()` on a helper server is this same
            // rule: never discard the output that identifies the cause.
            let tail: Vec<String> = log
                .iter()
                .rev()
                .take(40)
                .rev()
                .map(|l| l.chars().take(220).collect())
                .collect();
            return Err(format!(
                "wait_for_boot: never saw 'Frame loop started' in {timeout_ms}ms \
                 ({} log lines captured). Last {} of them:\n  {}",
                log.len(),
                tail.len(),
                tail.join("\n  ")
            )
            .into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// Run `script` until `ready` holds or `budget` elapses, returning the **last**
/// value either way — so the caller still asserts, and still gets the real
/// value in its failure message.
///
/// Use this instead of `sleep(fixed)` + one read. A fixed sleep encodes a guess
/// about how long an async re-render takes; on a loaded box the guess is wrong
/// and the phase fails for a reason that has nothing to do with the behaviour
/// under test. Polling is also what keeps a headless page honest: with no
/// WebDriver interaction the rAF loop can go unpumped, and each `execute`
/// forces a style/layout flush.
///
/// The budget is an upper bound, not a wait — a healthy run returns on the
/// first poll, so a generous budget costs nothing except on the failure path.
async fn poll_json(
    client: &Client,
    script: &str,
    budget: Duration,
    ready: impl Fn(&serde_json::Value) -> bool,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + budget;
    loop {
        let value = client.execute(script, vec![]).await?;
        if ready(&value) || Instant::now() >= deadline {
            return Ok(value);
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
    // Every test comes through here, so this is where the stall watchdog gets
    // armed — including for the dozen standalone tests, which have no phases of
    // their own to report progress from.
    arm_stall_watchdog();
    note_progress("setup");

    // Defensive: Phase 27 drops a `dist/entity-deployment.json` to test the
    // served-config boot path, then removes it. If a prior run crashed mid-phase
    // it could linger and silently change EVERY earlier phase's cold boot (the
    // served config applies when no durable config exists). Remove it before any
    // phase navigates so phases 1–26 always see the default (no-config) path.
    let _ = std::fs::remove_file("dist/entity-deployment.json");

    // Hand back any session a previous (failed / interrupted / E2E_UNTIL-cut)
    // run left holding the standalone's single slot.
    reap_stale_sessions();

    let server = start_dist_server().map_err(|e| {
        format!(
            "failed to start python3 -m http.server: {e}. \
             Is dist/ built? Run `make wasm` first (or `make e2e-worker`)."
        )
    })?;

    let mut caps = serde_json::Map::new();
    caps.insert(
        "moz:firefoxOptions".to_string(),
        serde_json::json!({ "args": ["-headless"] }),
    );
    let url = webdriver_url();
    let client = ClientBuilder::native()
        .capabilities(caps)
        .connect(&url)
        .await
        .map_err(|e| {
            format!(
                "failed to connect to WebDriver at {url}: {e}\n\
                Is the selenium-firefox container running? Try:\n\
                podman run -d --rm --name e2e-firefox --network=host \\\n\
                    docker.io/selenium/standalone-firefox:149.0.2-geckodriver-0.36.0-20260404"
            )
        })?;

    // Bound the two server-side waits explicitly rather than inheriting the
    // WebDriver defaults, where `pageLoad` is **300s**: a page that wedges
    // mid-navigation would otherwise stall one `goto` for five minutes and
    // read as a hang. These make the browser return an error we can attribute,
    // and they are the layer *under* the stall watchdog — the watchdog is the
    // backstop for wedges these cannot see (a dead container, a hung socket).
    client
        .update_timeouts(fantoccini::wd::TimeoutConfiguration::new(
            Some(Duration::from_secs(30)), // script
            Some(Duration::from_secs(60)), // pageLoad
            Some(Duration::from_secs(0)),  // implicit — keep 0 (see fantoccini docs)
        ))
        .await
        .map_err(|e| format!("failed to set WebDriver timeouts: {e}"))?;

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

    phase_gate!(client, "1");
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

    phase_gate!(client, "1b");
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

    phase_gate!(client, "2");
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

    // Floor sanity check: today we ship 21 window types. The Inspect
    // family (Chain Trace, Path Tap, Wire Recorder, Content Stream)
    // landed; Content Site, then the JS-Apps platform (Games
    // 16th, Apps 17th), the Storage window (18th), the Site Editor
    // (19th), and the System Backend window (System-scoped) landed
    // later. The standalone "System Overview" window was then merged INTO
    // System Backend (one System window, S2), dropping the count 22 → 21.
    // If the DOM returns 0 we're parsing wrong; if it returns far
    // fewer than expected we've silently regressed the palette renderer or
    // dropped a window. Update the floor on intentional removals. Discovery
    // clicks each (including System Backend, now labelled "System Overview")
    // and panic-checks it below, so a new window rides this loop rather than
    // needing a bespoke phase. (22 → 23: the Theme Editor landed. 24 → 25: the
    // Chat window (app/chat) landed.)
    // 25 → 24: Games and Apps merged into one launcher with category chips.
    // 24 → 25: the Registry Browser. It needs no bespoke phase — discovery reads
    // the live palette, so it is spawned and panic-checked by this loop like
    // every other window, which is the whole reason the array was replaced by a
    // query in the first place.
    const MIN_DISCOVERED_WINDOW_TYPES: usize = 25;
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

    phase_gate!(client, "2-SE");
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
    // Poll for the delete to reflect rather than sampling ONCE after a fixed
    // sleep. The subgraph delete itself is fast — measured at ~160 ms to clear
    // the list, identically before and after the i18n extraction pass — but a
    // single sample taken after a bare `sleep` intermittently read the stale
    // list anyway: with no WebDriver interaction in the interval, the headless
    // page's rAF loop can go unpumped, so the re-render has not run by the time
    // we look. Each `execute` forces a style/layout flush, which is why polling
    // is deterministic where one delayed read is not.
    //
    // Same assertion, same meaning — it still fails if the site never leaves the
    // list. Only the "how long do we wait" is now bounded-and-observed instead
    // of guessed. (A fixed sleep + one read is the shape to avoid in this suite.)
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut se_after_delete;
    loop {
        se_after_delete = client
            .execute(
                r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.site-editor')) continue;
                const chips = Array.from(sec.querySelectorAll('button')).map(b => b.textContent.trim());
                const offenders = chips.filter(c => c.includes('e2e-site'));
                // Diagnostics carried on EVERY sample, not just the last: this
                // phase fails intermittently (~1 in 3 long runs) and has never
                // been reproduced on demand, so the failure has to explain
                // itself. `offenders` says WHICH control still names the site
                // (the site list? the editor header? a stale row?), which is
                // the split that tells us whether the tree still holds the
                // entities or only the DOM is stale. See the handoff's
                // open-bug section before theorising further.
                return { still_lists: offenders.length > 0, offenders, chips };
            }
            return { still_lists: false, offenders: [], chips: [] };
            "#,
                vec![],
            )
            .await?;
        let still = se_after_delete
            .get("still_lists")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !still || std::time::Instant::now() >= deadline {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    println!("  site-editor after delete still-lists: {se_after_delete}");
    // The Worker-arm cache-mirror trace. This phase's "a surviving list row
    // means the entities outlived the delete" rule is UNDER-DETERMINED: the
    // site list is built from `tree_listing`, which on this arm is
    // `cache_list` — the union of every subscription's mirror — so a surviving
    // row is equally consistent with a stale mirror. The proxy's per-removal
    // `remaining_holders` line settles which
    // (AUDIT-THEME-DELETE-STALE-DROPDOWN F2; same disease, and this phase
    // reproduces it far more often than Phase 26.8 does).
    let se_proxy_trace: Vec<String> = capture_log(&client)
        .await?
        .into_iter()
        .filter(|l| l.contains("worker-proxy:") || l.contains("panicked at"))
        .collect();
    let se_trace_tail: Vec<&String> =
        se_proxy_trace.iter().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect();
    assert!(
        !se_after_delete.get("still_lists").and_then(|v| v.as_bool()).unwrap_or(true),
        "Site Editor still lists the deleted site (subgraph delete didn't reflect) \
         after a 10s poll. `offenders` names the exact control(s) still carrying \
         the site id. A site-list row does NOT by itself mean the entities \
         survived — on this arm the list comes from the cache-mirror union, so a \
         stale mirror looks identical. The `worker-proxy: removal …` lines below \
         are what separates them: `remaining_holders` non-empty with no later \
         removal for those subs means the mirror is stale, not the tree. \
         Detail: {se_after_delete}\
         \n--- worker-proxy trace (last {} of {} lines) ---\n{}",
        se_trace_tail.len(),
        se_proxy_trace.len(),
        se_trace_tail.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"),
    );

    phase_gate!(client, "2b");
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

    phase_gate!(client, "2c");
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

    phase_gate!(client, "2d");
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

    phase_gate!(client, "2e");
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

    phase_gate!(client, "2-net");
    // -- Phase 2-net: the connectivity preflight, in a real browser ----
    //
    // `readiness::assess` is pure and natively tested to death. What no native
    // test can reach is `readiness::collect` — `window.isSecureContext`, the
    // `RTCPeerConnection` lookup, the arm, the booted-provisioning snapshot —
    // and that half is the whole point: the report exists to be typed by a
    // person on a *second machine* who has nothing else to look at. A `net`
    // that panicked or reported nothing in the browser would be discovered by
    // them, during the one run this is meant to rescue.
    //
    // Three things are asserted, and each fails for a different reason:
    let net_sb = shell_submit(&client, "net", 400).await?;
    println!("  net report:\n{net_sb}");
    // (1) Every check id reaches the surface — the collector ran and the render
    //     is not empty. Ids, not prose: the wording is `readiness`' to change.
    for id in ["origin", "webrtc-api", "rendezvous", "establisher", "this-peer"] {
        assert!(
            net_sb.contains(id),
            "`net` did not report the `{id}` check — the collector may have \
             failed in the browser. Got:\n{net_sb}"
        );
    }
    // (2) This harness serves over `localhost`, which IS a secure context, so
    //     the origin row must read OK. If `is_secure_context()` were misread
    //     the preflight would tell every user their origin is broken — the
    //     cry-wolf direction, and the one that gets a diagnostic ignored.
    assert!(
        net_sb.contains("OK   origin") || net_sb.contains("OK  origin"),
        "localhost is a secure context and the origin row must say so. \
         Got:\n{net_sb}"
    );
    // (3) The FAIL path renders too. This session boots with no connector, so
    //     the rendezvous row must fail AND carry the action. A preflight whose
    //     failure arm has never been seen is not a preflight — and a report
    //     that is `OK` everywhere by construction proves only that it prints.
    assert!(
        net_sb.contains("FAIL rendezvous") && net_sb.contains("connector add"),
        "with no connector configured, `net` must fail the rendezvous row and \
         say how to fix it. Got:\n{net_sb}"
    );

    phase_gate!(client, "2f");
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

    phase_gate!(client, "2f.1");
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

    phase_gate!(client, "2f.3");
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

    phase_gate!(client, "2f.2");
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

    phase_gate!(client, "2g");
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

    phase_gate!(client, "2h");
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

    phase_gate!(client, "2h.2");
    // -- Phase 2h.2: ONE launcher grid over both app-sets, with working chips --
    //
    // The JS-Apps platform shipped with ZERO e2e coverage: Phase 2's spawn loop
    // opened the launchers but only checked for panics. This pins the real
    // user-visible contract. On a plain boot (no origins registered) the single
    // Apps window seeds every set's baked demo token (`ensure_demo_set` per set,
    // from the factory) and renders one launcher grid — "War" from the `games`
    // set and "Calculator" from `apps`. We assert BOTH appear in ONE window (the
    // merge — before it, no single window could show both), that the category
    // chips filter what they claim to, and then launch War and confirm the
    // sandboxed iframe mounts with the bundle `srcdoc` — proving the catalog +
    // bundle round-trip out of the store (the two-hop) and that the app runs.
    //
    // Selector: each window's `section.window` has `<header><h3>{title}`; the
    // grid is one `<button>` card per catalog entry (the app name in a child
    // `<div>`) and the chip row is `button[data-chip]`, all in the single
    // `#dom-layer` shadow root. Chips are read by `data-chip` / `aria-pressed`
    // rather than by their inline style: which chip is active must be
    // recoverable without parsing colors back out of a style attribute.
    let read_apps = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Apps') continue;
            const chips = Array.from(sec.querySelectorAll('button[data-chip]'));
            return {
                found: true,
                cards: Array.from(sec.querySelectorAll('button'))
                    .filter(b => !b.hasAttribute('data-chip'))
                    .map(b => b.textContent.trim()),
                chips: chips.map(c => c.getAttribute('data-chip')),
                active: chips.filter(c => c.getAttribute('aria-pressed') === 'true')
                    .map(c => c.getAttribute('data-chip')),
            };
        }
        return { found: false };
    "#;
    let grid = client.execute(read_apps, vec![]).await?;
    assert!(
        grid.get("found").and_then(|v| v.as_bool()).unwrap_or(false),
        "Phase 2h.2: no window titled 'Apps' — the merged launcher should be open \
         from Phase 2's spawn loop. Got: {grid:?}"
    );
    let cards_hold = |g: &serde_json::Value, name: &str| {
        g.get("cards")
            .and_then(|v| v.as_array())
            .map(|cards| {
                cards
                    .iter()
                    .any(|c| c.as_str().map(|s| s.contains(name)).unwrap_or(false))
            })
            .unwrap_or(false)
    };
    assert!(
        cards_hold(&grid, "War") && cards_hold(&grid, "Calculator"),
        "The ONE Apps launcher must show both sets' baked demos — 'War' (games) \
         and 'Calculator' (apps). Two windows became one; if only one of these \
         is present the merged grid is reading a single set. Got: {grid:?}"
    );

    // The chip row is derived from what the catalogs actually carry, so an empty
    // category renders no chip at all. War is `cards` -> Games, Calculator is
    // `utility` -> Tools, Ping publishes no category -> Other.
    let chips: Vec<String> = grid
        .get("chips")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|c| c.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        chips,
        vec!["all", "games", "tools", "other"],
        "Chip row should be exactly the non-empty coarse categories, in order. \
         An 'art' or 'music' chip here would show an empty grid when pressed. \
         Got: {grid:?}"
    );
    assert_eq!(
        grid.get("active").and_then(|v| v.as_array()).map(|a| a.len()),
        Some(1),
        "Exactly one chip may be active, and on first paint it is 'all'. Got: {grid:?}"
    );

    // Press "Games": the filter must actually filter. An exit code cannot tell
    // "filtered correctly" from "filtered correctly and rendered nowhere", so
    // this reads the cards back out of the DOM (AP25).
    let clicked_chip = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const chip = sec.querySelector('button[data-chip="games"]');
                if (!chip) return 'no-games-chip';
                chip.click();
                return 'clicked';
            }
            return 'no-apps-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        clicked_chip.as_str().unwrap_or(""),
        "clicked",
        "Games chip click failed: {clicked_chip:?}"
    );
    let filtered = poll_json(&client, read_apps, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("active")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().any(|c| c.as_str() == Some("games")))
            .unwrap_or(false)
    })
    .await?;
    assert!(
        cards_hold(&filtered, "War"),
        "Filtering to Games must keep the game. Got: {filtered:?}"
    );
    assert!(
        !cards_hold(&filtered, "Calculator"),
        "Filtering to Games must drop the tool — a chip that changes only its own \
         highlight is a control that does nothing. Got: {filtered:?}"
    );

    // Back to All, so the rest of this phase (and 2h.2b's Ping launch) sees the
    // whole grid. The selected chip is persisted view-state, not a render-local
    // toggle, so leaving it filtered would leak into every later phase.
    let _ = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const chip = sec.querySelector('button[data-chip="all"]');
                if (chip) chip.click();
            }
            return true;
            "#,
            vec![],
        )
        .await?;
    let restored = poll_json(&client, read_apps, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("active")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().any(|c| c.as_str() == Some("all")))
            .unwrap_or(false)
    })
    .await?;
    assert!(
        cards_hold(&restored, "Calculator"),
        "Clearing the filter must bring every app back. Got: {restored:?}"
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
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => !b.hasAttribute('data-chip') && b.textContent.includes('War'));
                if (!card) return 'no-war-card';
                card.click();
                return 'clicked';
            }
            return 'no-apps-window';
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
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
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
        "Launching War should mount a sandboxed iframe in the Apps window. \
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
        "Phase 2h.2: one Apps grid renders both sets' demos, the Games chip \
         filters to the game only, and War launches into a sandboxed iframe \
         (srcdoc {srcdoc_len} bytes)"
    );

    // Return the merged launcher to its grid. Before the merge, Games and Apps
    // were separate windows and launching a game left the Apps grid untouched;
    // they are the SAME window now, so 2h.2b's Ping card is behind this click.
    let war_back = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const b = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                if (!b) return 'no-back';
                b.click();
                return 'back';
            }
            return 'no-apps-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        war_back.as_str().unwrap_or(""),
        "back",
        "Back-to-grid after War failed: {war_back:?}"
    );
    sleep(Duration::from_millis(400)).await;

    phase_gate!(client, "2h.2s");
    // -- Phase 2h.2s: the Saves panel is the window's third view --
    //
    // Back up / restore / send are proven natively and mutation-checked
    // (`a_save_can_be_snapshotted_rolled_back_and_the_snapshot_dropped`). What
    // no native test can see is the WIRING: that the launcher offers a way in,
    // that the panel replaces the window body rather than rendering nowhere,
    // and that leaving it returns to the grid. A view reachable only from code
    // is a feature nobody has.
    //
    // Deliberately asserts nothing about WHICH saves are listed: whether the
    // demo game wrote one during 2h.2 depends on the fixture's own behaviour,
    // and a gate that depends on that is a flake waiting for a fixture edit.
    let saves_panel = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const entry = sec.querySelector('button[data-control="saves"]');
                if (!entry) return { entry: false };
                entry.click();
                return { entry: true };
            }
            return { entry: false };
            "#,
            vec![],
        )
        .await?;
    assert!(
        saves_panel.get("entry").and_then(|v| v.as_bool()).unwrap_or(false),
        "The launcher must offer a way into the Saves panel. Got: {saves_panel:?}"
    );

    let read_panel = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Apps') continue;
            return {
                // The panel replaces the grid: its own back button is present,
                // and the launcher's chips and cards are gone.
                back: !!Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←')),
                chips: sec.querySelectorAll('button[data-chip]').length,
                entry: sec.querySelectorAll('button[data-control="saves"]').length,
                // Count the grid's CARDS, not the app's name in the text: the
                // panel legitimately lists a save named after the app that
                // wrote it, so "does 'War' appear anywhere" cannot tell
                // "the grid is gone" from "the save is listed". (Found by this
                // very assertion failing on its first run.)
                cards: sec.querySelectorAll('button.app-card').length,
            };
        }
        return { back: false };
    "#;
    let panel = poll_json(&client, read_panel, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("back").and_then(|b| b.as_bool()).unwrap_or(false)
    })
    .await?;
    assert!(
        panel.get("back").and_then(|v| v.as_bool()).unwrap_or(false),
        "The Saves panel should render with its own way back. Got: {panel:?}"
    );
    assert_eq!(
        panel.get("chips").and_then(|v| v.as_u64()),
        Some(0),
        "The panel REPLACES the launcher — chips still showing means it rendered \
         under the grid rather than instead of it. Got: {panel:?}"
    );
    assert_eq!(
        panel.get("cards").and_then(|v| v.as_u64()),
        Some(0),
        "…and the launcher's app cards are gone with them. Got: {panel:?}"
    );

    let _ = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const b = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                if (b) b.click();
            }
            return true;
            "#,
            vec![],
        )
        .await?;
    let back_to_grid = poll_json(&client, read_apps, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("chips")
            .and_then(|c| c.as_array())
            .map(|c| !c.is_empty())
            .unwrap_or(false)
    })
    .await?;
    assert!(
        cards_hold(&back_to_grid, "War"),
        "Leaving the Saves panel must put the launcher back. Got: {back_to_grid:?}"
    );
    eprintln!("Phase 2h.2s: the Saves panel opens over the launcher and hands it back");

    phase_gate!(client, "2h.2b");
    // -- Phase 2h.2b: L5 app-hosting — a WASM entity-peer payload in an iframe --
    //
    // The first L5 app (review §4): browser-rust booted in stripped `?app-host=`
    // mode inside the SAME sandboxed-iframe host path the JS apps use. This is the
    // delivery + boot + ③α-handshake proof (P1) — the sandboxed iframe must fetch
    // this wasm, instantiate it, run start(), and complete the entity-apps handshake
    // with the outer host. We can't read into the iframe's document, so we assert on
    // what crosses the boundary: it is delivered by `src` (`index.html?app-host=ping`),
    // NOT srcdoc (a multi-MB wasm can't inline), sandboxed `allow-scripts
    // allow-same-origin` (the trusted payload loads its own wasm without opaque-origin
    // CORS/CSP friction), and the host stamps `data-host-locale` on it — which it does
    // ONLY in reply to the payload's `ready-for-init`. A non-empty value therefore
    // proves the wasm booted in the iframe and spoke the contract. (An empty value
    // means the payload never came up — the delivery/CSP failure mode.)
    let l5_launched = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.includes('Ping'));
                if (!card) return 'no-ping-card';
                card.click();
                return 'clicked';
            }
            return 'no-apps-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        l5_launched.as_str().unwrap_or(""),
        "clicked",
        "Ping (L5) card click failed: {l5_launched:?}"
    );

    // A full wasm boot inside the iframe: give it time to fetch + instantiate +
    // run start() + post ready-for-init (host replies, stamping data-host-locale).
    let mut l5 = serde_json::Value::Null;
    for _ in 0..40 {
        sleep(Duration::from_millis(200)).await;
        l5 = client
            .execute(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                for (const sec of root.querySelectorAll('section.window')) {
                    const h3 = sec.querySelector('header h3');
                    if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                    const fr = sec.querySelector('iframe[sandbox]');
                    if (!fr) return { found: false };
                    return {
                        found: true,
                        sandbox: fr.getAttribute('sandbox'),
                        src: fr.getAttribute('src') || '',
                        srcdoc_len: (fr.getAttribute('srcdoc') || '').length,
                        host_locale: fr.getAttribute('data-host-locale'),
                    };
                }
                return { found: false };
                "#,
                vec![],
            )
            .await?;
        if l5
            .get("host_locale")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            break;
        }
    }
    assert!(
        l5.get("found").and_then(|v| v.as_bool()).unwrap_or(false),
        "Launching Ping (L5) should mount a sandboxed iframe in the Apps window. Got: {l5:?}"
    );
    assert_eq!(
        l5.get("sandbox").and_then(|v| v.as_str()).unwrap_or(""),
        "allow-scripts allow-same-origin",
        "L5 iframe must be sandboxed allow-scripts allow-same-origin — the trusted \
         payload needs same-origin to load its wasm without opaque-origin CORS/CSP \
         friction (regular JS apps stay opaque `allow-scripts`). Got: {l5:?}"
    );
    let l5_src = l5.get("src").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        l5_src.contains("app-host=ping"),
        "L5 app must be delivered via src=index.html?app-host=ping, not srcdoc. Got src={l5_src:?}, {l5:?}"
    );
    assert_eq!(
        l5.get("srcdoc_len").and_then(|v| v.as_u64()).unwrap_or(0),
        0,
        "L5 app must NOT carry srcdoc (it can't inline a multi-MB wasm). Got: {l5:?}"
    );
    let l5_host_locale = l5.get("host_locale").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        !l5_host_locale.is_empty(),
        "The host must stamp data-host-locale on the L5 iframe in reply to the payload's \
         ready-for-init — proving the wasm booted in the opaque-origin sandbox and spoke the \
         ③α contract. Empty means the payload never came up (delivery / CORS / CSP). Got: {l5:?}"
    );
    eprintln!(
        "Phase 2h.2b: L5 app booted in a sandboxed src-iframe and completed the \
         entity-apps handshake (host locale '{l5_host_locale}')"
    );

    phase_gate!(client, "2h.2c");
    // -- Phase 2h.2c: L5 compute — Life runs in the iframe peer, state advances --
    //
    // The first *real* L5 app: browser-rust runs the generic compute host behind
    // the boundary. We launch Life from the PRODUCTION Programs launcher (the L5
    // demo-app duplicates were dropped — one honest "run a program" surface) and
    // assert both halves of the proof:
    //   (a) delivery + handshake — src=index.html?app-host=life and the host stamps
    //       data-host-locale (the wasm booted in the opaque-origin sandbox), and
    //   (b) the host's `data-app-state-seq` climbs past 1 — the inner peer emitted
    //       multiple DISTINCT states (the payload dedups by content hash), i.e. Life
    //       actually ADVANCED inside its own peer, not merely booted.
    // We cannot read into the sandboxed document, so the growing seq IS the
    // cross-boundary proof that compute ran behind the boundary.

    // Reset the Apps window (the Ping player from 2h.2b) back to its launcher
    // grid — it is otherwise untouched now, and Phase 2h.3's tail launches
    // Calculator from that grid. Life/Snake/Asteroids run through the PRODUCTION
    // Programs launcher below (the demo-app duplicates were dropped — one honest
    // "run a program" surface, the same `?app-host=<key>` iframe delivery).
    let l5_back = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const b = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                if (!b) return 'no-back';
                b.click();
                return 'back';
            }
            return 'no-apps';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(l5_back.as_str().unwrap_or(""), "back", "Apps back-to-grid click failed: {l5_back:?}");
    sleep(Duration::from_millis(300)).await;

    let life_launched = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.includes('Life'));
                if (!card) return 'no-life-card';
                card.click();
                return 'clicked';
            }
            return 'no-programs-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(life_launched.as_str().unwrap_or(""), "clicked", "Life (L5) tile click failed: {life_launched:?}");

    // Boot the wasm + mount Life + run enough ticks to emit ≥2 distinct states.
    let mut life = serde_json::Value::Null;
    for _ in 0..60 {
        sleep(Duration::from_millis(200)).await;
        life = client
            .execute(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                for (const sec of root.querySelectorAll('section.window')) {
                    const h3 = sec.querySelector('header h3');
                    if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                    const fr = sec.querySelector('iframe[sandbox]');
                    if (!fr) return { found: false };
                    // Reach into the same-origin L5 iframe for the rendered display —
                    // Life is now a filled display-list grid, not a <pre> terminal.
                    let display = null, drawn = -1, filled = false;
                    try {
                        const doc = fr.contentDocument;
                        const svg = doc && doc.querySelector('[data-program-display="display-list"]');
                        if (svg) {
                            display = 'display-list';
                            drawn = parseInt(svg.getAttribute('data-actor-count') || '0', 10);
                            const poly = svg.querySelector('polygon');
                            const f = poly && poly.getAttribute('fill');
                            filled = !!f && f !== 'none';
                        }
                    } catch (e) {}
                    return {
                        found: true,
                        sandbox: fr.getAttribute('sandbox'),
                        src: fr.getAttribute('src') || '',
                        host_locale: fr.getAttribute('data-host-locale'),
                        state_seq: parseInt(fr.getAttribute('data-app-state-seq') || '0', 10),
                        display, drawn, filled,
                    };
                }
                return { found: false };
                "#,
                vec![],
            )
            .await?;
        let seq = life.get("state_seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let handshaked = life
            .get("host_locale")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let painted = life.get("display").and_then(|v| v.as_str()) == Some("display-list")
            && life.get("drawn").and_then(|v| v.as_i64()).unwrap_or(0) >= 1;
        if handshaked && seq >= 2 && painted {
            break;
        }
    }
    assert!(
        life.get("found").and_then(|v| v.as_bool()).unwrap_or(false),
        "Launching Life (L5) should mount a sandboxed iframe in the Programs window. Got: {life:?}"
    );
    let life_src = life.get("src").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        life_src.contains("app-host=life"),
        "Life (L5) must be delivered via src=index.html?app-host=life. Got src={life_src:?}, {life:?}"
    );
    assert!(
        life.get("host_locale").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false),
        "The host must stamp data-host-locale on the Life iframe (the wasm booted + handshook). Got: {life:?}"
    );
    let life_seq = life.get("state_seq").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        life_seq >= 2,
        "The host must see ≥2 DISTINCT state emissions from the Life payload \
         (data-app-state-seq) — proving the compute host ADVANCED Life inside its own \
         sandboxed peer, behind the boundary, not merely booted. Got seq={life_seq}, {life:?}"
    );
    // Render contract: Life was rebound text → display-list. It must render as a
    // display-list SVG (a grid, not a <pre> terminal), with ≥1 quad actually
    // DRAWN (the dense grid carries every cell as a kind-0 background quad, which
    // the host MUST skip — a non-zero drawn count proves the skip works and the
    // board isn't painted solid), and those quads FILLED (scene.render=fill
    // honored — solid coloured cells, not wireframe).
    assert_eq!(
        life.get("display").and_then(|v| v.as_str()),
        Some("display-list"),
        "Life must render as a display-list grid (rebound from text). Got: {life:?}"
    );
    let life_drawn = life.get("drawn").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        life_drawn >= 1,
        "Life's display-list must draw ≥1 live cell with kind-0 background quads SKIPPED \
         (dense grid; drawing kind 0 paints the whole board). Got drawn={life_drawn}, {life:?}"
    );
    assert_eq!(
        life.get("filled").and_then(|v| v.as_bool()),
        Some(true),
        "Life's cells must be FILLED, not wireframe (scene.render=fill honored). Got: {life:?}"
    );
    eprintln!(
        "Phase 2h.2c: Life ran on the compute host inside a sandboxed L5 iframe-peer \
         ({life_seq} distinct evolved states, P1) and now renders as a FILLED display-list \
         grid ({life_drawn} live cells drawn, kind-0 background skipped) — not a <pre> terminal"
    );

    // -- Program chrome: program-owned status caption + generic reset/pause --
    // RESPONSE-PROGRAM-CHROME-STATUS-AND-RESET, adopted browser-side. Three
    // proofs, all read INSIDE the same-origin sandbox:
    //   (a) the program-owned `status` port renders as a caption (`POP NNNN ▶`)
    //       via the text driver — bytes the host relays blind, never formats;
    //   (b) PAUSE gates the clock — while paused the inner peer emits no new
    //       distinct state, so the host's `data-app-state-seq` freezes; then
    //   (c) RESET reseeds to state₀ WHILE PAUSED — a reseed is a state change, so
    //       it emits exactly once (seq bumps past the frozen baseline) even with
    //       the clock stopped; nothing else can advance a paused sim. (The reseed's
    //       tick-for-tick correctness is pinned natively by
    //       `oracle_tests::reseed_replays_deterministically`; here we prove the
    //       button is wired to it.)
    // A tiny JS helper reads the Programs iframe's status + controls + host seq.
    let read_chrome = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
            const fr = sec.querySelector('iframe[sandbox]');
            const doc = fr && fr.contentDocument;
            if (!doc) return { err: 'no-doc' };
            const s = doc.querySelector('[data-app-host-status]');
            const reset = doc.querySelector('[data-host-reset]');
            const pause = doc.querySelector('[data-host-pause]');
            const dbgPanel = doc.querySelector('[data-app-host-debug]');
            return {
                status: s ? s.textContent.trim() : null,
                has_reset: !!reset, has_pause: !!pause,
                paused: pause ? pause.getAttribute('data-host-paused') : null,
                seq: parseInt(fr.getAttribute('data-app-state-seq') || '0', 10),
                has_step: !!doc.querySelector('[data-host-step]'),
                has_debug_chip: !!doc.querySelector('[data-debug-toggle]'),
                debug_mode: dbgPanel ? dbgPanel.getAttribute('data-mode') : null,
                // The topology (wiring/relationship) and live tree dump used to be
                // two separately-refreshed panes read by two selectors; they're now
                // ONE merged per-row view (name/path/shape/relationship + live value
                // together, debug.rs `WiringRow`) under a single summary line — read
                // the whole panel's textContent, which carries both the static
                // per-row wiring (built at panel-construction time) and the live
                // values (patched in place by `debug::refresh`).
                debug_text: dbgPanel ? dbgPanel.textContent : '',
            };
        }
        return { err: 'no-window' };
    "#;
    let click_ctl = |sel: &str| {
        format!(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {{
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const doc = sec.querySelector('iframe[sandbox]').contentDocument;
                const b = doc.querySelector('{sel}');
                if (!b) return 'no-button';
                b.click();
                return 'clicked';
            }}
            return 'no-window';
            "#
        )
    };

    // (a) The program-owned status caption renders, and both host controls exist.
    let chrome = client.execute(read_chrome, vec![]).await?;
    let status0 = chrome.get("status").and_then(|v| v.as_str()).unwrap_or("");
    // `POP NNNN ▶` (alive) or `POP NNNN ✖` (extinct) — Life may have died out over
    // the run, so accept either run-state glyph; the point is the program-owned
    // caption renders via the text driver (the host relays the bytes blind).
    let well_formed_status =
        |s: &str| s.contains("POP") && (s.contains('\u{25B6}') || s.contains('\u{2716}'));
    assert!(
        well_formed_status(status0),
        "Life must render its program-owned status caption `POP NNNN <glyph>` via the text \
         driver (the host relays the program's bytes blind). Got: {chrome:?}"
    );
    assert_eq!(
        chrome.get("has_reset").and_then(|v| v.as_bool()),
        Some(true),
        "The reset (↻) control must be present. Got: {chrome:?}"
    );
    assert_eq!(
        chrome.get("has_pause").and_then(|v| v.as_bool()),
        Some(true),
        "The pause (⏸) control must be present. Got: {chrome:?}"
    );

    // (b) Pause, let any in-flight tick settle, then confirm the clock is frozen.
    let paused = client.execute(&click_ctl("[data-host-pause]"), vec![]).await?;
    assert_eq!(paused.as_str(), Some("clicked"), "pause click failed: {paused:?}");
    sleep(Duration::from_millis(500)).await; // let the in-flight tick complete
    let baseline = client.execute(read_chrome, vec![]).await?;
    assert_eq!(
        baseline.get("paused").and_then(|v| v.as_str()),
        Some("1"),
        "The pause control must report data-host-paused=1. Got: {baseline:?}"
    );
    let seq_baseline = baseline.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
    sleep(Duration::from_millis(1000)).await;
    let frozen = client.execute(read_chrome, vec![]).await?;
    let seq_frozen = frozen.get("seq").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert_eq!(
        seq_frozen, seq_baseline,
        "A PAUSED compute program must emit no new states — data-app-state-seq must not climb \
         while paused. baseline={seq_baseline}, after 1s={seq_frozen}, {frozen:?}"
    );

    // (c) Reset WHILE paused → the reseed emits state₀ exactly once (seq climbs
    // past the frozen baseline); the caption is re-rendered and still well-formed.
    let reset_click = client.execute(&click_ctl("[data-host-reset]"), vec![]).await?;
    assert_eq!(reset_click.as_str(), Some("clicked"), "reset click failed: {reset_click:?}");
    sleep(Duration::from_millis(600)).await;
    let after_reset = client.execute(read_chrome, vec![]).await?;
    let seq_reset = after_reset.get("seq").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert!(
        seq_reset > seq_baseline,
        "RESET while PAUSED must reseed to state₀ and emit it — the host's seq must climb past \
         the frozen baseline (nothing else can advance a paused sim). baseline={seq_baseline}, \
         after reset={seq_reset}, {after_reset:?}"
    );
    let status_reset = after_reset.get("status").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        well_formed_status(status_reset),
        "The status caption must still be a well-formed `POP NNNN <glyph>` line after reset. Got: {after_reset:?}"
    );

    // (d) The 🐞 debug overlay: present on every program (host-owned, no
    // program knowledge), hidden by default, and — while still PAUSED from
    // (c) above — a topology panel from the descriptor + a live tree dump,
    // plus single-STEP advancing exactly one tick and no further.
    assert_eq!(
        after_reset.get("has_debug_chip").and_then(|v| v.as_bool()),
        Some(true),
        "The 🐞 debug toggle chip must be present. Got: {after_reset:?}"
    );
    assert_eq!(
        after_reset.get("has_step").and_then(|v| v.as_bool()),
        Some(true),
        "The ⏭ step control must be present. Got: {after_reset:?}"
    );
    assert_eq!(
        after_reset.get("debug_mode").and_then(|v| v.as_str()),
        Some("hidden"),
        "The debug panel must be hidden by default. Got: {after_reset:?}"
    );
    let debug_toggled = client.execute(&click_ctl("[data-debug-toggle]"), vec![]).await?;
    assert_eq!(debug_toggled.as_str(), Some("clicked"), "debug toggle click failed: {debug_toggled:?}");
    let shown = client.execute(read_chrome, vec![]).await?;
    assert_eq!(
        shown.get("debug_mode").and_then(|v| v.as_str()),
        Some("shown"),
        "Clicking the 🐞 chip must flip the panel to shown. Got: {shown:?}"
    );
    let wiring = shown.get("debug_text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        wiring.contains("app/life/state"),
        "The debug panel must render the descriptor's own wiring (state_path), free of any \
         evaluation. Got: {wiring:?}"
    );
    // The live values refresh on the NEXT tick/reset (the loop only pays for
    // `tree_listing` while shown) — reset once more to force one, well within
    // the still-paused window so nothing else advances the clock.
    let redo_reset = client.execute(&click_ctl("[data-host-reset]"), vec![]).await?;
    assert_eq!(redo_reset.as_str(), Some("clicked"), "second reset click failed: {redo_reset:?}");
    sleep(Duration::from_millis(400)).await;
    let with_tree = client.execute(read_chrome, vec![]).await?;
    let tree = with_tree.get("debug_text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        tree.contains("entities under /") && tree.contains("app/life/state"),
        "The live wiring view must list the program's namespace and decode the dynamic state path \
         inline. Got: {tree:?}"
    );
    let seq_before_step = with_tree.get("seq").and_then(|v| v.as_i64()).unwrap_or(-1);
    let step_click = client.execute(&click_ctl("[data-host-step]"), vec![]).await?;
    assert_eq!(step_click.as_str(), Some("clicked"), "step click failed: {step_click:?}");
    sleep(Duration::from_millis(400)).await;
    let after_step = client.execute(read_chrome, vec![]).await?;
    let seq_after_step = after_step.get("seq").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert!(
        seq_after_step > seq_before_step,
        "STEP while paused must advance exactly one tick — the host's seq must climb past the \
         paused baseline. baseline={seq_before_step}, after step={seq_after_step}, {after_step:?}"
    );
    sleep(Duration::from_millis(700)).await;
    let still_paused = client.execute(read_chrome, vec![]).await?;
    let seq_settled = still_paused.get("seq").and_then(|v| v.as_i64()).unwrap_or(-1);
    assert_eq!(
        seq_settled, seq_after_step,
        "STEP must advance exactly ONE tick, not resume the clock — seq must hold after the step. \
         after step={seq_after_step}, 700ms later={seq_settled}, {still_paused:?}"
    );

    eprintln!(
        "Phase 2h.2c: program chrome — status caption `{status0}` (program-owned, host-relayed \
         via text_driver), PAUSE froze the clock (seq {seq_baseline} held over 1s), RESET reseeded \
         while paused (seq {seq_baseline}→{seq_reset}); generic controls, program-blind. Debug \
         overlay: topology from the descriptor, live tree dump (namespace + decoded state), STEP \
         advanced exactly one tick ({seq_before_step}→{seq_after_step}) and held there"
    );

    // -- Tick-loop fault resilience (AUDIT-L5-COMPUTE-HOST-FOUNDATION #1) -------
    // A faulting tick must DEGRADE to a visible caption + a stopped clock, never
    // a silently frozen board (D13/AP3). Navigate the SAME iframe to the fault
    // seam (`&app-host-fault-tick=2`): Life boots, runs one real tick, then the
    // 2nd tick injects a recoverable fault through the identical
    // `guarded → Err → visible-surface → break` path the real (release-only)
    // panic containment uses. (A REAL panic can't be asserted here — the e2e
    // dist is a dev/abort build where a wasm panic aborts the module;
    // `program_host::host::guarded`'s native unit test covers the true
    // panic→Err catch. This proves the user-facing surface.) The fault reason
    // lands on `[data-app-host-fault]` and the caption reads `… stopped — …`;
    // crucially the iframe document is still ALIVE (queryable), not blank/dead.
    let fault_nav = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const fr = sec.querySelector('iframe[sandbox]');
                if (!fr) return 'no-iframe';
                const base = fr.getAttribute('src').split('&')[0];
                fr.setAttribute('src', base + '&app-host-fault-tick=2');
                return 'navigated';
            }
            return 'no-programs';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(fault_nav.as_str().unwrap_or(""), "navigated", "fault-seam nav: {fault_nav:?}");
    // Boot + 1 tick + the injected fault — generous for Life's rate on the
    // in-memory Direct app-host peer.
    sleep(Duration::from_millis(2500)).await;
    let fault = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const fr = sec.querySelector('iframe[sandbox]');
                const doc = fr && fr.contentDocument;
                if (!doc) return { err: 'no-doc' };
                const app = doc.querySelector('[data-app-host]');
                const board = doc.querySelector('[data-app-host-display]');
                // The iframe document being queryable at all proves it did NOT
                // abort/reload to a blank frame. Read the DISPLAY element's text
                // (not doc.body — that leads with the injected <style> CSS).
                return {
                    alive: !!app,
                    fault: app ? app.getAttribute('data-app-host-fault') : null,
                    caption: board ? board.textContent.trim() : null,
                };
            }
            return { err: 'no-programs' };
            "#,
            vec![],
        )
        .await?;
    let fault_reason = fault.get("fault").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(
        fault.get("alive").and_then(|v| v.as_bool()),
        Some(true),
        "A faulting tick must not blank/abort the iframe — the payload root must still be present. {fault:?}"
    );
    assert!(
        fault_reason.contains("injected fault"),
        "A faulting tick must surface a visible fault reason on [data-app-host-fault], not freeze silently. Got {fault:?}"
    );
    assert!(
        fault.get("caption").and_then(|v| v.as_str()).unwrap_or("").contains("stopped"),
        "The faulted board must show a visible `… stopped — …` caption (t(apphost.stopped)), not a frozen last frame. Got {fault:?}"
    );
    eprintln!(
        "Phase 2h.2c: tick-loop fault resilience — an injected tick fault DEGRADED to a visible \
         `stopped` caption + [data-app-host-fault={fault_reason:?}], iframe still alive (not a \
         silent frozen board); real panic→Err containment covered natively by host::guarded"
    );

    // Leave the Programs window back on its launcher grid for the next program
    // (each phase leaves state as its successors expect). Without this, the Life
    // player stays up and 2h.2d sees no Snake tile.
    let l5_reset = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const b = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                if (!b) return 'no-back';
                b.click();
                return 'back';
            }
            return 'no-programs';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(l5_reset.as_str().unwrap_or(""), "back", "Programs reset-to-grid after Life failed: {l5_reset:?}");
    sleep(Duration::from_millis(300)).await;

    phase_gate!(client, "2h.2d");
    // -- Phase 2h.2d: L5 INPUT — Snake runs behind the boundary AND steers ----
    //
    // The second real L5 app and the first INPUT-driven one. Snake is pure-builtin
    // (runs on wasm today — the `compute/apply` stub gates only import-bearing
    // programs) and binds the `direction` input shape. This phase proves the two
    // things the native oracle test CANNOT (it drives inputs directly, never the
    // wasm/iframe delivery path):
    //   (a) a SECOND program mounts + advances behind the boundary (state_seq
    //       climbs past 1, exactly as Life) — admission accepted `text`+`direction`,
    //   (b) a real keydown delivered into the sandboxed (same-origin) iframe is
    //       captured, encoded from the SEED's field name, and stamped on the
    //       payload's D13 surface (`data-app-host-input`). That surface is inside
    //       the iframe and never crosses ③α — the host stays blind (P1). The
    //       oracle already proves input→compute agreement tick-for-tick; this
    //       proves the capture→encode→write path exists on the shipped surface.
    let snake_launched = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.includes('Snake'));
                if (!card) return 'no-snake-card';
                card.click();
                return 'clicked';
            }
            return 'no-programs-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(snake_launched.as_str().unwrap_or(""), "clicked", "Snake (L5) tile click failed: {snake_launched:?}");

    // Boot the wasm + mount Snake + run enough ticks to emit ≥2 distinct states.
    let mut snake = serde_json::Value::Null;
    for _ in 0..60 {
        sleep(Duration::from_millis(200)).await;
        snake = client
            .execute(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                for (const sec of root.querySelectorAll('section.window')) {
                    const h3 = sec.querySelector('header h3');
                    if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                    const fr = sec.querySelector('iframe[sandbox]');
                    if (!fr) return { found: false };
                    return {
                        found: true,
                        sandbox: fr.getAttribute('sandbox'),
                        src: fr.getAttribute('src') || '',
                        host_locale: fr.getAttribute('data-host-locale'),
                        state_seq: parseInt(fr.getAttribute('data-app-state-seq') || '0', 10),
                    };
                }
                return { found: false };
                "#,
                vec![],
            )
            .await?;
        let seq = snake.get("state_seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let handshaked = snake
            .get("host_locale")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if handshaked && seq >= 2 {
            break;
        }
    }
    assert!(
        snake.get("found").and_then(|v| v.as_bool()).unwrap_or(false),
        "Launching Snake (L5) should mount a sandboxed iframe in the Programs window. Got: {snake:?}"
    );
    let snake_src = snake.get("src").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        snake_src.contains("app-host=snake"),
        "Snake (L5) must be delivered via src=index.html?app-host=snake. Got src={snake_src:?}, {snake:?}"
    );
    let snake_seq = snake.get("state_seq").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        snake_seq >= 2,
        "The host must see ≥2 DISTINCT state emissions from the Snake payload — the \
         compute host ADVANCED Snake behind the boundary. Got seq={snake_seq}, {snake:?}"
    );

    // (b) The input proof: deliver a real ArrowUp keydown into the same-origin
    // sandbox and read the payload's D13 input surface back. `allow-same-origin`
    // makes contentWindow/contentDocument reachable; the driver listens on the
    // payload window, so a KeyboardEvent dispatched there fires it. We poll
    // because the driver is installed after seed (a couple seconds in).
    let mut input_stamp = String::new();
    for _ in 0..30 {
        let probe = client
            .execute(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                for (const sec of root.querySelectorAll('section.window')) {
                    const h3 = sec.querySelector('header h3');
                    if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                    const fr = sec.querySelector('iframe[sandbox]');
                    if (!fr) return 'no-iframe';
                    const win = fr.contentWindow, doc = fr.contentDocument;
                    if (!win || !doc) return 'no-same-origin';
                    // Deliver ArrowUp to the payload window (the driver's target).
                    win.dispatchEvent(new win.KeyboardEvent('keydown', { key: 'ArrowUp', bubbles: true }));
                    const el = doc.querySelector('[data-app-host]');
                    return el ? (el.getAttribute('data-app-host-input') || '') : 'no-root';
                }
                return 'no-programs-window';
                "#,
                vec![],
            )
            .await?;
        let s = probe.as_str().unwrap_or("").to_string();
        if s.starts_with("dir:") {
            input_stamp = s;
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(
        input_stamp, "dir:0",
        "Delivering ArrowUp into the Snake payload must capture the key, encode it from \
         the seed's field name (`dir`), and stamp the D13 input surface `dir:0` (DIR_UP). \
         Got: {input_stamp:?} — the in-iframe input driver did not run"
    );

    // (c) The SECOND input source — the on-screen D-pad. It drives the SAME
    // `InputTarget` as the keyboard (one shared state), proving "a second source,
    // not a second code path" on the shipped surface. The pad is now a floating
    // gamepad HUD with AUTO visibility (shown on touch, hidden on a precise-
    // pointer desktop) + an always-present 🎮 chip. Headless is desktop-ish, so
    // we drive it to a known state via the chip rather than assuming a default.
    // We: (i) force the pad SHOWN via the chip and confirm it renders; (ii)
    // dispatch a pointerdown on the Right pad button and read the D13 surface
    // back as `dir:1` (DIR_RIGHT) — a DIFFERENT value than the keyboard's `dir:0`,
    // so only the pad wrote it; and (iii) click the chip again and confirm it
    // toggles the pad OFF (computed display → none).
    let pad_probe = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const fr = sec.querySelector('iframe[sandbox]');
                if (!fr) return { err: 'no-iframe' };
                const win = fr.contentWindow, doc = fr.contentDocument;
                if (!win || !doc) return { err: 'no-same-origin' };
                const pad = doc.querySelector('[data-app-host-controls="direction"]');
                if (!pad) return { err: 'no-pad' };
                const toggle = doc.querySelector('[data-controls-toggle]');
                if (!toggle) return { err: 'no-toggle' };
                const shown = () => win.getComputedStyle(pad).display !== 'none';
                // (i) drive to SHOWN (auto default is device-dependent).
                if (!shown()) toggle.click();
                const can_show = shown();
                // (ii) tap the Right pad button.
                const right = doc.querySelector('[data-control="dir:1"]');
                if (!right) return { err: 'no-right-button' };
                right.dispatchEvent(new win.PointerEvent('pointerdown', { bubbles: true }));
                const el = doc.querySelector('[data-app-host]');
                const stamp = el ? (el.getAttribute('data-app-host-input') || '') : 'no-root';
                // (iii) chip toggles the pad OFF.
                toggle.click();
                return { stamp, can_show, toggles_off: !shown() };
            }
            return { err: 'no-programs-window' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        pad_probe.get("can_show").and_then(|v| v.as_bool()),
        Some(true),
        "The on-screen pad must render (computed display != none) once shown via the 🎮 chip. \
         Got: {pad_probe:?}"
    );
    assert_eq!(
        pad_probe.get("stamp").and_then(|v| v.as_str()).unwrap_or(""),
        "dir:1",
        "A pointerdown on the on-screen D-pad's Right button must drive the SAME target \
         as the keyboard and stamp `dir:1` (DIR_RIGHT). Got: {pad_probe:?} — the on-screen \
         input source did not run (or is not sharing the keyboard's target)"
    );
    assert_eq!(
        pad_probe.get("toggles_off").and_then(|v| v.as_bool()),
        Some(true),
        "The 🎮 chip must toggle the pad OFF (computed display → none). Got: {pad_probe:?}"
    );
    eprintln!(
        "Phase 2h.2d: Snake ran behind the boundary ({snake_seq} distinct states) and \
         steered on BOTH a real keydown (data-app-host-input={input_stamp}) AND the \
         on-screen D-pad (dir:1, same target); floating pad shows/hides via the 🎮 chip \
         (auto default per device); input captured in-iframe, host blind (P1)"
    );

    // Leave the Programs window back on its launcher grid for the next program.
    let snake_reset = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const b = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                if (!b) return 'no-back';
                b.click();
                return 'back';
            }
            return 'no-programs';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(snake_reset.as_str().unwrap_or(""), "back", "Programs reset-to-grid after Snake failed: {snake_reset:?}");
    sleep(Duration::from_millis(300)).await;

    phase_gate!(client, "2h.2e");
    // -- Phase 2h.2e: L5 vector display + held-key input — Asteroids -----------
    //
    // The third real L5 app exercises the two shapes Life/Snake don't: the
    // `display-list` OUTPUT (rendered as inline SVG behind the boundary) and the
    // `key-set` INPUT (a held-key bitmask). Asteroids is pure-builtin (its
    // descriptor declares no imports → runs on wasm). We prove:
    //   (a) it mounts + advances behind the boundary (state_seq climbs),
    //   (b) the display-list decoded and drew actors — the payload's SVG carries
    //       data-program-display="display-list" with data-actor-count ≥ 1, read
    //       across the same-origin boundary, and
    //   (c) a real ArrowRight is captured, mapped key→action→bit via the program's
    //       OWN scene keymap ("right"=bit 1 → mask 2), and stamped `keys:2` on the
    //       D13 input surface (in-iframe; never crosses ③α, P1).
    let ast_launched = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.includes('Asteroids'));
                if (!card) return 'no-asteroids-card';
                card.click();
                return 'clicked';
            }
            return 'no-programs-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(ast_launched.as_str().unwrap_or(""), "clicked", "Asteroids (L5) tile click failed: {ast_launched:?}");

    // Boot + mount + advance, and reach into the same-origin sandbox for the SVG.
    let mut ast = serde_json::Value::Null;
    for _ in 0..60 {
        sleep(Duration::from_millis(200)).await;
        ast = client
            .execute(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                for (const sec of root.querySelectorAll('section.window')) {
                    const h3 = sec.querySelector('header h3');
                    if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                    const fr = sec.querySelector('iframe[sandbox]');
                    if (!fr) return { found: false };
                    let actors = -1, display = null;
                    try {
                        const doc = fr.contentDocument;
                        const svg = doc && doc.querySelector('[data-program-display="display-list"]');
                        if (svg) { display = 'display-list'; actors = parseInt(svg.getAttribute('data-actor-count') || '0', 10); }
                    } catch (e) {}
                    return {
                        found: true,
                        src: fr.getAttribute('src') || '',
                        host_locale: fr.getAttribute('data-host-locale'),
                        state_seq: parseInt(fr.getAttribute('data-app-state-seq') || '0', 10),
                        display, actors,
                    };
                }
                return { found: false };
                "#,
                vec![],
            )
            .await?;
        let seq = ast.get("state_seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let handshaked = ast.get("host_locale").and_then(|v| v.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
        let actors = ast.get("actors").and_then(|v| v.as_i64()).unwrap_or(-1);
        if handshaked && seq >= 2 && actors >= 1 {
            break;
        }
    }
    let ast_src = ast.get("src").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        ast_src.contains("app-host=asteroids"),
        "Asteroids (L5) must be delivered via src=index.html?app-host=asteroids. Got src={ast_src:?}, {ast:?}"
    );
    let ast_seq = ast.get("state_seq").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        ast_seq >= 2,
        "The host must see ≥2 DISTINCT state emissions from Asteroids (it advanced behind the boundary). Got seq={ast_seq}, {ast:?}"
    );
    assert_eq!(
        ast.get("display").and_then(|v| v.as_str()),
        Some("display-list"),
        "Asteroids must render its `display-list` output as SVG inside the iframe. Got: {ast:?}"
    );
    let ast_actors = ast.get("actors").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        ast_actors >= 1,
        "The display-list driver must decode + draw ≥1 actor (data-actor-count). Got actors={ast_actors}, {ast:?}"
    );

    // (b2) The STANDARD CONTROLLER (read-only, before any input). Asteroids'
    // key-set port is now declared with control ROLES (rotate/thrust are
    // directional AXES, fire is a discrete ACTION — the re-declaration the
    // generic-host input contract asked for). So the host must present ONE
    // standard controller: the axes on the d-pad, the action as a glyphed button
    // — NOT four bespoke buttons. We read the panel structure without dispatching
    // (so the mask stays 0 for the accumulation test below).
    let ast_controller = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const fr = sec.querySelector('iframe[sandbox]');
                if (!fr) return { err: 'no-iframe' };
                const doc = fr.contentDocument;
                if (!doc) return { err: 'no-same-origin' };
                const panel = doc.querySelector('[data-app-host-controls="key-set"]');
                if (!panel) return { err: 'no-panel' };
                const fire = panel.querySelector('.ah-actions [data-control="press:fire"]');
                return {
                    // rotate + thrust are AXES → on the one d-pad (momentary).
                    right_axis_on_dpad: !!panel.querySelector('.ah-dpad [data-control="press:right"]'),
                    thrust_axis_on_dpad: !!panel.querySelector('.ah-dpad [data-control="press:up"]'),
                    // fire is an ACTION → a glyphed button in the action row.
                    fire_in_actions: !!fire,
                    fire_face: fire ? fire.textContent : '',
                };
            }
            return { err: 'no-programs-window' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        ast_controller.get("right_axis_on_dpad").and_then(|v| v.as_bool()),
        Some(true),
        "Asteroids' rotate-right is a directional AXIS — it must render on the standard \
         controller's d-pad (`.ah-dpad [data-control=press:right]`), not as a bespoke button. \
         Got: {ast_controller:?}"
    );
    assert_eq!(
        ast_controller.get("thrust_axis_on_dpad").and_then(|v| v.as_bool()),
        Some(true),
        "Asteroids' thrust is a directional AXIS (up) — it must render on the d-pad \
         (`.ah-dpad [data-control=press:up]`). Got: {ast_controller:?}"
    );
    assert_eq!(
        ast_controller.get("fire_in_actions").and_then(|v| v.as_bool()),
        Some(true),
        "Asteroids' fire is a discrete ACTION — it must render as a button in the action row \
         (`.ah-actions [data-control=press:fire]`). Got: {ast_controller:?}"
    );
    assert!(
        ast_controller
            .get("fire_face")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains('\u{1F525}'),
        "The Fire button must carry the program-declared glyph 🔥 (label+glyph from the scene \
         keymap, not a host guess). Got: {ast_controller:?}"
    );

    // (c) Held-key input: deliver ArrowRight and read the D13 input surface. The
    // key→bit mapping runs the arrows through the keyboard-position convention
    // (ArrowRight → the `right` axis position) and the program's OWN scene keymap
    // (right=bit 1 → mask 2), so `keys:2` proves the whole key-set path.
    let mut ast_stamp = String::new();
    for _ in 0..30 {
        let probe = client
            .execute(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                for (const sec of root.querySelectorAll('section.window')) {
                    const h3 = sec.querySelector('header h3');
                    if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                    const fr = sec.querySelector('iframe[sandbox]');
                    if (!fr) return 'no-iframe';
                    const win = fr.contentWindow, doc = fr.contentDocument;
                    if (!win || !doc) return 'no-same-origin';
                    win.dispatchEvent(new win.KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true }));
                    const el = doc.querySelector('[data-app-host]');
                    return el ? (el.getAttribute('data-app-host-input') || '') : 'no-root';
                }
                return 'no-programs-window';
                "#,
                vec![],
            )
            .await?;
        let s = probe.as_str().unwrap_or("").to_string();
        if s.starts_with("keys:") {
            ast_stamp = s;
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }
    assert_eq!(
        ast_stamp, "keys:2",
        "ArrowRight into Asteroids must map key→action→bit via the program's scene keymap \
         (right=bit 1 → held mask 2) and stamp `keys:2`. Got: {ast_stamp:?} — the key-set driver did not run"
    );

    // (d) The on-screen action buttons drive the SAME held-key mask as the
    // keyboard — the strongest proof of one shared target across the boundary.
    // ArrowRight above set the "right" bit (mask 2) and was never released, so a
    // pointerdown on the on-screen "Fire" button (bit 3 = 8) must ACCUMULATE into
    // the same mask → `keys:10` (2 | 8). If the two sources owned separate masks
    // we'd see `keys:8`, not `keys:10`. The button set is program-declared (from
    // the scene keymap), so finding a "fire" button also proves the panel is
    // program-blind, not host-hardcoded.
    let ast_pad = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const fr = sec.querySelector('iframe[sandbox]');
                if (!fr) return { err: 'no-iframe' };
                const win = fr.contentWindow, doc = fr.contentDocument;
                if (!win || !doc) return { err: 'no-same-origin' };
                const panel = doc.querySelector('[data-app-host-controls="key-set"]');
                if (!panel) return { err: 'no-panel' };
                const fire = doc.querySelector('[data-control="press:fire"]');
                if (!fire) return { err: 'no-fire-button' };
                fire.dispatchEvent(new win.PointerEvent('pointerdown', { bubbles: true }));
                const el = doc.querySelector('[data-app-host]');
                return { stamp: el ? (el.getAttribute('data-app-host-input') || '') : 'no-root' };
            }
            return { err: 'no-programs-window' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        ast_pad.get("stamp").and_then(|v| v.as_str()).unwrap_or(""),
        "keys:10",
        "A pointerdown on the on-screen 'Fire' button (bit 3 = 8) must ACCUMULATE into the \
         SAME held mask the keyboard's ArrowRight set (bit 1 = 2) → `keys:10`. A `keys:8` \
         would mean the two sources own separate masks (two code paths). Got: {ast_pad:?}"
    );

    // (e) Stuck-key guard: the mask is currently `10` (right + fire held, no
    // key/pointer release was delivered). Firing a `blur` on the payload window —
    // exactly what happens when focus leaves the iframe mid-hold — must
    // release-all → `keys:0`. Without the guard the ship would spin/thrust
    // forever — which is how the asteroids breakage was reported.
    let ast_blur = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const fr = sec.querySelector('iframe[sandbox]');
                if (!fr) return 'no-iframe';
                const win = fr.contentWindow, doc = fr.contentDocument;
                if (!win || !doc) return 'no-same-origin';
                win.dispatchEvent(new win.Event('blur'));
                const el = doc.querySelector('[data-app-host]');
                return el ? (el.getAttribute('data-app-host-input') || '') : 'no-root';
            }
            return 'no-programs-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        ast_blur.as_str().unwrap_or(""),
        "keys:0",
        "A window `blur` while keys are held must release-all the mask → `keys:0` \
         (stuck-key guard). Got: {ast_blur:?} — held keys would latch forever"
    );
    eprintln!(
        "Phase 2h.2e: Asteroids ran behind the boundary ({ast_seq} states, {ast_actors} actors \
         via SVG display-list), presented the STANDARD CONTROLLER (rotate/thrust axes on the \
         d-pad, 🔥 Fire as a glyphed action button — role-declared, not four bespoke buttons), \
         took held-key input from BOTH sources into one shared mask (keyboard right=2, on-screen \
         fire → keys:10), and cleared it on window blur (stuck-key guard → keys:0); P1 intact"
    );

    // Leave the Programs window back on its launcher grid for Phase 2h.3.
    let ast_reset = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const b = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                if (!b) return 'no-back';
                b.click();
                return 'back';
            }
            return 'no-programs';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(ast_reset.as_str().unwrap_or(""), "back", "Programs reset-to-grid after Asteroids failed: {ast_reset:?}");
    sleep(Duration::from_millis(300)).await;

    phase_gate!(client, "2h.3");
    // -- Phase 2h.3: Programs window — redirect to L5, not system-peer mount --
    //
    // The Programs window is the "entity native programs" launcher. It no longer
    // mounts compute on the system peer (the reframe §6-step-4 / discipline D21);
    // clicking a program runs it BEHIND the L5 boundary — the same sandboxed
    // iframe delivery the Apps L5 phases (2h.2c/d/e) exercise. This phase pins
    // the redirect: the launcher lists all three built-in programs, a tile click
    // yields a sandboxed `iframe` at `index.html?app-host=<key>` (NOT a
    // system-peer text grid or an Install/Start/tick surface), and Back returns
    // to the grid. The program RUNNING behind the boundary is already covered by
    // 2h.2c/d/e; here we prove the Programs surface routes to that one path.
    // POLLED, not a single sample after a fixed sleep. This assert fired twice
    // in one session with all three programs absent — the grid was found but
    // still empty, i.e. we sampled before the launcher had rendered its tiles.
    // A fixed sleep encodes a guess about how long that takes; on a loaded box
    // the guess is wrong and the phase fails for a reason unrelated to the
    // behaviour under test (AGENTS: "Never sleep(fixed) then assert"). The
    // budget is an upper bound — a healthy run returns on the first poll.
    let programs_deadline = Instant::now() + Duration::from_secs(15);
    let mut programs_grid;
    loop {
        programs_grid = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const names = Array.from(sec.querySelectorAll('button'))
                    .map(b => b.textContent.trim());
                return {
                    found: true,
                    // `includes('Life')` is TRUE OF "Interactive Life" TOO, so
                    // the plain-Life check must exclude it or one tile
                    // satisfies both and a missing program rides green. Same
                    // shape as Phase 2h.2s' "does the section still say 'War'"
                    // — a substring pretending to be a structural claim. The
                    // count below is the belt: two distinct Life tiles, not one
                    // matching twice.
                    has_life: names.some(n => n.includes('Life') && !n.includes('Interactive')),
                    has_life_edit: names.some(n => n.includes('Interactive Life')),
                    life_tiles: names.filter(n => n.includes('Life')).length,
                    has_snake: names.some(n => n.includes('Snake')),
                    has_asteroids: names.some(n => n.includes('Asteroids')),
                    // The STRUCTURAL count — tiles, not name matches. Reported
                    // in the phase's completion line so the roster size is
                    // measured rather than asserted from a literal.
                    tiles: sec.querySelectorAll('button.app-card').length,
                    // No system-peer host UI survives: the old mount surface had
                    // Install buttons and 'cannot mount' refusals.
                    install_buttons: names.filter(n => n === 'Install').length,
                    refusals: (sec.textContent.match(/cannot mount/g) || []).length,
                };
            }
            return { found: false };
            "#,
            vec![],
        )
        .await?;
        let ready = ["has_life", "has_life_edit", "has_snake", "has_asteroids"]
            .iter()
            .all(|k| programs_grid.get(*k).and_then(|v| v.as_bool()).unwrap_or(false));
        if ready || Instant::now() >= programs_deadline {
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }
    assert!(
        programs_grid.get("found").and_then(|v| v.as_bool()).unwrap_or(false),
        "Phase 2h.3: Programs window should be open after the spawn loop. Got: {programs_grid:?}"
    );
    for prog in ["has_life", "has_life_edit", "has_snake", "has_asteroids"] {
        assert!(
            programs_grid.get(prog).and_then(|v| v.as_bool()).unwrap_or(false),
            "Phase 2h.3: the launcher grid must list every built-in program \
             (missing {prog}). Got: {programs_grid:?}"
        );
    }
    // Two DISTINCT Life tiles. `has_life`/`has_life_edit` are both satisfiable
    // by the interactive tile alone if the substring guard above ever regresses,
    // so the count is what makes them independent claims.
    assert_eq!(
        programs_grid.get("life_tiles").and_then(|v| v.as_u64()),
        Some(2),
        "Phase 2h.3: Life and Interactive Life are two separate programs \
         (roots app/life and app/life-edit), so the grid must carry two tiles. \
         Got: {programs_grid:?}"
    );
    assert_eq!(
        programs_grid.get("install_buttons").and_then(|v| v.as_u64()),
        Some(0),
        "Phase 2h.3: the Programs window is a LAUNCHER now — no system-peer \
         'Install' buttons (compute must not run on the system peer, D21). \
         Got: {programs_grid:?}"
    );
    assert_eq!(
        programs_grid.get("refusals").and_then(|v| v.as_u64()),
        Some(0),
        "Phase 2h.3: no 'cannot mount' refusals — EVERY program runs behind the \
         L5 boundary. This is the assertion that catches a newly-imported \
         program whose shape the host does not drive: the honest-refusal gate \
         in `run_program` renders 'cannot mount' rather than failing loudly, so \
         a program that cannot run still LISTS. Interactive Life is the first \
         fixture to bind key-set control ROLES (axis + action), and this is \
         where that would surface. Got: {programs_grid:?}"
    );

    // Click the Life tile → the redirect renders a sandboxed L5 iframe.
    let clicked = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const tile = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.includes('Life'));
                if (!tile) return 'no-life-tile';
                tile.click();
                return 'clicked';
            }
            return 'no-programs-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        clicked.as_str().unwrap_or(""),
        "clicked",
        "Phase 2h.3: Life tile click failed: {clicked:?}"
    );
    sleep(Duration::from_millis(1000)).await;
    let player = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const fr = sec.querySelector('iframe[sandbox]');
                const back = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                return {
                    has_iframe: !!fr,
                    src: fr ? (fr.getAttribute('src') || '') : null,
                    sandbox: fr ? (fr.getAttribute('sandbox') || '') : null,
                    back: back ? back.textContent.trim() : null,
                };
            }
            return { has_iframe: false };
            "#,
            vec![],
        )
        .await?;
    assert!(
        player.get("has_iframe").and_then(|v| v.as_bool()).unwrap_or(false),
        "Phase 2h.3: clicking a program must render a sandboxed L5 iframe (the \
         redirect), not a system-peer display. Got: {player:?}"
    );
    let src = player.get("src").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        src.contains("app-host=life"),
        "Phase 2h.3: the program must be delivered via src=index.html?app-host=life \
         (run behind the boundary). Got src={src:?}, {player:?}"
    );
    assert!(
        player
            .get("sandbox")
            .and_then(|v| v.as_str())
            .map(|s| s.contains("allow-scripts"))
            .unwrap_or(false),
        "Phase 2h.3: the L5 payload must run in a sandboxed iframe. Got: {player:?}"
    );
    assert_eq!(
        player.get("back").and_then(|v| v.as_str()).unwrap_or(""),
        "← Entity Native Apps",
        "Phase 2h.3: the player back button must read '← Entity Native Apps' (the renamed \
         launcher title, i18n `window.programs`). Got: {player:?}"
    );

    // Back returns to the launcher grid (iframe torn down, tiles reappear) — so
    // the phase leaves no program running behind the boundary for its successors.
    let back_click = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const back = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                if (!back) return 'no-back-button';
                back.click();
                return 'clicked-back';
            }
            return 'no-programs-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        back_click.as_str().unwrap_or(""),
        "clicked-back",
        "Phase 2h.3: Back click failed: {back_click:?}"
    );
    sleep(Duration::from_millis(500)).await;
    let regrid = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const names = Array.from(sec.querySelectorAll('button'))
                    .map(b => b.textContent.trim());
                return {
                    has_iframe: !!sec.querySelector('iframe[sandbox]'),
                    has_life: names.some(n => n.includes('Life')),
                };
            }
            return { has_iframe: true, has_life: false };
            "#,
            vec![],
        )
        .await?;
    assert!(
        !regrid.get("has_iframe").and_then(|v| v.as_bool()).unwrap_or(true)
            && regrid.get("has_life").and_then(|v| v.as_bool()).unwrap_or(false),
        "Phase 2h.3: Back must return to the launcher grid (iframe gone, tiles \
         back). Got: {regrid:?}"
    );
    // Report what was MEASURED, not a literal. The old line said "3 built-in
    // programs" and stayed there while a 4th was imported — a phase message
    // that hard-codes a count is stale the moment the roster moves, and it is
    // read as evidence.
    eprintln!(
        "Phase 2h.3: Programs window redirects to L5 — grid lists {} program tile(s) \
         ({} of them Life), 0 refusals, Life tile → sandboxed app-host=life iframe, \
         Back → grid ✓",
        programs_grid.get("tiles").and_then(|v| v.as_u64()).unwrap_or(0),
        programs_grid.get("life_tiles").and_then(|v| v.as_u64()).unwrap_or(0),
    );

    // -- Phase 2h.4: INTERACTIVE LIFE — a real press moves the PROGRAM ---------
    //
    // The one gate here that asserts on **program state** rather than on a host
    // property, and it exists because workbench-go just proved the difference
    // the hard way. Their on-screen d-pad was inert for over three weeks while
    // their suites were green and their controller had been verified
    // pixel-for-pixel: "That verification was honest and it was about
    // rendering. The buttons rendered perfectly." Two defects, one on each side
    // of their language boundary, and neither side's tests could see the seam.
    // (workbench-go 7729cb5.)
    //
    // Phase 2h.2d — the existing on-screen-pad check — is on the wrong side of
    // exactly that line: it asserts `data-app-host-input == "dir:1"`, which is
    // the HOST's stamp saying it wrote to the target. A program that never
    // observes the write satisfies it. This phase instead presses PAUSE and
    // waits for the program's own status caption to flip to the paused glyph.
    // That glyph is projected by the program's `status` port, evaluated by the
    // compute evaluator from the `paused` field its own step wrote — so it
    // cannot be produced by any amount of correct plumbing that the program did
    // not actually see.
    //
    // It also covers the race go fixed in their generic host: a momentary press
    // that begins and ends between two ticks is invisible to a host that only
    // samples at tick time. We answer that race in the DRIVER instead
    // (`onscreen::MomentaryGuard` holds the release for one tick period,
    // `min_hold_ms`) rather than with go's host-side input queue. Two different
    // answers to one race across two implementations of the same generic host —
    // recorded in AGENTS.md, and this is the only thing that would notice if
    // ours stopped working.
    phase_gate!(client, "2h.4");

    // Open the interactive program from the launcher grid. Matched on the FULL
    // name: "Life" alone also matches this tile, which is what the grid
    // assertions above had to be taught.
    let open_edit = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const tile = Array.from(sec.querySelectorAll('button.app-card'))
                    .find(b => b.textContent.includes('Interactive Life'));
                if (!tile) return 'no-tile';
                tile.click();
                return 'clicked';
            }
            return 'no-programs-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        open_edit.as_str().unwrap_or(""),
        "clicked",
        "Phase 2h.4: could not open the Interactive Life tile: {open_edit:?}"
    );

    // Read the program's own status caption out of the payload. Same-origin
    // (the L5 payload is our own bundle), so no frame switch is needed — the
    // pattern Phase 2h.2d already uses.
    let read_status = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
            const fr = sec.querySelector('iframe[sandbox]');
            if (!fr) return { err: 'no-iframe' };
            const doc = fr.contentDocument;
            if (!doc) return { err: 'no-same-origin' };
            const st = doc.querySelector('[data-app-host-status]');
            const txt = st ? st.textContent.trim() : '';
            return {
                src: fr.getAttribute('src') || '',
                status: txt,
                // The program's own play/pause glyphs, from life_edit.go's
                // status fold. Reading the GLYPH rather than "did the text
                // change" is what makes this an assertion about `paused` and
                // not about repainting.
                running: txt.includes('▶'),
                paused: txt.includes('⏸'),
                has_pause_btn: !!doc.querySelector('[data-control="press:pause"]'),
            };
        }
        return { err: 'no-programs-window' };
    "#;

    let booted = poll_json(&client, read_status, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("running").and_then(|x| x.as_bool()).unwrap_or(false)
    })
    .await?;
    let src = booted.get("src").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        src.contains("app-host=life-edit"),
        "Phase 2h.4: Interactive Life must be delivered behind the L5 boundary as \
         app-host=life-edit. Got src={src:?}, {booted:?}"
    );
    assert_eq!(
        booted.get("running").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 2h.4: the program must boot RUNNING (its status port projects the \
         play glyph from `paused == 0`). Without this baseline the pause \
         assertion below is unfalsifiable — a program that never ran would also \
         never show the play glyph. Got: {booted:?}"
    );

    // Show the controller (auto-hidden on a precise-pointer desktop; headless is
    // desktop-ish) and press PAUSE with a real pointer down/up pair, not a
    // synthetic write. The release matters: it is what exercises
    // `MomentaryGuard`'s held-release, which is our answer to go's tick race.
    let pressed = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const fr = sec.querySelector('iframe[sandbox]');
                const win = fr && fr.contentWindow, doc = fr && fr.contentDocument;
                if (!win || !doc) return { err: 'no-same-origin' };
                const toggle = doc.querySelector('[data-controls-toggle]');
                const pad = doc.querySelector('[data-app-host-controls]');
                if (!toggle || !pad) return { err: 'no-controller' };
                if (win.getComputedStyle(pad).display === 'none') toggle.click();
                const btn = doc.querySelector('[data-control="press:pause"]');
                if (!btn) return { err: 'no-pause-button' };
                btn.dispatchEvent(new win.PointerEvent('pointerdown', { bubbles: true }));
                btn.dispatchEvent(new win.PointerEvent('pointerup', { bubbles: true }));
                return { pressed: true };
            }
            return { err: 'no-programs-window' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        pressed.get("pressed").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 2h.4: could not press the program's Pause action. The action bits \
         come from the program's own keymap (`press:pause`, control role \
         `action`) — a missing button means the standard-controller role parser \
         did not bind this program's declaration. Got: {pressed:?}"
    );

    // The assertion the phase exists for. Polled, not slept-then-read: the press
    // has to survive a tick boundary and a repaint, and a fixed sleep encodes a
    // guess about how long that takes.
    let after = poll_json(&client, read_status, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("paused").and_then(|x| x.as_bool()).unwrap_or(false)
    })
    .await?;
    assert_eq!(
        after.get("paused").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 2h.4: pressing Pause must reach the PROGRAM — its status port must \
         project the paused glyph, which only its own step can produce by writing \
         `paused`. A host that wrote the bit to the port and a program that never \
         observed it look identical from every other surface, and that is exactly \
         how workbench-go's d-pad was inert for three weeks with green suites. \
         Got: {after:?} (booted as {booted:?})"
    );
    eprintln!(
        "Phase 2h.4: Interactive Life ran behind the boundary and a real pointer \
         press on its declared `pause` action reached the PROGRAM — status \
         {:?} -> {:?} (the program's own glyph, not a host stamp) ✓",
        booted.get("status").and_then(|v| v.as_str()).unwrap_or(""),
        after.get("status").and_then(|v| v.as_str()).unwrap_or(""),
    );

    // Leave the Programs window on its launcher grid for the next phase.
    let _ = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Entity Native Apps') continue;
                const back = Array.from(sec.querySelectorAll('button'))
                    .find(b => b.textContent.trim().startsWith('←'));
                if (back) back.click();
                return 'ok';
            }
            return 'no-programs-window';
            "#,
            vec![],
        )
        .await?;


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
                const fr = sec.querySelector('iframe[sandbox]');
                return {
                    back: back ? back.textContent.trim() : null,
                    stage_style: stage ? (stage.getAttribute('style') || '') : null,
                    has_iframe: !!fr,
                    host_locale: fr ? (fr.getAttribute('data-host-locale') || '') : null,
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
    // i18n P2: the host must deliver its locale into the app `init` payload —
    // the sandboxed iframe records what it was handed as `data-host-locale`.
    // (`en` here: the app launched before any language switch in this suite.)
    assert_eq!(
        apps_player.get("host_locale").and_then(|v| v.as_str()).unwrap_or(""),
        "en",
        "Phase 2h.2: host must hand the app its locale in init (data-host-locale). \
         Got: {apps_player:?}"
    );
    eprintln!("Phase 2h.2: Calculator launches in Apps — back='← Apps', stage gm-fill, host_locale=en ✓");

    phase_gate!(client, "2i");
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

    phase_gate!(client, "2i.5");
    // -- Phase 2i.5: Access Log — app-tier access capture ---------------
    //
    // The user-facing capability-audit lens (actor · target · operation ·
    // outcome). Unlike Path Tap (per-peer inspect sink), the Access Log is
    // fed at the app's `ops::execute` chokepoint — every execute this app
    // issues, local OR remote, with the peer that issued it. The Phase
    // 2e-2h shell verbs dispatch through `ops::execute`, so they land here
    // as access rows regardless of which peer this window is bound to. We
    // assert the window rendered and at least one access row was logged.
    let al_state = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const al = sec.querySelector('.access-log');
                if (!al) continue;
                const h2 = al.querySelector('h2');
                const cdiv = al.querySelector("[data-field='access-log-count']");
                const rows = al.querySelectorAll('tbody tr').length;
                return {
                    found: true,
                    h2: h2 ? h2.textContent : null,
                    text: al.textContent,
                    count_field: cdiv ? cdiv.textContent : null,
                    rows,
                };
            }
            return { found: false };
            "#,
            vec![],
        )
        .await?;
    let al_found = al_state.get("found").and_then(|v| v.as_bool()).unwrap_or(false);
    assert!(
        al_found,
        "Access Log window should be rendered after the Phase 2 spawn loop. Got: {al_state:?}"
    );
    let al_h2 = al_state.get("h2").and_then(|v| v.as_str()).unwrap_or("");
    let al_text = al_state.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let al_rows = al_state.get("rows").and_then(|v| v.as_i64()).unwrap_or(0);
    assert_eq!(al_h2, "Access Log", "Access Log h2 mismatch: {al_h2:?}");
    assert!(
        !al_text.contains("No operations yet"),
        "Access Log empty — no accesses captured from the shell verbs. Text: {al_text:?}"
    );
    // Dispatches from the Phase 2e-2h shell verbs (query/count/inspect against
    // system/handler) route through `ops::execute` → should be ≥ 1 access row.
    assert!(
        al_rows >= 1,
        "Access Log should have at least one access row from the shell verbs. \
         rows={al_rows}, count_field={:?}, text={al_text:?}",
        al_state.get("count_field")
    );

    phase_gate!(client, "2j");
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

    phase_gate!(client, "2k");
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

    phase_gate!(client, "3");
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

    phase_gate!(client, "3-i18n");
    // -- Phase 3-i18n: language picker drives `dir`/`lang` (i18n P0) ----
    //
    // Boot (`i18n::apply(boot_choice())`, main.rs) must have set `lang`/`dir`
    // on BOTH `<html>` (light DOM) and the shadow host `#dom-layer` (so the
    // attribute inherits into the shadow tree — the whole point of finding 2:
    // `dir` on `<html>` alone is a silent RTL no-op). Assert both carry a
    // non-empty dir after boot.
    let boot_dir = client
        .execute(
            r#"
            const html = document.documentElement;
            const host = document.getElementById('dom-layer');
            return JSON.stringify({
                htmlDir: html.getAttribute('dir') || '',
                htmlLang: html.getAttribute('lang') || '',
                hostDir: host ? (host.getAttribute('dir') || '') : 'no-host',
                hostLang: host ? (host.getAttribute('lang') || '') : 'no-host',
            });
            "#,
            vec![],
        )
        .await?;
    let boot_dir = boot_dir.as_str().unwrap_or("").to_string();
    assert!(
        boot_dir.contains("\"htmlDir\":\"ltr\"")
            && boot_dir.contains("\"hostDir\":\"ltr\""),
        "Phase 3-i18n: boot must set dir=ltr on both <html> and the shadow host \
         (en boot). Got {boot_dir:?}"
    );

    // Switch to the `en-XA` pseudo-locale (dir=rtl) and assert BOTH surfaces
    // flip to rtl — the end-to-end proof that the shadow-host `dir` plumbing
    // works, exercised before any real translation exists.
    let lang_switch = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="language-"]');
            if (!s) return 'no-select';
            s.value = 'en-XA';
            s.dispatchEvent(new Event('change', { bubbles: true }));
            return 'changed';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        lang_switch.as_str(),
        Some("changed"),
        "Phase 3-i18n: the Settings Language dropdown (select[name^=language-]) \
         must exist and accept a change"
    );
    sleep(Duration::from_millis(400)).await;
    let rtl_dir = client
        .execute(
            r#"
            const html = document.documentElement;
            const host = document.getElementById('dom-layer');
            const layer = host;
            const root = layer.shadowRoot || layer;
            // The computed `direction` INSIDE the shadow tree — proves the
            // host's dir actually drives layout (not just the attribute), which
            // is what the P3 logical properties (margin-inline-*, text-align:
            // start, …) key off to mirror. This is the automated RTL check.
            const wm = root.querySelector('.window-manager');
            const computed = wm ? getComputedStyle(wm).direction : 'no-wm';
            return JSON.stringify({
                htmlDir: html.getAttribute('dir') || '',
                hostDir: host ? (host.getAttribute('dir') || '') : 'no-host',
                shadowDirection: computed,
            });
            "#,
            vec![],
        )
        .await?;
    let rtl_dir = rtl_dir.as_str().unwrap_or("").to_string();
    assert!(
        rtl_dir.contains("\"htmlDir\":\"rtl\"") && rtl_dir.contains("\"hostDir\":\"rtl\""),
        "Phase 3-i18n: selecting the en-XA pseudo-locale must flip dir=rtl on both \
         <html> and the shadow host. Got {rtl_dir:?}"
    );
    assert!(
        rtl_dir.contains("\"shadowDirection\":\"rtl\""),
        "Phase 3-i18n (P3): the shadow tree must COMPUTE direction:rtl under en-XA \
         (the host dir drives layout, which the logical CSS props mirror off). \
         Got {rtl_dir:?}"
    );

    // The string seam (P1): the Settings Appearance/Theme/Language labels
    // resolve through `t()`, so under en-XA they must render bracketed (⟦…⟧) —
    // the end-to-end proof of the catalog + pseudolocale AND that the section
    // actually re-rendered with the new locale. (Other windows' un-migrated
    // English is untouched, which is the point — only anchored strings move.)
    let pseudo_present = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            return (root.textContent || '').includes('⟦');
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        pseudo_present.as_bool(),
        Some(true),
        "Phase 3-i18n: en-XA must render the t()-backed Settings labels bracketed (⟦…⟧); \
         none found — catalog/pseudolocale/re-render path broken"
    );

    // Restore `en` so later phases (which assume LTR chrome) are unaffected.
    let _ = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="language-"]');
            if (s) { s.value = 'en'; s.dispatchEvent(new Event('change', { bubbles: true })); }
            return 'ok';
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(300)).await;
    // Back on `en`, the pseudo brackets must be gone — proves the switch
    // re-rendered in both directions (not a one-way transform).
    let pseudo_gone = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            return (root.textContent || '').includes('⟦');
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        pseudo_gone.as_bool(),
        Some(false),
        "Phase 3-i18n: restoring `en` must clear the ⟦…⟧ pseudo brackets (re-render back)"
    );
    println!(
        "  Phase 3-i18n OK — language picker drives dir (ltr↔rtl) + t()/pseudolocale labels (⟦…⟧ on/off)"
    );

    // P4: a REAL embedded overlay locale renders in the live Worker build (not
    // just the pseudo). Select `es` and assert the t()-backed Settings language
    // label shows the Spanish catalog string ("Idioma" = settings.language) —
    // end-to-end proof that the build-time-embedded locales/es.json overlay is
    // consulted through catalog_entry — and that no pseudo brackets appear (it's
    // a real translation, not en-XA). The compiled-in `en` fallback means an
    // un-translated surface stays English; the anchored label moves.
    let _ = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="language-"]');
            if (s) { s.value = 'es'; s.dispatchEvent(new Event('change', { bubbles: true })); }
            return 'ok';
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(300)).await;
    let es_idioma = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            return (root.textContent || '').includes('Idioma');
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        es_idioma.as_bool(),
        Some(true),
        "Phase 3-i18n (P4): selecting `es` must render the embedded overlay — the \
         Settings language label should read 'Idioma' (locales/es.json)"
    );
    let es_pseudo = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            return (root.textContent || '').includes('⟦');
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        es_pseudo.as_bool(),
        Some(false),
        "Phase 3-i18n (P4): a real locale (`es`) must NOT show pseudo brackets"
    );
    // Restore `en` again so later phases assume LTR English chrome.
    let _ = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="language-"]');
            if (s) { s.value = 'en'; s.dispatchEvent(new Event('change', { bubbles: true })); }
            return 'ok';
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(200)).await;
    println!(
        "  Phase 3-i18n OK (P4) — real overlay `es` renders 'Idioma' live (embedded locales/es.json)"
    );

    // P5: a real RTL locale — the combination neither phase above covers.
    // `en-XA` proves dir=rtl with English text; `es` proves a real catalog with
    // LTR layout. Only a shipped RTL locale exercises both at once, which is the
    // arrangement the four RTL overlays (ar he fa ur) actually ship in. An empty
    // or unconsulted `ar` catalog passes both earlier phases and fails here.
    let _ = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="language-"]');
            if (s) { s.value = 'ar'; s.dispatchEvent(new Event('change', { bubbles: true })); }
            return 'ok';
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(400)).await;
    let ar_state = client
        .execute(
            r#"
            const html = document.documentElement;
            const host = document.getElementById('dom-layer');
            const root = host.shadowRoot || host;
            const wm = root.querySelector('.window-manager');
            const text = root.textContent || '';
            return JSON.stringify({
                htmlDir: html.getAttribute('dir') || '',
                hostDir: host ? (host.getAttribute('dir') || '') : 'no-host',
                shadowDirection: wm ? getComputedStyle(wm).direction : 'no-wm',
                // settings.language / settings.appearance from locales/ar.json
                arabic: text.includes('اللغة') && text.includes('المظهر'),
                pseudo: text.includes('⟦'),
            });
            "#,
            vec![],
        )
        .await?;
    let ar_state = ar_state.as_str().unwrap_or("").to_string();
    assert!(
        ar_state.contains("\"htmlDir\":\"rtl\"")
            && ar_state.contains("\"hostDir\":\"rtl\"")
            && ar_state.contains("\"shadowDirection\":\"rtl\""),
        "Phase 3-i18n (P5): the real RTL locale `ar` must flip dir=rtl on <html>, the \
         shadow host, and the computed shadow direction. Got {ar_state:?}"
    );
    assert!(
        ar_state.contains("\"arabic\":true"),
        "Phase 3-i18n (P5): `ar` must render the embedded Arabic catalog — Settings \
         should show 'اللغة' (settings.language) and 'المظهر' (settings.appearance) \
         from locales/ar.json. Got {ar_state:?}"
    );
    assert!(
        ar_state.contains("\"pseudo\":false"),
        "Phase 3-i18n (P5): a real locale (`ar`) must NOT show pseudo brackets. \
         Got {ar_state:?}"
    );
    // Restore `en` (LTR chrome) for every later phase.
    let _ = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="language-"]');
            if (s) { s.value = 'en'; s.dispatchEvent(new Event('change', { bubbles: true })); }
            return 'ok';
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(300)).await;
    let restored = client
        .execute(
            r#"
            const html = document.documentElement;
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            return JSON.stringify({
                htmlDir: html.getAttribute('dir') || '',
                english: (root.textContent || '').includes('Language'),
            });
            "#,
            vec![],
        )
        .await?;
    let restored = restored.as_str().unwrap_or("").to_string();
    assert!(
        restored.contains("\"htmlDir\":\"ltr\"") && restored.contains("\"english\":true"),
        "Phase 3-i18n (P5): restoring `en` must return dir=ltr and English chrome — \
         proves the RTL switch is reversible, not a one-way latch. Got {restored:?}"
    );
    println!(
        "  Phase 3-i18n OK (P5) — real RTL overlay `ar` renders Arabic at dir=rtl, and reverts"
    );

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

    phase_gate!(client, "4");
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

    phase_gate!(client, "5");
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

    phase_gate!(client, "5.1");
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

    phase_gate!(client, "6");
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

    phase_gate!(client, "7");
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

    phase_gate!(client, "8");
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

    phase_gate!(client, "9");
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

    phase_gate!(client, "10");
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

    phase_gate!(client, "12");
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
                const rows = sec.querySelectorAll('tbody tr:not(.peer-group)');
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

    // Create a main-thread in-memory peer via the create form (kind "frontend",
    // was the "+ Frontend"/"+ Main thread (memory)" button); in Worker boot it
    // falls through to the same `create_new_peer_worker` path the original test
    // exercised.
    let new_peer_clicked = client
        .execute(&create_peer_form_js("frontend"), vec![])
        .await?;
    println!(
        "  create main-thread (memory) peer: {:?}",
        new_peer_clicked.as_str().unwrap_or("non-string")
    );
    assert_eq!(
        new_peer_clicked.as_str(),
        Some("clicked"),
        "Could not create a main-thread (memory) peer via the Peers create form."
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
                const rows = sec.querySelectorAll('tbody tr:not(.peer-group)');
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

    phase_gate!(client, "13");
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
            // role = system / main thread / worker / worker (OPFS) / native.
            // The just-created peer was made via "+ Main thread (memory)" and is
            // the only "(main thread)" option (the primary/system peer is
            // "(system)").
            const select = root.querySelector('.command-palette select');
            if (!select) return { ok: false, reason: 'no-palette-select' };
            let target = null;
            const seen = [];
            for (const opt of select.options) {
                seen.push(opt.text);
                const lower = opt.text.toLowerCase();
                if (lower.includes('(main thread)') && !lower.includes('(system)')) {
                    target = opt.value;
                    break;
                }
            }
            if (!target) return { ok: false, reason: 'no-main-thread-option', seen };
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
         Phase 12 created a peer but it didn't appear as a (main thread) \
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

    phase_gate!(client, "13.5");
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
                if (lower.includes('(main thread)') && !lower.includes('(system)')) {
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

    phase_gate!(client, "14");
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
    // window's "Known devices" table after the click, alongside a
    // "Reconnect" button (§13 the remembered-peer surface). That requires
    // the WS handshake to complete, the entity-protocol handshake to
    // succeed, the consumer's `handle_connect_peer` worker arm to insert
    // into the connection pool, AND the enriched connection record
    // (addr + reconnect) to be written + read back — i.e. the whole
    // Parity-D-narrow surface end-to-end.
    let tauri = start_tauri_listener()?;
    if tauri.is_none() {
        println!(
            "--- Phase 14 SKIPPED: no display in this environment \
             (WAYLAND_DISPLAY/DISPLAY unset) — the Tauri autostart + \
             ConnectPeer phases need a window server; run the e2e where \
             the container gets a display to exercise them ---"
        );
    }
    if let Some(tauri) = &tauri {
        println!("  tauri peer_id:         {}", tauri.peer_id);
        println!("  tauri ws_addr:         {}", tauri.ws_addr);
        println!("  tauri webview booted:  {}", tauri.webview_booted);
        // The MARGIN, on success. The assertion below is a fixed 60s budget,
        // and its failure text ("never booted in 60s") is a statement about the
        // budget as much as about the WebView — so a green run has to say how
        // much room it had, or nobody can tell a slow box from a broken one the
        // next time it goes red. Loud past half the budget: at that point the
        // next loaded run is a coin flip, and a *quiet* green is what lets a
        // load-sensitive budget go on being read as a product failure.
        println!("  tauri listener ready:  {} ms", tauri.ready_ms);
        match tauri.webview_ms {
            Some(ms) if ms > 30_000 => println!(
                "  tauri webview boot:    {ms} ms — OVER HALF the 60s budget. \
                 This phase is on the edge; a busier box will fail it, and the \
                 failure will not look like a timing problem."
            ),
            Some(ms) => println!("  tauri webview boot:    {ms} ms (budget 60000 ms)"),
            None => {}
        }
        // The milestone trace on SUCCESS, so a green run carries the baseline a
        // red one has to be read against. Without it nobody can say whether a
        // healthy boot also skips a milestone, and the failure message below
        // has nothing to be compared to.
        println!(
            "  tauri boot milestones: {}/{}",
            tauri.boot_milestones.len(),
            WEBVIEW_BOOT_MILESTONES.len()
        );
        // "Frame loop started" only logs from src/main.rs:163, which runs
        // strictly AFTER `EntityApp::new_wasm[_worker]` returns Ok. So this
        // is a universal "WebView UI booted" signal independent of Direct
        // vs Worker mode. If the autostart hook ever broke the WebView
        // load (or anything else regresses on the Tauri WebKitGTK path),
        // this assertion catches it before users see a blank window.
        assert!(
            tauri.webview_booted,
            "Tauri WebView never logged 'Frame loop started' within the \
             60s startup budget. Autostart printed the READY line so the \
             native backend is fine, but the WebView UI failed to boot. \
             A user running `make tauri-run` would see a blank window.\n\
             The child's stdout tail is below — look for [UNCAUGHT] errors, \
             OPFS init failures, or other WASM init failures. Note that \
             `libEGL warning: egl: failed to create dri2 screen` in \
             target/e2e-tauri-stderr.log is NOT a boot failure: it appears on \
             runs where the WebView goes on to boot fine (software \
             compositing via WEBKIT_DISABLE_DMABUF_RENDERER=1), so don't stop \
             at it.\n\
             \n\
             HOW FAR THE WEBVIEW GOT — read this FIRST; it is the only thing \
             here that separates causes. Each line below is logged by the WASM \
             module itself through the console bridge, so `none` means the \
             module never began executing (look at instantiation) while a \
             partial trace means app boot hung at a named step (look there). \
             Compare against the `tauri boot milestones: N/{}` a green run \
             prints:\n{}\n\
             \n\
             Child stdout tail:\n{}",
            WEBVIEW_BOOT_MILESTONES.len(),
            render_boot_milestones(&tauri.boot_milestones),
            tauri.stdout_tail(),
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
                        // Fire the input event like a real keystroke would:
                        // the field is a draft-tracked atom (components::
                        // text_input) and Connect submits from the drafts map,
                        // not the DOM — a silent value-set is a fill no user
                        // can produce and would submit the default suggestion.
                        input.dispatchEvent(new Event('input', {{ bubbles: true }}));
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
        // handshake + connection-pool insert + post-connect type fetch, and
        // only then a re-render of the Known devices table. This used to be a
        // fixed 2.5s sleep — a guess at how long that chain takes, which on a
        // loaded box (this runs minutes into the suite, beside Firefox, the
        // dist server, cargo, and a second Tauri process) is exactly the shape
        // that fails for reasons unrelated to the behaviour under test. Poll
        // instead: a healthy run returns on the first poll and the generous
        // budget is only ever paid on the failure path.
        let connect_started = std::time::Instant::now();
        let connected_v = poll_json(
            &client,
            &format!(
                r#"
                    const layer = document.getElementById('dom-layer');
                    const root = layer.shadowRoot || layer;
                    const sections = root.querySelectorAll('section.window');
                    for (const sec of sections) {{
                        if (!sec.querySelector('.peer-connections')) continue;
                        // After ConnectPeer success the renderer lists the peer
                        // in the "Known devices" table: its short_pid, a
                        // "Connected" conn_chip, and the saved address in the
                        // Address column (the §13.2 enrichment, asserted
                        // directly — a CONNECTED row deliberately renders no
                        // Reconnect button, only Forget).
                        const text = sec.textContent;
                        const knownLabel = text.includes('Known devices');
                        const foundPeer = text.includes('{tauri_short}');
                        const connectedChip = text.includes('Connected');
                        const hasAddr = text.includes('{ws_addr}');
                        return {{
                            connected: knownLabel && foundPeer && connectedChip,
                            knownLabel, foundPeer, connectedChip, hasAddr,
                            text: text.slice(0, 400),
                        }};
                    }}
                    return {{ connected: false, reason: 'no-section' }};
                    "#,
                ws_addr = tauri.ws_addr,
            ),
            ASYNC_ROUND_TRIP_BUDGET,
            // Both assertions below read this one value, so wait for BOTH
            // facts — settling on `connected` alone would race the address
            // enrichment and make `has_addr` the flaky one instead.
            |v| {
                v.get("connected").and_then(|c| c.as_bool()).unwrap_or(false)
                    && v.get("hasAddr").and_then(|c| c.as_bool()).unwrap_or(false)
            },
        )
        .await?;
        let connected = connected_v
            .get("connected")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let has_addr = connected_v
            .get("hasAddr")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Elapsed on success for the same reason as 15.6 — this replaced a fixed
        // 2.5s sleep, so the number also tells you how much of that sleep was
        // ever real.
        println!(
            "  known + connected: {connected} (addr persisted: {has_addr}) in {}ms",
            connect_started.elapsed().as_millis()
        );
        assert!(
            connected,
            "Browser primary peer did not list Tauri peer's short_pid as \
             Connected under Known devices after click. ConnectPeer flow broken \
             end-to-end. Could be: WS handshake failed, entity-protocol \
             handshake failed, worker arm of handle_connect_peer didn't pool \
             the connection, or the post-connect refresh isn't writing the \
             enriched connection record back into the tree the window \
             subscribes to. Detail: {connected_v}"
        );
        assert!(
            has_addr,
            "Known devices listed the peer but its Address column is missing \
             the connect address — the remembered-peer record has no saved \
             address (§13.2 enrichment not persisted through the worker arm). \
             Detail: {connected_v}"
        );

        // -- Phase 14b: FILE TRANSFER over `local/files` — the desktop path --
        //
        // **This is the transfer most users hit first and it had no coverage at
        // all**: until now this file contained zero occurrences of "file
        // transfer" (buildout item 17). `make e2e-webrtc-file` proves
        // browser↔browser over `system/content` + offer manifests; this proves
        // the *other* kind of serving peer — a native backend mounting a real
        // directory at `local/files` — through the same window, the same rows
        // and the same Pull button.
        //
        // It lives inside the display-gated Tauri block **because it needs that
        // listener**, which is the only thing that belongs in here (`AGENTS.md`:
        // 14 and 15.6 are the phases allowed in this gate; this is 14's own
        // second half, sharing its child process, not a new tenant). It shares
        // 14's `phase_gate!` for the same reason.
        //
        // Four claims, in the order a person would make them:
        //   1. the window lists what the backend shares (`welcome.txt`, seeded
        //      by `ensure_share_root`);
        //   2. Pull saves it — the `local/files:read` half of `PullPlan::Share`;
        //   3. Upload lands, and the app's own read-back verification agrees;
        //   4. **the bytes are on the backend's disk, byte for byte** — read
        //      from this process, which shares a filesystem with the child it
        //      spawned. Nothing else in either suite asserts that: every other
        //      transfer check reads the app's own report of its own work.
        println!("--- Phase 14b: file transfer over local/files (desktop path) ---");

        // A unique name per run. NOT cosmetic: a fixed name would be left in
        // the share by the previous run, and then the "it appeared in the
        // listing" assertions would pass on a stale file while a broken upload
        // wrote nothing. It is removed at the end of the phase.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let up_name = format!("from-browser-{stamp}.bin");
        // Deterministic content, and multi-KiB so this is a real write rather
        // than a token one. Same generator the Shell's `offer … size=` uses, so
        // the two fixtures read alike.
        let up_bytes: Vec<u8> = (0..200_000usize)
            .map(|i| (i.wrapping_mul(31) % 251) as u8)
            .collect();
        let share_dir = std::env::var("HOME")
            .map(|h| std::path::PathBuf::from(h).join(".entity").join("tori-share"))
            .expect("HOME must be set — the Tauri child inherits it and shares its filesystem");
        let up_path = share_dir.join(&up_name);

        // The File Transfer window is spawned by the Phase 2 boot loop; click
        // it up again only if something closed it, so this phase does not
        // depend on window-lifecycle decisions made 5,000 lines earlier.
        let ft_open = client
            .execute(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                for (const sec of root.querySelectorAll('section.window')) {
                    if (sec.querySelector('.file-transfer')) return 'present';
                }
                for (const b of root.querySelectorAll('button.spawn-btn')) {
                    if (b.textContent.trim() === '+ File Transfer') {
                        b.click();
                        return 'spawned';
                    }
                }
                return 'no-spawn-btn';
                "#,
                vec![],
            )
            .await?;
        assert_ne!(
            ft_open.as_str(),
            Some("no-spawn-btn"),
            "No File Transfer window and no '+ File Transfer' button to open one."
        );

        // Point the window at the Tauri peer explicitly. The picker only
        // renders with more than one remembered peer, so "no selector" is a
        // legitimate answer — but choosing by hand is what keeps this phase
        // honest if a later phase ever adds a second remembered peer above it.
        let ft_target = poll_json(
            &client,
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                let sec = null;
                for (const s of root.querySelectorAll('section.window')) {{
                    if (s.querySelector('.file-transfer')) {{ sec = s; break; }}
                }}
                if (!sec) return {{ ready: false, reason: 'no-window' }};
                const sel = sec.querySelector('[data-field="ft-target"]');
                if (!sel) return {{ ready: true, mode: 'single-target' }};
                const values = [...sel.options].map(o => o.value);
                if (sel.value !== '{tauri_pid}') {{
                    sel.value = '{tauri_pid}';
                    sel.dispatchEvent(new Event('change', {{ bubbles: true }}));
                }}
                return {{ ready: values.includes('{tauri_pid}'), mode: 'picked', values }};
                "#,
                tauri_pid = tauri.peer_id,
            ),
            ASYNC_ROUND_TRIP_BUDGET,
            |v| v.get("ready").and_then(|r| r.as_bool()).unwrap_or(false),
        )
        .await?;
        assert!(
            ft_target.get("ready").and_then(|r| r.as_bool()).unwrap_or(false),
            "File Transfer could not target the Tauri peer. The window reads the \
             ever-connected registry, so this failing means the Connect above did \
             not leave a row for it. Detail: {ft_target}"
        );

        // 1. The backend's share, listed. The root auto-loads on first render;
        //    the Refresh press is the retry, not the trigger.
        let listed_started = std::time::Instant::now();
        let share_listed = poll_json(
            &client,
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            let sec = null;
            for (const s of root.querySelectorAll('section.window')) {
                if (s.querySelector('.file-transfer')) { sec = s; break; }
            }
            if (!sec) return { found: false, reason: 'no-window' };
            const row = sec.querySelector('[data-row-name="welcome.txt"]');
            if (!row) {
                const refresh = sec.querySelector('[data-field="ft-refresh"]');
                if (refresh) refresh.click();
            }
            const results = sec.querySelector('[data-field="ft-results"]');
            return {
                found: !!row,
                results: results ? results.textContent.slice(-300) : '',
            };
            "#,
            ASYNC_ROUND_TRIP_BUDGET,
            |v| v.get("found").and_then(|f| f.as_bool()).unwrap_or(false),
        )
        .await?;
        assert!(
            share_listed.get("found").and_then(|f| f.as_bool()).unwrap_or(false),
            "The Tauri backend's share never listed `welcome.txt` in the File \
             Transfer window. The backend seeds that file in `ensure_share_root`, \
             so this is either the `local/files:list` dispatch, the listing \
             decode, or the window's target. Detail: {share_listed}"
        );
        println!(
            "  share listed (welcome.txt) in {}ms",
            listed_started.elapsed().as_millis()
        );

        // 2. Pull it. Same row → same Pull button as an offered file: the whole
        //    point of `PullPlan` is that this layer cannot tell the two apart.
        let pulled = poll_json(
            &client,
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            let sec = null;
            for (const s of root.querySelectorAll('section.window')) {
                if (s.querySelector('.file-transfer')) { sec = s; break; }
            }
            if (!sec) return { saved: false, reason: 'no-window' };
            const results = sec.querySelector('[data-field="ft-results"]');
            const text = results ? results.textContent : '';
            if (text.includes('✓ saved') && text.includes('welcome.txt')) {
                return { saved: true, tail: text.slice(-200) };
            }
            // Re-press each poll: the row is re-rendered on every repaint, so a
            // click landed on a stale element does nothing and must be retried
            // rather than waited on.
            const row = sec.querySelector('[data-row-name="welcome.txt"]');
            if (row) row.click();
            const pull = sec.querySelector('[data-field="ft-pull"]');
            if (pull && !pull.disabled) pull.click();
            return { saved: false, tail: text.slice(-200) };
            "#,
            ASYNC_ROUND_TRIP_BUDGET,
            |v| v.get("saved").and_then(|s| s.as_bool()).unwrap_or(false),
        )
        .await?;
        assert!(
            pulled.get("saved").and_then(|s| s.as_bool()).unwrap_or(false),
            "Pull of `welcome.txt` from the Tauri share never reported a save. \
             This is the `local/files:read` + reassemble + browser-download path \
             (`PullPlan::Share`). Detail: {pulled}"
        );
        println!("  pulled welcome.txt from the backend share");

        // 3. Upload the other way. The picker's hidden input is driven
        //    directly — a native file dialog cannot be answered by a harness —
        //    which still enters the app exactly where a real choice does, at
        //    the `change` listener, `array_buffer()` and all.
        let upload_sent = client
            .execute(
                &format!(
                    r#"
                    const layer = document.getElementById('dom-layer');
                    const root = layer.shadowRoot || layer;
                    let sec = null;
                    for (const s of root.querySelectorAll('section.window')) {{
                        if (s.querySelector('.file-transfer')) {{ sec = s; break; }}
                    }}
                    if (!sec) return 'no-window';
                    const inp = sec.querySelector('[data-field="ft-upload-input"]');
                    if (!inp) return 'no-input';
                    const n = {len};
                    const a = new Uint8Array(n);
                    for (let i = 0; i < n; i++) a[i] = (i * 31) % 251;
                    const f = new File([a], '{name}', {{ type: 'application/octet-stream' }});
                    const dt = new DataTransfer();
                    dt.items.add(f);
                    inp.files = dt.files;
                    inp.dispatchEvent(new Event('change'));
                    return 'sent';
                    "#,
                    len = up_bytes.len(),
                    name = up_name,
                ),
                vec![],
            )
            .await?;
        assert_eq!(
            upload_sent.as_str(),
            Some("sent"),
            "Could not drive the File Transfer upload picker. Detail: {upload_sent}"
        );

        let uploaded = poll_json(
            &client,
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                let sec = null;
                for (const s of root.querySelectorAll('section.window')) {{
                    if (s.querySelector('.file-transfer')) {{ sec = s; break; }}
                }}
                if (!sec) return {{ ok: false, reason: 'no-window' }};
                const results = sec.querySelector('[data-field="ft-results"]');
                const text = results ? results.textContent : '';
                return {{
                    ok: text.includes('uploaded') && text.includes('{name}'),
                    verified: text.includes('verified') && text.includes('{name}'),
                    rejected: text.includes('upload REJECTED') || text.includes('VERIFY FAILED'),
                    // The S5 write-refresh claim: the app re-lists the share
                    // itself after a confirmed write, so the new file must
                    // appear with nobody pressing Refresh.
                    row: !!sec.querySelector('[data-row-name="{name}"]'),
                    tail: text.slice(-300),
                }};
                "#,
                name = up_name,
            ),
            ASYNC_ROUND_TRIP_BUDGET,
            |v| {
                (v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false)
                    && v.get("verified").and_then(|o| o.as_bool()).unwrap_or(false)
                    && v.get("row").and_then(|o| o.as_bool()).unwrap_or(false))
                    || v.get("rejected").and_then(|o| o.as_bool()).unwrap_or(false)
            },
        )
        .await?;
        assert!(
            uploaded.get("ok").and_then(|o| o.as_bool()).unwrap_or(false),
            "Upload to the Tauri share never reported success. This is \
             `Action::UploadFile` → `local/files:write` across the worker arm. \
             Detail: {uploaded}"
        );
        assert!(
            uploaded.get("verified").and_then(|o| o.as_bool()).unwrap_or(false),
            "The upload reported OK but the app's own read-back verification did \
             not confirm it — exactly the 'says OK but no file' case that check \
             exists to catch. Detail: {uploaded}"
        );
        assert!(
            uploaded.get("row").and_then(|o| o.as_bool()).unwrap_or(false),
            "The uploaded file never appeared in the share listing without a \
             manual Refresh — the S5 write-refresh (`handle_upload_file` \
             enqueueing `ft_refresh` for the initiating window) is not firing. \
             The write itself landed. Detail: {uploaded}"
        );

        // 4. The bytes, on the backend's disk. Every check above reads the
        //    app's report of its own work; this one leaves the app entirely —
        //    the Tauri child was spawned by this process and shares its HOME, so
        //    the share root is a directory we can simply open. A `write` that
        //    is acknowledged, verified, and yet absent here would be invisible
        //    to every other assertion in either suite.
        let on_disk = std::fs::read(&up_path).unwrap_or_else(|e| {
            panic!(
                "the uploaded file is not on the backend's disk at {up_path:?}: {e}\n\
                 The app reported a confirmed write AND a successful read-back, so \
                 either the share root moved (ensure_share_root) or the handler \
                 acknowledged a write it did not persist."
            )
        });
        assert_eq!(
            on_disk.len(),
            up_bytes.len(),
            "the file on disk is {} bytes, the browser sent {}",
            on_disk.len(),
            up_bytes.len()
        );
        assert!(
            on_disk == up_bytes,
            "the file on the backend's disk is not the file the browser sent \
             (same length, different bytes) — a corrupting re-encode somewhere \
             between the picker and `local/files:write`"
        );
        println!(
            "  uploaded {} ({} bytes) — verified byte-for-byte at {:?}",
            up_name,
            up_bytes.len(),
            up_path
        );
        // Leave the share as we found it: the next run's uniqueness check does
        // not depend on this, but an accumulating demo share is its own slow
        // mess (and it is shared with `make tauri-run` on a dev box).
        let _ = std::fs::remove_file(&up_path);
    }
    // ^^ end of the display-gated Tauri block. NOTHING that must run on every
    // box belongs above this line. Phase 11 below (the OPFS reload-persistence
    // acceptance test) was accidentally re-indented INTO this block by the
    // display-gating commit (d24ee12) and silently stopped running on headless
    // hosts — the suite reported green while never exercising the one failure
    // mode Phase 11 exists to catch, and every later phase ran without the
    // reload. Keep it out here.

    // -- Phase 14.2: OFFER + WITHDRAW on the WORKER ARM -----------------
    //
    // The offer path has two gates and both run the main-thread arm:
    // `e2e-webrtc-file` drives a plain browser (no `?worker=1`), and the
    // native proof is Direct by construction. So every Worker-arm hop in it —
    // `DispatchHandle::Worker` carrying a `system/content:ingest` across the
    // worker boundary, `put_and_wait_for_cache` landing the manifest, the
    // offers-prefix subscription seeding the cache mirror that
    // `read_own_offers` reads — was **unproven on the arm this suite runs**.
    // That is the arm-split footgun this repo keeps meeting, and its signature
    // failure is silent: the write succeeds, another peer can pull it, and only
    // *our own* view of it is empty forever (an unsubscribed prefix on the
    // Worker arm is an unseeded mirror, not an error).
    //
    // That this really is the Worker arm is not taken on the URL's word: the
    // session boots `?worker=1` from a secure `localhost` origin (an insecure
    // one falls back to Direct **silently**, `main.rs`), and Phase 11 — the OPFS
    // reload-persistence acceptance test, which only passes under Worker+OPFS —
    // runs later in this same session. A silent fallback turns that red.
    //
    // **Deliberately OUTSIDE the display gate above.** Offering is untargeted —
    // it publishes on our side and needs no peer, no listener and no display —
    // so gating it would silently retire it on every headless box, which is the
    // exact mistake Phase 11 and the QR check below were rescued from.
    //
    // Not covered here, on purpose: the `MAX_OFFER_BYTES` refusal (it needs a
    // 16 MiB+ allocation in the browser to reach, and the model-level refusal is
    // already mutation-checked natively) and the cross-peer half (that is what
    // `make e2e-webrtc-file` is for).
    phase_gate!(client, "14.2");
    println!("--- Phase 14.2: offer + withdraw on the Worker arm ---");
    let offer_name = "worker-arm-offer.bin";
    let offer_sent = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                let sec = null;
                for (const s of root.querySelectorAll('section.window')) {{
                    if (s.querySelector('.file-transfer')) {{ sec = s; break; }}
                }}
                if (!sec) {{
                    for (const b of root.querySelectorAll('button.spawn-btn')) {{
                        if (b.textContent.trim() === '+ File Transfer') {{ b.click(); break; }}
                    }}
                    return 'spawned-retry';
                }}
                const inp = sec.querySelector('[data-field="ft-offer-input"]');
                if (!inp) return 'no-offer-input';
                // Two full chunks and a tail: a one-chunk offer would ingest in a
                // single entity and prove less about the envelope crossing the
                // worker boundary.
                const n = 600000;
                const a = new Uint8Array(n);
                for (let i = 0; i < n; i++) a[i] = (i * 31) % 251;
                const f = new File([a], '{offer_name}', {{ type: 'application/octet-stream' }});
                const dt = new DataTransfer();
                dt.items.add(f);
                inp.files = dt.files;
                inp.dispatchEvent(new Event('change'));
                return 'sent';
                "#,
            ),
            vec![],
        )
        .await?;
    assert_eq!(
        offer_sent.as_str(),
        Some("sent"),
        "Could not drive the File Transfer offer picker. The offer card renders \
         with no target selected by design, so 'no window' here is a real \
         failure, not a missing precondition. Detail: {offer_sent}"
    );

    let offered_started = std::time::Instant::now();
    let offered = poll_json(
        &client,
        &format!(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            let sec = null;
            for (const s of root.querySelectorAll('section.window')) {{
                if (s.querySelector('.file-transfer')) {{ sec = s; break; }}
            }}
            if (!sec) return {{ row: false, reason: 'no-window' }};
            const results = sec.querySelector('[data-field="ft-results"]');
            const text = results ? results.textContent : '';
            return {{
                row: !!sec.querySelector('[data-field="ft-offer-row"][data-offer-name="{offer_name}"]'),
                reported: text.includes('✓ offering') && text.includes('{offer_name}'),
                failed: text.includes('✗ offer'),
                tail: text.slice(-300),
            }};
            "#,
        ),
        ASYNC_ROUND_TRIP_BUDGET,
        |v| {
            (v.get("row").and_then(|r| r.as_bool()).unwrap_or(false)
                && v.get("reported").and_then(|r| r.as_bool()).unwrap_or(false))
                || v.get("failed").and_then(|r| r.as_bool()).unwrap_or(false)
        },
    )
    .await?;
    assert!(
        offered.get("reported").and_then(|r| r.as_bool()).unwrap_or(false),
        "The Worker arm never completed an offer. This is `Action::OfferFile` → \
         `DispatchHandle::Worker` carrying `system/content:ingest` across the \
         worker boundary, then the manifest `put`. Detail: {offered}"
    );
    assert!(
        offered.get("row").and_then(|r| r.as_bool()).unwrap_or(false),
        "The offer completed but the window never listed it — the read
         (`read_own_offers`) or the render is broken. NOTE what this canNOT be, \
         measured: the window's own offers subscription. Every window is open in \
         this session and two of them (Entity Tree, Storage) subscribe the whole \
         peer tree, and the Worker proxy's cache is a UNION over all mirrors — so \
         deleting that `watch_prefix` leaves this phase green. \
         `a_lone_file_transfer_window_lists_what_it_offers` is the test that \
         covers it. Detail: {offered}"
    );
    println!(
        "  offered {offer_name} and listed it in {}ms",
        offered_started.elapsed().as_millis()
    );

    // Withdraw it again: the Worker-arm `remove` plus the invalidation that has
    // to reach the same subscription the read came from.
    let withdrawn = poll_json(
        &client,
        &format!(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            let sec = null;
            for (const s of root.querySelectorAll('section.window')) {{
                if (s.querySelector('.file-transfer')) {{ sec = s; break; }}
            }}
            if (!sec) return {{ gone: false, reason: 'no-window' }};
            const row = sec.querySelector('[data-field="ft-offer-row"][data-offer-name="{offer_name}"]');
            if (!row) return {{ gone: true }};
            // Re-press each poll: every repaint rebuilds the row, so a click on a
            // stale button does nothing and must be retried rather than waited on.
            const btn = sec.querySelector('[data-field="ft-stop-offer"][data-offer-name="{offer_name}"]');
            if (btn) btn.click();
            return {{ gone: false }};
            "#,
        ),
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("gone").and_then(|g| g.as_bool()).unwrap_or(false),
    )
    .await?;
    assert!(
        withdrawn.get("gone").and_then(|g| g.as_bool()).unwrap_or(false),
        "Stop offering did not remove the row on the Worker arm — the manifest \
         `remove` did not land, or its invalidation never reached the \
         subscription this window reads through. Detail: {withdrawn}"
    );
    println!("  stopped offering it, and the row went");

    // -- QR pairing must be hidden without a local listener -------------
    //
    // QR pairing advertises an address another device dials to reach a peer
    // *here*. A pure browser can't bind a listener, so it has nothing to
    // advertise — the QR Pairing <details> is rendered ONLY when
    // `qr_payload` is Some (this process's native WS listener, or the
    // system's Tauri-managed backend surfaced via IPC — see
    // `dom/peer_connections.rs` + the model's `qr_payload` derivation). This
    // e2e IS a pure Selenium browser (the Tauri peer is a *remote* ws://
    // connection, not a local listener), so the QR display MUST be absent.
    // (The lazy-generate-on-toggle Stage-F guard only applies where the QR
    // actually renders — a Tauri-desktop run — so it lives with that path,
    // not this browser suite.) The Scan-QR side stays available regardless.
    //
    // Deliberately OUTSIDE the display gate: this needs no Tauri and no
    // listener — the *absence* of a listener is precisely the fixture. It sat
    // inside the gate until now (the same class of mistake as Phase 11, noted
    // as a standing trap in AGENTS.md), so on a headless box the assertion
    // never ran and the suite reported green without it. If anything, the
    // headless run is the more faithful fixture of the two.
    let qr_present = client
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
                            return { found: true };
                        }
                    }
                    return { found: false };
                }
                return { found: false, reason: 'no-section' };
                "#,
            vec![],
        )
        .await?;
    assert_eq!(
        qr_present.get("found").and_then(|v| v.as_bool()),
        Some(false),
        "A QR Pairing <details> is showing in a PURE BROWSER — but a browser \
         has no listener to advertise, so QR display must be gated off \
         (`qr_payload` None). Either the display gate regressed or a remote \
         ws-connected peer is wrongly feeding `qr_payload`. Detail: {qr_present}"
    );
    println!("  QR pairing correctly hidden in the browser (no local listener to advertise)");

    // -- Measurement checkpoint -----------------------------------------
    //
    // Capture per-window render numbers from Phases 1–10's interactions
    // before the Phase 11 reload wipes the page-local log buffer. Only
    // fires under `--features measurement`; otherwise the filter is a no-op
    // because the render-counter logs aren't emitted.
    //
    // Also moved out of the display gate: the samples it reports come from
    // Phases 1–10, so gating them on a display meant a headless measurement run
    // silently reported nothing.
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

    phase_gate!(client, "14.3");
    // -- Phase 14.3: a REFUSED file picker must say so ------------------
    //
    // Reported from Android/Firefox on the published site: *"I go to File
    // Transfer, offer a file, it just doesn't do anything. It doesn't ask me.
    // Doesn't give me an error."* The same button works on desktop.
    //
    // The cause of the SILENCE (not necessarily of the Android refusal, which
    // is unreproduced here): the button called `HTMLElement::click()` on the
    // hidden `<input type=file>`, which is fire-and-forget. An engine that
    // declines to open a chooser produces no dialog, no `change`, no exception
    // and no console entry — there is nothing for any surface to report. It
    // goes through `showPicker()` now, which *throws* instead.
    //
    // **This phase is possible only because a scripted click carries no user
    // activation** — the harness's ordinary `el.click()` is itself the refusal
    // case, so `showPicker` raises `NotAllowedError` and the status line must
    // carry it. That makes the assertion free here and impossible by hand.
    //
    // It does NOT prove the Android button now works; nothing on this box can.
    // It proves the refusal has a voice, which is the half that was missing.
    println!("--- Phase 14.3: a refused file picker reports instead of going silent ---");
    let refused = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        let sec = null;
        for (const s of root.querySelectorAll('section.window')) {
            if (s.querySelector('.file-transfer')) { sec = s; break; }
        }
        if (!sec) return { ok: false, why: 'no-window' };
        if (typeof HTMLInputElement.prototype.showPicker !== 'function')
            return { ok: false, why: 'engine-has-no-showPicker' };
        const btn = sec.querySelector('[data-field="ft-offer"]');
        if (!btn) return { ok: false, why: 'no-ft-offer-button' };
        if (!window.__e2e_picker_clicked) { window.__e2e_picker_clicked = 1; btn.click(); }
        const st = sec.querySelector('[data-field="ft-offer-status"]');
        const text = st ? st.textContent.trim() : '';
        return { ok: text.length > 0, text };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    let picker_text = refused.get("text").and_then(|t| t.as_str()).unwrap_or("");
    assert!(
        refused.get("ok").and_then(|b| b.as_bool()).unwrap_or(false),
        "A refused file picker said NOTHING. A scripted click has no user \
         activation, so `showPicker()` must throw `NotAllowedError` and the \
         offer status line must render it. Silence here is the exact Android \
         symptom this phase exists for — and the regression to look for is a \
         call site that swallows the `Err` from `util::show_file_picker`. \
         Detail: {refused}"
    );
    // The REASON, not merely some text: a status line that renders an empty
    // failure is the same dead end wearing a ✗.
    assert!(
        picker_text.contains("NotAllowedError") || picker_text.contains("activation"),
        "The picker refusal rendered, but not with the engine's reason — the \
         whole point is that the person is told WHY. Got: {picker_text:?}"
    );
    println!("  refused picker reported: {picker_text}");

    phase_gate!(client, "14.5");
    // -- Phase 14.5: a failed Connect must SAY SO ---------------------
    //
    // The reported bug: press Connect, the address box empties, and nothing —
    // no error, no row, no hint — appears anywhere in the window. It looked
    // like a dead button. Two separate defects produced it, and this phase
    // covers both, headless, on every box:
    //
    //   1. The outcome was invisible. `handle_connect_peer`'s failure path
    //      wrote only a `tracing::error!` and an Event Log line; the Peer
    //      Connections window had no outcome surface at all, so it could not
    //      report a failure even when there was one. (A *success* was equally
    //      invisible, which is the nastier half — see the model unit tests.)
    //   2. The typed address was consumed in the click handler, BEFORE the dial
    //      resolved, so a failure also ate the thing the user had typed.
    //
    // Deliberately OUTSIDE the display gate above: this needs no Tauri and no
    // listener. It dials an address nothing answers, which is the whole point —
    // the failure is the fixture. That also makes it the one connect-path phase
    // that runs on a headless box.
    let bogus = "ws://127.0.0.1:1/nothing-listens-here";
    let click = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                for (const sec of root.querySelectorAll('section.window')) {{
                    if (!sec.querySelector('.peer-connections')) continue;
                    const input = sec.querySelector('input[data-field="address"]');
                    if (!input) return {{ ok: false, reason: 'no-address-input' }};
                    input.value = '{bogus}';
                    // Real keystroke semantics: the field is a draft-tracked
                    // atom and Connect submits from the drafts map, not the DOM.
                    input.dispatchEvent(new Event('input', {{ bubbles: true }}));
                    for (const b of sec.querySelectorAll('button')) {{
                        if (b.textContent.trim() === 'Connect') {{
                            b.click();
                            return {{ ok: true }};
                        }}
                    }}
                    return {{ ok: false, reason: 'no-connect-btn' }};
                }}
                return {{ ok: false, reason: 'no-peer-connections-section' }};
                "#
            ),
            vec![],
        )
        .await?;
    assert_eq!(
        click.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "Could not drive Peer Connections → Connect. Detail: {click}"
    );

    // Poll, don't sleep: the dial fails asynchronously and each `execute` also
    // pumps the rAF loop a headless page would otherwise leave unpumped. The
    // budget is an upper bound — a healthy run returns on the first poll.
    let outcome = poll_json(
        &client,
        &format!(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {{
                if (!sec.querySelector('.peer-connections')) continue;
                const text = sec.innerText || '';
                const input = sec.querySelector('input[data-field="address"]');
                return {{
                    found: true,
                    // The ✗ error line the renderer emits for a failed attempt.
                    reported: text.includes('✗'),
                    // The address must still be in the box to retry/read.
                    kept: input ? input.value : null,
                }};
            }}
            return {{ found: false }};
            "#
        ),
        Duration::from_secs(20),
        |v| v.get("reported").and_then(|r| r.as_bool()) == Some(true),
    )
    .await?;

    assert_eq!(
        outcome.get("found").and_then(|v| v.as_bool()),
        Some(true),
        "Peer Connections section vanished mid-phase. Detail: {outcome}"
    );
    assert_eq!(
        outcome.get("reported").and_then(|v| v.as_bool()),
        Some(true),
        "A FAILED connect reported nothing in the Peer Connections window. \
         This is the reported bug: the user presses Connect, the dial fails, \
         and the window shows no error — indistinguishable from a dead button. \
         The failure must render as an error line (`components::error`). \
         Detail: {outcome}"
    );
    assert_eq!(
        outcome.get("kept").and_then(|v| v.as_str()),
        Some(bogus),
        "The address box was emptied by a FAILED connect. It used to be cleared \
         in the click handler, before the dial resolved, so a failure ate what \
         the user typed and left them retyping an address they could no longer \
         see. It must survive a failure. Detail: {outcome}"
    );
    println!("  failed connect reported in-window, and the typed address survived");


    phase_gate!(client, "14.6");
    // -- Phase 14.6: the connector registry, end to end on the Worker arm ---
    //
    // The Connectors section of Peer Connections: add a signaling node, select
    // it, remove it. Drives the SAME `crate::connectors` functions the
    // `connector` shell verb does — one model, two surfaces — so this covers
    // both.
    //
    // Deliberately OUTSIDE the display gate: it needs no Tauri and no listener.
    //
    // **The add DIALS now, and the address here is deliberately one that
    // refuses instantly.** The form learns a node's peer-id by asking the
    // address who it is; nothing is listening on `127.0.0.1:65535`, so this
    // exercises the *named* branch — a node you have identified but cannot
    // reach right now — which is the only branch that keeps this phase about
    // the registry write rather than about a node being up. A hostname would
    // put a DNS lookup in the everyday suite's critical path; a refused
    // loopback connection needs no resolver and fails in microseconds.
    //
    // **Why this phase has to exist at all.** The write goes to the system
    // peer's tree and the section reads it back through a subscription-fed
    // mirror. On the Worker arm a surface that reads a prefix nobody subscribed
    // reads silently EMPTY — so the add could succeed and the row never appear,
    // with every native test green (the native `Peers` is Direct, where the
    // mirror does not exist). That is exactly the failure mode that landed a
    // broken `delete_site` earlier the same day with 965 native tests passing.
    println!("--- Phase 14.6: connector registry (add / use / rm) ---");
    let add_connector = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.peer-connections')) continue;
                const inputs = sec.querySelectorAll('input');
                let id = null, addr = null;
                for (const i of inputs) {
                    const ph = i.getAttribute('placeholder') || '';
                    if (ph.startsWith('2K')) id = i;
                    if (ph.startsWith('wss://')) addr = i;
                }
                if (!id || !addr) return { ok: false, reason: 'no-connector-fields' };
                // Real keystroke events: the fields are draft-tracked atoms and
                // Add submits from the drafts map, not from the DOM — a silent
                // value-set is a fill no user can produce.
                id.value = '2KE2eNodeSeven';
                id.dispatchEvent(new Event('input', { bubbles: true }));
                addr.value = 'ws://127.0.0.1:65535';
                addr.dispatchEvent(new Event('input', { bubbles: true }));
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.trim() === 'Add connector') { b.click(); return { ok: true }; }
                }
                return { ok: false, reason: 'no-add-button' };
            }
            return { ok: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        add_connector.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "Could not drive the Connectors add form. Detail: {add_connector}"
    );

    let listed = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            if (!sec.querySelector('.peer-connections')) continue;
            const text = sec.textContent;
            return {
                listed: text.includes('2KE2eNod'),
                addr: text.includes('ws://127.0.0.1:65535'),
            };
        }
        return { listed: false, addr: false, reason: 'no-section' };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("listed").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert_eq!(
        listed.get("listed").and_then(|v| v.as_bool()),
        Some(true),
        "The added connector never appeared in the Connectors table. On the          Worker arm this is what an unsubscribed prefix looks like: the write          lands in the tree and the surface reading it stays silently empty          (the window must watch `connectors_prefix` + the selection path).          Detail: {listed}"
    );
    println!("  connector added and listed: {listed}");

    // It must ALREADY be the selection — nobody pressed Use.
    //
    // `add_connector` writes the selection when there is none, because a
    // registry holding rows and no selection resolves to no provisioning at
    // all: an app that looks configured, installs no establisher, and hands out
    // ids nobody can reach [AP22]. This phase used to click **Use** here; the
    // selected row deliberately renders no Use button, so that click now waits
    // forever for a control that should not exist. Asserting the mark appears on
    // its own is strictly stronger — it covers the add AND the selection, on the
    // Worker arm, through the surface a user drives.
    //
    // The Use button itself is still gated, on a SECOND connector, by
    // `selecting_a_connector_says_it_needs_a_reload` — where the button count is
    // also what proves the first row took the selection.
    let in_use = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            if (!sec.querySelector('.peer-connections')) continue;
            return { in_use: sec.textContent.includes('In use') };
        }
        return { in_use: false, reason: 'no-section' };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("in_use").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert_eq!(
        in_use.get("in_use").and_then(|v| v.as_bool()),
        Some(true),
        "The first connector added did not become the selection. Two things can          cause this: `add_connector` not writing the selection, or the window not          watching it — the selection is its own entity OUTSIDE the connectors          prefix, so it needs its OWN watch, without which the mark appears only          when some unrelated registry change happens to dirty the window.          Detail: {in_use}"
    );
    println!("  connector added and selected in one act (no Use press)");

    // Remove it. The selection must go with it — a selection naming a deleted
    // node is the dangling case, and provisioning must then resolve to nothing
    // rather than to some other row.
    let removed = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.peer-connections')) continue;
                const rows = sec.querySelectorAll('tr');
                for (const r of rows) {
                    if (!r.textContent.includes('2KE2eNod')) continue;
                    for (const b of r.querySelectorAll('button')) {
                        if (b.textContent.trim() === 'Delete') { b.click(); return { ok: true }; }
                    }
                }
                return { ok: false, reason: 'no-delete-button' };
            }
            return { ok: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        removed.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "Could not press Delete on the connector row. Detail: {removed}"
    );
    let gone = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            if (!sec.querySelector('.peer-connections')) continue;
            const text = sec.textContent;
            return { gone: !text.includes('2KE2eNod'), in_use: text.includes('In use') };
        }
        return { gone: false, reason: 'no-section' };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("gone").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert_eq!(
        gone.get("gone").and_then(|v| v.as_bool()),
        Some(true),
        "The removed connector is still listed. Detail: {gone}"
    );
    assert_eq!(
        gone.get("in_use").and_then(|v| v.as_bool()),
        Some(false),
        "The registry is empty but something still reports being in use — the          selection outlived the node it named. Detail: {gone}"
    );
    println!("  connector removed, and the selection went with it");

    phase_gate!(client, "14.7");
    // -- Phase 14.7: meet at a name — the surface reports, never spins -------
    //
    // The `lobby` / `tag` / `secret` naming modes (`crate::rendezvous`), driven
    // through the window the way a user drives them. Phase 14.6 left the
    // registry EMPTY, which is this phase's first fixture: with no connector
    // there is nowhere to meet, and the section has to say so rather than offer
    // a button that cannot work.
    //
    // What this phase can and cannot cover: a real meeting needs a real
    // signaling node and a second browser — that is `make e2e-webrtc-chat`'s
    // shape, not this suite's. What it covers instead is everything between the
    // click and the node, on the **Worker arm**, and specifically that BOTH
    // failure paths are visible: a refused input (`Mode::parse`) and a node that
    // cannot be reached. A search that reported neither would render as a
    // spinner that never resolves — the dead-button disease wearing a progress
    // indicator, and the reason the section carries its own status at all.
    println!("--- Phase 14.7: meet at a name (refusal + unreachable node) ---");
    let empty_state = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.peer-connections')) continue;
                const text = sec.textContent;
                let meet_btn = false;
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.trim() === 'Meet') meet_btn = true;
                }
                return {
                    card: text.includes('Meet at a name'),
                    needs_connector: text.includes('Select a connector first'),
                    meet_btn,
                };
            }
            return { card: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        empty_state.get("card").and_then(|v| v.as_bool()),
        Some(true),
        "The Meet section is missing from Peer Connections. Detail: {empty_state}"
    );
    assert_eq!(
        empty_state.get("needs_connector").and_then(|v| v.as_bool()),
        Some(true),
        "With an empty registry the Meet section must say a connector is needed. \
         Detail: {empty_state}"
    );
    assert_eq!(
        empty_state.get("meet_btn").and_then(|v| v.as_bool()),
        Some(false),
        "The Meet button is offered with no connector to meet through — pressing it \
         could only fail. Detail: {empty_state}"
    );
    println!("  no connector ⇒ the section says so, and offers no button");

    // Put a connector back and select it, so the form has somewhere to go. Same
    // drive as 14.6 — the address is deliberately unreachable, which is the
    // second half of this phase, and it is why the peer-id field (Advanced,
    // `connector_expect`) is filled: with nothing answering, the id can only
    // come from the person adding it.
    let armed = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.peer-connections')) continue;
                let id = null, addr = null;
                for (const i of sec.querySelectorAll('input')) {
                    const ph = i.getAttribute('placeholder') || '';
                    if (ph.startsWith('2K')) id = i;
                    if (ph.startsWith('wss://')) addr = i;
                }
                if (!id || !addr) return { ok: false, reason: 'no-connector-fields' };
                id.value = '2KE2eMeetNode';
                id.dispatchEvent(new Event('input', { bubbles: true }));
                addr.value = 'ws://127.0.0.1:65534';
                addr.dispatchEvent(new Event('input', { bubbles: true }));
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.trim() === 'Add connector') { b.click(); return { ok: true }; }
                }
                return { ok: false, reason: 'no-add-button' };
            }
            return { ok: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        armed.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "Could not re-add a connector for the meet. Detail: {armed}"
    );
    let selected = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            if (!sec.querySelector('.peer-connections')) continue;
            if (sec.textContent.includes('In use')) return { in_use: true };
            for (const b of sec.querySelectorAll('button')) {
                if (b.textContent.trim() === 'Use') { b.click(); return { in_use: false, clicked: true }; }
            }
            return { in_use: false, clicked: false };
        }
        return { in_use: false, reason: 'no-section' };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("in_use").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert_eq!(
        selected.get("in_use").and_then(|v| v.as_bool()),
        Some(true),
        "The meet's connector never became the selected one. Detail: {selected}"
    );

    // The form must now exist — and the mode picker with it.
    let form = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            if (!sec.querySelector('.peer-connections')) continue;
            const mode = sec.querySelector("[data-field='meet_mode']");
            const name = sec.querySelector("[data-field='meet_input']");
            let meet_btn = false;
            for (const b of sec.querySelectorAll('button')) {
                if (b.textContent.trim() === 'Meet') meet_btn = true;
            }
            return {
                mode: !!mode,
                name: !!name,
                meet_btn,
                modes: mode ? Array.from(mode.options).map(o => o.value).join(',') : '',
            };
        }
        return { mode: false, reason: 'no-section' };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("meet_btn").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert_eq!(
        form.get("meet_btn").and_then(|v| v.as_bool()),
        Some(true),
        "With a connector selected the Meet form must appear. Detail: {form}"
    );
    // TWO modes, not three, and `secret` is deliberately NOT among them.
    // `key::tag_key` and `secret_key` are the SAME derivation — SHA-256 over a
    // domain string, the mode tag and the bytes you typed — so neither word
    // leaves the device and the only difference is which domain the hash lands
    // in. That difference is invisible to the person choosing and produces a
    // silent never-meet: two people typing the same word under different modes
    // derive different keys and are told "nobody else was there". The Shell
    // keeps `meet secret <phrase>` for a client that uses it.
    //
    // **This assertion was left at three when the picker was narrowed to two modes,
    // and it is why the unfiltered suite was red on `dev` for two commits with
    // nobody noticing** — the one place that catches a rule change is a gate
    // that clicks the control, and this suite is expensive enough that it gets
    // run last. If you are here because you are re-adding `secret`, read
    // `render_meet`'s comment first: the mode is not what makes a name private,
    // entropy is.
    assert_eq!(
        form.get("modes").and_then(|v| v.as_str()),
        Some("tag,lobby"),
        "The mode picker must offer exactly the two naming modes it ships — these \
         are upstream mode tags, and a wrong one derives a key nobody else uses. \
         Detail: {form}"
    );
    println!("  connector selected ⇒ the form and all three modes are offered: {form}");

    // (a) A refused input reports itself. `tag` with nothing to meet at would
    //     otherwise derive a key from the empty string — silently.
    let refused = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.peer-connections')) continue;
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.trim() === 'Meet') { b.click(); return { ok: true }; }
                }
                return { ok: false, reason: 'no-meet-button' };
            }
            return { ok: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        refused.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "Could not press Meet. Detail: {refused}"
    );
    let refusal = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            if (!sec.querySelector('.peer-connections')) continue;
            const text = sec.textContent;
            return {
                said: text.includes('needs something to meet at'),
                searching: text.includes('Searching at'),
            };
        }
        return { said: false, reason: 'no-section' };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("said").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert_eq!(
        refusal.get("said").and_then(|v| v.as_bool()),
        Some(true),
        "Meet with an empty name reported nothing. A press that produces no visible \
         result is indistinguishable from a dead button. Detail: {refusal}"
    );
    assert_eq!(
        refusal.get("searching").and_then(|v| v.as_bool()),
        Some(false),
        "A refused input still started a search. Detail: {refusal}"
    );
    println!("  an empty name is refused, visibly, and starts nothing");

    // (b) A real search against an unreachable node ends in a visible failure.
    let started = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                if (!sec.querySelector('.peer-connections')) continue;
                const name = sec.querySelector("[data-field='meet_input']");
                if (!name) return { ok: false, reason: 'no-name-field' };
                name.value = 'chess';
                name.dispatchEvent(new Event('input', { bubbles: true }));
                for (const b of sec.querySelectorAll('button')) {
                    if (b.textContent.trim() === 'Meet') { b.click(); return { ok: true }; }
                }
                return { ok: false, reason: 'no-meet-button' };
            }
            return { ok: false, reason: 'no-section' };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        started.get("ok").and_then(|v| v.as_bool()),
        Some(true),
        "Could not start a meet at a tag. Detail: {started}"
    );
    // A started search must **report something** — and then it must END. Which
    // of the two lands first is a race we do not pretend to control: the node is
    // unreachable, and the dial can fail before the first poll of this test ever
    // sees the in-progress state. So the first wait accepts either, and only the
    // second one is an assertion about the outcome.
    let running = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            if (!sec.querySelector('.peer-connections')) continue;
            const text = sec.textContent;
            return {
                searching: text.includes('Searching at'),
                // The reach_node failure text specifically — NOT the address,
                // which is also sitting in the connector row two cards up and
                // would make this pass on a meet that reported nothing at all.
                failed: text.includes('connector: dialing'),
                stop_btn: Array.from(sec.querySelectorAll('button'))
                    .some(b => b.textContent.trim() === 'Stop'),
            };
        }
        return { searching: false, reason: 'no-section' };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| {
            v.get("searching").and_then(|b| b.as_bool()).unwrap_or(false)
                || v.get("failed").and_then(|b| b.as_bool()).unwrap_or(false)
        },
    )
    .await?;
    assert!(
        running.get("searching").and_then(|v| v.as_bool()).unwrap_or(false)
            || running.get("failed").and_then(|v| v.as_bool()).unwrap_or(false),
        "Starting a meet produced no visible state at all — neither a search in \
         progress nor a failure. Detail: {running}"
    );
    if running.get("searching").and_then(|v| v.as_bool()).unwrap_or(false) {
        // Caught it mid-search: then there must be a way out of it.
        assert_eq!(
            running.get("stop_btn").and_then(|v| v.as_bool()),
            Some(true),
            "A running meet offers no way to stop it — it would poll for its whole \
             window with no way out. Detail: {running}"
        );
        println!("  meet observed in progress, with a way to stop it: {running}");
    } else {
        println!("  meet failed before the first look — that is a report too: {running}");
    }

    // It ENDS, and says why. The node is unreachable by construction, so a
    // session that neither finds nor fails is the spinner this whole surface is
    // written to avoid.
    //
    // What ends it is the failed dial, not `rendezvous::SETUP_BUDGET` — the dial
    // rejects promptly here. The budget is the backstop for a round trip that
    // never returns at all, and this phase does not exercise it.
    let settled = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            if (!sec.querySelector('.peer-connections')) continue;
            const text = sec.textContent;
            return {
                failed: text.includes('connector: dialing'),
                searching: text.includes('Searching at'),
            };
        }
        return { failed: false, reason: 'no-section' };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("failed").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert_eq!(
        settled.get("failed").and_then(|v| v.as_bool()),
        Some(true),
        "A meet at an unreachable node never reported a failure. This is the exact \
         shape that was broken once already: the dial DID fail, and the window kept \
         rendering 'Searching…' because nothing repainted it (`MeetSession::\
         take_changed`). Detail: {settled}"
    );
    assert_eq!(
        settled.get("searching").and_then(|v| v.as_bool()),
        Some(false),
        "The meet failed but still claims to be searching. Detail: {settled}"
    );
    println!("  meet at an unreachable node failed visibly: {settled}");

    // Leave the registry as we found it. The selection is mirrored into
    // localStorage for the pre-peer boot path, so a leftover row would hand the
    // §6.5 provisioning an unreachable node for every phase after this one —
    // including the reload in Phase 11, which would come back up provisioned
    // with it.
    let cleaned = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            if (!sec.querySelector('.peer-connections')) continue;
            if (!sec.textContent.includes('2KE2eMeetNod')) return { clean: true };
            for (const r of sec.querySelectorAll('tr')) {
                if (!r.textContent.includes('2KE2eMeetNod')) continue;
                for (const b of r.querySelectorAll('button')) {
                    if (b.textContent.trim() === 'Delete') { b.click(); return { clean: false }; }
                }
            }
            return { clean: false, reason: 'no-delete-button' };
        }
        return { clean: false, reason: 'no-section' };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("clean").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert_eq!(
        cleaned.get("clean").and_then(|v| v.as_bool()),
        Some(true),
        "The meet's connector outlived the phase — later phases (and the Phase 11 \
         reload) would boot provisioned with an unreachable node. Detail: {cleaned}"
    );
    println!("  registry left clean for the phases that follow");
    phase_gate!(client, "11");
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
    let phase11_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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

    phase_gate!(client, "15");
    // -- Phase 15: Stage 2B — multi-SDK backend peer creation --------
    //
    // Clicks the "+ Worker (memory)" and "+ Worker (OPFS)" buttons.
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
    //
    // The footer is localized, which broke this parse in TWO ways once the
    // peers surface was extracted:
    //
    //  1. The peer count is the `peer.count` CLDR plural (`{n} peer` /
    //     `{n} peers`) — never the literal `N peer(s)` this used to match.
    //  2. `t()`/`t_plural()` **bidi-isolate every interpolated arg**, wrapping
    //     it in FSI (U+2068) … PDI (U+2069) so an LTR number stays LTR inside
    //     an RTL sentence. So the rendered text is `3 peers`, and `\d+ peers`
    //     cannot match across the invisible PDI. This defeated *both* regexes
    //     below, which is why the parse returned -1.
    //
    // Strip the isolate marks before matching. Any assertion in this suite that
    // regex-matches rendered UI text across an interpolated value needs to do
    // the same — the marks are correct output, not noise to be removed at the
    // source. (This went unnoticed because Phase 14 sits earlier and hard-errors
    // without the Tauri binary, so nothing down here ran.) The suite pins
    // English (`host_locale=en`), so matching the en plural forms is correct.
    let read_sdk_count_script = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        const sections = root.querySelectorAll('section.window');
        for (const sec of sections) {
            const h2 = sec.querySelector('h2');
            if (!h2 || h2.textContent.trim() !== 'Peers') continue;
            // Drop Unicode bidi isolates (FSI/LRI/RLI/PDI) that t() wraps
            // around every interpolated arg — they sit between the number and
            // its noun and would break every match below.
            const text = (sec.textContent || '').replace(/[\u2066-\u2069]/g, '');
            const dedicated_re = /1 boot \+ (\d+) dedicated worker/;
            const m = text.match(dedicated_re);
            if (m) return 1 + parseInt(m[1], 10);
            // Footer with no dedicated workers: just the peer count
            // (`1 peer` / `3 peers`). sdk_count is 1 (only the boot SDK).
            if (/\d+ peers?\b/.test(text)) return 1;
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
    // `kind` is the create-form option value (persist_key), not a button label —
    // see `create_peer_form_js`.
    fn click_mode_btn_js(kind: &str) -> String {
        create_peer_form_js(kind)
    }

    let bm_status_v = client
        .execute(&click_mode_btn_js("backend-memory"), vec![])
        .await?;
    let bm_status = bm_status_v.as_str().unwrap_or("non-string");
    println!("  + Worker (memory):    {bm_status}");
    assert_eq!(bm_status, "clicked", "Couldn't click '+ Worker (memory)' button");
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
        .execute(&click_mode_btn_js("backend-opfs"), vec![])
        .await?;
    let bo_status = bo_status_v.as_str().unwrap_or("non-string");
    println!("  + Worker (OPFS):      {bo_status}");
    assert_eq!(bo_status, "clicked", "Couldn't click '+ Worker (OPFS)' button");
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
                return sec.querySelectorAll('tbody tr:not(.peer-group)').length;
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

    phase_gate!(client, "15.5");
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
    let new_shell_deadline = std::time::Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
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
    let connect_deadline = std::time::Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
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

    if tauri.is_none() {
        println!("--- Phase 15.6 SKIPPED: no display (needs the Phase-14 Tauri listener) ---");
    }
    if let Some(tauri) = &tauri {
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

        // Use the same short_pid format the verb prints (first-8...last-6).
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
        // 30s, not the 3s this used to be. The old budget was a guess at how
        // long a loopback ws connect takes ("fast on loopback — 2s is plenty"),
        // and it was measured failing on a loaded box in the 2026-08-13 soak
        // (`f3-7`) with a sibling failure pre-dating that session's fixes — the
        // fixed-budget-against-a-spawned-process shape, not a defect in the
        // connect. Cost of the headroom is zero on a healthy run: the loop
        // returns on the first scrollback read that carries the success line,
        // and an explicit `✗ connect ws://` still breaks out immediately, so a
        // genuine failure is still reported fast rather than waiting out 30s.
        let ws_started = std::time::Instant::now();
        let ws_deadline = ws_started + ASYNC_ROUND_TRIP_BUDGET;
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
                 success line within {}s. If the scrollback shows \
                 \"expected xworker:// scheme\", the host's MultiConnector \
                 composition regressed (kernel-side).\n\
                 Last scrollback:\n{}\n\
                 Tauri child stdout tail (the listener side of this connect):\n{}",
                tauri.ws_addr,
                ASYNC_ROUND_TRIP_BUDGET.as_secs(),
                ws_last_sb,
                tauri.stdout_tail(),
            );
        }
        // Print the elapsed time on SUCCESS, not just on failure. Raising this
        // budget from 3s to 30s removed an accidental signal — a tight budget
        // fails when something gets slower, which is information. Printing the
        // measurement keeps that information without the false failures: a
        // healthy connect lands in well under a second, so a green run that
        // suddenly reports seconds here is a regression worth chasing even
        // though it passed.
        println!(
            "  ✓ ws connect from backend Worker completed in {}ms \
             (MultiConnector composes both schemes)",
            ws_started.elapsed().as_millis()
        );
    }

    phase_gate!(client, "15.7");
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
    // Same reasoning as 15.6's budget: an upper bound, not a wait. This one is
    // NOT display-gated, so its fixed 3s was exposed on every headless run too.
    let boot_started = std::time::Instant::now();
    let boot_deadline = boot_started + ASYNC_ROUND_TRIP_BUDGET;
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
             not produce success line within {}s.\nLast scrollback:\n{}",
            system_pid,
            ASYNC_ROUND_TRIP_BUDGET.as_secs(),
            boot_last_sb
        );
    }
    println!(
        "  ✓ boot-worker primary reachable via xworker:// from a sibling Worker ({}ms)",
        boot_started.elapsed().as_millis()
    );

    phase_gate!(client, "15.8");
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

    // Click `+ Main thread (memory)` in the Peers window — same button (a
    // main-thread in-memory peer, was "+ Frontend") Phase 12 exercised
    // pre-reload. The click dispatches `Action::CreateFrontendPeer` →
    // `create_frontend_peer`.
    let frontend_status = client
        .execute(&click_mode_btn_js("frontend"), vec![])
        .await?;
    assert_eq!(
        frontend_status.as_str().unwrap_or(""),
        "clicked",
        "couldn't click '+ Main thread (memory)' in Peers window"
    );

    // Wait for localStorage to grow — that's our signal that the
    // CreatePeer round-trip completed and `create_frontend_peer`
    // persisted the new entry. 3s budget for the worker round-trip
    // on a dev box.
    let create_deadline = std::time::Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
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
    let runtime_deadline = std::time::Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
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

    phase_gate!(client, "16");
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
    let phase16_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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
    // POLL, do not sleep-then-read. A respawned backend peer lands through the
    // `pending` queue on some later frame, and 800ms was a guess that happened
    // to hold while the respawns were fired IN PARALLEL with the boot worker's
    // handshake. They are deliberately fired AFTER it now (that parallelism
    // wedged boot — see `app.rs`' note at the `pending` declaration), so they
    // start later and this sampled before the second one had attached. The
    // property under test is unchanged — the count must MATCH across the reload
    // — and the budget is an upper bound a healthy run returns from on the first
    // poll.
    let post_reload_sdk_count = poll_json(
        &client,
        read_sdk_count_script,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.as_i64().unwrap_or(-3) >= pre_phase16_sdk_count,
    )
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

    phase_gate!(client, "17");
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
            for (const row of sec.querySelectorAll('tbody tr:not(.peer-group)')) {{
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
            const rows = sec.querySelectorAll('tbody tr:not(.peer-group)');
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
    let phase17_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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
    phase_gate!(client, "18");
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
    let phase18_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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
    let persist_deadline = std::time::Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
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
    let phase18_reload_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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
    phase_gate!(client, "19");
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

    phase_gate!(client, "19-doc");
    // -- Phase 19-doc: a `format:html` page renders in a RESTRICTED sandbox --
    //
    // The web-tier escape hatch (convention §3.1) is what carries a
    // pre-rendered Pandoc paper/book. Its safety does not come from a
    // sanitizer — there is none — it comes from WHERE the bytes are mounted:
    // an `<iframe sandbox="">` with every restriction on. So this phase asserts
    // the two independent halves of that claim, and deliberately does NOT
    // settle for the easy one:
    //
    //   (a) the document did NOT reach our origin — the marker id inside it is
    //       unreachable from our DOM. Only a regression to `set_inner_html`
    //       can make this assertion fail, which is why it is phrased as a
    //       negative on the document's own content rather than as "an iframe
    //       exists".
    //   (b) nothing in it executed — the document carries a script that would
    //       rewrite its own paragraph. Comparing the sandbox ATTRIBUTE would
    //       only check a spelling; reading the rendered text checks the
    //       property. WebDriver can enter an opaque-origin frame; the app
    //       cannot, and that asymmetry is exactly what makes this checkable
    //       from here and nowhere else.
    //
    // Navigation is a real click on the index page's link, so the resolve →
    // render → mount path is the shipped one.
    println!("--- Phase 19-doc: format:html page → restricted sandbox ---");

    let doc_nav = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const link = Array.from(sl.querySelectorAll('a'))
                .find(a => (a.textContent || '').includes('Pre-Rendered Document'));
            if (!link) return { clicked: false,
                links: Array.from(sl.querySelectorAll('a')).map(a => a.textContent.trim()) };
            link.click();
            return { clicked: true };
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        doc_nav.get("clicked").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 19-doc: no link to the document page on the demo index: {doc_nav:?}"
    );

    // Poll rather than sleep on a guess — the nav routes through an Action and
    // a rebuild, and a fixed sleep is the known source of load-dependent flake.
    let mut doc_state = serde_json::Value::Null;
    let doc_deadline = std::time::Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
    while std::time::Instant::now() < doc_deadline {
        doc_state = client
            .execute(
                r#"
                const sl = document.getElementById('site-layer');
                const frames = Array.from(sl.querySelectorAll('iframe'));
                const f = frames[0];
                return {
                    frame_count: frames.length,
                    // "" and null are DIFFERENT: a missing attribute is NO
                    // sandbox at all, and getAttribute returns null for it.
                    // Read both so the two can never be confused.
                    sandbox_attr: f ? f.getAttribute('sandbox') : null,
                    has_sandbox: f ? f.hasAttribute('sandbox') : false,
                    // The document is delivered as its own `data:` URL, not
                    // inline `srcdoc` — that is what gives it a base URL of
                    // its own, which is what makes its table of contents
                    // resolve to itself instead of to OUR page.
                    src_len: f ? (f.getAttribute('src') || '').length : 0,
                    src_scheme: f ? (f.getAttribute('src') || '').slice(0, 33) : '',
                    has_srcdoc: f ? f.hasAttribute('srcdoc') : false,
                    // (a) The document's own marker must NOT be in our tree.
                    marker_in_our_dom:
                        !!document.getElementById('entity-demo-doc-marker'),
                    // Nor may the markdown pane have been used for it.
                    markup_pane_present: !!sl.querySelector('.cs-doc'),
                };
                "#,
                vec![],
            )
            .await?;
        if doc_state.get("frame_count").and_then(|v| v.as_i64()).unwrap_or(0) > 0 {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }

    assert_eq!(
        doc_state.get("frame_count").and_then(|v| v.as_i64()),
        Some(1),
        "Phase 19-doc: the document page did not mount exactly one frame: {doc_state:?}"
    );
    assert_eq!(
        doc_state.get("has_sandbox").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 19-doc: the document frame carries NO sandbox attribute — an \
         absent attribute is no sandbox at all: {doc_state:?}"
    );
    assert_eq!(
        doc_state.get("marker_in_our_dom").and_then(|v| v.as_bool()),
        Some(false),
        "Phase 19-doc: the document's own element is in OUR DOM — it was \
         injected into our origin instead of the frame (stored XSS): {doc_state:?}"
    );
    assert_eq!(
        doc_state.get("markup_pane_present").and_then(|v| v.as_bool()),
        Some(false),
        "Phase 19-doc: a document page rendered through the markdown pane: {doc_state:?}"
    );
    // NOTE: "is the body big enough" is a check on the DELIVERY SPELLING and
    // lives at the bottom with the others, not here. Placed here it fired
    // first under mutation and the anchor assertion below was never reached —
    // the same shadowing that already cost this phase its script-inertness
    // gate once. "Did the body arrive" is covered behaviourally by
    // `marker_here` inside the frame.

    // (b) Step INSIDE the frame and read what a reader would see. This is the
    // only assertion that can tell "scripts are blocked" from "the attribute
    // is spelled right", and it is why this phase enters the frame at all.
    client.enter_frame(0).await?;
    let inside = client
        .execute(
            r#"
            const probe = document.getElementById('script-probe');
            const deep = document.getElementById('entity-demo-doc-deep');
            return {
                probe_text: probe ? probe.textContent : null,
                marker_here: !!document.getElementById('entity-demo-doc-marker'),
                // The document's own stylesheet must have applied — that is
                // the whole point of carrying it verbatim.
                serif: probe ? getComputedStyle(probe).fontFamily : '',
                // Where the anchor target sits BEFORE any jump. It has to
                // start off-screen or "it jumped" is unfalsifiable — that is
                // exactly how the previous demo document, which was short
                // enough to fit, carried a claim about anchors that was false.
                deep_top_before: deep
                    ? Math.round(deep.getBoundingClientRect().top) : null,
                hash_before: location.hash,
            };
            "#,
            vec![],
        )
        .await?;
    client.enter_parent_frame().await?;

    assert_eq!(
        inside.get("marker_here").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 19-doc: the document did not render inside the frame — a blank \
         frame is indistinguishable from a working one, so this is the check \
         that says it actually arrived: {inside:?}"
    );
    let probe_text = inside.get("probe_text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        !probe_text.contains("SCRIPT RAN"),
        "Phase 19-doc: a script INSIDE the document executed — the sandbox is \
         not restricting scripts. This is the stored-XSS boundary: {inside:?}"
    );
    assert!(
        probe_text.contains("Scripts do not run here"),
        "Phase 19-doc: the document's own paragraph text is missing: {inside:?}"
    );
    assert!(
        inside.get("serif").and_then(|v| v.as_str()).unwrap_or("").contains("serif"),
        "Phase 19-doc: the document's own stylesheet did not apply — carrying \
         it verbatim is pointless if its typography is lost: {inside:?}"
    );

    // (c) THE DOCUMENT'S OWN TABLE OF CONTENTS. A pre-rendered paper navigates
    // entirely by `#anchor`, and a book is one file whose every chapter jump is
    // one. Under `srcdoc` these resolved against OUR base URL, so a TOC click
    // replaced the paper with the app's host page — the document rendered
    // perfectly and could not be read past the first screen. The fix is the
    // `data:` delivery above, and this is the assertion that holds it.
    //
    // Both halves are needed. "Still the document" alone passes if the click
    // did nothing; "scrolled" alone passes if the frame navigated somewhere
    // that happens to be tall.
    let deep_before = inside.get("deep_top_before").and_then(|v| v.as_i64()).unwrap_or(0);
    assert!(
        deep_before > 400,
        "Phase 19-doc: the anchor target starts on screen, so a jump to it is \
         unobservable and this gate proves nothing. The demo document's spacer \
         must keep it below the fold: {inside:?}"
    );

    client.enter_frame(0).await?;
    client
        .execute("document.getElementById('entity-demo-doc-toc').click(); return true;", vec![])
        .await?;
    // Poll, don't sleep: the jump is a same-document navigation plus a scroll,
    // and a fixed wait here reported a working tier as broken on one run out
    // of six while measuring this.
    let mut jumped = serde_json::Value::Null;
    let jump_deadline = std::time::Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
    while std::time::Instant::now() < jump_deadline {
        jumped = client
            .execute(
                r#"
                const deep = document.getElementById('entity-demo-doc-deep');
                return {
                    hash: location.hash,
                    title: document.title,
                    // Still OUR document? A frame that navigated away to the
                    // app's host page has no marker in it.
                    marker_here: !!document.getElementById('entity-demo-doc-marker'),
                    deep_top: deep
                        ? Math.round(deep.getBoundingClientRect().top) : null,
                    scrolled: Math.round(window.scrollY),
                };
                "#,
                vec![],
            )
            .await?;
        if jumped.get("hash").and_then(|v| v.as_str()) == Some("#entity-demo-doc-deep")
            && jumped.get("deep_top").and_then(|v| v.as_i64()).map(|t| t < 100).unwrap_or(false)
        {
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    client.enter_parent_frame().await?;

    assert_eq!(
        jumped.get("marker_here").and_then(|v| v.as_bool()),
        Some(true),
        "Phase 19-doc: clicking the document's own table-of-contents link \
         navigated the frame AWAY from the document — this is the `srcdoc` \
         base-URL bug: the anchor resolved against our page instead of the \
         paper's. The document must be delivered with a base URL of its own: \
         {jumped:?}"
    );
    let deep_after = jumped.get("deep_top").and_then(|v| v.as_i64()).unwrap_or(i64::MAX);
    // Tight on purpose. The demo document carries trailing height so the target
    // can reach the TOP of the frame; without that it stops part-way down and
    // only a loose bound is assertable, which would also pass for a document
    // that merely scrolled a bit.
    assert!(
        deep_after < 100,
        "Phase 19-doc: the table-of-contents link did not move the document to \
         its target — it is still {deep_after}px away (was {deep_before}px). \
         An anchor that is inert is what `blob:` under this sandbox tier does; \
         a paper's whole navigation is these links: {jumped:?}"
    );
    println!(
        "  document TOC anchor jumped: target {deep_before}px → {deep_after}px, \
         still the same document"
    );

    // (c2) THE SAME JUMP, EIGHT MORE TIMES — because one jump cannot tell
    // "anchors work" from "anchors work once". The check above clicks a single
    // anchor, and a single anchor is roughly how far the original delivery got:
    // the `blob:` URL was revoked on the frame's `load` event, which serves the
    // first several same-document navigations and then silently stops granting
    // them, so every real book went dead a few chapters in.
    //
    // **READ THIS BEFORE TRUSTING IT: this run does NOT catch that defect.**
    // Mutation-checked, twice. Reintroducing the exact shipped bug (revoke on
    // the frame's own `load`) leaves this phase GREEN at 8/8 with
    // `history.length` incrementing the whole way. A cruder mutation — revoking
    // before the frame loads — does go red, but on the earlier "did it render
    // at all" assertion, so it proves nothing about this loop either.
    //
    // What DOES separate the two, measured in an isolated page with trusted
    // WebDriver clicks (not the synthetic `.click()` used here), varying only
    // the revoke:
    //
    //   revoke on load        7/12 jumps      revoke on replacement   12/12
    //
    // and that split holds at EVERY document size tried — 21 KB, 1.2 MB, and
    // all seven real books from 0.81 MB to 7.82 MB. So the blind spot is not
    // fixture size, which is what it was for the 2 MiB `data:` ceiling; it is
    // something about this harness, and it is **unidentified**. Until it is,
    // the real check is the one that found the bug: serve a real book and click
    // its table of contents a dozen times. Do not read a green 19-doc as
    // evidence that document navigation survives repetition.
    //
    // The loop is kept because it is still a gate against *total* anchor
    // breakage and it prints its trace on pass, so a future run degrading from
    // 8 to 5 is visible rather than binary. The assertion is on EVERY jump, not
    // the last — a run that dies at 5 and is only checked at 8 is
    // indistinguishable from one that never worked. `history.length` rides
    // along because it is the underlying signal: it stops incrementing at the
    // exact click the hash stops changing.
    client.enter_frame(0).await?;
    let mut chapter_trace: Vec<String> = Vec::new();
    let mut first_dead: Option<usize> = None;
    for n in 1..=8u32 {
        let id = format!("entity-demo-ch{n}");
        client
            .execute(
                &format!(
                    "document.querySelector('a[href=\"#{id}\"]').click(); return true;"
                ),
                vec![],
            )
            .await?;
        let mut moved = serde_json::Value::Null;
        let deadline = std::time::Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
        while std::time::Instant::now() < deadline {
            moved = client
                .execute(
                    &format!(
                        r#"
                        const t = document.getElementById('{id}');
                        return {{
                            hash: location.hash,
                            hist: history.length,
                            top: t ? Math.round(t.getBoundingClientRect().top) : null,
                            alive: !!document.getElementById('entity-demo-doc-marker'),
                        }};
                        "#
                    ),
                    vec![],
                )
                .await?;
            if moved.get("hash").and_then(|v| v.as_str()) == Some(&format!("#{id}")) {
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        let hash = moved.get("hash").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let hist = moved.get("hist").and_then(|v| v.as_i64()).unwrap_or(-1);
        chapter_trace.push(format!("ch{n}:{hash}(h{hist})"));
        if hash != format!("#{id}") && first_dead.is_none() {
            first_dead = Some(n as usize);
        }
        assert_eq!(
            moved.get("alive").and_then(|v| v.as_bool()),
            Some(true),
            "Phase 19-doc: the document vanished on chapter jump {n} — the frame \
             navigated away or the delivery died mid-read: {moved:?}"
        );
    }
    client.enter_parent_frame().await?;
    // Printed on PASS as well as fail: a green run still carries the evidence,
    // which is what lets a future reader see a run degrade from 8 to 5 rather
    // than only see it cross zero.
    println!("  document chapter jumps: {}", chapter_trace.join(" "));
    assert!(
        first_dead.is_none(),
        "Phase 19-doc: the document stopped honouring its own anchors at chapter \
         {} of 8 — the earlier jumps worked, so this is not 'anchors are broken', \
         it is the delivery being revoked out from under a document that is still \
         mounted. A reader meets this as a book that goes dead a few chapters in. \
         Trace: {}",
        first_dead.unwrap_or(0),
        chapter_trace.join(" ")
    );

    // The attribute check comes LAST, deliberately. It is the cheap one, and
    // when it ran first it *shadowed* the behavioural check above: mutating the
    // tier to `allow-scripts` failed here and the suite never reached the
    // assertion that actually proves scripts are inert — so that assertion had
    // never been seen red. Ordering the property before its spelling is what
    // makes both of them gates. (Verified by mutation in both orders.)
    // EXACTLY this string. `allow-same-origin` alone grants an origin to a
    // document that cannot use it — no scripts, so nothing in the frame can
    // reach a cookie, storage or our DOM (the inertness assertion above is what
    // proves that half). Adding `allow-scripts` beside it is the one edit that
    // turns this into full origin access for every publisher on the network,
    // which is why the comparison is `==` against the whole attribute rather
    // than a "contains" that a second token would slip past.
    assert_eq!(
        doc_state.get("sandbox_attr").and_then(|v| v.as_str()),
        Some("allow-same-origin"),
        "Phase 19-doc: the document sandbox must be EXACTLY `allow-same-origin` \
         — and never with `allow-scripts`, which together hand every publisher \
         our origin via parent.document: {doc_state:?}"
    );
    // Same reasoning, same position: the DELIVERY spelling is cheap and the
    // behaviour above is what matters, so it is checked after. Its value is
    // naming the cause when the anchor assertion goes red — `srcdoc` present
    // is the bug, in one word.
    //
    // `blob:` and not `data:`, which shipped for one commit: Chrome caps a
    // `data:` URL at 2 MiB and renders a BLANK FRAME past it, with no error and
    // no event. The published corpus book is 10.9 MB base64, so every real book
    // was blank in one of our two engines while this phase's 5 KB demo document
    // stayed green. A ceiling only the real payload crosses cannot be caught by
    // a fixture that never approaches it — which is why the size number lives in
    // the module docs and this assertion pins the delivery that has no ceiling.
    assert!(
        doc_state
            .get("src_scheme")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .starts_with("blob:"),
        "Phase 19-doc: the document frame is not delivered from a `blob:` URL. \
         `srcdoc` inherits OUR base URL and breaks every anchor; `data:` is \
         capped at 2 MiB in Chrome and renders real books blank: {doc_state:?}"
    );
    assert_eq!(
        doc_state.get("has_srcdoc").and_then(|v| v.as_bool()),
        Some(false),
        "Phase 19-doc: the frame still carries a `srcdoc` attribute — with both \
         set, `srcdoc` wins and the base-URL bug is back: {doc_state:?}"
    );
    println!("  document mounted sandboxed; scripts inert, own stylesheet applied");

    let doc_log = capture_log(&client).await?;
    let doc_panics = count_panics(&doc_log);
    assert!(
        doc_panics.is_empty(),
        "Phase 19-doc: rendering the document page panicked:\n{}",
        doc_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );

    // Return to the index so the phases after this one start where they expect.
    client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const home = Array.from(sl.querySelectorAll('a'))
                .find(a => (a.textContent || '').trim() === 'Home');
            if (home) home.click();
            return !!home;
            "#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(400)).await;

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

    // 19c-x — CROSS-SITE navigation (the feature billslab ships on). Return
    // Home (the demo index carries a `site:demo-notes/index` cross-site link),
    // click it, and assert the COMPANION site rendered — proving `site:` nav
    // routes through the shared rewrite_links → classify_link → go_to path.
    // Then follow the companion's `site:demo/index` link back and assert we
    // land on the primary demo index again (the round trip).
    let home_click = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            for (const a of (sl ? sl.querySelectorAll('a') : [])) {
                if ((a.textContent || '').trim() === 'Home') { a.click(); return 'clicked'; }
            }
            return 'no-home-link';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        home_click.as_str(),
        Some("clicked"),
        "Phase 19: couldn't find the 'Home' nav link to return to the demo index"
    );
    sleep(Duration::from_millis(500)).await;

    // 19c-t setup — MANIFEST SITE THEME (S-T2) needs the "Site's theme"
    // appearance mode (the manifest applies ONLY there). Phase 3 left the
    // localStorage mirror on "system", so pin it to "site" for this
    // sub-phase and restore the prior value at the end (state hygiene for
    // later phases). The mirror IS the render loop's designed input
    // (`site_appearance_current`, read per frame); the Settings-select →
    // set_site_appearance → apply_site_appearance delivery path is Phase
    // 3's coverage. Also empty the #site-theme-vars :root block "system"
    // installed, as selecting "site" in Settings would.
    let prior_site_mode = client
        .execute(
            r#"
            const prev = localStorage.getItem('entity_site_appearance');
            localStorage.setItem('entity_site_appearance', 'site');
            const el = document.getElementById('site-theme-vars');
            if (el) el.textContent = '';
            return prev;
            "#,
            vec![],
        )
        .await?;
    let prior_site_mode = prior_site_mode.as_str().unwrap_or("site").to_string();
    sleep(Duration::from_millis(400)).await;

    // The PRIMARY demo declares no manifest theme, so in "site" mode its
    // wrapper renders the default dark site palette (S-T2 must not leak the
    // companion's theme onto it).
    let demo_bg = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const w = sl ? sl.querySelector('div') : null;
            return w ? getComputedStyle(w).backgroundColor : 'no-wrapper';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        demo_bg.as_str(),
        Some("rgb(16, 16, 24)"), // #101018 — the --site-bg default
        "Phase 19: the unthemed demo site should render the default dark palette: {demo_bg:?}"
    );

    let cross_click = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            for (const a of (sl ? sl.querySelectorAll('a') : [])) {
                if ((a.textContent || '').trim() === 'Field Notes') { a.click(); return 'clicked'; }
            }
            return 'no-field-notes-link';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        cross_click.as_str(),
        Some("clicked"),
        "Phase 19: couldn't find the 'Field Notes' cross-site link on the demo index"
    );
    sleep(Duration::from_millis(600)).await;

    let cross = client
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
        cross.get("container_class").and_then(|v| v.as_str()),
        Some("mode-site"),
        "Phase 19: cross-site nav must stay in site mode (no reload): {cross:?}"
    );
    let cross_text = cross.get("site_text").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        cross_text.contains("Field Notes") && cross_text.contains("cross-site link"),
        "Phase 19: the cross-site link didn't render the companion site; got: {cross_text:?}"
    );
    println!("  overlay followed a cross-site link to the companion site");

    // 19c-t — MANIFEST SITE THEME (S-T2). The companion's manifest declares
    // `"theme": "light"`; in the default "Site's theme" appearance mode the
    // renderer freezes the whole `--site-*` family onto the site's own
    // wrapper (container-scoped — the primary demo just asserted it stayed
    // dark). Assert the LIGHT palette actually painted, then flip the
    // appearance mode to a strict override and assert the user's choice
    // wins WHILE the themed site stays open — the container block must
    // vanish on the next rebuild (the output-equality guard carries the
    // mode, the exact mechanism a renderer-side gate would break). The
    // Settings-select → set_site_appearance → apply_site_appearance path is
    // Phase 3's coverage; the localStorage mirror driven here IS the render
    // loop's designed input (`site_appearance_current`, read per frame).
    let themed_bg = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const w = sl ? sl.querySelector('div') : null;
            return w ? getComputedStyle(w).backgroundColor : 'no-wrapper';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        themed_bg.as_str(),
        Some("rgb(246, 246, 250)"), // #f6f6fa — LIGHT's --overlay-bg via --site-bg
        "Phase 19: the companion's manifest theme (light) didn't paint its wrapper: {themed_bg:?}"
    );

    // Strict override: "Always Dark" must beat the manifest theme.
    client
        .execute(
            r#"localStorage.setItem('entity_site_appearance', 'dark'); return true;"#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(400)).await;
    let strict_bg = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const w = sl ? sl.querySelector('div') : null;
            return w ? getComputedStyle(w).backgroundColor : 'no-wrapper';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        strict_bg.as_str(),
        Some("rgb(16, 16, 24)"),
        "Phase 19: strict 'Always Dark' must override the manifest theme \
         (stale container vars defeat the user's choice): {strict_bg:?}"
    );

    // Back to "Site's theme" → the manifest theme re-applies live.
    client
        .execute(
            r#"localStorage.setItem('entity_site_appearance', 'site'); return true;"#,
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(400)).await;
    let restored_bg = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const w = sl ? sl.querySelector('div') : null;
            return w ? getComputedStyle(w).backgroundColor : 'no-wrapper';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        restored_bg.as_str(),
        Some("rgb(246, 246, 250)"),
        "Phase 19: returning to 'Site's theme' must re-apply the manifest theme: {restored_bg:?}"
    );
    let theme_log = capture_log(&client).await?;
    let theme_panics = count_panics(&theme_log);
    assert!(
        theme_panics.is_empty(),
        "Phase 19: manifest-theme flips panicked the frame loop:\n{}",
        theme_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );
    println!("  manifest site theme: light applied · strict override wins · site mode restores");

    // Restore the appearance mode this sub-phase found (Phase 3 set it via
    // the real Settings path; later phases see the state they expect).
    client
        .execute(
            &format!(
                r#"localStorage.setItem('entity_site_appearance', '{prior_site_mode}'); return true;"#
            ),
            vec![],
        )
        .await?;
    sleep(Duration::from_millis(300)).await;

    // Home-reset fix: while on the COMPANION site, click the ⌂ Home button (the
    // site-title anchor, `title="Go to site home"`). It must return to the
    // configured HOME site (the demo), NOT the companion's own root — the old
    // code wired Home to `/` (the current site's root), which is exactly what
    // stranded a user on an unresolvable location ("No site manifest…", where
    // `/` just reloads the error). Landing on the demo index proves the fix.
    let home_btn = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            const a = sl ? sl.querySelector('a[title="Go to site home"]') : null;
            if (a) { a.click(); return 'clicked'; }
            return 'no-home-button';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        home_btn.as_str(),
        Some("clicked"),
        "Phase 19: couldn't find the ⌂ Home button on the companion site"
    );
    sleep(Duration::from_millis(600)).await;

    let home_text = client
        .execute(
            r#"
            const sl = document.getElementById('site-layer');
            return (sl ? (sl.textContent || '').trim() : '');
            "#,
            vec![],
        )
        .await?;
    let home_text = home_text.as_str().unwrap_or("");
    assert!(
        home_text.contains("Welcome") && home_text.contains("Entity Demo Site"),
        "Phase 19: the ⌂ Home button didn't reset to the configured home site \
         (demo) from the companion; got: {home_text:?}"
    );
    println!("  ⌂ Home reset to the configured home site from a different site");

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
    phase_gate!(client, "20");
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
    let phase20_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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
    phase_gate!(client, "21");
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
    // Bound once and reused, so the fixture path and the share-link assertion
    // below cannot drift apart if the fixture peer ever changes.
    const REMOTE_FIXTURE_PEER: &str = "2KFAQwKL6XzdwLkoHkxZ9WE7kvBtS59piFA2AkdBBiQUt5";
    let manifest_bin =
        format!("dist/remote-fixture/{REMOTE_FIXTURE_PEER}/sites/labs/manifest.bin");
    let manifest_bin = manifest_bin.as_str();
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
    let phase21_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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

    // -- A shared link to a FOREIGN site must carry that publisher's peer -----
    //
    // The renderer half of the `self`-sentinel bug, which the native gate
    // (`a_shared_link_carries_the_publishers_peer_not_the_readers`) cannot see:
    // that one proves the RULE, this proves the Share button CALLS it. Emitting
    // `self` here shipped on every published domain and was found by a person —
    // `self` resolves at boot to the *reader's own* peer, so the link reported
    // "No site manifest" at a peer-id that differed per visitor.
    //
    // A foreign site is on screen right now (that is all of Phase 21), so this
    // is a two-line assertion on an existing fixture rather than a new rig.
    let read_share = r#"
        const sl = document.getElementById('site-layer');
        if (!sl) return 'NO-SITE-LAYER';
        const b = sl.querySelector('[data-share-link]');
        return b ? b.getAttribute('data-share-link') : 'NO-SHARE-BUTTON';
    "#;
    let mut share_link = String::new();
    for _ in 0..20 {
        share_link =
            client.execute(read_share, vec![]).await?.as_str().unwrap_or("").to_string();
        if share_link.starts_with("http") || share_link.starts_with('/') {
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }
    assert!(
        share_link.contains(REMOTE_FIXTURE_PEER),
        "Phase 21: a shared link to a FOREIGN site must carry that publisher's peer-id \
         ({REMOTE_FIXTURE_PEER}), got {share_link:?}"
    );
    // The negative half is the load-bearing one: `self` is a well-formed link
    // that opens nothing for anybody, so every positive shape check passes while
    // the bug is live.
    assert!(
        !share_link.contains("site=self"),
        "Phase 21: the Share button emitted the `self` sentinel for a foreign site — \
         that link resolves to the READER's own peer and 404s as 'No site manifest': {share_link:?}"
    );
    println!("  Phase 21 share-link OK — carries the publisher's peer: {share_link}");

    let phase21_log = capture_log(&client).await?;
    let phase21_panics = count_panics(&phase21_log);
    assert!(
        phase21_panics.is_empty(),
        "Phase 21: HTTP-poll remote render panicked:\n{}",
        phase21_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );
    println!("  Phase 21 OK — cross-peer HTTP-poll remote site rendered live");

    phase_gate!(client, "21b");
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
    let phase21b_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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

    phase_gate!(client, "22");
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
    // (peer, kind, target) controls: the boot-kind radios (default "chrome",
    // the primary axis that replaced the old profile <select>), the peer
    // <select>, the target <select>, and the show_toggle checkbox. The old
    // single default-site text field is gone — "which site" is now the
    // peer-qualified Target dropdown.
    let section = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const kindChecked = (() => {
                const r = root.querySelector('input[data-kind][checked], input[data-kind]:checked');
                return r ? r.getAttribute('data-kind') : null;
            })();
            return {
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

    phase_gate!(client, "22.5");
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

    phase_gate!(client, "23");
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

    phase_gate!(client, "24");
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
    let phase24_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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

    phase_gate!(client, "25");
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
    let phase25_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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

    phase_gate!(client, "26");
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
    let phase26_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
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
    phase_gate!(client, "26.8");
    // Phase 26.8: user-defined themes end-to-end (Worker arm, warm boot).
    // Exercises the WHOLE feature through the real delivery path: Theme
    // Editor create (duplicate dark) → live preview (input event rewrites
    // #theme-vars from the draft) → Save (tree write) → select it as the
    // chrome theme in Settings → RELOAD (the registry must reload from the
    // tree through the app-level subscription — the Worker-arm cache-mirror
    // cell that native tests can't reach) → load-in-editor → delete.
    // ====================================================================
    println!("--- Phase 26.8: user-defined themes end-to-end ---");
    let pre_theme_log = capture_log(&client).await?;
    let pre_theme_panics = count_panics(&pre_theme_log).len();

    let spawn_editor = click_spawn_btn(&client, "+ Theme Editor").await?;
    assert_eq!(spawn_editor, "clicked", "Phase 26.8: no '+ Theme Editor' palette button");
    sleep(Duration::from_millis(600)).await;

    // Create "e2e-neon" from the dark base.
    let created = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const name = root.querySelector("input[data-field^='theme-new-name-']");
            const create = root.querySelector("[data-field='theme-create']");
            if (!name || !create) return 'no-controls';
            name.value = 'e2e-neon';
            create.click();
            return 'created';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(created.as_str(), Some("created"), "Phase 26.8: create controls missing");
    sleep(Duration::from_millis(700)).await;

    let editor_state = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const status = root.querySelector("[data-field='theme-editor-status']");
            const load = root.querySelector("select[name^='theme-editor-load-']");
            const bg = root.querySelector("input[data-token='--bg']");
            return {
                status: status ? status.textContent : 'missing',
                load_options: load ? Array.from(load.options).map(o => o.value) : [],
                has_bg_row: !!bg,
            };
            "#,
            vec![],
        )
        .await?;
    let ed_status = editor_state.get("status").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        ed_status.contains("Created"),
        "Phase 26.8: create didn't report success; status: {editor_state:?}"
    );
    assert!(
        editor_state.get("has_bg_row").and_then(|v| v.as_bool()).unwrap_or(false),
        "Phase 26.8: token rows didn't render after create: {editor_state:?}"
    );

    // Live preview: type a distinctive --bg into the token input. The input
    // event must rewrite #theme-vars from the DRAFT (no save, no rebuild).
    let preview = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const bg = root.querySelector("input[data-token='--bg']");
            if (!bg) return 'no-input';
            bg.value = '#123456';
            bg.dispatchEvent(new Event('input', { bubbles: true }));
            const vars = document.getElementById('theme-vars');
            return vars ? vars.textContent : 'no-theme-vars';
            "#,
            vec![],
        )
        .await?;
    let preview = preview.as_str().unwrap_or("").to_string();
    assert!(
        preview.contains("--bg:#123456;"),
        "Phase 26.8: live preview didn't rewrite #theme-vars from the draft; got: {preview:?}"
    );

    // Save, then make it the current chrome theme via the Settings dropdown
    // (the registry-driven option must be there without any wiring).
    let saved = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const save = root.querySelector("[data-field='theme-save']");
            if (!save) return 'no-save';
            save.click();
            return 'saved';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(saved.as_str(), Some("saved"), "Phase 26.8: no Save button");
    sleep(Duration::from_millis(700)).await;

    // Settings may not be open on this post-deep-link boot — spawn if needed.
    let _ = click_spawn_btn(&client, "+ Settings").await;
    sleep(Duration::from_millis(600)).await;
    let selected = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="theme-"]:not([name^="theme-editor"])');
            if (!s) return 'no-select';
            if (!Array.from(s.options).some(o => o.value === 'e2e-neon')) return 'option-missing';
            s.value = 'e2e-neon';
            s.dispatchEvent(new Event('change', { bubbles: true }));
            return 'selected';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        selected.as_str(),
        Some("selected"),
        "Phase 26.8: the saved user theme must appear in the Settings theme dropdown \
         (registry-driven) and be selectable"
    );
    // Applying a selected theme is an async re-render, so poll for the recolor
    // rather than sleeping a guess and reading once — this assertion was an
    // observed flake under load (the box was busy; 600ms was not enough).
    let applied = poll_json(
        &client,
        r#"
            const vars = document.getElementById('theme-vars');
            return {
                css: vars ? vars.textContent : 'missing',
                ls_name: localStorage.getItem('entity_theme'),
                ls_css: (localStorage.getItem('entity_theme_css') || '').slice(0, 60),
            };
            "#,
        Duration::from_secs(15),
        |v| {
            v.get("css")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.contains("--bg:#123456;"))
        },
    )
    .await?;
    let applied_css = applied.get("css").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        applied_css.contains("--bg:#123456;"),
        "Phase 26.8: selecting the user theme didn't recolor #theme-vars: {applied:?}"
    );
    assert_eq!(
        applied.get("ls_name").and_then(|v| v.as_str()),
        Some("e2e-neon"),
        "Phase 26.8: the localStorage name mirror must carry the user theme: {applied:?}"
    );

    // Diagnostic breadcrumbs for the persistence assert below: which paths
    // the settings writes actually hit, and the peer-identity vault keys
    // (a system-peer id drift between sessions would silently repath
    // settings/ui). Printed, not asserted.
    let pre_log = capture_log(&client).await?;
    for l in pre_log.iter().filter(|l| l.contains("settings/ui") || l.contains("ensure_state")) {
        println!("  [pre-reload] {l}");
    }
    let pre_vault = client
        .execute(
            r#"return Object.keys(localStorage).sort().join(',') + ' || peers=' +
                (localStorage.getItem('entity_peers') || '').slice(0, 120);"#,
            vec![],
        )
        .await?;
    println!("  [pre-reload] localStorage: {}", pre_vault.as_str().unwrap_or(""));
    assert!(
        applied.get("ls_css").and_then(|v| v.as_str()).unwrap_or("").contains(":root{"),
        "Phase 26.8: the CSS paint-hint mirror (entity_theme_css) must be written for a \
         user chrome theme — without it the next boot flashes dark: {applied:?}"
    );

    // Panic check BEFORE the reload — the reload wipes the in-page log, so
    // a create/save/select panic would otherwise vanish (the false-0 gotcha).
    let pre_reload_log = capture_log(&client).await?;
    let pre_reload_panics = count_panics(&pre_reload_log);
    assert!(
        pre_reload_panics.len() <= pre_theme_panics,
        "Phase 26.8: create/preview/save/select triggered new panic(s):\n{}",
        pre_reload_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );

    // RELOAD — the warm-boot cell. The theme must come back from the TREE
    // (app-level subscription → registry sync), not just the mirrors: the
    // Settings dropdown must list it again, selected, and the live palette
    // must carry the edited value.
    client
        .goto(&format!("http://localhost:{}/?worker=1&log=trace", http_server_port()))
        .await?;
    let theme_boot_ms = wait_for_boot(&client, 60_000).await?;
    println!("  phase 26.8 reload boot: {theme_boot_ms}ms");
    sleep(Duration::from_millis(1500)).await;

    let after_reload = client
        .execute(
            r#"
            const vars = document.getElementById('theme-vars');
            return vars ? vars.textContent : 'missing';
            "#,
            vec![],
        )
        .await?;
    let after_reload = after_reload.as_str().unwrap_or("").to_string();
    assert!(
        after_reload.contains("--bg:#123456;"),
        "Phase 26.8: after reload the user theme must paint (mirror at boot, tree-synced \
         registry after); #theme-vars: {after_reload:?}"
    );

    let _ = click_spawn_btn(&client, "+ Settings").await;
    // The registry re-syncs from the tree via the app-level subscription —
    // asynchronous on the Worker arm (cache-mirror seed → dirty → frame
    // sync). Poll briefly instead of trusting one fixed sleep.
    let mut dropdown_after = serde_json::Value::Null;
    let mut dd_options: Vec<String> = Vec::new();
    for _ in 0..12 {
        sleep(Duration::from_millis(500)).await;
        dropdown_after = client
            .execute(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const s = root.querySelector('select[name^="theme-"]:not([name^="theme-editor"])');
                if (!s) return 'no-select';
                return { options: Array.from(s.options).map(o => o.value), value: s.value };
                "#,
                vec![],
            )
            .await?;
        dd_options = dropdown_after
            .get("options")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        // Both facts converge asynchronously (the themes-prefix sync fills
        // the options; the settings-entity snapshot seed selects the value)
        // — poll until BOTH hold, then assert.
        if dd_options.iter().any(|o| o == "e2e-neon")
            && dropdown_after.get("value").and_then(|v| v.as_str()) == Some("e2e-neon")
        {
            break;
        }
    }
    // Diagnostic breadcrumbs (see pre-reload twin above).
    let post_log = capture_log(&client).await?;
    for l in post_log.iter().filter(|l| l.contains("settings/ui") || l.contains("ensure_state")) {
        println!("  [post-reload] {l}");
    }
    let post_vault = client
        .execute(
            r#"return Object.keys(localStorage).sort().join(',') + ' || peers=' +
                (localStorage.getItem('entity_peers') || '').slice(0, 120);"#,
            vec![],
        )
        .await?;
    println!("  [post-reload] localStorage: {}", post_vault.as_str().unwrap_or(""));

    assert!(
        dd_options.iter().any(|o| o == "e2e-neon"),
        "Phase 26.8: after reload the registry must re-sync the user theme from the tree \
         (Worker-arm cache-mirror cell); dropdown: {dropdown_after:?}"
    );
    assert_eq!(
        dropdown_after.get("value").and_then(|v| v.as_str()),
        Some("e2e-neon"),
        "Phase 26.8: the persisted selection must survive reload (a fresh Settings spawn \
         must NOT reseed defaults over persisted settings — the ensure_state \
         get-then-write clobber): {dropdown_after:?}"
    );

    // Delete path: switch back to the pre-phase theme first (delete of an
    // in-use theme is refused by design), then delete through the editor.
    let restored = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="theme-"]:not([name^="theme-editor"])');
            if (!s) return 'no-select';
            s.value = 'light';
            s.dispatchEvent(new Event('change', { bubbles: true }));
            return 'restored';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(restored.as_str(), Some("restored"), "Phase 26.8: theme restore failed");
    sleep(Duration::from_millis(500)).await;

    let spawn_editor2 = click_spawn_btn(&client, "+ Theme Editor").await?;
    assert_eq!(spawn_editor2, "clicked", "Phase 26.8: editor respawn failed");
    sleep(Duration::from_millis(600)).await;
    let deleted = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const load = root.querySelector("select[name^='theme-editor-load-']");
            if (!load) return 'no-load-select';
            load.value = 'e2e-neon';
            load.dispatchEvent(new Event('change', { bubbles: true }));
            return 'loaded';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        deleted.as_str(),
        Some("loaded"),
        "Phase 26.8: fresh editor must list the persisted theme in its loader"
    );
    sleep(Duration::from_millis(500)).await;
    let del_result = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const del = root.querySelector("[data-field='theme-delete']");
            if (!del) return 'no-delete';
            if (del.disabled) return 'disabled';
            del.click();
            return 'clicked';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        del_result.as_str(),
        Some("clicked"),
        "Phase 26.8: Delete must be enabled once the theme is no longer in use"
    );
    // The delete propagates to the Settings dropdown via the themes-prefix
    // watch (async event → dirty → re-render) — poll, don't trust one sleep.
    // 15s, not 10×500ms: the old 5s bound was an observed flake under load, and
    // a bound that only matters on the failure path should be generous.
    let post_delete_deadline = Instant::now() + Duration::from_secs(15);
    let mut post_delete = serde_json::Value::Null;
    let mut post_options: Vec<String> = Vec::new();
    loop {
        sleep(Duration::from_millis(250)).await;
        post_delete = client
            .execute(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const s = root.querySelector('select[name^="theme-"]:not([name^="theme-editor"])');
                const status = root.querySelector("[data-field='theme-editor-status']");
                return {
                    options: s ? Array.from(s.options).map(o => o.value) : [],
                    status: status ? status.textContent : 'missing',
                };
                "#,
                vec![],
            )
            .await?;
        post_options = post_delete
            .get("options")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if !post_options.is_empty() && !post_options.iter().any(|o| o == "e2e-neon") {
            break;
        }
        if Instant::now() >= post_delete_deadline {
            break;
        }
    }
    // AUDIT-THEME-DELETE-STALE-DROPDOWN Pass A: capture the reconcile/render
    // trace BEFORE the assert, not after — the panic check further down never
    // ran on a failing run, so we have never even known whether a frame
    // panicked here. These lines answer §3 directly: a `register` of the
    // deleted name after the `delete` line is H-B (resurrection from a stale
    // listing); a `reconcile` that removes it with no later `settings: theme
    // options built` is H-A (reconcile-after-render); no post-delete
    // `reconcile` at all is H-C (the removal Change never arrives).
    let theme_trace: Vec<String> = capture_log(&client)
        .await?
        .into_iter()
        .filter(|l| {
            l.contains("user-themes:")
                || l.contains("settings: theme options built")
                || l.contains("worker-proxy:")
                || l.contains("panicked at")
        })
        .collect();
    // Printed on PASS as well as fail, deliberately. The failure is
    // intermittent and expensive to catch, but the *mechanism* (§3: does a
    // post-delete reconcile ever re-register the deleted name from a stale
    // mirror union?) is visible on a passing run too — a green run that shows
    // the resurrection is the same evidence, minus the lost race.
    println!("  Phase 26.8 theme reconcile/render trace ({} lines):", theme_trace.len());
    for line in &theme_trace {
        println!("    {line}");
    }
    assert!(
        !post_options.iter().any(|o| o == "e2e-neon"),
        "Phase 26.8: after delete the theme must leave every registry-driven dropdown: \
         {post_delete:?}\n--- theme reconcile/render trace ---\n{}",
        theme_trace.join("\n")
    );
    assert!(
        post_delete.get("status").and_then(|v| v.as_str()).unwrap_or("").contains("Deleted"),
        "Phase 26.8: delete must report its outcome in the status line: {post_delete:?}"
    );

    // Post-reload log is fresh (the goto reset it) — any panic here is new.
    let theme_log = capture_log(&client).await?;
    let theme_panics = count_panics(&theme_log);
    assert!(
        theme_panics.is_empty(),
        "Phase 26.8: reload/load/delete triggered panic(s):\n{}",
        theme_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );
    println!("  Phase 26.8 OK — create → preview → save → select → reload persistence → delete");

    // ====================================================================
    phase_gate!(client, "26.9");
    // Phase 26.9: a theme removed from the TREE must leave the Settings
    // dropdown. The permanent gate for AUDIT-THEME-DELETE-STALE-DROPDOWN
    // finding F1 (`docs/plans/`).
    //
    // Why this exists as its own phase rather than more polling in 26.8:
    // 26.8's delete goes through `user_themes::delete_theme`, which
    // unregisters from the runtime registry **synchronously** before
    // dispatching the tree remove — so the dropdown is already correct
    // before any re-render, and the reconcile→render path it depends on is
    // never actually tested. 26.8 passes on the optimistic local update, not
    // on the projection working.
    //
    // Deleting straight from the tree (the Shell's `rm`) removes that
    // crutch: nothing touches the registry locally, so the ONLY route from
    // "entity gone" to "option gone" is
    //   removal Change → watch dirty → `UserThemes::sync` reconciles →
    //   Settings re-renders.
    // That is exactly the path F1 says is broken, and it is a real user
    // scenario (a theme dropped by another tab, a peer sync, or the shell).
    //
    // Expected to fail DETERMINISTICALLY before the fix and pass after. If
    // it ever starts passing on an unfixed build, the ordering changed —
    // don't relax the assert, re-read `EntityApp::frame`.
    // ====================================================================
    println!("--- Phase 26.9: a tree-side theme delete must leave the dropdown ---");

    let spawn_editor3 = click_spawn_btn(&client, "+ Theme Editor").await?;
    assert_eq!(spawn_editor3, "clicked", "Phase 26.9: editor respawn failed");
    sleep(Duration::from_millis(600)).await;

    let ghost_created = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const name = root.querySelector("input[data-field^='theme-new-name-']");
            const create = root.querySelector("[data-field='theme-create']");
            if (!name || !create) return 'no-controls';
            name.value = 'e2e-ghost';
            create.click();
            return 'created';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(ghost_created.as_str(), Some("created"), "Phase 26.9: create controls missing");
    sleep(Duration::from_millis(700)).await;
    let ghost_saved = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const save = root.querySelector("[data-field='theme-save']");
            if (!save) return 'no-save';
            save.click();
            return 'saved';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(ghost_saved.as_str(), Some("saved"), "Phase 26.9: no Save button");

    // Setup check: the option must be there before we can prove it leaves.
    let ghost_present = poll_json(
        &client,
        r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="theme-"]:not([name^="theme-editor"])');
            return { options: s ? Array.from(s.options).map(o => o.value) : [] };
        "#,
        Duration::from_secs(10),
        |v| {
            v.get("options")
                .and_then(|o| o.as_array())
                .is_some_and(|a| a.iter().any(|o| o.as_str() == Some("e2e-ghost")))
        },
    )
    .await?;
    assert!(
        ghost_present
            .get("options")
            .and_then(|o| o.as_array())
            .is_some_and(|a| a.iter().any(|o| o.as_str() == Some("e2e-ghost"))),
        "Phase 26.9 setup: the saved theme must reach the Settings dropdown first: \
         {ghost_present:?}"
    );

    // The tree-side delete. `@primary` expands to the bound peer, which is
    // where `user_themes` writes (`system_peer_id` == `primary_peer_id`).
    let rm_sb = shell_submit(&client, "rm @primary/app/entity-browser/themes/e2e-ghost", 600).await?;
    assert!(
        !rm_sb.contains("usage: rm"),
        "Phase 26.9: the shell rejected the remove — path/alias wrong, so the \
         phase would pass vacuously. Scrollback: {rm_sb:?}"
    );

    // Generous budget: this must be decided by change-detection, not speed.
    // A healthy build returns on the first poll.
    let ghost_gone = poll_json(
        &client,
        r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const s = root.querySelector('select[name^="theme-"]:not([name^="theme-editor"])');
            return { options: s ? Array.from(s.options).map(o => o.value) : [] };
        "#,
        Duration::from_secs(12),
        |v| {
            v.get("options")
                .and_then(|o| o.as_array())
                .is_some_and(|a| {
                    !a.is_empty() && !a.iter().any(|o| o.as_str() == Some("e2e-ghost"))
                })
        },
    )
    .await?;
    let ghost_trace: Vec<String> = capture_log(&client)
        .await?
        .into_iter()
        .filter(|l| {
            l.contains("user-themes:")
                || l.contains("settings: theme options built")
                || l.contains("worker-proxy:")
                || l.contains("panicked at")
        })
        .collect();
    assert!(
        ghost_gone
            .get("options")
            .and_then(|o| o.as_array())
            .is_some_and(|a| !a.iter().any(|o| o.as_str() == Some("e2e-ghost"))),
        "Phase 26.9: a theme deleted from the tree must leave the Settings dropdown. \
         The entity is gone and the surface still shows it. Two distinct causes have \
         produced this (AUDIT-THEME-DELETE-STALE-DROPDOWN): F1 — the reconcile ran \
         but AFTER the render that reads it, so look for a final `reconcile` with \
         `removed=[…]` and no `theme options built` following it; F3 — the reconcile \
         never ran again at all, so look for a `reconcile` whose `listing` still has \
         the theme, followed by `worker-proxy: removal` lines draining \
         `remaining_holders` to empty with no reconcile after. \
         Dropdown: {ghost_gone:?}\n--- theme reconcile/render trace ---\n{}",
        ghost_trace.join("\n")
    );
    println!("  Phase 26.9 OK — a tree-side delete reaches the dropdown");

    // ====================================================================
    phase_gate!(client, "27");
    // Phase 27: per-domain deployment config served at /entity-deployment.json
    // (boot-closure cut 2b). The harness builds a GENERIC `Full` bundle; this
    // phase publishes a locked-site, same-origin config + the demo content into
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

    // Publish the demo + a locked-site same-origin config into dist/ (the
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
    let phase27_boot_ms = wait_for_boot(&client, BOOT_BUDGET_MS).await?;
    println!("  phase 27 boot: {phase27_boot_ms}ms");

    // Poll for the published home to render. Fast-paint's pre-peer paint is
    // DISABLED for the consolidation, so the LIVE overlay is the sole
    // `#site-layer` owner: it paints the config's home post-peer over
    // same-origin HTTP once boot_load applies the served locked-site config.
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
    // build adopting a SERVED locked-site config + rendering its home) is still
    // verified end-to-end below — by (c) mode-site and (d) the home rendering
    // via the LIVE overlay — just post-peer instead of pre-peer.
    assert!(
        !phase27_log.iter().any(|l| l.contains("fast-paint: painted remote home")),
        "Phase 27: fast-paint is disabled for the consolidation — it must not paint pre-peer"
    );
    // (c) Posture applied: locked-site config boots the generic bundle into the
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
        "Phase 27: locked-site config should boot the generic bundle into the site overlay"
    );
    // (d) End-to-end: the published home rendered, fetched same-origin.
    assert!(
        home_text.contains("Welcome to the Entity Demo Site"),
        "Phase 27: the published home didn't render same-origin; got: {home_text:?}"
    );
    // (e) Boot-time site-discovery warm-up: the served config registers the
    // publisher peer's ORIGIN, and the warm-up must then fetch its `sites.list` +
    // manifests and cache them into MY store on this boot — so the foreign peer's
    // sites are present on FIRST paint (the directory rail lists them without a
    // manual navigate), not lazily on first browse. The fixture publishes the
    // demo SET (demo + its cross-site companion demo-notes + entity-info), so the
    // warm-up caches ≥1 (the home `demo` may already be resolved by the overlay;
    // the non-home sites prove the warm-up fetched beyond home).
    // Poll: the warm-up is fire-and-forget async, so it can land shortly after the
    // home renders.
    let mut warm_line = String::new();
    for _ in 0..15 {
        let log = capture_log(&client).await?;
        if let Some(l) = log.iter().find(|l| l.contains("warm_peer_sites: cached")) {
            warm_line = l.clone();
            break;
        }
        sleep(Duration::from_millis(300)).await;
    }
    assert!(
        !warm_line.is_empty(),
        "Phase 27: boot-time site warm-up never ran — the registered peer's sites \
         would appear only after a manual navigate (the discovery regression). \
         Expected a 'warm_peer_sites: cached' log line."
    );
    assert!(
        !warm_line.contains("cached 0 "),
        "Phase 27: site warm-up ran but cached 0 manifests — the foreign peer's \
         sites.list/manifests were not fetched on boot: {warm_line:?}"
    );
    println!("  Phase 27e OK — boot-time warm-up cached the published peer's sites on first paint");

    let phase27_panics = count_panics(&phase27_log);
    assert!(
        phase27_panics.is_empty(),
        "Phase 27: served-config boot panicked:\n{}",
        phase27_panics.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
    );

    // Remove the served config so it can't change any later phase / re-run's
    // default boot path (setup() also clears it defensively).
    let _ = std::fs::remove_file(cfg_file);
    println!("  Phase 27 OK — generic bundle adopted the served locked-site config + home (cut 2b)");

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
    // guard, not a hidden button, is what blocks it. Drive the create form
    // (kind "frontend") the same way the primary tab does.
    let click_status = client
        .execute(&create_peer_form_js("frontend"), vec![])
        .await?;
    assert_eq!(
        click_status.as_str().unwrap_or(""),
        "clicked",
        "the create form should still render in a secondary tab \
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

/// End-to-end lifecycle proof for the durable this-tab (`frontend-idb`) peer:
///   1. create one → it gets its own `entity-peer-{id}` IndexedDB database;
///   2. write a USER entity to its tree → reload → the app REHYDRATES the peer
///      (`replay_persisted_idb_peer`) and the entity is still readable;
///   3. delete it → reload → its IndexedDB database is DROPPED (no orphan, D9).
/// Direct/IDB posture (`?worker=0`), single leader tab (peer creation allowed).
/// This is the committed reload-survival proof for a NON-primary durable peer —
/// the gap Phase 18e in the monolith explicitly could not cover (its non-primary
/// peers were in-memory). Standalone (not a monolith phase) because it is
/// Direct-only and independent of the display-gated Tauri phase.
/// (DESIGN-PERSISTENT-THIS-TAB-PEER.md — the whole point of the feature.)
#[tokio::test(flavor = "current_thread")]
async fn frontend_idb_peer_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=0&log=trace", http_server_port());

    // Fresh profile: IDB + localStorage persist across sessions otherwise.
    client.goto(&url).await?;
    wipe_all_storage(&client).await?;

    // Boot Direct/IDB as the single (leader) tab — durable, peer creation allowed.
    client.goto(&url).await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;
    sleep(Duration::from_millis(1500)).await; // let the IDB primary settle

    // Spawn the Peers window so the create form (its <select>) is in the DOM.
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
        peers_spawn.as_str(),
        Some("clicked"),
        "couldn't spawn the Peers window: {peers_spawn:?}"
    );
    sleep(Duration::from_millis(600)).await;

    // Baseline entity-peer-* IDB databases (just the primary system peer).
    let dbs_before = list_entity_peer_dbs(&client).await?;

    // Create a frontend-idb peer via the Peers form (Direct-only option).
    let created = client
        .execute(&create_peer_form_js("frontend-idb"), vec![])
        .await?;
    assert_eq!(
        created.as_str(),
        Some("clicked"),
        "couldn't create a frontend-idb peer — is the 'This tab · saved' option \
         present + enabled in Direct mode? {created:?}"
    );

    // Async build → drain-insert → persist → checkpoint. Poll for the peer's OWN
    // entity-peer-{id} database to appear.
    let mut new_pid = String::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        let now = list_entity_peer_dbs(&client).await?;
        if let Some(name) = now.iter().find(|n| !dbs_before.contains(*n)) {
            new_pid = name
                .strip_prefix("entity-peer-")
                .unwrap_or(name)
                .to_string();
            break;
        }
        sleep(Duration::from_millis(250)).await;
    }
    assert!(
        !new_pid.is_empty(),
        "no new entity-peer-* IndexedDB database appeared after creating a frontend-idb \
         peer — the async build → drain-insert → persist path is broken."
    );
    println!("frontend-idb peer created — own IDB db: entity-peer-{new_pid}");

    {
        let l = capture_log(&client).await?;
        let p = count_panics(&l);
        assert!(
            p.is_empty(),
            "create panicked:\n{}",
            p.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
        );
    }

    // Write a distinctive USER entity into the peer's own tree via its Shell.
    open_peer_shell(&client, &new_pid).await?;
    let marker_path = format!("/{new_pid}/e2e/idb-marker");
    let put_sb = shell_submit_for_peer(
        &client,
        &new_pid,
        &format!("put {marker_path} note/text \"durable-marker-42\""),
        800,
    )
    .await?;
    assert!(
        put_sb.contains("e2e/idb-marker"),
        "put into the frontend-idb peer's tree didn't confirm; scrollback: {put_sb:?}"
    );
    let pre_sb =
        shell_submit_for_peer(&client, &new_pid, &format!("cat {marker_path}"), 500).await?;
    assert!(
        pre_sb.contains("durable-marker-42"),
        "marker not readable before reload; scrollback: {pre_sb:?}"
    );

    // A plain `put` rides the ~250ms write-behind debounce (no checkpoint) — give
    // it margin to flush to IndexedDB before reload.
    sleep(Duration::from_secs(2)).await;

    // RELOAD.
    client.goto(&url).await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;
    sleep(Duration::from_millis(1500)).await;
    {
        let l = capture_log(&client).await?;
        let p = count_panics(&l);
        assert!(
            p.is_empty(),
            "reload panicked:\n{}",
            p.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n---\n")
        );
    }

    // The peer's own IDB database must still exist after reload.
    let dbs_after = list_entity_peer_dbs(&client).await?;
    assert!(
        dbs_after
            .iter()
            .any(|n| n == &format!("entity-peer-{new_pid}")),
        "the frontend-idb peer's IndexedDB database vanished across reload (have: {dbs_after:?})."
    );

    // THE PROOF: the app rehydrated the peer (`replay_persisted_idb_peer`) AND its
    // user tree survived — open a fresh peer-scoped Shell and read the marker back.
    // (`open_peer_shell` fails at 'no-peer-option' if the peer wasn't rehydrated.)
    open_peer_shell(&client, &new_pid).await?;
    let post_sb =
        shell_submit_for_peer(&client, &new_pid, &format!("cat {marker_path}"), 800).await?;
    assert!(
        post_sb.contains("durable-marker-42"),
        "the user entity written to the frontend-idb peer did NOT survive reload — either \
         the peer wasn't rehydrated (replay_persisted_idb_peer) or its IDB store wasn't \
         reopened. scrollback: {post_sb:?}"
    );
    println!("frontend-idb peer + user tree SURVIVED reload (rehydrated + readable)");

    // --- Delete → the peer's IndexedDB database is dropped (D9 cleanup) -------
    // A deleted durable peer must not orphan its entity-peer-{id} database. The
    // delete tombstones it synchronously; the boot-time idb_cleanup drain drops
    // it at the next boot (race-free — the peer is off the roster, never reopened).
    let peers_spawn2 = client
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
        peers_spawn2.as_str(),
        Some("clicked"),
        "couldn't re-spawn the Peers window to delete the peer: {peers_spawn2:?}"
    );
    sleep(Duration::from_millis(600)).await;

    // The frontend-idb peer is the only deletable (non-system) peer here.
    let deleted_n = delete_all_deletable_peers(&client).await?;
    assert!(
        deleted_n >= 1,
        "expected to delete the frontend-idb peer via its Delete button; deleted {deleted_n}"
    );
    // Sync vault/roster removal + tombstone are immediate; the SDK teardown is
    // async. Give it a moment, then reload so the boot-time idb_cleanup runs.
    sleep(Duration::from_millis(1500)).await;
    client.goto(&url).await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;
    sleep(Duration::from_millis(1500)).await;

    let dbs_post_delete = list_entity_peer_dbs(&client).await?;
    assert!(
        !dbs_post_delete
            .iter()
            .any(|n| n == &format!("entity-peer-{new_pid}")),
        "the deleted frontend-idb peer's IndexedDB database was NOT dropped at boot \
         (orphaned store — D9 cleanup broke); dbs present: {dbs_post_delete:?}"
    );
    println!("frontend-idb peer DELETE dropped its IndexedDB database (no orphan)");

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

/// rung-0.5 of S5 (design §3a / the F2-task1b note) — the FIRST live exercise of
/// v11/v12 WebRTC provisioning through the real delivery path. It proves the
/// app-side chain end to end: URL provisioning source → `InitParams.webrtc` +
/// per-peer `webrtc_enabled` → the worker installs the §6.5 establisher at the
/// §10.3 seam → the v12 `WireCaps.webrtc_peers` report → our capability diff
/// logs "confirmed".
///
/// A **dummy, unreachable** node is deliberate and sufficient: install ≠
/// connect. `PeerCarrier::new` only stores the addr (`conn: None`); it is dialed
/// lazily at negotiate, and the worker reaches `RTCPeerConnection` through the
/// main-thread broker (control port), so nothing here needs a live node or
/// worker-side WebRTC. **Honesty line (§11.5.1):** this proves the leg
/// *installs*, NOT that transport works — that is rung-1 (two peers + a real
/// signaling node + a channel), which has open infra unknowns (a 2nd browser
/// session; the standalone caps at one — see setup()).
#[tokio::test(flavor = "current_thread")]
async fn webrtc_provisioning_installs_the_establisher_on_the_enabled_primary(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    client
        .goto(&format!(
            // Worker arm (webrtc is worker-only) + the URL provisioning source:
            // a dummy node that will never be dialed, and the explicit per-peer
            // enable on the primary.
            "http://localhost:{}/?worker=1\
             &webrtc_node=ws://127.0.0.1:65535&webrtc_node_peer=dummy-node\
             &webrtc_enable=1&log=trace",
            http_server_port()
        ))
        .await?;
    wait_for_boot(&client, 30_000).await?;
    // Let the Ready handshake + the capability diff land in the log sink.
    sleep(Duration::from_millis(500)).await;

    let log = capture_log(&client).await?;
    // The provisioning line — we asked for the capability at all.
    assert!(
        log.iter()
            .any(|l| l.contains("provisioning") && l.contains("establisher capability")),
        "expected the provisioning log (URL source → InitParams.webrtc). Log:\n{}",
        log.join("\n")
    );
    // The v12 report confirmed the establisher installed on the enabled primary.
    assert!(
        log.iter().any(|l| l.contains("establisher confirmed on the primary")),
        "expected the v12 install-confirmed line (provision→install→report). If \
         absent, the worker did not install the establisher and Init likely fell \
         back to Direct. Log:\n{}",
        log.join("\n")
    );
    // And no shortfall — an enabled primary the worker did NOT install would have
    // logged an error instead (D13: never silent, either way).
    assert!(
        !log.iter().any(|l| l.contains("install shortfall")),
        "the enabled primary reported an install shortfall. Log:\n{}",
        log.join("\n")
    );
    client.close().await.ok();
    Ok(())
}

/// Chat's reachability header on the **Worker arm** — the half a native test
/// cannot reach.
///
/// What this covers and what it deliberately does not: a single browser has
/// nobody to actually connect to, so the LIVE `Connected` case is asserted by
/// `make e2e-webrtc-meet` §6, where two browsers have genuinely exchanged
/// messages. What is provable here is the other half, and it is the half that
/// was silent before: bind a conversation to a peer that does not exist, and
/// the window must SAY it cannot be reached rather than looking identical to a
/// working one. That is the whole defect — a failed delivery was a
/// `tracing::warn!` and nothing a user could see.
///
/// Worker arm specifically because `peer_has_webrtc` reads `WireCaps` there
/// (the v12 install report) rather than the Direct-arm seam, so a projection
/// that only ever ran on Direct proves nothing about the shipped surface.
#[tokio::test(flavor = "current_thread")]
async fn chat_says_when_a_bound_conversation_cannot_be_reached(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    client
        .goto(&format!(
            "http://localhost:{}/?worker=1&log=trace",
            http_server_port()
        ))
        .await?;
    wait_for_boot(&client, 30_000).await?;

    // Spawn Chat from the palette.
    let spawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const b of root.querySelectorAll('button.spawn-btn')) {
                if (b.textContent.trim() === '+ Chat') { b.click(); return 'clicked'; }
            }
            return 'no-chat-btn';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(spawn.as_str(), Some("clicked"), "could not spawn Chat: {spawn:?}");

    // Read the reachability row out of the LAST Chat window. `(none)` and
    // `(no-window)` are distinct on purpose: an absent row is the assertion for
    // the unbound state, and an absent *window* would otherwise pass as one.
    let read_reach = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        const wins = [];
        for (const sec of root.querySelectorAll('section.window')) {
            const h = sec.querySelector('header h3');
            if (h && h.textContent.trim() === 'Chat') wins.push(sec);
        }
        if (!wins.length) return '(no-window)';
        const el = wins[wins.length - 1].querySelector('[data-field="chat-reachability"]');
        return el ? (el.textContent || '').trim() : '(none)';
    "#;

    // Unbound: the default self-conversation is about nobody, so there must be
    // no row at all. A chip here would invent a relationship the user never
    // created — and it is the state every freshly-opened Chat starts in.
    let unbound = poll_json(&client, read_reach, Duration::from_secs(10), |v| {
        v.as_str().map(|s| s != "(no-window)").unwrap_or(false)
    })
    .await?;
    assert_eq!(
        unbound.as_str(),
        Some("(none)"),
        "an unbound Chat must paint no reachability row, got {unbound:?}"
    );

    // Bind to a well-formed id that nothing has ever connected to, through the
    // shipped by-id affordance (not a synthesized action) — so this exercises
    // the same path a user walks.
    let dark_pid = "2KnobodyHomeAtThisPeerdForTheReachabiityGatezz";
    let bind = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                const wins = [];
                for (const sec of root.querySelectorAll('section.window')) {{
                    const h = sec.querySelector('header h3');
                    if (h && h.textContent.trim() === 'Chat') wins.push(sec);
                }}
                if (!wins.length) return 'no-window';
                const inp = wins[wins.length - 1]
                    .querySelector('[data-field="chat-start-peer"]');
                if (!inp) return 'no-field';
                inp.focus();
                inp.value = {dark:?};
                inp.dispatchEvent(new Event('input', {{ bubbles: true }}));
                inp.dispatchEvent(new KeyboardEvent('keydown', {{
                    key: 'Enter', bubbles: true
                }}));
                return 'typed';
                "#,
                dark = dark_pid
            ),
            vec![],
        )
        .await?;
    assert_eq!(bind.as_str(), Some("typed"), "could not drive the by-id bind: {bind:?}");

    // Now the row must exist AND say something honest. Polled, never slept-then-
    // asserted: the bind lands on a later frame and the budget is an upper bound
    // a healthy run returns from on the first pass.
    let reach = poll_json(&client, read_reach, Duration::from_secs(15), |v| {
        v.as_str().map(|s| s.contains("reached back")).unwrap_or(false)
    })
    .await?;
    let reach = reach.as_str().unwrap_or("");
    assert!(
        !reach.is_empty() && reach != "(none)" && reach != "(no-window)",
        "a bound conversation must paint a reachability row, got {reach:?}"
    );
    // The reason line. This peer installs no establisher (no connector is
    // provisioned in this boot) and nothing here is reachable, so the note is
    // both true and the actual explanation for the silence.
    assert!(
        reach.contains("reached back"),
        "an unreachable bound conversation must say why it is unreachable, got {reach:?}"
    );
    // And never a Connected chip against a peer that does not exist — the
    // stale-`Connected` lie the kernel read-model exists to prevent.
    assert!(
        !reach.contains("Connected"),
        "nothing was ever connected to {dark_pid}, so the header must not say \
         Connected: {reach:?}"
    );

    client.close().await.ok();
    Ok(())
}

/// **Binding a chat used to be one-way.** The start picker renders only while
/// the window is unbound, so the first peer you chatted with was the only peer
/// that window could ever talk to; the escape was to open a second Chat window,
/// and that is how it was reported from a real two-device run — *"once I connect
/// to a peer, it's the only peer I can chat with"*.
///
/// Why this is an e2e and not a native test: the native pair
/// (`leaving_a_conversation_returns_the_window_to_the_picker`,
/// `switching_peers_swaps_the_conversation_and_keeps_both`) proves the model
/// unbinds and re-binds. Neither can see the thing that was actually missing,
/// which is a **control on screen** — the model has always been able to
/// re-bind, and `ChatStartWith` has always accepted a second peer. So the
/// assertions here are deliberately about the DOM: the button exists while
/// bound, pressing it brings the picker back, and the window then re-points at
/// a different conversation without being closed.
///
/// The conversation id is the proof of the re-point, because it is derived from
/// the participant pair — a window that "left" but stayed bound would keep it.
#[tokio::test(flavor = "current_thread")]
async fn a_chat_window_can_be_pointed_at_a_second_peer_without_reopening_it(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    client
        .goto(&format!(
            "http://localhost:{}/?worker=1&log=trace",
            http_server_port()
        ))
        .await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;

    let spawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const b of root.querySelectorAll('button.spawn-btn')) {
                if (b.textContent.trim() === '+ Chat') { b.click(); return 'clicked'; }
            }
            return 'no-chat-btn';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(spawn.as_str(), Some("clicked"), "could not spawn Chat: {spawn:?}");

    // One probe for the whole window state, so every assertion below reads the
    // same frame: which controls are present, and which conversation is bound.
    // `(no-window)` is distinct from an absent control — an absent *window*
    // would otherwise satisfy "the picker is gone".
    let probe = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        const wins = [];
        for (const sec of root.querySelectorAll('section.window')) {
            const h = sec.querySelector('header h3');
            if (h && h.textContent.trim() === 'Chat') wins.push(sec);
        }
        if (!wins.length) return '(no-window)';
        const w = wins[wins.length - 1];
        const conv = w.querySelector('[data-field="chat-conversation"]');
        return JSON.stringify({
            picker: !!w.querySelector('[data-field="chat-start-peer"]'),
            leave: !!w.querySelector('[data-field="chat-leave"]'),
            conversation: conv ? (conv.textContent || '').trim() : '',
        });
    "#;
    let state = |v: &serde_json::Value| -> serde_json::Value {
        serde_json::from_str(v.as_str().unwrap_or("{}")).unwrap_or_default()
    };

    // Two well-formed ids nothing has ever connected to. Same length by
    // construction (a peer id is 46 Base58 chars, and `is_peer_id` enforces it —
    // a hand-typed second literal is how that quietly stops being true).
    let peer_a = "2KnobodyHomeAtThisPeerdForTheReachabiityGatezz";
    let peer_b = "2KnobodyHomeAtThisPeerdForTheReachabiityGateyy";
    assert_eq!(peer_a.len(), peer_b.len(), "both fixtures must be well-formed peer ids");

    let bind_to = |pid: &str| {
        format!(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const wins = [];
            for (const sec of root.querySelectorAll('section.window')) {{
                const h = sec.querySelector('header h3');
                if (h && h.textContent.trim() === 'Chat') wins.push(sec);
            }}
            if (!wins.length) return 'no-window';
            const inp = wins[wins.length - 1]
                .querySelector('[data-field="chat-start-peer"]');
            if (!inp) return 'no-field';
            inp.focus();
            inp.value = {pid:?};
            inp.dispatchEvent(new Event('input', {{ bubbles: true }}));
            inp.dispatchEvent(new KeyboardEvent('keydown', {{ key: 'Enter', bubbles: true }}));
            return 'typed';
            "#,
            pid = pid
        )
    };

    // Fresh window: the picker, and nothing to leave.
    let fresh = poll_json(&client, probe, Duration::from_secs(10), |v| {
        v.as_str().map(|s| s != "(no-window)").unwrap_or(false)
    })
    .await?;
    let fresh = state(&fresh);
    assert_eq!(fresh["picker"], serde_json::json!(true), "a fresh Chat offers the picker");
    assert_eq!(
        fresh["leave"],
        serde_json::json!(false),
        "…and nothing to leave, because nothing is bound: {fresh:?}"
    );

    // Bind the first peer. The picker goes; the way out must arrive with it —
    // that swap is the entire bug, and asserting only one half would pass on
    // the shipped behaviour.
    let typed = client.execute(&bind_to(peer_a), vec![]).await?;
    assert_eq!(typed.as_str(), Some("typed"), "could not drive the by-id bind: {typed:?}");
    let bound_a = poll_json(&client, probe, Duration::from_secs(15), |v| {
        state(v)["leave"] == serde_json::json!(true)
    })
    .await?;
    let bound_a = state(&bound_a);
    assert_eq!(
        bound_a["picker"],
        serde_json::json!(false),
        "a bound Chat hides the picker (unchanged): {bound_a:?}"
    );
    let conv_a = bound_a["conversation"].as_str().unwrap_or("").to_string();
    assert!(!conv_a.is_empty(), "a bound conversation names itself: {bound_a:?}");

    // Leave. The picker must come back — this is the control that did not exist.
    let left = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const wins = [];
            for (const sec of root.querySelectorAll('section.window')) {
                const h = sec.querySelector('header h3');
                if (h && h.textContent.trim() === 'Chat') wins.push(sec);
            }
            if (!wins.length) return 'no-window';
            const btn = wins[wins.length - 1].querySelector('[data-field="chat-leave"]');
            if (!btn) return 'no-button';
            btn.click();
            return 'clicked';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(left.as_str(), Some("clicked"), "no way out of a bound chat: {left:?}");

    let unbound = poll_json(&client, probe, Duration::from_secs(15), |v| {
        state(v)["picker"] == serde_json::json!(true)
    })
    .await?;
    let unbound = state(&unbound);
    assert_eq!(
        unbound["leave"],
        serde_json::json!(false),
        "leaving must also retract the leave control: {unbound:?}"
    );
    assert_ne!(
        unbound["conversation"].as_str().unwrap_or(""),
        conv_a,
        "the window is back on its own scratch, not still in the conversation it left"
    );

    // …and now the point of all of it: a SECOND peer, in the same window.
    let typed = client.execute(&bind_to(peer_b), vec![]).await?;
    assert_eq!(typed.as_str(), Some("typed"), "could not re-bind: {typed:?}");
    let bound_b = poll_json(&client, probe, Duration::from_secs(15), |v| {
        state(v)["leave"] == serde_json::json!(true)
    })
    .await?;
    let bound_b = state(&bound_b);
    let conv_b = bound_b["conversation"].as_str().unwrap_or("");
    assert!(!conv_b.is_empty(), "the second bind names its conversation: {bound_b:?}");
    assert_ne!(
        conv_b, conv_a,
        "a different peer is a different conversation — an id that did not move \
         means the window never actually re-pointed"
    );

    client.close().await.ok();
    Ok(())
}

/// Choosing a connector does nothing until reload, and the window must say so.
///
/// `InitParams.webrtc` is Init-only upstream, so a selection made mid-session
/// cannot reach the running establisher. The gate `e2e-webrtc-meet` documents
/// that reload in Python; the *user* was told nothing, which made "I selected it
/// and meet still fails" the expected first experience.
///
/// Both halves are asserted, and the first is the one that keeps this honest: a
/// cold window must be QUIET. A static "changes need a reload" caption would
/// pass the second assertion and fail this one, which is exactly the difference
/// between a live state and permanent furniture.
///
/// **Adding the FIRST connector is what raises the notice now**, because that
/// add is also the selection (`connectors::add_connector` — a registry with rows
/// and no selection provisions nothing). This test used to Add and then click
/// *Use*; the selected row deliberately offers no Use button, so the old
/// sequence now waits forever for a control that should not exist. The Use path
/// is still exercised, on a **second** connector — where the button count is
/// also the cheapest proof that the first add really took the selection.
#[tokio::test(flavor = "current_thread")]
async fn selecting_a_connector_says_it_needs_a_reload(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    client
        .goto(&format!(
            "http://localhost:{}/?worker=1&log=trace",
            http_server_port()
        ))
        .await?;
    wait_for_boot(&client, 30_000).await?;

    let spawn = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const b of root.querySelectorAll('button.spawn-btn')) {
                if (b.textContent.trim() === '+ Peer Connections') { b.click(); return 'clicked'; }
            }
            return 'no-btn';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(spawn.as_str(), Some("clicked"), "could not spawn Peer Connections: {spawn:?}");

    // Whole-window text; the notice is a line inside the connectors card.
    let window_text = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h = sec.querySelector('header h3');
            if (h && h.textContent.trim() === 'Peer Connections') {
                return (sec.textContent || '').trim();
            }
        }
        return '(no-window)';
    "#;

    // Cold: nothing provisioned, nothing selected, so a reload would change
    // nothing and the window must not nag.
    let cold = poll_json(&client, window_text, Duration::from_secs(10), |v| {
        v.as_str().map(|s| s != "(no-window)").unwrap_or(false)
    })
    .await?;
    let cold = cold.as_str().unwrap_or("");
    assert!(!cold.is_empty() && cold != "(no-window)", "Peer Connections did not render");
    assert!(
        !cold.contains("takes effect on reload"),
        "a session with no connector must not claim a pending reload: {cold:?}"
    );

    // Add a connector through the window's own form — the click a user makes.
    // Factored so the SECOND add below is the same code path as the first; two
    // hand-copied blobs is how the two stop being the same click.
    //
    // **The peer-id now goes in `connector_expect`, inside the Advanced
    // disclosure, and that is load-bearing here rather than incidental.** The
    // form learns a node's id by dialing it; this rig's address is deliberately
    // dead (`:65535`), so the add can only succeed down the *named* branch —
    // which is exactly the "the node is not up yet" case worth exercising, and
    // the one that keeps this gate about the RELOAD NOTICE rather than about
    // whether a node happened to answer.
    //
    // It also proves the Advanced fields work while the `<details>` is closed:
    // the element is in the DOM, the `input` event lands in `ctx.drafts`, and
    // the submit-time read finds it. Nothing here opens the disclosure.
    let add_connector_js = |pid: &str| {
        format!(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            let card = null;
            for (const sec of root.querySelectorAll('section.window')) {{
                const h = sec.querySelector('header h3');
                if (h && h.textContent.trim() === 'Peer Connections') card = sec;
            }}
            if (!card) return 'no-window';
            const set = (field, val) => {{
                const el = card.querySelector(`[data-field="${{field}}"]`);
                if (!el) return false;
                el.focus();
                el.value = val;
                el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                return true;
            }};
            if (!set('connector_addr', 'ws://127.0.0.1:65535')) return 'no-addr-field';
            if (!set('connector_expect', {pid:?})) return 'no-expect-field';
            for (const b of card.querySelectorAll('button')) {{
                if (b.textContent.trim() === 'Add connector') {{ b.click(); return 'added'; }}
            }}
            return 'no-add-button';
            "#,
            pid = pid
        )
    };

    let added = client
        .execute(&add_connector_js("2KnodeForTheReoadNoticeGatezzzzzzzzzzzzzzzzzzz"), vec![])
        .await?;
    assert_eq!(added.as_str(), Some("added"), "could not add a connector: {added:?}");

    // The FIRST add is also the selection, so a reload would now resolve a node
    // this session did not boot with — and the window must say so without the
    // user touching anything else.
    let hot = poll_json(&client, window_text, Duration::from_secs(15), |v| {
        v.as_str().map(|s| s.contains("takes effect on reload")).unwrap_or(false)
    })
    .await?;
    assert!(
        hot.as_str().unwrap_or("").contains("takes effect on reload"),
        "adding the first connector selects it, so the window must report the \
         pending reload: {hot:?}"
    );

    // Add a SECOND connector and drive `Use`. Two things at once: the Use path
    // is still covered, and the button COUNT is the cheapest proof the first add
    // really took the selection — a selected row renders no Use button, so
    // "exactly one of two rows offers Use" is only true if one is selected.
    let added2 = client
        .execute(&add_connector_js("2KsecondNodeForTheReloadNoticeGatezzzzzzzzzz"), vec![])
        .await?;
    assert_eq!(added2.as_str(), Some("added"), "could not add a second connector: {added2:?}");

    let use_buttons = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        let card = null;
        for (const sec of root.querySelectorAll('section.window')) {
            const h = sec.querySelector('header h3');
            if (h && h.textContent.trim() === 'Peer Connections') card = sec;
        }
        if (!card) return -1;
        let n = 0;
        for (const b of card.querySelectorAll('button')) {
            if (b.textContent.trim() === 'Use') n++;
        }
        return n;
    "#;
    let count = poll_json(&client, use_buttons, Duration::from_secs(15), |v| {
        v.as_i64() == Some(1)
    })
    .await?;
    assert_eq!(
        count.as_i64(),
        Some(1),
        "with two connectors and one of them selected, exactly one row may offer \
         Use — got {count:?}"
    );

    // Click it: selecting the other node mid-session must leave the notice up.
    let used = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            let card = null;
            for (const sec of root.querySelectorAll('section.window')) {
                const h = sec.querySelector('header h3');
                if (h && h.textContent.trim() === 'Peer Connections') card = sec;
            }
            if (!card) return 'no-window';
            for (const b of card.querySelectorAll('button')) {
                if (b.textContent.trim() === 'Use') { b.click(); return 'used'; }
            }
            return 'no-use-button';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(used.as_str(), Some("used"), "the unselected connector never offered Use: {used:?}");

    let still = poll_json(&client, window_text, Duration::from_secs(15), |v| {
        v.as_str().map(|s| s.contains("takes effect on reload")).unwrap_or(false)
    })
    .await?;
    assert!(
        still.as_str().unwrap_or("").contains("takes effect on reload"),
        "selecting a different connector mid-session is still pending until \
         reload: {still:?}"
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

    // The version identity panel must name the build actually being served, not
    // merely have a heading. The point of the panel is that the console can
    // *measure* what it used to assert ("almost always a stale service worker"),
    // and a heading over an empty value would be the same guess with better
    // typography. Asserted against `dist/` for the same reason the C5 gate is:
    // the browser's answer has to come from the same bytes the server serves.
    let expect_build = std::fs::read_to_string("dist/index.html")
        .map_err(|e| format!("cannot read dist/index.html — run `make wasm` first: {e}"))?
        .split("name=\"entity-build\" content=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string)
        .ok_or("dist/index.html carries no entity-build stamp")?;
    // Anti-vacuity: `contains("")` is true of everything, so an empty stamp
    // would make the assertion below pass against a panel that rendered nothing.
    assert!(
        !expect_build.is_empty(),
        "dist/index.html has an EMPTY entity-build stamp — the assertion below would \
         pass vacuously. Check tools/build-stamp.sh."
    );
    let version = client
        .execute(
            r#"const v = document.getElementById('version'); return v ? v.textContent : '';"#,
            vec![],
        )
        .await?;
    let version = version.as_str().unwrap_or("");
    assert!(
        version.contains(&expect_build),
        "the recovery version panel does not name the running build {expect_build:?} — \
         the console still cannot answer \"which version am I on\". Got: {version:?}"
    );
    assert!(
        version.contains("update waiting"),
        "the version panel omits the service-worker `waiting` state, which is the one \
         signal that distinguishes \"an update is downloaded and blocked\" from a guess. \
         Got: {version:?}"
    );

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
/// JS that creates a peer via the Peers-window create form: set the
/// `.peer-create-kind` <select> to `kind` (value = `PeerMode::persist_key`, e.g.
/// `frontend` / `backend-memory` / `backend-opfs`, or the special `native`),
/// then click the single "Add peer" button. Centralizes the form-driving JS so a
/// future create-UI change touches ONE place, not every call site (the lesson
/// from the vocabulary relabel: create-UI text is a cross-surface coupling).
/// Returns "clicked" on success, else a diagnostic token.
fn create_peer_form_js(kind: &str) -> String {
    format!(
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {{
            const h2 = sec.querySelector('h2');
            if (!h2 || h2.textContent.trim() !== 'Peers') continue;
            const select = sec.querySelector('select.peer-create-kind');
            if (!select) return 'no-kind-select';
            let found = false;
            for (const opt of select.options) {{ if (opt.value === '{kind}') {{ found = true; break; }} }}
            if (!found) return 'no-kind-option';
            select.value = '{kind}';
            select.dispatchEvent(new Event('change', {{ bubbles: true }}));
            for (const b of sec.querySelectorAll('button')) {{
                if (b.textContent.trim() === 'Add peer') {{ b.click(); return 'clicked'; }}
            }}
            return 'no-add-btn';
        }}
        return 'no-peers-section';
        "#
    )
}

/// Create a peer via the Peers-window create form (see [`create_peer_form_js`]).
/// `kind` is the option value, not a button label. Returns "clicked" on success.
async fn click_peers_mode_btn(
    client: &Client,
    kind: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let v = client.execute(&create_peer_form_js(kind), vec![]).await?;
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

    assert_eq!(click_peers_mode_btn(&client, "backend-memory").await?, "clicked",
        "couldn't click '+ Worker (memory)'");
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(click_peers_mode_btn(&client, "backend-opfs").await?, "clicked",
        "couldn't click '+ Worker (OPFS)'");
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
    assert_eq!(click_peers_mode_btn(&client, "backend-memory").await?, "clicked");
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(click_peers_mode_btn(&client, "backend-opfs").await?, "clicked");
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
    assert_eq!(click_peers_mode_btn(&client, "backend-memory").await?, "clicked");
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
    assert_eq!(click_peers_mode_btn(&client, "backend-memory").await?, "clicked");
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

    // Publish the demo under a foreign publish-peer + a locked-site same-origin
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
        let home_text = poll_rendered(&client, read_site, "Welcome to the Entity Demo Site").await?;
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

/// Run one of `publish.rs`'s `#[ignore]`d re-key fixture generators and fail
/// LOUDLY if it did not. A fixture that silently no-ops would leave the next
/// boot reading a stale `dist/`, and the test would then measure the wrong
/// thing while looking like it ran.
fn run_rekey_fixture(name: &str, out_dir: &str) {
    // Two things here, both earned in one sitting.
    //
    // `--exact` is LOAD-BEARING. libtest's filter is a SUBSTRING match and these
    // fixture names are prefixes of one another (`…_before` also selects
    // `…_before_window`), so without it BOTH emitters run, write the same
    // `dist/entity-deployment.json`, and the last one wins — the site scenario
    // then silently booted a WINDOW surface, read an empty `#site-layer`, and
    // failed with a message about the product.
    //
    // And `--exact` compares against the FULLY-QUALIFIED name, so the module
    // path is required. Getting that wrong selects nothing, and libtest exits
    // **0** for "ran no tests" — the fixture became a silent no-op that passed
    // its own status check. Hence the `1 passed` assertion: a run that matched
    // nothing must fail here, at the cause, not later as a confusing boot.
    // ISOLATED `ENTITY_DATA_DIR`, and this one cost a full-suite run to find.
    //
    // Every nested cargo the suite spawns lives in the SAME container, so they
    // share one publisher store. These fixtures publish under explicit
    // `--identity-seed`s, and writing four extra publisher identities into that
    // shared store left Phase 27's *durable* publisher unable to bind its own
    // signature:
    //
    //   publish: signed root failed: publisher bound no signature at
    //   /2KCMk1G1…/system/signature/00e031c4…
    //
    // Phase 27 passed when the monolith ran alone and failed only when these
    // tests ran first — i.e. it read as a flaky monolith, in a phase that has
    // nothing to do with re-keying. A fixture that reaches into shared state is
    // not entitled to be convenient about it.
    let full = format!("content_site::publish::tests::{name}");
    let data_dir = std::env::temp_dir().join(format!("entity-rekey-fixture-{name}"));
    let _ = std::fs::create_dir_all(&data_dir);
    let out = Command::new(env!("CARGO"))
        .args(["test", "--bin", "entity-browser", &full, "--", "--ignored", "--exact"])
        .env("ENTITY_DATA_DIR", &data_dir)
        .env("ENTITY_REKEY_OUT", out_dir)
        .output()
        .unwrap_or_else(|e| panic!("could not run fixture {full}: {e}"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "fixture {full} failed:\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "fixture {full} matched no test (libtest exits 0 for that). stdout:\n{stdout}"
    );
}

/// The publisher the freshly-emitted `dist/entity-deployment.json` names.
///
/// Read back out of the artifact rather than re-derived from the seed on the
/// test side: the browser's belief comes from this file, so the test's notion of
/// "who is the publisher" must come from the same bytes. Deriving it
/// independently would let a key-derivation change keep the test and the app
/// agreeing with each other while both drifted from what was published.
fn deployment_home_peer(out_dir: &str) -> String {
    let path = format!("{out_dir}/entity-deployment.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("re-key fixture did not emit {path}: {e}"));
    let v: serde_json::Value =
        serde_json::from_str(&raw).expect("emitted deployment config must be valid JSON");
    v["home_site"]["peer"]
        .as_str()
        .expect("emitted deployment config must carry home_site.peer")
        .to_string()
}

/// **The `ecdeos.org` 2026-08-24 re-key, reproduced offline — and the acceptance
/// test for R1 (boot-time routing reconcile).**
///
/// *The incident.* Devops re-keyed a live domain off a publicly-computable demo
/// seed onto a durable identity. That was correct, and a security fix. But every
/// returning visitor's browser had persisted the OLD publisher id at first
/// contact and has fetched `/entity-deployment.json` exactly zero times since,
/// so every request goes to a publisher that no longer publishes: newest WASM,
/// 404s on everything not already cached, and it looks like a working app.
///
/// *The mechanism*, confirmed against the operator's live bricked profile on
/// `2026-08-27` (`docs/plans/DESIGN-RESILIENCE-RECONCILIATION-AND-ENTITY-DOCTOR.md`
/// §1.1a):
///
/// 1. `app.rs:1875` gates `deployment_config::fetch()` on `durable.is_none()`,
///    so a warm boot never re-reads the doc.
/// 2. The (1.2.5) reconcile at `app.rs:1963` is the only warm-boot escape, and
///    it fires **only when the stale peer's origin is unregistered**. The first
///    cold boot registered it, so it never fires — the `app.rs:1978` warn is
///    absent from the real boot log while three other `WARN`s printed.
/// 3. So `deployment` is `None` at `app.rs:2113`, and the browser asks a dead
///    publisher forever. Nothing is missing; everything present is wrong.
///
/// *What this stages*, deterministically and with no domain: publish as A, cold
/// boot — which persists `home_site.peer_id = A` **and registers A's origin**,
/// step 2's precondition — then re-publish as B and delete A's tree so it 404s
/// (production listed *"the abandoned demo peer 404s"* as a success criterion),
/// then reload the SAME browser profile.
///
/// Deliberately the **default** URL (no `?worker`): the operator's profile is
/// `try_worker = false` / `DurableDirectIdb`, and the Direct IDB arm is the
/// shipped browser default.
///
/// **This fails until R1 lands, and that is the point.** The failure prints the
/// browser console, which is the same artifact as the operator's. The staging
/// assertions run first and hold independently of R1, so a red result can only
/// mean the heal is missing — never that the fixture drifted.
/// Poll a surface until it renders `want`, returning whatever it last read.
///
/// The budget is deliberately generous. A 7.5 s bound was observed failing on
/// the FIRST boot after `dist/` changes — that boot pays a cold WASM compile and
/// a cold fetch of a ~29 MB bundle, while every warm boot after it renders in
/// well under a second. A budget that only holds on a warm cache is a flake that
/// fires in CI and nowhere else, which is worse than a slow test. Returns early
/// on success, so the bound costs nothing on the common path.
async fn poll_rendered(
    client: &Client,
    read_js: &str,
    want: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut last = String::new();
    for _ in 0..80 {
        sleep(Duration::from_millis(300)).await;
        last = client.execute(read_js, vec![]).await?.as_str().unwrap_or("").to_string();
        if last.contains(want) {
            break;
        }
    }
    Ok(last)
}

/// The scenario body, parameterised by **deployment surface**.
///
/// Parameterised rather than written once because the first version of this fix
/// healed `surface = site` and left `surface = window` broken — and a repair that
/// depends on which surface the domain happens to ship is not a repair, it just
/// moves which door the bug is reachable through. Both callers below run the
/// identical scenario; only the fixture's surface and the DOM read differ.
async fn rekey_scenario(
    before_fixture: &str,
    after_fixture: &str,
    read_js: &str,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let read_site = read_js;

    // **An isolated copy of the served tree, on its own port.**
    //
    // The first version published these fixtures straight into `dist/`, cleaned
    // up after itself, and still broke the monolith: Phase 27's fixture failed
    // with *"publisher bound no signature"*, because a publish whose content is
    // already present takes the engine's idempotent path and then wants a prior
    // signed head that another publisher's artifacts cannot supply. It passed
    // when the monolith ran alone. A test that can only be trusted when it runs
    // alone is not a gate, and "clean up carefully" is a weaker guarantee than
    // "never share the directory". So: copy the SPA, publish into the copy,
    // serve the copy. `dist/` is never written to.
    let root = format!("target/e2e-rekey-{}", label.replace(['=', ' '], "-"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let cp = Command::new("cp").args(["-a", "dist/.", &root]).status()?;
    if !cp.success() {
        return Err(format!("could not stage an isolated SPA copy at {root}: {cp}").into());
    }
    // A publish emits `entity-deployment.json`; a stale one copied out of `dist/`
    // would be read before the first fixture writes its own.
    let _ = std::fs::remove_file(format!("{root}/entity-deployment.json"));

    let port = pick_free_port()?;
    let server = Command::new("python3")
        .args(["tools/cors-serve.py", &root, &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let _serving = FederationServer(server);
    sleep(Duration::from_millis(400)).await;
    let url = format!("http://localhost:{port}/?log=trace");

    let mut planted: Vec<String> = Vec::new();

    let r = async {
        // ── 1. The domain as the returning visitor first met it ──────────────
        run_rekey_fixture(before_fixture, &root);
        let peer_a = deployment_home_peer(&root);
        planted.push(peer_a.clone());
        println!("  [{label}] re-key repro: published as A = {peer_a}");

        // A GENUINE cold boot, which this scenario is worthless without: the
        // whole point is that the FIRST contact persists publisher A and
        // registers A's origin, and a warm profile does neither. IndexedDB
        // survives WebDriver sessions, so without this wipe the two surface
        // variants (and consecutive runs) inherit each other's durable config,
        // the deployment doc is never fetched, and the staging assertion below
        // fires on a test bug that looks exactly like a product bug. Land a
        // document first — storage APIs need an origin.
        client.goto(&url).await?;
        wipe_all_storage(&client).await?;

        client.goto(&url).await?;
        wait_for_boot(&client, 30_000).await?;

        let home_text = poll_rendered(&client, read_site, "Welcome to the Entity Demo Site").await?;
        let cold = capture_log(&client).await?;
        if !home_text.contains("Welcome to the Entity Demo Site") {
            print_log(&cold);
        }
        // Staging: the cold boot must really have adopted A, on the IDB arm, and
        // rendered A's home. If this fails the re-key was never set up and every
        // assertion after it would be meaningless.
        assert!(
            cold.iter().any(|l| l.contains("DurableDirectIdb")),
            "re-key repro: the default boot must select the main-thread IDB peer \
             (the arm the incident is on)"
        );
        assert!(
            cold.iter().any(|l| l.contains("deployment-config: applied")),
            "re-key repro: cold boot did not apply the served /entity-deployment.json"
        );
        assert!(
            home_text.contains("Welcome to the Entity Demo Site"),
            "re-key repro: publisher A's home did not render on the cold boot; got: {home_text:?}"
        );

        // ── 2. The re-key ────────────────────────────────────────────────────
        // `publish` cleans only its OWN peer's subtree, so B's publish leaves A
        // standing; deleting A is the second half of a real re-key and is done
        // here rather than in the fixture so the two halves stay separable.
        run_rekey_fixture(after_fixture, &root);
        let peer_b = deployment_home_peer(&root);
        planted.push(peer_b.clone());
        assert_ne!(peer_a, peer_b, "re-key repro: the fixture did not change publisher");

        std::fs::remove_dir_all(format!("{root}/{peer_a}"))
            .unwrap_or_else(|e| panic!("re-key repro: could not retire A's tree: {e}"));
        assert!(
            !std::path::Path::new(&format!("{root}/{peer_a}")).exists(),
            "re-key repro: A's tree must be gone (the abandoned peer 404s)"
        );
        assert!(
            std::path::Path::new(&format!("{root}/{peer_b}/sites.list")).exists(),
            "re-key repro: B's tree must be serving"
        );
        println!("  [{label}] re-key repro: re-keyed A -> B = {peer_b}; A's tree retired (404s)");

        // ── 3. The returning visitor's next load ─────────────────────────────
        // Same client ⇒ same profile ⇒ same IndexedDB ⇒ a genuine WARM boot with
        // `home_site.peer_id = A` persisted and A's origin registered.
        client.goto(&url).await?;
        wait_for_boot(&client, 30_000).await?;

        let warm_text = poll_rendered(&client, read_site, "Welcome to the Entity Demo Site").await?;
        let warm = capture_log(&client).await?;

        // ANTI-VACUITY. Everything below asserts that the browser ends up on B —
        // which is also what a browser that simply COLD-booted would do, having
        // never held A at all. That would pass this test while exercising none of
        // it. So first prove the scenario was real: the boot must have been warm
        // (it carried a durable config naming A) AND must have detected the
        // re-key. Both are one line in the log, and without them a hermeticity
        // slip turns this whole file green for the wrong reason.
        let detected = warm.iter().any(|l| l.contains("DIFFERENT identity"));
        let recorded = warm.iter().any(|l| l.contains("recorded a retired publisher"));
        if !detected || !recorded {
            print_log(&warm);
        }
        assert!(
            detected,
            "the warm boot never detected the re-key — it was not a warm boot carrying \
             publisher {peer_a}, so this run proves nothing about healing"
        );
        assert!(
            recorded,
            "the re-key was detected but no supersession was recorded — every OTHER durable \
             reference to {peer_a} (window nav state, the overlay's) depends on that record"
        );

        // The reproduction signature, printed whether or not we go on to fail —
        // it is the artifact this test exists to produce, and it is the same
        // shape as the operator's captured console.
        let unresolvable: Vec<&String> = warm
            .iter()
            .filter(|l| l.contains("remote home has no registered origin"))
            .collect();
        let reconcile_fired = warm
            .iter()
            .any(|l| l.contains("home origin missing from the durable registry"));
        // The site overlay's own words for this state. Worth naming separately:
        // it is the ONLY user-visible report the incident produces, it appears
        // only on the site surface (the operator's profile is `chrome`, which is
        // why they saw nothing), and it describes the wrong thing — "the source
        // is unreachable" rather than "the publisher you are asking for was
        // replaced" — so it cannot be acted on even when it is seen.
        const STALE_OUTLINE: &str = "This site's source is unreachable";
        let showed_stale_outline = warm_text.contains(STALE_OUTLINE);
        println!(
            "  [{label}] warm boot — (1.2.5) reconcile fired: {reconcile_fired}; \
             unresolvable-home reports: {}; stale cached outline shown: {showed_stale_outline}",
            unresolvable.len()
        );
        for l in &unresolvable {
            println!("    {}", l.chars().take(220).collect::<String>());
        }

        if !warm_text.contains("Welcome to the Entity Demo Site") {
            print_log(&warm);
            // The site layer carries its whole stylesheet, so echo only the tail
            // — the rendered copy — rather than 3 KB of CSS that buries it.
            let tail: String = {
                let t = warm_text.trim_end();
                let n = t.chars().count();
                t.chars().skip(n.saturating_sub(180)).collect()
            };
            println!(
                "\n  ---- re-key reproduction ----\n  \
                 persisted publisher (stale): {peer_a}\n  \
                 published publisher (live):  {peer_b}\n  \
                 (1.2.5) reconcile fired:     {reconcile_fired}   \
                 <- false is the confirmed incident path (§1.1a)\n  \
                 stale cached outline shown:  {showed_stale_outline}\n  \
                 site-layer tail:             …{tail}\n  \
                 R1 is what turns this green: re-read /entity-deployment.json on a\n  \
                 warm boot with a remote home, compare IDENTITY, adopt the routing\n  \
                 facts, and re-point the persisted navigation state.\n"
            );
        }

        // ── 4. The heal (R1) ─────────────────────────────────────────────────
        // Asserted as BEHAVIOUR, not as a log string: this test was written
        // before the fix, so it must not encode the fix's implementation.
        assert!(
            warm_text.contains("Welcome to the Entity Demo Site"),
            "a re-keyed domain did not heal on the next boot — the browser is still \
             pointed at the retired publisher {peer_a} instead of {peer_b}"
        );
        assert!(
            unresolvable.is_empty(),
            "the browser still reports an unresolvable home after the re-key: {unresolvable:#?}"
        );

        // ── 5. The adoption must PERSIST ─────────────────────────────────────
        // A heal that re-derives itself every boot is a different (and quieter)
        // bug than no heal at all: it would keep working while never writing the
        // correction down, so the first offline boot after a re-key would fail.
        client.goto(&url).await?;
        wait_for_boot(&client, 30_000).await?;
        let again = poll_rendered(&client, read_site, "Welcome to the Entity Demo Site").await?;
        let third = capture_log(&client).await?;
        if !again.contains("Welcome to the Entity Demo Site") {
            print_log(&third);
        }
        assert!(
            again.contains("Welcome to the Entity Demo Site"),
            "the adopted publisher did not persist — the boot after the heal broke again"
        );

        let panics = count_panics(&third);
        assert!(panics.is_empty(), "panics across the re-key boots:\n{panics:#?}");
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    // The whole staging area goes, on every path. Nothing to unpick in `dist/`
    // because nothing was ever written there — which is the point of the copy.
    let _ = planted;
    let _ = std::fs::remove_dir_all(&root);
    r?;

    println!("  rekey_scenario[{label}] OK");
    client.close().await.ok();
    Ok(())
}

/// The locked-kiosk **site** surface — `ecdeos.org`-shaped, and the surface the
/// operator's own bricked profile was closest to.
#[tokio::test(flavor = "current_thread")]
async fn rekeyed_domain_heals_on_next_boot() -> Result<(), Box<dyn std::error::Error>> {
    rekey_scenario(
        "emit_rekey_fixture_before",
        "emit_rekey_fixture_after",
        r#"const sl=document.getElementById('site-layer');return sl?(sl.textContent||'').trim():'';"#,
        "surface=site",
    )
    .await
}

/// The maximized Site Browser **window** surface — `entitychurchfoundation.org`-
/// shaped, and **the case the first version of this fix did not cover.**
///
/// It is a separate deployment shape with separately-persisted navigation state
/// (per-window, not the overlay's app-level path), so a repair that only
/// re-points the overlay leaves this one pointed at the retired publisher. That
/// is why the fix records the supersession against the *peer* and resolves at
/// the single decode point both surfaces share, rather than re-pointing surfaces
/// it happens to know about.
#[tokio::test(flavor = "current_thread")]
async fn rekeyed_domain_heals_on_next_boot_window_surface(
) -> Result<(), Box<dyn std::error::Error>> {
    rekey_scenario(
        "emit_rekey_fixture_before_window",
        "emit_rekey_fixture_after_window",
        r#"const layer=document.getElementById('dom-layer');if(!layer)return '';
           const root=layer.shadowRoot||layer;
           const w=root.querySelector('section.window.maximized')||root.querySelector('section.window');
           return w?(w.textContent||'').trim():'';"#,
        "surface=window",
    )
    .await
}

/// How many durable supersession records `load()` found on this boot.
///
/// Read from the boot log rather than from the DOM because that is the only
/// place the *durable* registry is reported — and the durable registry, not the
/// live map, is what finding F2 is about. `load()` stays silent at zero, so an
/// absent line is zero records.
fn supersession_records(log: &[String]) -> usize {
    log.iter()
        .filter(|l| l.contains("loaded retired-publisher records"))
        .filter_map(|l| {
            let after = l.split("records = ").nth(1)?;
            after
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<usize>()
                .ok()
        })
        .next_back()
        .unwrap_or(0)
}

/// **The F2 gate — a supersession record must not be permanent.**
///
/// *The finding* (`AUDIT-REKEY-RECONCILE-2026-08-27` F2, HIGH). R1 writes a
/// `retired → replacement` record durably when the deployment document names a
/// different publisher. The first version had **no delete path, no listing and
/// no expiry**, so the trigger is not a re-key at all — it is a *mistake*. A
/// misconfigured publish, a bad templating run, a brief compromise: any
/// `/entity-deployment.json` that names the wrong peer for as long as one boot
/// is adopted by every browser that loads in that window, and then **survives
/// the origin being fixed**, because nothing ever re-reads it. That is brick
/// matrix cell **#6 at E5** — recovery is clear-site-data, which is destructive
/// and, with no export, costs the user everything else they had.
///
/// *What this reproduces.* Three states of one document, which is the shape of
/// the mistake rather than the shape of a re-key: the domain publishes as **A**,
/// briefly declares **B**, then declares **A** again. The false record `A→B` is
/// the durable damage, and after the correction the browser also holds the true
/// record `B→A` — so this is precisely the case a naive "drop what the document
/// disagrees with" would get backwards, condemning the good record along with
/// the bad one.
///
/// *Note A's tree is never deleted.* The re-key scenario deletes it because a
/// real re-key abandons the old publisher; here A never stopped publishing, and
/// deleting it would quietly convert this into a second copy of that test.
///
/// **The assertion is the record count, not the render, and that is deliberate.**
/// Both surfaces heal here either way — the overlay is re-pointed directly by
/// the adoption path — so a render assertion would pass on the unfixed tree and
/// prove nothing. That is exactly the trap finding F5 named in the sibling
/// gates ("one of the two surface gates does not exercise the new mechanism"),
/// and repeating it would be the whole bug. The discriminator is that a boot
/// after the correction loads **one** record and not two, which can only be true
/// if the drop reached durable storage.
#[tokio::test(flavor = "current_thread")]
async fn a_supersession_the_domain_contradicts_is_dropped(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let read_site =
        r#"const sl=document.getElementById('site-layer');return sl?(sl.textContent||'').trim():'';"#;

    // Same isolation rule as `rekey_scenario`: publish into a copy, never into
    // the shared `dist/`.
    let root = "target/e2e-supersession-revalidate".to_string();
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let cp = Command::new("cp").args(["-a", "dist/.", &root]).status()?;
    if !cp.success() {
        return Err(format!("could not stage an isolated SPA copy at {root}: {cp}").into());
    }
    let _ = std::fs::remove_file(format!("{root}/entity-deployment.json"));

    let port = pick_free_port()?;
    let server = Command::new("python3")
        .args(["tools/cors-serve.py", &root, &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let _serving = FederationServer(server);
    sleep(Duration::from_millis(400)).await;
    let url = format!("http://localhost:{port}/?log=trace");

    let r = async {
        // ── 1. The domain as it really is: publisher A, met cold ─────────────
        run_rekey_fixture("emit_rekey_fixture_before", &root);
        let peer_a = deployment_home_peer(&root);
        // The exact bytes the domain served before the mistake. Step 3 restores
        // these rather than re-running the fixture: a republish of content that
        // is already in the tree takes the engine's idempotent path and leaves
        // the emitted document still naming B, which made the first version of
        // this test fail on a fixture artifact instead of on the product. It is
        // also the more faithful reproduction — F2 is about a *document* being
        // wrong for one boot and then corrected, not about a republish.
        let doc_a = std::fs::read(format!("{root}/entity-deployment.json"))?;

        client.goto(&url).await?;
        wipe_all_storage(&client).await?;
        client.goto(&url).await?;
        wait_for_boot(&client, 30_000).await?;
        let home = poll_rendered(&client, read_site, "Welcome to the Entity Demo Site").await?;
        let cold = capture_log(&client).await?;
        assert!(
            home.contains("Welcome to the Entity Demo Site"),
            "staging: publisher A's home did not render on the cold boot; got {home:?}"
        );
        assert_eq!(
            supersession_records(&cold),
            0,
            "staging: a fresh profile must start with no supersession records"
        );

        // ── 2. The mistake: the document names B for one boot ────────────────
        // B is published so it is a real peer rather than a dangling id — a
        // document naming a peer that serves nothing would be rejected further
        // up and would never reach the record-writing path being tested.
        run_rekey_fixture("emit_rekey_fixture_after", &root);
        let peer_b = deployment_home_peer(&root);
        assert_ne!(peer_a, peer_b, "the fixture did not change the declared publisher");

        client.goto(&url).await?;
        wait_for_boot(&client, 30_000).await?;
        let _ = poll_rendered(&client, read_site, "Welcome to the Entity Demo Site").await?;
        let adopted = capture_log(&client).await?;
        assert!(
            adopted.iter().any(|l| l.contains("peer-supersession: recorded a retired publisher")),
            "staging: the bad document was not adopted, so there is no damage to repair. \
             Without this the rest of the test is vacuous."
        );

        // ── 3. The origin is fixed: the document names A again ───────────────
        std::fs::write(format!("{root}/entity-deployment.json"), &doc_a)?;
        assert_eq!(
            deployment_home_peer(&root),
            peer_a,
            "the corrected document must name A again"
        );

        client.goto(&url).await?;
        wait_for_boot(&client, 30_000).await?;
        let _ = poll_rendered(&client, read_site, "Welcome to the Entity Demo Site").await?;
        let corrected = capture_log(&client).await?;

        // Anti-vacuity: the boot that repairs must have actually SEEN the false
        // record. A boot that loaded nothing would satisfy the count assertion
        // below for the wrong reason.
        assert_eq!(
            supersession_records(&corrected),
            1,
            "the boot after the correction must load the one false record ({peer_a} → \
             {peer_b}) before it can drop it — it loaded a different number, so this \
             run is not reproducing F2"
        );
        assert!(
            corrected.iter().any(|l| l.contains("DROPPING a record the domain contradicts")),
            "F2 RED — the browser read a deployment document naming {peer_a} as the current \
             publisher while holding a durable record saying {peer_a} was RETIRED, and kept \
             the record.\n\
             This is brick-matrix cell #6 at E5: a transient bad /entity-deployment.json \
             becomes permanent client state that survives the origin being fixed, and the \
             only recovery is clear-site-data.\n\
             The fix is to re-check the records against the live document on every boot \
             that has one, and drop the ones it contradicts."
        );

        // ── 4. And the drop must be DURABLE ──────────────────────────────────
        // The half that makes this a fix rather than a per-session cosmetic: a
        // removal from the in-process map alone leaves the entity on disk, and
        // `load()` puts it straight back on the next boot. One record must
        // survive — the TRUE one (B→A) written when the document was corrected
        // — so this also proves the predicate did not take the good record with
        // the bad one.
        client.goto(&url).await?;
        wait_for_boot(&client, 30_000).await?;
        let after = poll_rendered(&client, read_site, "Welcome to the Entity Demo Site").await?;
        let final_log = capture_log(&client).await?;
        let n = supersession_records(&final_log);
        if n != 1 {
            print_log(&final_log);
        }
        assert_eq!(
            n, 1,
            "the drop did not reach durable storage (or took the good record with it): the \
             boot after the repair loaded {n} record(s), expected exactly 1 — the true \
             {peer_b} → {peer_a} written when the document was corrected"
        );
        assert!(
            after.contains("Welcome to the Entity Demo Site"),
            "the site stopped rendering after the records were revalidated — the repair \
             broke the thing it exists to protect; got {after:?}"
        );

        let panics = count_panics(&final_log);
        assert!(panics.is_empty(), "panics across the revalidation boots:\n{panics:#?}");
        println!("  F2: false record dropped, {n} true record retained across a reload");
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    let _ = std::fs::remove_dir_all(&root);
    r?;
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
    assert_eq!(click_peers_mode_btn(&client, "backend-memory").await?, "clicked");
    sleep(Duration::from_millis(1500)).await;
    assert_eq!(click_peers_mode_btn(&client, "backend-opfs").await?, "clicked");
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
    let t2_create = click_peers_mode_btn(&client, "backend-memory").await?;
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
                return sec.querySelectorAll('tbody tr:not(.peer-group)').length;
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

/// **The subscription proof — and it needs a boot of its own to mean anything.**
///
/// A Worker-arm surface must subscribe every prefix it reads: `get_entity` /
/// `tree_listing` there are a main-thread **cache mirror**, seeded only for
/// subscribed prefixes, so an unsubscribed read is not an error — it is an empty
/// answer, forever, on a surface whose data is perfectly readable by everyone
/// else. `FileTransferWindow` reads its own offers prefix and subscribes it for
/// exactly that reason.
///
/// **Phase 14.2 cannot prove that, and this test exists because measuring said
/// so.** Removing the subscription leaves 14.2 green: the monolith has every
/// window open, and **two of them subscribe the whole peer tree** (Entity Tree's
/// `observe_with_events` on `/{pid}/` and Storage's `watch_prefix` on the same),
/// while `WorkerProxy::cache_list`/`cache_get` are a **union over every
/// subscription's mirror**. So in that session a window that forgot to subscribe
/// still reads. **This generalizes: no assertion in the monolith can catch a
/// missing app-tier subscription while those two windows are open** — a fact
/// worth knowing before trusting any "the window subscribes what it reads"
/// claim made from a green suite.
///
/// Here the File Transfer window is the only one open, so the mirror it reads is
/// its own. Mutation-checked: delete the offers `watch_prefix` in
/// `FileTransferWindow::window_type` and this goes red (the offer still
/// completes and reports; only the owner's own view of it is missing) while the
/// monolith stays green.
#[tokio::test]
async fn a_lone_file_transfer_window_lists_what_it_offers() -> Result<(), Box<dyn std::error::Error>>
{
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(600)).await;

    // Open File Transfer, and nothing else.
    assert_eq!(
        click_spawn_btn(&client, "+ File Transfer").await?,
        "clicked",
        "couldn't open the File Transfer window"
    );
    sleep(Duration::from_millis(600)).await;

    // THE PRECONDITION, asserted rather than assumed: no whole-tree subscriber
    // is open. If a future boot spawns Entity Tree or Storage by default, this
    // test would silently stop proving anything — so it fails loudly instead.
    let lone = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            const open = [];
            for (const s of root.querySelectorAll('section.window')) {
                const h = s.querySelector('h2, h3, header');
                open.push((h ? h.textContent : s.className).trim().slice(0, 40));
            }
            return {
                tree: !!root.querySelector('.tree-panel'),
                storage: !!root.querySelector('.storage'),
                ft: !!root.querySelector('.file-transfer'),
                open,
            };
            "#,
            vec![],
        )
        .await?;
    assert!(
        lone.get("ft").and_then(|v| v.as_bool()).unwrap_or(false),
        "the File Transfer window is not open: {lone}"
    );
    assert!(
        !lone.get("tree").and_then(|v| v.as_bool()).unwrap_or(true)
            && !lone.get("storage").and_then(|v| v.as_bool()).unwrap_or(true),
        "a whole-tree subscriber (Entity Tree / Storage) is open, so this test \
         would pass on ITS mirror and prove nothing about File Transfer's own \
         subscription. Open windows: {lone}"
    );
    println!("  open windows: {}", lone.get("open").unwrap_or(&serde_json::Value::Null));

    let name = "lone-window-offer.bin";
    let sent = client
        .execute(
            &format!(
                r#"
                const layer = document.getElementById('dom-layer');
                const root = layer.shadowRoot || layer;
                let sec = null;
                for (const s of root.querySelectorAll('section.window')) {{
                    if (s.querySelector('.file-transfer')) {{ sec = s; break; }}
                }}
                if (!sec) return 'no-window';
                const inp = sec.querySelector('[data-field="ft-offer-input"]');
                if (!inp) return 'no-offer-input';
                const n = 600000;
                const a = new Uint8Array(n);
                for (let i = 0; i < n; i++) a[i] = (i * 31) % 251;
                const f = new File([a], '{name}', {{ type: 'application/octet-stream' }});
                const dt = new DataTransfer();
                dt.items.add(f);
                inp.files = dt.files;
                inp.dispatchEvent(new Event('change'));
                return 'sent';
                "#,
            ),
            vec![],
        )
        .await?;
    assert_eq!(sent.as_str(), Some("sent"), "could not drive the offer picker: {sent}");

    let listed = poll_json(
        &client,
        &format!(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            let sec = null;
            for (const s of root.querySelectorAll('section.window')) {{
                if (s.querySelector('.file-transfer')) {{ sec = s; break; }}
            }}
            if (!sec) return {{ row: false, reason: 'no-window' }};
            const results = sec.querySelector('[data-field="ft-results"]');
            const text = results ? results.textContent : '';
            return {{
                row: !!sec.querySelector('[data-field="ft-offer-row"][data-offer-name="{name}"]'),
                reported: text.includes('✓ offering') && text.includes('{name}'),
                tail: text.slice(-300),
            }};
            "#,
        ),
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("row").and_then(|r| r.as_bool()).unwrap_or(false),
    )
    .await?;
    assert!(
        listed.get("reported").and_then(|r| r.as_bool()).unwrap_or(false),
        "the offer itself never completed on the Worker arm — that is the ingest \
         or the manifest put, not the subscription. Detail: {listed}"
    );
    assert!(
        listed.get("row").and_then(|r| r.as_bool()).unwrap_or(false),
        "the offer completed and the window never listed it. With no other window \
         open there is no mirror to borrow: this is `FileTransferWindow`'s own \
         `watch_prefix` on the offers prefix. Detail: {listed}"
    );
    println!("  a lone File Transfer window listed its own offer");

    client.close().await.ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// The naming chain, in a real browser, cross-origin
// ---------------------------------------------------------------------------

/// A `tools/cors-serve.py` child serving the emitted federation on its own port.
///
/// **Not `python3 -m http.server`** — that is what `start_dist_server` uses, and
/// it sends **no CORS headers at all**. Same-origin (the app's own bundle) does
/// not care; a *cross-origin* fetch of a published tree does, and the browser
/// drops the response no matter how good the bytes are. That difference is the
/// entire point of this test, so the two servers are deliberately different
/// programs (`RUNBOOK-CDN-BROWSER-DEPLOYMENT` §2.1).
struct FederationServer(Child);

impl Drop for FederationServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Emit a four-domain federation whose bindings carry **absolute** origins on
/// `port`, then serve it there with CORS. Returns `(server, registry_peer_id)`.
fn start_federation(port: u16) -> Result<(FederationServer, String), Box<dyn std::error::Error>> {
    let out = "target/e2e-federation";
    // `ORIGIN_BASE` is what makes each binding's `http-poll` transport an
    // absolute cross-origin URL instead of a path slug — i.e. what makes the
    // resolved name reachable from an app served somewhere else entirely.
    let status = Command::new("bash")
        .args(["tools/local-federation.sh", out])
        .env("ORIGIN_BASE", format!("http://localhost:{port}/"))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err(format!("local-federation.sh failed: {status}").into());
    }

    let mapping = std::fs::read_to_string(format!("{out}/MAPPING.txt"))?;
    // The registry peer-id is the ONE string a consumer holds a priori, and the
    // mapping file is where the publish reports it.
    let registry = mapping
        .lines()
        .skip_while(|l| !l.contains("registry peer"))
        .nth(1)
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or("no registry peer-id in MAPPING.txt")?;

    // stderr is NOT discarded: a bind failure is the one thing that makes this
    // rig lie. A squatter on the port serves 404s with no CORS, the browser
    // reports `NetworkError`, and it reads exactly like the app failing CORS —
    // which is what happened the first time this ran (a stale `http.server` from
    // another session held the port).
    let child = Command::new("python3")
        .args(["tools/cors-serve.py", out, &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    Ok((FederationServer(child), registry))
}

/// A free TCP port. The federation's binding origins are **baked in at publish
/// time**, so the port has to be known before the emit — it cannot be an
/// ephemeral listener handed to a server later.
fn pick_free_port() -> Result<u16, std::io::Error> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    let p = l.local_addr()?.port();
    drop(l);
    Ok(p)
}

/// Where the published federation lives for this run, and who is serving it.
///
/// **Two modes, and the difference is the whole point of the multi-host gate.**
/// In the default (local) mode the federation is emitted and served by a child of
/// this process on `localhost` — real HTTP, real CORS, but the consumer and the
/// origin share a host, so every fetch is a loopback hop. In **external** mode the
/// origin is already standing somewhere else (`tools/e2e/federation-multihost.sh`
/// puts it in its own container with its own routable IP) and this process only
/// *consumes* it.
///
/// `EXTENSION-SIGNALING` §11.5.1's blindness class is why the second mode exists:
/// a loopback run reports success for things that cannot work off it, and "every
/// individual step reports success" is exactly what it looks like.
struct FederationTarget {
    /// Origin base with **no** trailing slash — e.g. `http://10.89.1.5:8099`.
    base: String,
    registry_pid: String,
    /// `Some` only in local mode; dropping it kills the child server.
    _server: Option<FederationServer>,
}

/// Resolve the federation for this run — external if `E2E_FED_ORIGIN` names one,
/// otherwise emitted and served locally exactly as before.
///
/// External mode also needs `E2E_FED_REGISTRY` (the registry peer-id), because
/// the consumer is deliberately not the publisher and must not read the
/// publisher's `MAPPING.txt` off a shared filesystem to learn it — that would be
/// the same-process shortcut this gate exists to remove. It is the one string a
/// consumer holds a priori, and here it genuinely arrives out of band.
fn federation_target() -> Result<FederationTarget, Box<dyn std::error::Error>> {
    if let Ok(origin) = std::env::var("E2E_FED_ORIGIN") {
        let base = origin.trim().trim_end_matches('/').to_string();
        let registry_pid = std::env::var("E2E_FED_REGISTRY").map_err(|_| {
            "E2E_FED_ORIGIN is set but E2E_FED_REGISTRY is not — the consumer has no pin, and \
             reading it off the publisher's disk would reintroduce the shared-filesystem \
             shortcut this mode exists to remove"
        })?;
        assert_not_loopback(&base)?;
        // **Deliberately NOT `assert_origin_healthy` here — the vantage is wrong.**
        // In external mode this process is the *orchestrator*, not the consumer:
        // the browser is a container on the publisher's bridge and this test runs
        // on the host, which under rootless podman has no route into that bridge
        // at all. Probing from here would fail on a perfectly healthy origin — it
        // did, first run — and worse, a probe that *did* pass from here would be
        // evidence about a path nobody under test takes. The rig owns this check
        // and runs it from inside the network, which is where the consumer is.
        return Ok(FederationTarget { base, registry_pid: registry_pid.trim().to_string(), _server: None });
    }
    let port = match std::env::var("E2E_FED_PORT").ok().and_then(|v| v.trim().parse().ok()) {
        Some(p) => p,
        None => pick_free_port()?,
    };
    let (server, registry_pid) = start_federation(port)?;
    let base = format!("http://localhost:{port}");
    assert_origin_healthy(&base)?;
    Ok(FederationTarget { base, registry_pid, _server: Some(server) })
}

/// **The control that keeps "multi-host" from silently becoming "same host".**
///
/// `E2E_FED_ORIGIN=http://localhost:8099` would run this gate end-to-end, pass
/// every assertion, and prove exactly what the local mode already proves — while
/// the target's name says otherwise. A rig that can quietly degrade into the
/// thing it was built to replace is the NAT rig's lesson one layer over: **probe
/// your own controls, or a green run means something you did not measure.**
fn assert_not_loopback(base: &str) -> Result<(), Box<dyn std::error::Error>> {
    let host = host_of(base);
    let loopback = host == "localhost"
        || host == "::1"
        || host.parse::<std::net::Ipv4Addr>().map(|a| a.is_loopback()).unwrap_or(false);
    if loopback {
        return Err(format!(
            "E2E_FED_ORIGIN points at {host:?}, which is loopback — this is the MULTI-HOST gate \
             and a loopback origin makes it a slower copy of the local one. §11.5.1's blindness \
             class is precisely what the separate host is here to remove."
        )
        .into());
    }
    Ok(())
}

/// `http://10.89.1.5:8099` → `10.89.1.5`; `http://localhost:8099` → `localhost`.
fn host_of(base: &str) -> String {
    base.trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit_once(':')
        .map(|(h, _)| h.to_string())
        .unwrap_or_else(|| base.trim_start_matches("http://").to_string())
}

/// `http://10.89.1.5:8099` → `8099`, defaulting to 80.
fn port_of(base: &str) -> u16 {
    base.trim_start_matches("http://")
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit_once(':')
        .and_then(|(_, p)| p.parse().ok())
        .unwrap_or(80)
}

/// Assert the federation origin is up, is **ours**, and sends CORS — before the
/// browser is asked to trust any of it.
///
/// Without this the rig fails open: anything else listening on the port answers,
/// the browser reports a bare `NetworkError`, and the diagnosis lands on the app.
/// "Prove the network path before blaming the application" (AGENTS.md, learned on
/// the NAT rig) applies to a static origin just as well.
fn assert_origin_healthy(base: &str) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    let (host, port) = (host_of(base), port_of(base));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = String::from("never connected");
    while Instant::now() < deadline {
        if let Ok(mut sock) = std::net::TcpStream::connect((host.as_str(), port)) {
            let req = format!(
                "GET /MAPPING.txt HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
            );
            if sock.write_all(req.as_bytes()).is_ok() {
                let mut raw = Vec::new();
                let _ = sock.read_to_end(&mut raw);
                let text = String::from_utf8_lossy(&raw).to_string();
                let ok = text.starts_with("HTTP/1.0 200") || text.starts_with("HTTP/1.1 200");
                let cors = text.to_ascii_lowercase().contains("access-control-allow-origin");
                if ok && cors {
                    return Ok(());
                }
                last = format!(
                    "200={ok} cors={cors} — first line: {:?}",
                    text.lines().next().unwrap_or("")
                );
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!(
        "the federation origin at {base} is not healthy ({last}). Something else is \
         probably holding the port — a plain `http.server` answers without CORS and the \
         browser then reports a bare NetworkError that looks like an app bug."
    )
    .into())
}

/// **The whole naming chain, in a browser, across two origins.**
///
/// Everything else that exercises this runs native: the `LocalWeb` unit tests
/// read from disk, and `four_domains_resolve_and_serve_over_real_http_with_cors`
/// speaks HTTP from a Rust client. Neither can prove the part that only a
/// browser has — **CORS enforcement** and the real `window.fetch`. A native
/// client happily reads a response a browser would discard, which is exactly the
/// gap `RUNBOOK-CDN-BROWSER-DEPLOYMENT` was written for and the one this repo is
/// assigned by name.
///
/// The app is served from one origin (`:8092`) and the published federation from
/// another (`:8099`-ish), so **every fetch in the walk is cross-origin**. The
/// consumer types one string it could have got off a business card — the
/// registry's peer-id — and ends up holding verified page bytes from a domain
/// nothing in the build knows about.
///
/// What each assertion is really for:
/// - `name pin` — the peer-id alone is a usable pin (it embeds the public key).
/// - `name resolve` — hop 1 crosses an origin boundary and the evidence fields
///   report that D1's name check and the §6a.6 revocation probe actually ran.
/// - `name open` — hop 2 pins a *different* publisher at a *different* path and
///   walks its signed root to bytes whose hash was recomputed on the way down.
#[tokio::test]
async fn a_name_resolves_cross_origin_to_a_verified_page_in_a_browser(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;

    // Local by default (a child on `localhost`), or an already-standing origin on
    // its own host when `E2E_FED_ORIGIN` names one — see `federation_target`. The
    // walk below is byte-identical either way, which is the point: the *topology*
    // is the variable under test, not the code path.
    let fed = federation_target()?;
    let (base, registry_pid) = (fed.base.clone(), fed.registry_pid.clone());
    println!("  federation at {base}, registry {}", &registry_pid[..12.min(registry_pid.len())]);

    let url = format!("{}/?log=trace", app_base());
    client.goto(&url).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(600)).await;

    // The Shell is the surface; `name` is app-local, like `connector` and `meet`.
    assert_eq!(
        click_spawn_btn(&client, "+ Shell").await?,
        "clicked",
        "couldn't open the Shell window"
    );
    sleep(Duration::from_millis(500)).await;

    // --- the pin: one string, and it is not a key ---
    let pin_out = shell_submit(
        &client,
        &format!("name pin {registry_pid} {base}/registry"),
        400,
    )
    .await?;
    assert!(
        pin_out.contains("pinned registry"),
        "the registry did not pin — scrollback:\n{pin_out}"
    );

    // --- hop 1: cross-origin, and the evidence says what was checked ---
    let resolved = shell_submit(&client, "name resolve entitychurch.org", 2500).await?;
    assert!(
        resolved.contains("entitychurch.org →"),
        "the name did not resolve cross-origin — scrollback:\n{resolved}"
    );
    for claim in ["association=true", "name=true", "revocation=true"] {
        assert!(
            resolved.contains(claim),
            "the resolution did not report {claim} — a check was skipped:\n{resolved}"
        );
    }
    assert!(
        resolved.contains(&host_of(&base)),
        "the binding must carry the cross-origin where — scrollback:\n{resolved}"
    );

    // --- hop 2: a second publisher, pinned by the id the registry named ---
    // `demo`/`index` is what `local-federation.sh` actually publishes — the site
    // id is the PUBLISH's, not the domain slug, and asking for a slug-named site
    // walks the signed root to a key that is simply absent.
    let opened = shell_submit(&client, "name open entitychurch.org demo index", 6000).await?;
    assert!(
        opened.contains("verified"),
        "the page did not verify — scrollback:\n{opened}"
    );
    assert!(
        opened.contains("sites/demo/pages/index"),
        "the verified key is not the page we asked for — scrollback:\n{opened}"
    );

    // --- the DEFAULTS, which used to be guesses (audit F5) ---
    // `name open <name>` with no site defaulted to `"home"` — a site id that
    // exists nowhere in a real publish, only in this repo's test fixtures — so
    // the obvious first command always failed on a key nothing publishes and
    // read as "that page isn't there". It must ask, using the publisher's own
    // listing, and must NOT walk to a site it invented.
    let no_site = shell_submit(&client, "name open entitychurch.org", 8000).await?;
    assert!(
        no_site.contains("which site?") && no_site.contains("demo"),
        "with no site given the verb must offer what the publisher lists — scrollback:\n{no_site}"
    );
    assert!(
        !no_site.contains("sites/home/"),
        "the verb must not fall back to a guessed site id — scrollback:\n{no_site}"
    );

    // With a site but no page, the landing page comes from that site's OWN
    // manifest, read through the signed root — verified, where `"index"` was a
    // guess that happened to match our emitter and nobody else's.
    let no_page = shell_submit(&client, "name open entitychurch.org demo", 8000).await?;
    assert!(
        no_page.contains("sites/demo/pages/index") && no_page.contains("verified"),
        "the landing page must come from the manifest and verify — scrollback:\n{no_page}"
    );

    // A key the publisher never published must be refused, not invented — the
    // control that says "verified" above means the walk, not a default.
    let absent = shell_submit(&client, "name open entitychurch.org demo nosuchpage", 8000).await?;
    assert!(
        !absent.contains("sites/demo/pages/nosuchpage verified"),
        "an absent page must not verify — scrollback:\n{absent}"
    );

    // A name the registry does not carry must fail closed, not fall back to
    // anything. This is the same fail-closed posture as `chain_exhausted`.
    // Budget note: a MISS is slower than a hit, not faster — the walk still
    // fetches the manifest, the signature and the interior nodes before the key
    // turns out to be absent. 2.5s was not enough and read as "no output".
    let missing = shell_submit(&client, "name resolve not-a-real-name.example", 6000).await?;
    assert!(
        missing.contains("no binding for this name in the signed registry"),
        "an unbound name must fail closed with the signed-absence message — scrollback:\n{missing}"
    );

    // Nothing may have panicked: a dropped rejecting promise reloads the app and
    // would make every assertion above meaningless (see AGENTS.md).
    let log_lines = capture_log(&client).await?;
    let panics = count_panics(&log_lines);
    assert!(panics.is_empty(), "a window panicked during the walk: {panics:?}");

    println!("  a browser resolved a name cross-origin and verified the page it named");
    client.close().await.ok();
    Ok(())
}

/// A copy of `dist/` carrying a `/entity-deployment.json` that seeds a registry
/// pin — the shape a real deployment ships.
///
/// **Hardlinks, and inside `target/`**, so the 28 MB debug wasm is not copied and
/// the link cannot fail across filesystems. Served on its own port, which also
/// makes it a distinct browser ORIGIN from the dist server — that is what
/// guarantees a **cold** boot (separate localStorage/IDB), and a cold boot is the
/// only one that reads a deployment document at all.
fn stage_pinned_spa(
    registry_pid: &str,
    registry_origin: &str,
    port: u16,
) -> Result<(FederationServer, std::path::PathBuf), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("target/e2e-pinned-spa");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    fn link_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            let (src, dst) = (e.path(), to.join(e.file_name()));
            if e.file_type()?.is_dir() {
                link_tree(&src, &dst)?;
            } else {
                // Hardlink, falling back to a copy for anything exotic.
                if std::fs::hard_link(&src, &dst).is_err() {
                    std::fs::copy(&src, &dst)?;
                }
            }
        }
        Ok(())
    }
    link_tree(std::path::Path::new("dist"), &root)?;

    // **Only the pin.** A minimal document keeps the blast radius minimal: every
    // other key (surface, home_site, origins, site_mode) would move this boot's
    // posture away from the default the rest of the suite asserts against, and
    // the property under test is the pin alone.
    std::fs::write(
        root.join("entity-deployment.json"),
        format!(
            "{{\n  \"name_registry_pin\": {{ \"peer_id\": \"{registry_pid}\", \"origin\": \"{registry_origin}\" }}\n}}\n"
        ),
    )?;

    let child = Command::new("python3")
        .args(["tools/cors-serve.py", root.to_str().unwrap(), &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    Ok((FederationServer(child), root))
}

/// **The §7.4 preload, in a browser, with the user typing nothing.**
///
/// Everything under `name_registry_pin` is proven natively — the deployment
/// document parses, the pin rides the durable `SessionConfig`, the boot mirror
/// carries it, and the user's own pin outranks it. **None of that is evidence
/// that a browser boot installs it**, and this repo's standing failure is exactly
/// that shape: a control proven on the path the test takes and absent on the one
/// it does not (`pack_mirror` ate a relay while every native test passed;
/// `resolve_name` had zero callers for a session).
///
/// So this boots a real browser against a real deployment document served from a
/// real origin, and then **types no pin**. If the seeded pin does not reach
/// `session_config::active_registry_pin()`, the resolve below fails with "no
/// registry pinned" and says so.
///
/// The control is the previous test: it types a pin and resolves the same name.
/// If both fail, the chain moved; if only this one fails, the *seeding* moved.
#[tokio::test]
async fn a_deployment_that_seeds_a_registry_pin_resolves_a_name_with_nothing_typed(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;

    let fed = federation_target()?;
    let registry_pid = fed.registry_pid.clone();

    let spa_port = pick_free_port()?;
    let registry_origin = format!("{}/registry", fed.base);
    let (_spa, staged) = stage_pinned_spa(&registry_pid, &registry_origin, spa_port)?;
    // Precondition: the document we are about to rely on is actually served, and
    // carries the pin. Without this, "no registry pinned" below would be
    // indistinguishable from a staging bug — the same lesson as probing the
    // federation origin before trusting the browser's NetworkError.
    let served = std::process::Command::new("curl")
        .args(["-fsS", "--retry", "20", "--retry-all-errors", "--retry-delay", "1",
               &format!("http://localhost:{spa_port}/entity-deployment.json")])
        .output()?;
    let body = String::from_utf8_lossy(&served.stdout).to_string();
    assert!(
        body.contains(&registry_pid) && body.contains("name_registry_pin"),
        "the staged deployment document is not being served (staged at {}): {body:?}",
        staged.display()
    );
    println!("  pinned SPA on :{spa_port}, seeding {}", &registry_pid[..12.min(registry_pid.len())]);

    // A DIFFERENT origin from the dist server, so this is a cold boot and the
    // deployment document is read.
    client.goto(&format!("http://localhost:{spa_port}/?log=trace")).await?;
    wait_for_boot(&client, 30_000).await?;
    sleep(Duration::from_millis(600)).await;

    assert_eq!(
        click_spawn_btn(&client, "+ Shell").await?,
        "clicked",
        "couldn't open the Shell window"
    );
    sleep(Duration::from_millis(500)).await;

    // (1) The shell reports the seeded pin AND says where it came from. The
    // source label is asserted because two pins that resolve identically are not
    // the same fact — resolving through a registry nobody in this tab chose is
    // precisely what must not be silent.
    let pins = shell_submit(&client, "name pins", 1500).await?;
    assert!(
        pins.contains("seeded by this deployment"),
        "the deployment pin did not reach the shell — scrollback:\n{pins}"
    );
    assert!(
        pins.contains(&registry_pid[..8.min(registry_pid.len())]),
        "the shell reports a pin, but not the one this deployment seeded — scrollback:\n{pins}"
    );

    // (2) And it RESOLVES, with no `name pin` ever typed. This is the whole
    // deliverable: the distance between "we built a naming system" and "a user
    // who types nothing can use it".
    let resolved = shell_submit(&client, "name resolve entitychurch.org", 6000).await?;
    assert!(
        !resolved.contains("no registry pinned"),
        "the seeded pin was not in force at resolution time — scrollback:\n{resolved}"
    );
    assert!(
        resolved.contains("entitychurch.org →"),
        "the name did not resolve through the seeded pin — scrollback:\n{resolved}"
    );
    for claim in ["association=true", "name=true", "revocation=true"] {
        assert!(
            resolved.contains(claim),
            "a seeded-pin resolve skipped {claim} — it must check exactly what a typed pin does:\n{resolved}"
        );
    }

    // (3) The user's own pin still outranks it, in the live surface and not just
    // in the unit test. Pinning the SAME registry at a bogus origin is the clean
    // discriminator: if the deployment value were still winning, the resolve
    // would keep working.
    let repin = shell_submit(
        &client,
        &format!("name pin {registry_pid} http://localhost:{spa_port}/not-a-registry"),
        1500,
    )
    .await?;
    assert!(repin.contains("pinned registry"), "the re-pin was refused — scrollback:\n{repin}");
    let after = shell_submit(&client, "name pins", 1500).await?;
    assert!(
        after.contains("yours"),
        "after `name pin` the shell must report the USER's pin as in force — scrollback:\n{after}"
    );

    let log_lines = capture_log(&client).await?;
    let panics = count_panics(&log_lines);
    assert!(panics.is_empty(), "a window panicked during the seeded-pin boot: {panics:?}");

    println!("  a deployment seeded the pin and the browser resolved a name with nothing typed");
    client.close().await.ok();
    Ok(())
}

/// **The 17 MB worker bundle must be downloaded once per BUILD, not once per
/// load** — measured at the wire, because nothing in the browser can see it.
///
/// `entity-worker_bg.wasm` is 17.1 MB and trunk's `data-type="worker"` pipeline
/// emits it under a FIXED filename, so it misses `sw.js`'s `HASHED_ASSET` rule,
/// falls to `networkFirst`, and `networkFirst` fetches with `cache: 'reload'` —
/// a deliberate, unconditional, HTTP-cache-bypassing download. Every visitor
/// paid it on every load, **once per worker** (boot spawns one; every persisted
/// `Backend*` peer respawns another), and no HTTP cache could help because the
/// fetch opts out of one.
///
/// It was also the cause of this suite's intermittent
/// `Navigation timed out after 60000 ms`, which landed on a different phase
/// every run (19, 20 and 21 were all seen) and read as flake for three
/// sessions: by Phase 16 there are two workers, so one navigation pulls ~51 MB
/// while the navigation itself is on the same `networkFirst` path, competing
/// with them.
///
/// **Why the assertion is server-side.** The obvious in-page check does not
/// work: Firefox zeroes `transferSize` *and* `encodedBodySize` for any response
/// a service worker supplied, whether the SW hit its cache or went to the
/// network. Measured both ways — a load that demonstrably fetched 17 MB and one
/// that fetched nothing both report `transferSize=0`. A gate built on it would
/// have been vacuous and looked fine. The server's request log is the only
/// honest account of bytes crossing the wire.
///
/// Mutation-checked in both directions, against a real Firefox:
/// - Restore the pre-fix `sw.js` → the server sees **3** requests for 3 loads.
/// - With the fix → **1**.
/// - And the deploy path, which is the failure that would be silent: after the
///   main bundle's hash changes, the worker is refetched exactly once. An
///   earlier version of the fix served the STALE worker to the new bundle — a
///   `entity-wasm-worker-protocol` mismatch, worse than the bug — because
///   `cache.match('/', {ignoreSearch: true})` resolves in insertion order and
///   kept returning the shell written at install. That is why the SW now caches
///   navigations under one canonical key.
#[tokio::test]
async fn the_worker_bundle_is_fetched_once_per_build_not_once_per_load(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=1", http_server_port());

    const LOADS: usize = 3;
    for n in 1..=LOADS {
        client.goto(&url).await?;
        wait_for_boot(&client, BOOT_BUDGET_MS).await?;
        // Wait for the WORKER specifically, not just the frame loop — the
        // bundle we are counting is fetched in the worker's realm, after boot.
        // Polled, never slept: a fixed wait here would make the count depend on
        // how loaded the box is, which is the very shape being fixed.
        poll_json(
            &client,
            "return (window.__entity_browser_log||[])\
             .map(e => (e && e.args ? e.args.join(' ') : String(e)))\
             .filter(s => s.indexOf('wasm_bindgen init resolved') !== -1).length;",
            ASYNC_ROUND_TRIP_BUDGET,
            |v| v.as_u64().unwrap_or(0) >= 1,
        )
        .await
        .map_err(|e| format!("load {n}: the worker never finished wasm init ({e})"))?;
    }

    let fetches = server.request_count("entity-worker_bg.wasm");
    assert_eq!(
        fetches, 1,
        "the worker bundle (17 MB) was fetched {fetches}× across {LOADS} loads of the same \
         build — it must be fetched exactly once and served from the service-worker cache \
         thereafter. {} See `WORKER_ASSET` / `buildScopedAsset` in assets/sw.js.",
        if fetches > 1 {
            "This is the pre-fix behaviour: the bundle is on the networkFirst path, whose \
             `cache: 'reload'` bypasses the HTTP cache unconditionally."
        } else {
            "Zero fetches means it was never requested at all — the worker probably did not \
             spawn, so this run proves nothing about caching."
        }
    );

    println!("  the 17 MB worker bundle was fetched once across {LOADS} loads (was once per load)");
    client.close().await.ok();
    Ok(())
}

/// A running app must survive its own save.
///
/// The Apps window watches the save prefix — the Saves panel lists it, and on
/// the Worker arm a read only lands in the cache mirror for a *subscribed*
/// prefix, so leaving it unwatched reads empty after a reload. That
/// subscription also flipped the window's dirty flag, and a dirty window is
/// rebuilt: `render_player` clears the container and creates a **new** iframe.
/// So every debounced save write tore the running app down and remounted it
/// from its start screen, roughly a second after each move — reported from the
/// outside as "it keeps knocking me out of the app".
///
/// Two facts are asserted and BOTH are load-bearing:
///
///  1. the save actually **landed** (`window.__entity_app_save_seq` advances),
///     which is what stops this being satisfied by an app that never saved; and
///  2. the iframe is the **same element** — a JS expando set before the wait is
///     still there afterwards. An attribute would not do: the regression's
///     signature is that the element is replaced, so anything stamped on it is
///     destroyed exactly when the gate needs to read it.
///
/// The survival check polls for the *failure* rather than sleeping and reading
/// once, so the red path returns as soon as the teardown happens and the green
/// path genuinely spends the budget instead of checking too early.
#[tokio::test]
async fn a_running_app_survives_its_own_save() -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());
    client.goto(&url).await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;

    // Open Apps, and nothing else — this test enters the app's frame by index,
    // and the assertion below is what keeps that index from being a guess.
    assert_eq!(
        click_spawn_btn(&client, "+ Apps").await?,
        "clicked",
        "couldn't open the Apps window"
    );

    // Launch War: it is the demo app that persists on every move
    // (`flipOnce` -> `persist()` -> `ctx.save(state)`), which is the trigger.
    let read_apps = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Apps') continue;
            return {
                open: true,
                war: !!Array.from(sec.querySelectorAll('button'))
                    .find(b => !b.hasAttribute('data-chip') && b.textContent.includes('War')),
            };
        }
        return { open: false, war: false };
    "#;
    let grid = poll_json(&client, read_apps, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("war").and_then(|b| b.as_bool()).unwrap_or(false)
    })
    .await?;
    assert!(
        grid.get("war").and_then(|b| b.as_bool()).unwrap_or(false),
        "the Apps launcher never listed the War demo, so there is nothing to \
         play. Got: {grid:?}"
    );
    let launched = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => !b.hasAttribute('data-chip') && b.textContent.includes('War'));
                if (!card) return 'no-war-card';
                card.click();
                return 'clicked';
            }
            return 'no-apps-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(launched.as_str().unwrap_or(""), "clicked", "War launch failed: {launched:?}");

    // Mark the mounted iframe with an expando. A section rebuild creates a
    // fresh element, so the probe's survival IS "the app was never torn down".
    // Also assert this is the page's ONLY frame, so `enter_frame(0)` below
    // addresses the app rather than whatever else happened to mount first.
    let mark = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        let fr = null;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Apps') continue;
            fr = sec.querySelector('iframe[sandbox]');
        }
        if (!fr) return { mounted: false, frames: window.frames.length };
        fr.__aliveProbe = 'war-still-running';
        return {
            mounted: true,
            frames: window.frames.length,
            has_cw: !!fr.contentWindow,
            doc_frames: document.querySelectorAll('iframe').length,
        };
    "#;
    let marked = poll_json(&client, mark, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("mounted").and_then(|b| b.as_bool()).unwrap_or(false)
            && v.get("has_cw").and_then(|b| b.as_bool()).unwrap_or(false)
    })
    .await?;
    assert!(
        marked.get("has_cw").and_then(|b| b.as_bool()).unwrap_or(false),
        "War never mounted its sandboxed iframe. Got: {marked:?}"
    );
    // NOTE `window.frames.length` reads 0 here and `document.querySelectorAll`
    // finds nothing: the player's iframe lives inside the window section's
    // **shadow root**, which injected script cannot enumerate from the top
    // document. WebDriver's own frame enumeration is separate — that is why the
    // frame is entered by index below rather than located by script — and this
    // window is the only one this test opens.
    println!("  frame mounted (window.frames sees {} from script)",
        marked.get("frames").unwrap_or(&serde_json::Value::Null));

    // Play one move INSIDE the sandbox. The frame is an opaque origin, so the
    // parent cannot reach its document — WebDriver can, and this is the only
    // way to make the real app emit a real `state` message.
    client.enter_frame(0).await?;
    let flipped = client
        .execute(
            r#"
            const b = Array.from(document.querySelectorAll('button'))
                .find(b => b.textContent.trim() === 'Flip');
            if (!b) return 'no-flip-button';
            b.click();
            return 'flipped';
            "#,
            vec![],
        )
        .await?;
    client.enter_parent_frame().await?;
    assert_eq!(
        flipped.as_str().unwrap_or(""),
        "flipped",
        "couldn't press Flip inside the War frame: {flipped:?}"
    );

    // (1) The save has to LAND, or nothing below is testing anything. The write
    // is a trailing-edge debounce behind the move, so this is the one place a
    // real wait is unavoidable; the counter is page-level and survives the very
    // teardown this gate is about, so it stays readable either way.
    let read_state = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        let fr = null;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Apps') continue;
            fr = sec.querySelector('iframe[sandbox]');
        }
        return {
            saves: window.__entity_app_save_seq || 0,
            alive: !!(fr && fr.__aliveProbe === 'war-still-running'),
            mounted: !!fr,
            state_seq: fr ? (fr.getAttribute('data-app-state-seq') || '0') : null,
        };
    "#;
    let saved = poll_json(&client, read_state, Duration::from_secs(10), |v| {
        v.get("saves").and_then(|n| n.as_u64()).unwrap_or(0) >= 1
    })
    .await?;
    let saves = saved.get("saves").and_then(|n| n.as_u64()).unwrap_or(0);
    assert!(
        saves >= 1,
        "no save was written in 10s after a move, so this gate would prove \
         nothing about what a save does to the running app — the app is \
         probably not emitting state (did the Flip click land?). Got: {saved:?}"
    );

    // (2) ...and the app must still be the same running app. Poll for the
    // FAILURE so the red path reports as soon as the teardown lands and the
    // green path spends the whole budget rather than reading too early.
    let after = poll_json(&client, read_state, Duration::from_secs(3), |v| {
        !v.get("alive").and_then(|b| b.as_bool()).unwrap_or(false)
    })
    .await?;
    assert!(
        after.get("alive").and_then(|b| b.as_bool()).unwrap_or(false),
        "the running app was torn down by its own save: the Apps window \
         watches the save prefix, so the debounced write flipped its dirty \
         flag, the section rebuilt, and `render_player` replaced the iframe — \
         which restarts the app at its start screen. Got: {after:?}"
    );

    println!(
        "  the app survived {saves} save write(s) without being remounted \
         (state messages: {})",
        after.get("state_seq").unwrap_or(&serde_json::Value::Null)
    );
    client.close().await.ok();
    Ok(())
}

/// A **trusted** left click at viewport coordinates.
///
/// Necessary rather than stylistic: our windows render inside a shadow root, so
/// a CSS locator cannot reach their buttons, and the suite's usual answer —
/// `execute` + `el.click()` — produces an event with **no user activation**.
/// That is fine for every other control and fatal for the Fullscreen API, which
/// Firefox gates on `full-screen-api.allow-trusted-requests-only` (default
/// true). Measured against this grid: a script click is refused with
/// `TypeError: Fullscreen request denied`; the pointer sequence below is
/// granted. So the element is located by script (for its rect) and pressed by
/// the driver.
async fn trusted_click_at(
    client: &Client,
    x: f64,
    y: f64,
) -> Result<(), Box<dyn std::error::Error>> {
    use fantoccini::actions::{InputSource, MouseActions, PointerAction, MOUSE_BUTTON_LEFT};
    let seq = MouseActions::new("mouse".to_string())
        .then(PointerAction::MoveTo { duration: None, x, y })
        .then(PointerAction::Down { button: MOUSE_BUTTON_LEFT })
        .then(PointerAction::Pause { duration: Duration::from_millis(30) })
        .then(PointerAction::Up { button: MOUSE_BUTTON_LEFT });
    client.perform_actions(seq).await?;
    Ok(())
}

/// JS that reads everything the app player's size controls are made of: the two
/// buttons' labels and whether each is *offered* (rendered AND not hidden by a
/// rule), the stage's full-bleed class, whether the player is the element the
/// engine is showing full-screen, its width against the viewport's, and the
/// survival expando on the running app's iframe.
///
/// One probe for every assertion in the gate below, deliberately: these facts
/// have to be read at the same instant or a mid-transition sample can show a
/// relabelled button beside an un-expanded stage and read as a bug.
const READ_PLAYER_CHROME: &str = r#"
    const layer = document.getElementById('dom-layer');
    const root = layer.shadowRoot || layer;
    for (const sec of root.querySelectorAll('section.window')) {
        const h3 = sec.querySelector('header h3');
        if (!h3 || h3.textContent.trim() !== 'Apps') continue;
        const player = sec.querySelector('.gm-player');
        if (!player) return { player: false };
        const area = sec.querySelector('.gm-stage-area');
        const expand = sec.querySelector('.gm-expand-btn');
        const full = sec.querySelector('.gm-full-btn');
        const frame = sec.querySelector('iframe[sandbox]');
        const shown = el => !!el && getComputedStyle(el).display !== 'none';
        return {
            player: true,
            fullscreen: player.matches(':fullscreen'),
            player_w: Math.round(player.getBoundingClientRect().width),
            inner_w: window.innerWidth,
            expanded: !!area && area.className.includes('gm-expanded'),
            expand_offered: shown(expand),
            expand_label: expand ? expand.textContent.trim() : null,
            full_present: !!full,
            full_offered: shown(full),
            full_label: full ? full.textContent.trim() : null,
            alive: !!(frame && frame.__aliveProbe === 'war-still-running'),
        };
    }
    return { player: false };
"#;

/// FULL SCREEN — the app leaves the page and fills the physical screen, and the
/// running app is NOT remounted on the way in or out.
///
/// The Apps window already had two ways to get bigger and they read as one:
/// `▢` on the window header (the *window* covers the viewport) and `⤢ Expand`
/// in the player bar (the *stage* fills the window). Both together still leave
/// two bars of chrome above the app plus the browser's own, which on a laptop
/// is most of the vertical space a game is missing. `⛶ Full screen` is the
/// third, and this gate is what says it works.
///
/// Four properties, and the last is the one a plausible implementation breaks:
///
///  1. the button is **offered** — i.e. `util::fullscreen_supported()` answered
///     true in a real engine. Only a browser can tell us that;
///  2. a trusted press actually **enters** full screen (the player matches
///     `:fullscreen` and widens to the viewport);
///  3. the chrome converges — the stage goes full-bleed, Expand is withdrawn
///     (it could only be "on" there), the button names the way out;
///  4. **the app keeps running.** The transition is a class flip plus a
///     Fullscreen call, with no action, no dirty-mark and no rebuild — the
///     obvious implementation raises an `Action`, the section rebuilds, and
///     `render_player` replaces the iframe, restarting the game at its start
///     screen. The expando is the only probe that survives the check it is for.
///
/// The exit is driven by `document.exitFullscreen()` from **script**, not by
/// pressing the button again. That is deliberate: it is an exit our click
/// handler never sees — the shape of a user pressing Esc — so it proves the
/// chrome is reconciled from `fullscreenchange` rather than from our own press.
#[tokio::test]
async fn full_screen_fills_the_screen_and_the_app_keeps_running(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());
    client.goto(&url).await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;

    // Apps, and nothing else: the press below is by viewport coordinate, so a
    // second window stacked above would move the target under the pointer.
    assert_eq!(
        click_spawn_btn(&client, "+ Apps").await?,
        "clicked",
        "couldn't open the Apps window"
    );

    let read_apps = r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Apps') continue;
            return { open: true, war: !!Array.from(sec.querySelectorAll('button'))
                .find(b => !b.hasAttribute('data-chip') && b.textContent.includes('War')) };
        }
        return { open: false, war: false };
    "#;
    let grid = poll_json(&client, read_apps, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("war").and_then(|b| b.as_bool()).unwrap_or(false)
    })
    .await?;
    assert!(
        grid.get("war").and_then(|b| b.as_bool()).unwrap_or(false),
        "the Apps launcher never listed the War demo, so there is no running \
         app to take full screen. Got: {grid:?}"
    );
    let launched = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => !b.hasAttribute('data-chip') && b.textContent.includes('War'));
                if (!card) return 'no-war-card';
                card.click();
                return 'clicked';
            }
            return 'no-apps-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(launched.as_str().unwrap_or(""), "clicked", "War launch failed: {launched:?}");

    // Stamp the mounted iframe. A section rebuild creates a fresh element, so
    // the expando's survival IS "the app was never torn down" — an attribute
    // would be destroyed by exactly the failure it is meant to detect.
    let marked = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Apps') continue;
            const fr = sec.querySelector('iframe[sandbox]');
            if (!fr) return { mounted: false };
            fr.__aliveProbe = 'war-still-running';
            return { mounted: true, has_cw: !!fr.contentWindow };
        }
        return { mounted: false };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("has_cw").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert!(
        marked.get("has_cw").and_then(|b| b.as_bool()).unwrap_or(false),
        "War never mounted its sandboxed iframe. Got: {marked:?}"
    );

    // (1) Both controls are offered, and they say which is which.
    let before = poll_json(&client, READ_PLAYER_CHROME, ASYNC_ROUND_TRIP_BUDGET, |v| {
        v.get("player").and_then(|b| b.as_bool()).unwrap_or(false)
    })
    .await?;
    assert!(
        before.get("full_present").and_then(|b| b.as_bool()).unwrap_or(false),
        "the player bar carries no full-screen button. `fullscreen_supported()` \
         reads `document.fullscreenEnabled`, so this means the engine reports \
         the Fullscreen API unavailable — which is a real answer on some \
         WebViews and would be news on Firefox. Got: {before:?}"
    );
    assert_eq!(
        before.get("full_label").and_then(|v| v.as_str()),
        Some("\u{26f6} Full screen"),
        "the full-screen button must name the SCREEN, not the window — it sits \
         beside Expand, which names the window, and the glyphs alone do not \
         carry the difference. Got: {before:?}"
    );
    assert_eq!(
        before.get("expand_label").and_then(|v| v.as_str()),
        Some("\u{2922} Expand"),
        "Expand should be offered in its collapsed state on a fresh mount. \
         Got: {before:?}"
    );
    assert_eq!(
        before.get("expanded").and_then(|b| b.as_bool()),
        Some(false),
        "a freshly mounted stage is capped and centered, not full-bleed. \
         Got: {before:?}"
    );

    // (2) Press it — with the driver, not with script. See `trusted_click_at`.
    let rect = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const b = sec.querySelector('.gm-full-btn');
                if (!b) return null;
                b.scrollIntoView({block: 'center'});
                const r = b.getBoundingClientRect();
                return { x: r.x + r.width / 2, y: r.y + r.height / 2,
                         w: r.width, h: r.height };
            }
            return null;
            "#,
            vec![],
        )
        .await?;
    let (bx, by) = (
        rect.get("x").and_then(|v| v.as_f64()).unwrap_or(-1.0),
        rect.get("y").and_then(|v| v.as_f64()).unwrap_or(-1.0),
    );
    assert!(
        bx > 0.0 && by > 0.0 && rect.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0,
        "the full-screen button has no clickable box — a zero-size or \
         off-viewport target would make the press below land on whatever is \
         underneath and the gate fail for the wrong reason. Got: {rect:?}"
    );
    trusted_click_at(&client, bx, by).await?;

    // Poll for the CHROME to converge, not for `:fullscreen` alone. Measured on
    // this grid: the engine flips `:fullscreen` and dispatches
    // `fullscreenchange` as two steps, so there is a real instant where the
    // player already matches and our listener has not run yet. A predicate on
    // the engine's half alone samples that instant and reports a state the app
    // was never in — which is a race in the GATE, and it reads exactly like the
    // product bug the gate is for.
    let entered = poll_json(&client, READ_PLAYER_CHROME, ASYNC_ROUND_TRIP_BUDGET, |v| {
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        b("fullscreen") && b("expanded")
    })
    .await?;
    assert_eq!(
        entered.get("fullscreen").and_then(|b| b.as_bool()),
        Some(true),
        "a trusted press did not take the player full screen. Got: {entered:?}"
    );
    let (pw, iw) = (
        entered.get("player_w").and_then(|v| v.as_i64()).unwrap_or(0),
        entered.get("inner_w").and_then(|v| v.as_i64()).unwrap_or(-1),
    );
    assert_eq!(
        pw, iw,
        "the player matches `:fullscreen` but is not filling the viewport \
         ({pw}px of {iw}px) — something in the player's own box model is \
         fighting the promotion, which is what the user sees as a letterboxed \
         app on a full screen. Got: {entered:?}"
    );

    // (3) ...and the chrome converged on it.
    assert_eq!(
        entered.get("expanded").and_then(|b| b.as_bool()),
        Some(true),
        "full screen must force the stage full-bleed: a 680px-capped stage \
         centered on a whole screen reads as a failed transition. \
         Got: {entered:?}"
    );
    assert_eq!(
        entered.get("expand_offered").and_then(|b| b.as_bool()),
        Some(false),
        "Expand must be withdrawn while full screen — it can only be `on` \
         there, so rendering it is chrome pretending to be a control. \
         Got: {entered:?}"
    );
    assert_eq!(
        entered.get("full_label").and_then(|v| v.as_str()),
        Some("\u{26f6} Exit full screen"),
        "the button must name the way out once we are in. Got: {entered:?}"
    );

    // (4) The app was never remounted.
    assert_eq!(
        entered.get("alive").and_then(|b| b.as_bool()),
        Some(true),
        "going full screen tore the running app down and remounted it — the \
         transition raised an action / marked the window dirty, the section \
         rebuilt, and `render_player` replaced the iframe, restarting the game \
         at its start screen. It must be a class flip plus a Fullscreen call, \
         nothing more. Got: {entered:?}"
    );
    println!("  entered full screen: player {pw}px = viewport {iw}px, app still running");

    // (5) Leave by a route the button never sees.
    client.execute("document.exitFullscreen(); return 1;", vec![]).await?;
    // Same two-step as the entry, in reverse — wait for our own reconcile.
    let left = poll_json(&client, READ_PLAYER_CHROME, ASYNC_ROUND_TRIP_BUDGET, |v| {
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(true);
        !b("fullscreen") && !b("expanded")
    })
    .await?;
    assert_eq!(
        left.get("fullscreen").and_then(|b| b.as_bool()),
        Some(false),
        "still full screen after `exitFullscreen()`. Got: {left:?}"
    );
    assert_eq!(
        left.get("expanded").and_then(|b| b.as_bool()),
        Some(false),
        "leaving full screen must return the stage to the size it had in the \
         window — the trip is not a decision about how big the app should be \
         inside its window. Got: {left:?}"
    );
    assert_eq!(
        left.get("expand_offered").and_then(|b| b.as_bool()),
        Some(true),
        "Expand must come back when the player returns to the window. \
         Got: {left:?}"
    );
    assert_eq!(
        left.get("full_label").and_then(|v| v.as_str()),
        Some("\u{26f6} Full screen"),
        "the button must offer the way IN again after an exit it did not \
         initiate — if this still reads `Exit full screen`, the chrome is \
         being reconciled from our own click rather than from \
         `fullscreenchange`, and an Esc leaves the bar lying. Got: {left:?}"
    );
    assert_eq!(
        left.get("alive").and_then(|b| b.as_bool()),
        Some(true),
        "leaving full screen remounted the app. Got: {left:?}"
    );
    println!("  left full screen by a route the button never saw; chrome and app intact");

    client.close().await.ok();
    Ok(())
}

/// WAKE LOCK — a running app can keep the screen awake, through our sandbox.
///
/// An idle-watchable app (Warlord with the AI driving, a long turn, anything
/// you watch rather than touch) has to hold a screen wake lock or the display
/// blanks mid-game. It cannot grant itself one: `screen-wake-lock` is denied in
/// a frame by default and **only the embedder can delegate it**, via Permissions
/// Policy — a different mechanism from `sandbox`, and one an app has no way to
/// ask for.
///
/// **The assertion order here is the point.** The behavioural check — does a
/// request inside the real frame get GRANTED — comes first; the attribute
/// spelling comes last. This suite has now twice been bitten by a cheap check
/// shadowing an expensive one for the same property (Phase 19-doc, twice), and
/// this is the exact shape: the tempting gate is `getAttribute('allow')`, which
/// passes for `allow="screen-wake-lock"` — a spelling measured to be **inert**
/// for an opaque-origin `srcdoc` frame, because its default `'src'` allowlist
/// names an origin the frame does not have. A spelling-only gate would be green
/// over a completely broken feature.
///
/// It runs INSIDE the frame (`enter_frame`) because that is the only vantage
/// the permission applies to: the host page holds the feature regardless, so
/// asking from out here proves nothing about what we delegated.
#[tokio::test]
async fn a_running_app_can_hold_a_screen_wake_lock() -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    // `localhost` matters: `navigator.wakeLock` is undefined off a secure
    // origin, measured at every sandbox tier, so on a plain-http LAN address
    // this feature is absent no matter what we delegate.
    let url = format!("http://localhost:{}/?worker=1&log=trace", http_server_port());
    client.goto(&url).await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;
    let secure = client
        .execute("return {secure: isSecureContext, api: typeof navigator.wakeLock};", vec![])
        .await?;
    assert_eq!(
        secure.get("secure").and_then(|b| b.as_bool()),
        Some(true),
        "this gate needs a secure origin — the API does not exist otherwise, and \
         a red here would be about the harness. Got: {secure:?}"
    );
    if secure.get("api").and_then(|v| v.as_str()) == Some("undefined") {
        // Not a failure: an engine without Screen Wake Lock is a real answer
        // (WebKit lagged for years). Say so loudly rather than asserting a
        // permanent red that masks regressions.
        println!(
            "  SKIPPED: this engine exposes no navigator.wakeLock at all, so \
             there is no delegation to observe. Got: {secure:?}"
        );
        client.close().await.ok();
        return Ok(());
    }

    // Apps, and nothing else — this test enters the app's frame by index.
    assert_eq!(
        click_spawn_btn(&client, "+ Apps").await?,
        "clicked",
        "couldn't open the Apps window"
    );
    let grid = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Apps') continue;
            return { war: !!Array.from(sec.querySelectorAll('button'))
                .find(b => !b.hasAttribute('data-chip') && b.textContent.includes('War')) };
        }
        return { war: false };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("war").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert!(
        grid.get("war").and_then(|b| b.as_bool()).unwrap_or(false),
        "the Apps launcher never listed the War demo. Got: {grid:?}"
    );
    let launched = client
        .execute(
            r#"
            const layer = document.getElementById('dom-layer');
            const root = layer.shadowRoot || layer;
            for (const sec of root.querySelectorAll('section.window')) {
                const h3 = sec.querySelector('header h3');
                if (!h3 || h3.textContent.trim() !== 'Apps') continue;
                const card = Array.from(sec.querySelectorAll('button'))
                    .find(b => !b.hasAttribute('data-chip') && b.textContent.includes('War'));
                if (!card) return 'no-war-card';
                card.click();
                return 'clicked';
            }
            return 'no-apps-window';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(launched.as_str().unwrap_or(""), "clicked", "War launch failed: {launched:?}");

    // Wait for the mount, and confirm this is the page's only sandboxed frame
    // so `enter_frame(0)` addresses the app rather than whatever mounted first.
    let mounted = poll_json(
        &client,
        r#"
        const layer = document.getElementById('dom-layer');
        const root = layer.shadowRoot || layer;
        for (const sec of root.querySelectorAll('section.window')) {
            const h3 = sec.querySelector('header h3');
            if (!h3 || h3.textContent.trim() !== 'Apps') continue;
            const frames = sec.querySelectorAll('iframe[sandbox]');
            const fr = frames[0];
            return { mounted: !!fr, count: frames.length, has_cw: !!(fr && fr.contentWindow),
                     allow: fr ? fr.getAttribute('allow') : null,
                     sandbox: fr ? fr.getAttribute('sandbox') : null };
        }
        return { mounted: false };
        "#,
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("has_cw").and_then(|b| b.as_bool()).unwrap_or(false),
    )
    .await?;
    assert_eq!(
        mounted.get("count").and_then(|n| n.as_u64()),
        Some(1),
        "expected exactly one sandboxed frame so the frame index below is not a \
         guess. Got: {mounted:?}"
    );

    // (1) THE BEHAVIOUR. Ask for the lock from inside the frame — the only
    // vantage the delegation applies to — and require it to be GRANTED.
    client.enter_frame(0).await?;
    let started = client
        .execute(
            r#"
            window.__wl = { state: 'pending', api: typeof navigator.wakeLock };
            if (!navigator.wakeLock) { window.__wl.state = 'no-api'; return 'no-api'; }
            navigator.wakeLock.request('screen').then(
                s => { window.__wl = { state: 'granted', type: s.type }; },
                e => { window.__wl = { state: 'denied', err: e.name + ': ' + e.message }; });
            return 'asked';
            "#,
            vec![],
        )
        .await?;
    assert_eq!(
        started.as_str().unwrap_or(""),
        "asked",
        "could not even ask for a wake lock inside the app frame: {started:?}"
    );
    let verdict = poll_json(
        &client,
        "return window.__wl;",
        ASYNC_ROUND_TRIP_BUDGET,
        |v| v.get("state").and_then(|s| s.as_str()).unwrap_or("pending") != "pending",
    )
    .await?;
    client.enter_parent_frame().await?;
    assert_eq!(
        verdict.get("state").and_then(|s| s.as_str()),
        Some("granted"),
        "a running app was REFUSED a screen wake lock, so a game you watch \
         without touching will blank the display mid-play — silently, with the \
         same bundle working fine standalone.\n\
         `screen-wake-lock` is Permissions Policy, not sandbox: only the \
         embedder can delegate it, and the delegation must be spelled \
         `allow=\"screen-wake-lock *\"`. The bare `allow=\"screen-wake-lock\"` \
         defaults its allowlist to `'src'`, which an opaque-origin srcdoc frame \
         does not have — measured, it is refused exactly like no attribute at \
         all. Got: {verdict:?} (frame allow={:?})",
        mounted.get("allow")
    );

    // (2) ...and only now the spelling, which is the cheap check and therefore
    // goes last. It is kept because it names the ONE token that must not be
    // dropped, and it fails with the reason rather than with a denied promise.
    assert_eq!(
        mounted.get("allow").and_then(|v| v.as_str()),
        Some("screen-wake-lock *"),
        "the app frame's Permissions Policy delegation changed. Got: {mounted:?}"
    );

    println!(
        "  the app frame holds a '{}' wake lock through sandbox={:?}",
        verdict.get("type").and_then(|v| v.as_str()).unwrap_or("?"),
        mounted.get("sandbox").and_then(|v| v.as_str()).unwrap_or("?")
    );
    client.close().await.ok();
    Ok(())
}

// ── G1 — the black-hole origin ────────────────────────────────────────────
//
// The two gates below are the falsifiers for the code-axis design's central
// claim, and they are written to be seen RED on the unfixed tree before their
// fix lands (§5.1). Read
// `docs/plans/DESIGN-CODE-AXIS-RECOVERY-AND-BOOT-SLOTS.md` §1.1, §4A and §4B
// before changing either.
//
// **The distinction they turn on is not "offline".** A network that REJECTS —
// interface down, DNS failure, connection refused — reaches every `.catch` on
// the path promptly, and offline boot works. That case has always worked and is
// why the symptom is intermittent. A network that ACCEPTS AND NEVER ANSWERS
// reaches no catch at all: an unbounded `await` on it is a blank page for as
// long as the OS is willing to wait, which behind a captive portal is
// unbounded. `tools/e2e/blackhole-serve.py` is the only thing in this rig that
// can produce the second case.
//
// They cover brick-matrix cells #1 and #2 — two of the six live E6 cells — and
// they are the enforcement point that makes D23 a discipline rather than an
// assertion.

/// Connect a browser session, without starting the dist server.
///
/// Split out of [`setup`] so a test can supply its own origin. Everything else
/// [`setup`] does is preserved and matters: the stall watchdog has to be armed
/// for a standalone test (it has no phases to report progress from), the stale
/// session reaper has to hand back the single Selenium slot, and the two
/// server-side timeouts have to be bounded rather than left at WebDriver's 300 s
/// `pageLoad` default — which for a black-hole test would turn the exact failure
/// under test into a five-minute silence.
async fn connect_browser() -> Result<Client, Box<dyn std::error::Error>> {
    arm_stall_watchdog();
    note_progress("setup");
    reap_stale_sessions();

    let mut caps = serde_json::Map::new();
    caps.insert(
        "moz:firefoxOptions".to_string(),
        serde_json::json!({ "args": ["-headless"] }),
    );
    let url = webdriver_url();
    let client = ClientBuilder::native()
        .capabilities(caps)
        .connect(&url)
        .await
        .map_err(|e| format!("failed to connect to WebDriver at {url}: {e}"))?;
    client
        .update_timeouts(fantoccini::wd::TimeoutConfiguration::new(
            Some(Duration::from_secs(30)),
            Some(Duration::from_secs(45)),
            Some(Duration::from_secs(0)),
        ))
        .await
        .map_err(|e| format!("failed to set WebDriver timeouts: {e}"))?;
    Ok(client)
}

/// **G1 — cold boot against an origin that black-holes `/entity-deployment.json`.**
///
/// The claim under test, from the design's §4A: between page load and the frame
/// loop there are exactly two network awaits and both are
/// `deployment_config::fetch()`, which had no deadline. `boot_load` is awaited
/// before the rAF loop starts *and* before the frozen-frame watchdog installs
/// (`main.rs`), so a stalling origin is a blank page with no watchdog, no
/// message, and no exit — brick-matrix cell **#1**, at **E6**.
///
/// **This is a cold boot on purpose.** No staged warm profile, no re-key
/// fixture, no persisted state: the cold path (`durable.is_none()`) has carried
/// the unbounded fetch since long before the warm-boot reconcile widened it, so
/// this gate fails on shipped code rather than only on unmerged work. A gate
/// that can only fail on a branch is a regression test for a bug nobody has run.
///
/// **Anti-vacuity.** A boot that never asked for the config satisfies every
/// assertion below for the wrong reason, so the server's own log is checked:
/// the request must have arrived and must have been stalled. Measured at the
/// wire, not reported by the thing under test.
#[tokio::test]
async fn boot_survives_a_blackholed_deployment_config() -> Result<(), Box<dyn std::error::Error>> {
    let server = start_blackhole_server(&["/entity-deployment.json"])?;
    let client = connect_browser().await?;
    let port = blackhole_server_port();

    // `localhost` rather than 127.0.0.1 so this runs on the same secure-context
    // terms as production: the service worker registers, exactly as it does for
    // a real visitor. C1's deadline lives in the page, so it bounds the fetch
    // whether the SW is in the path or not — and a gate that quietly excluded
    // the SW would be testing a configuration no user is in.
    let url = format!("http://localhost:{port}/?log=trace");
    let goto = client.goto(&url).await;

    let boot = wait_for_boot(&client, BOOT_BUDGET_MS).await;

    // Read the wire evidence BEFORE asserting, so the failure message can say
    // which of the two possible reds this is.
    let stalled = server.request_count("STALL");
    let asked = server.request_count("entity-deployment.json");
    client.close().await.ok();

    if let Err(e) = goto {
        return Err(format!(
            "the navigation itself never completed against a black-holing origin: {e}\n\
             (the config fetch is a runtime fetch, not a document subresource, so this \
             is a different failure from the one this gate is about — check whether \
             something on the document's critical path is now fetching a stalled URL)"
        )
        .into());
    }

    assert!(
        asked >= 1,
        "VACUOUS: the app never requested /entity-deployment.json, so nothing was \
         black-holed and a green here would mean nothing. The server logged {} \
         stalled request(s) in total. Either the boot path stopped fetching the \
         deployment config, or the origin is not the one the browser loaded.",
        stalled
    );

    let boot_ms = boot.as_ref().copied().unwrap_or(0);
    boot.map_err(|e| {
        format!(
            "G1 RED — boot never reached the frame loop against an origin that accepts \
             and never answers /entity-deployment.json ({asked} such request(s) stalled).\n\
             This is brick-matrix cell #1 at E6: no frame loop, so no frozen-frame \
             watchdog, no banner, no message, and no exit for the user.\n\
             The fix is a deadline on `deployment_config::fetch()` (C1): a timeout must \
             be a state the boot PROCEEDS FROM (`None` → build-time defaults), never a \
             stall.\n\nUnderlying failure: {e}"
        )
    })?;

    // Print the margin on success, per this suite's standing rule: a budget that
    // only ever speaks when it fails cannot tell a loaded box from a broken one.
    // Here it says something sharper — boot should land at roughly
    // `BOOT_FETCH_DEADLINE_MS` plus a healthy boot (108-711 ms measured). A time
    // far BELOW the deadline would mean the stall was never actually hit and the
    // pass is vacuous; far above would mean something else on the path is also
    // waiting, and the §4A enumeration is incomplete.
    println!(
        "  G1: booted in {boot_ms}ms with {asked} black-holed \
         /entity-deployment.json request(s) ({stalled} stalled at the wire). \
         Expect ~3s (the deadline) + a healthy boot."
    );
    Ok(())
}

/// **G1 (recovery variant) — the BIOS must survive a black-holing origin.**
///
/// The System Recovery console gained network probes on 2026-08-27 (the version
/// identity panel: what build is running, what is cached, what the origin
/// serves, what the domain declares). **That is a self-inflicted risk of exactly
/// the kind this whole gate family exists for**, and it is the worst placement
/// of it available: this is the screen a user opens *because* something is
/// already hanging. A network read here that never returns replaces a broken app
/// with a broken diagnostic, and there is no third tier to fall back to.
///
/// So the probes are bounded (D23) and their expiry is a *reported state* —
/// "could not reach the origin" — rather than a spinner. This asserts that the
/// panel reaches a definite answer, and that the probes did not take the
/// storage inventory down with them: the parts of the report that need no
/// network must still be there, because on a black-holing origin they are the
/// only parts that can be.
#[tokio::test]
async fn the_recovery_console_survives_a_blackholed_origin(
) -> Result<(), Box<dyn std::error::Error>> {
    // Start CLEAN and black-hole at runtime. Seeding `/index.html` at startup
    // deadlocks the server's own readiness probe, which fetches exactly that
    // path — the harness says so rather than letting it read as an app fault,
    // and this is the same reason the stall set is runtime-settable for the
    // service-worker variant.
    let server = start_blackhole_server(&[])?;
    let client = connect_browser().await?;
    let port = blackhole_server_port();
    // `/` is deliberately NOT stalled — the navigation has to land, or we are
    // testing whether a page can load rather than whether the console degrades.
    set_blackhole(&["/index.html", "/entity-deployment.json"])?;

    let url = format!("http://localhost:{port}/?systemrecovery=1");
    client.goto(&url).await?;

    // Poll for a *settled* panel rather than sleeping: the deadline is 3 s, so a
    // healthy run lands just after it, and a fixed sleep would either be flaky
    // or hide a regression by being generous.
    let read = r#"const v = document.getElementById('version');
                  const d = document.getElementById('domain');
                  return (v ? v.textContent : '') + ' ' + (d ? d.textContent : '');"#;
    let started = std::time::Instant::now();
    let mut panel = String::new();
    for _ in 0..75 {
        sleep(Duration::from_millis(200)).await;
        panel = client
            .execute(read, vec![])
            .await?
            .as_str()
            .unwrap_or("")
            .to_string();
        if !panel.is_empty() && !panel.contains("probing…") {
            break;
        }
    }
    let settled_ms = started.elapsed().as_millis();

    let stalled = server.request_count("STALL");
    let asked = server.request_count("index.html");
    let console = client
        .execute(
            r#"const r = document.getElementById('entity-recovery'); return r ? r.textContent : '';"#,
            vec![],
        )
        .await?
        .as_str()
        .unwrap_or("")
        .to_string();
    client.close().await.ok();

    assert!(
        asked >= 1,
        "VACUOUS: the recovery console never requested /index.html, so nothing was \
         black-holed and a green here would mean nothing ({stalled} stalled overall)"
    );
    assert!(
        !panel.contains("probing…"),
        "G1/BIOS RED — the System Recovery version panel never settled against an origin \
         that accepts and never answers ({asked} such request(s) stalled, {settled_ms}ms \
         elapsed).\n\
         This is the recovery console itself hanging: the screen a user reaches BECAUSE \
         the app already hung, now hanging for the same reason one tier down.\n\
         The probes must be bounded and expiry must be a reported state, not a spinner.\n\
         Panel text was: {panel:?}"
    );
    assert!(
        panel.contains("Could not reach the origin"),
        "the panel settled but did not SAY the origin was unreachable — a diagnostic that \
         goes quiet is worse than one that reports a negative. Got: {panel:?}"
    );
    // The network probes must not have taken the local inventory with them. On a
    // black-holing origin these sections are the only ones that can work, so
    // they are the ones that matter most here.
    for needle in ["Storage Inventory", "IndexedDB", "localStorage"] {
        assert!(
            console.contains(needle),
            "the {needle:?} section is missing — a stalled network probe blocked the \
             local inventory, which needs no network at all"
        );
    }

    // Same margin rule as G1: a settle far below the deadline would mean the
    // stall was never reached and the green is empty.
    println!(
        "  G1/BIOS: recovery panel settled in {settled_ms}ms with {asked} black-holed \
         request(s) ({stalled} stalled at the wire). Expect ~3s (the probe deadline)."
    );
    Ok(())
}

/// **A first contact that missed the deployment config must not be permanent.**
///
/// Written as a release-risk probe, from a code read, before deploying anything:
///
/// * the cold path fetches `/entity-deployment.json` only when there is **no**
///   durable session config (`durable.is_none()`);
/// * if that fetch yields `None` — a 404, an unparseable document, or (since C1)
///   a timeout at 3 s — `cfg` falls back to `boot_default()`, whose `home_site`
///   is `SiteRef::default()`, i.e. an **empty** `peer_id`;
/// * that config is then **persisted**;
/// * and on every later boot the R1 reconcile guards on
///   `home_is_local = stale_peer.is_empty() || stale_peer == system_pid`, so an
///   empty home means it never re-reads the document.
///
/// One unlucky first visit therefore pins a profile to the build-time default
/// surface **forever**, and the only exit is clearing site data — an **E5**,
/// which is the rung the whole brick matrix exists to keep empty. Confirmed by
/// this test on 2026-08-27: the site never renders.
///
/// **The guard is not the bug — read this before "fixing" it.** An empty
/// `home_site.peer_id` is a legitimate sentinel meaning *the system peer*:
/// `set_home_site` documents "empty `target_peer` = the system peer", and
/// `repair_for_deleted_peer` writes exactly that when a home's peer is deleted.
/// So treating empty as local is correct for a user who chose a local home.
/// The defect is one level up — **a failed fetch persists a value that is
/// indistinguishable from a deliberate choice.** `deployment_config::fetch()`
/// collapses three different outcomes into `None`: the document said nothing
/// (a 404 — this origin genuinely has no config), the document was unreadable,
/// and (since C1) the origin did not answer in time. Only the first is a fact
/// worth persisting; the other two are "we do not know yet" and must be
/// retried. Flipping the guard would make a local-home user's setting get
/// overwritten from the domain document on every boot — trading this defect for
/// a worse one.
///
/// Staged with a 404 rather than a stall because the sticky state is reached by
/// *any* `None`, and a missing-then-added document is the ordinary version of it
/// — a domain that publishes its deployment config after someone's first visit.
/// C1's deadline did not create this path, it **widened** it: before, only a
/// failing origin reached `None`; now a merely slow one does too.
#[tokio::test(flavor = "current_thread")]
async fn a_first_contact_that_missed_the_deployment_config_recovers(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;
    let read_site =
        r#"const sl=document.getElementById('site-layer');return sl?(sl.textContent||'').trim():'';"#;

    let root = "target/e2e-late-deployment-config".to_string();
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let cp = Command::new("cp").args(["-a", "dist/.", &root]).status()?;
    if !cp.success() {
        return Err(format!("could not stage an isolated SPA copy at {root}: {cp}").into());
    }
    // The whole point: at first contact this origin serves NO deployment config.
    let _ = std::fs::remove_file(format!("{root}/entity-deployment.json"));

    let port = pick_free_port()?;
    let server = Command::new("python3")
        .args(["tools/cors-serve.py", &root, &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let _serving = FederationServer(server);
    sleep(Duration::from_millis(400)).await;
    let url = format!("http://localhost:{port}/?log=trace");

    let r = async {
        // ── 1. First contact, with nothing served ────────────────────────────
        client.goto(&url).await?;
        wipe_all_storage(&client).await?;
        client.goto(&url).await?;
        wait_for_boot(&client, 30_000).await?;
        let first = capture_log(&client).await?;
        assert!(
            !first.iter().any(|l| l.contains("deployment-config: applied")),
            "staging: the origin was supposed to serve no deployment config on first contact"
        );

        // ── 2. The domain publishes its deployment config ────────────────────
        run_rekey_fixture("emit_rekey_fixture_before", &root);
        let peer_a = deployment_home_peer(&root);
        assert!(
            std::path::Path::new(&format!("{root}/entity-deployment.json")).exists(),
            "staging: the fixture did not emit a deployment config"
        );

        // ── 3. The visitor comes back ────────────────────────────────────────
        client.goto(&url).await?;
        wait_for_boot(&client, 30_000).await?;
        let second = poll_rendered(&client, read_site, "Welcome to the Entity Demo Site").await?;
        let log = capture_log(&client).await?;
        if !second.contains("Welcome to the Entity Demo Site") {
            print_log(&log);
        }

        assert!(
            second.contains("Welcome to the Entity Demo Site"),
            "RED — a profile whose FIRST contact missed /entity-deployment.json never reads \
             it again. The visitor is pinned to the build-time default surface with no exit \
             but clearing site data: an E5, and the rung the brick matrix exists to keep \
             empty.\n\
             The domain now publishes as {peer_a} and this profile still cannot see it.\n\
             DO NOT fix this by flipping R1's `home_is_local` guard — an empty home_site \
             legitimately means \"the system peer\" (`set_home_site`, \
             `repair_for_deleted_peer`), so that would overwrite a local-home user's \
             setting from the domain document on every boot. The fix is upstream: \
             `deployment_config::fetch()` collapses \"this origin has no config\" (404), \
             \"unreadable\" and \"did not answer in time\" into one `None`. Only the first \
             is a fact worth persisting; the others mean NOT YET and must be retried.\n\
             Rendered: {second:?}"
        );
        println!("  late deployment config: picked up on the next load");
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    let _ = std::fs::remove_dir_all(&root);
    r?;
    client.close().await.ok();
    Ok(())
}

/// **The `ecdeos.org` symptom, named at L1 — without the app booting at all.**
///
/// *Why this gate exists, in devops' words:* the re-key failure "isn't a crash.
/// `boot_load: complete`, frame loop armed, watchdog installed. The profile just
/// kept showing the old apps out of its own local tree while every remote path
/// under the retired peer 404'd. **Nothing on screen says anything is wrong.**"
/// That is what made it undiagnosable for days — not subtlety, but the complete
/// absence of any surface that could state it.
///
/// Diagnosing it needs exactly one comparison: who this profile is pointed at,
/// beside who the domain says publishes it. This asserts the recovery console
/// makes that comparison and **names the state**, through the whole arc:
///
/// 1. profile and domain agree → says so (the negative result, which is what
///    stops someone clearing site data on a wrong theory);
/// 2. the domain re-keys and the profile has **not** booted since → `STRANDED`,
///    naming both publishers;
/// 3. the app boots once and reconciles → back to agreement.
///
/// Step 2 is the one that could not previously be observed anywhere, and note
/// what it does *not* require: no broken app, no hung boot, no cleared storage.
/// The stranded condition is just "the profile's belief and the domain's
/// declaration disagree", which is why it can be reproduced this cheaply and why
/// it went unnoticed for so long.
///
/// Step 3 is not decoration — a panel that always cried `STRANDED` would pass a
/// one-shot test and be worthless in the field.
#[tokio::test(flavor = "current_thread")]
async fn the_recovery_console_names_a_stranded_profile(
) -> Result<(), Box<dyn std::error::Error>> {
    let (client, _server) = setup().await?;

    let root = "target/e2e-stranded-profile".to_string();
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)?;
    let cp = Command::new("cp").args(["-a", "dist/.", &root]).status()?;
    if !cp.success() {
        return Err(format!("could not stage an isolated SPA copy at {root}: {cp}").into());
    }
    let _ = std::fs::remove_file(format!("{root}/entity-deployment.json"));

    let port = pick_free_port()?;
    let server = Command::new("python3")
        .args(["tools/cors-serve.py", &root, &port.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let _serving = FederationServer(server);
    sleep(Duration::from_millis(400)).await;
    let app_url = format!("http://localhost:{port}/?log=trace");
    let bios_url = format!("http://localhost:{port}/?systemrecovery=1");

    // Read the console's routing card, polling past "probing…" so this never
    // races the bounded deployment-document fetch.
    async fn routing(client: &Client) -> Result<String, Box<dyn std::error::Error>> {
        let js = r#"const d = document.getElementById('domain'); return d ? d.textContent : '';"#;
        let mut text = String::new();
        for _ in 0..60 {
            sleep(Duration::from_millis(250)).await;
            text = client.execute(js, vec![]).await?.as_str().unwrap_or("").to_string();
            if !text.is_empty() && !text.contains("probing…") {
                break;
            }
        }
        Ok(text)
    }

    let r = async {
        // ── 1. A healthy returning visitor ───────────────────────────────────
        run_rekey_fixture("emit_rekey_fixture_before", &root);
        let peer_a = deployment_home_peer(&root);

        client.goto(&app_url).await?;
        wipe_all_storage(&client).await?;
        client.goto(&app_url).await?;
        wait_for_boot(&client, 30_000).await?;

        client.goto(&bios_url).await?;
        let healthy = routing(&client).await?;
        assert!(
            healthy.contains(&peer_a),
            "the console does not name the publisher this profile is pointed at ({peer_a}). \
             The routing mirror is the only channel across the L1/L5 boundary — if it is \
             absent, `boot_diagnostics::write_routing_mirror` did not run. Got: {healthy:?}"
        );
        assert!(
            !healthy.contains("STRANDED"),
            "the console reported STRANDED for a profile that agrees with its domain — a \
             panel that always cries stranded diagnoses nothing. Got: {healthy:?}"
        );

        // ── 2. The domain re-keys; this profile has NOT booted since ──────────
        // A's tree is deliberately left standing: what strands the visitor is the
        // profile still *pointing* at A, not A's artifacts vanishing.
        run_rekey_fixture("emit_rekey_fixture_after", &root);
        let peer_b = deployment_home_peer(&root);
        assert_ne!(peer_a, peer_b, "the fixture did not change the declared publisher");

        client.goto(&bios_url).await?;
        let stranded = routing(&client).await?;
        assert!(
            stranded.contains("STRANDED"),
            "RED — the profile is pointed at {peer_a} while the domain now publishes as \
             {peer_b}, and the recovery console did not say so.\n\
             This is the `ecdeos.org` state: local content keeps rendering, every remote \
             path 404s, and nothing on any screen states it. A console that cannot name \
             this is why the incident took days.\n\
             Got: {stranded:?}"
        );
        assert!(
            stranded.contains(&peer_a) && stranded.contains(&peer_b),
            "the console said STRANDED without naming BOTH publishers — the two ids are the \
             whole actionable content of the report. Got: {stranded:?}"
        );
        // It must also tell the user not to do the destructive thing.
        assert!(
            stranded.contains("must not be cleared"),
            "the stranded verdict does not tell the user their content is safe. Clearing \
             site data is the E5 action this console exists to talk people out of. \
             Got: {stranded:?}"
        );

        // ── 3. One normal boot reconciles it ─────────────────────────────────
        client.goto(&app_url).await?;
        wait_for_boot(&client, 30_000).await?;
        client.goto(&bios_url).await?;
        let healed = routing(&client).await?;
        assert!(
            !healed.contains("STRANDED"),
            "the profile still reads as stranded after a boot that should have reconciled \
             it to {peer_b}. Either R1 did not adopt, or the routing mirror was not \
             rewritten afterwards. Got: {healed:?}"
        );
        assert!(
            healed.contains(&peer_b),
            "after reconciling, the console does not show the profile pointed at {peer_b}. \
             Got: {healed:?}"
        );

        println!("  BIOS: agree → STRANDED({peer_a} vs {peer_b}) → reconciled, all at L1");
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;

    let _ = std::fs::remove_dir_all(&root);
    r?;
    client.close().await.ok();
    Ok(())
}

/// **G1 (service-worker variant) — a cached shell must survive a black-holed origin.**
///
/// `networkFirst` in `assets/sw.js` awaits `fetch(req, {cache:'reload'})` with no
/// `AbortController`, no `Promise.race` and no deadline — measured, `grep -c
/// setTimeout assets/sw.js` → 0. The cached shell sits one line below and is
/// only consulted in the `.catch`, which a black-holing origin never reaches.
/// Brick-matrix cell **#2**, at **E6**, and unlike cell #1 it applies to
/// production HTTPS deployments on any flaky network, not just to a cold boot.
///
/// Workbox — the reference implementation of this exact strategy — treats a
/// network timeout as the default-on mitigation and ships
/// `networkTimeoutSeconds: 3` for navigations. We hand-rolled network-first and
/// omitted the one option its author considers essential (AP28).
///
/// **The shell must be cached BEFORE the origin is black-holed**, or there is
/// nothing to fall back to and a pass would prove only that the app can fail.
/// That ordering is why `blackhole-serve.py` takes the stall set at runtime.
#[tokio::test]
async fn a_cached_shell_survives_a_blackholed_origin() -> Result<(), Box<dyn std::error::Error>> {
    let server = start_blackhole_server(&[])?;
    let client = connect_browser().await?;
    let port = blackhole_server_port();
    let url = format!("http://localhost:{port}/?log=trace");

    // (1) A normal load: register the SW, let it claim this page, and cache `/`.
    client.goto(&url).await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;

    // Registration happens on `load` and `clients.claim()` a beat later, so this
    // is an async round-trip, not a fact available on the first read. Polled
    // rather than slept: a fixed wait here would encode a guess about someone
    // else's install timing, and would fail as though the app were broken.
    let sw_probe = r#"
        const cb = arguments[arguments.length - 1];
        (async () => {
            const reg = await navigator.serviceWorker.getRegistration();
            const hit = await caches.match('/');
            return {
                registered: !!reg,
                controlled: !!navigator.serviceWorker.controller,
                shell_cached: !!hit,
            };
        })().then(r => cb(r), e => cb({error: String(e)}));
        "#;
    let deadline = Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
    let mut cached = client.execute_async(sw_probe, vec![]).await?;
    while Instant::now() < deadline {
        let ready = cached.get("shell_cached").and_then(|b| b.as_bool()) == Some(true)
            && cached.get("controlled").and_then(|b| b.as_bool()) == Some(true);
        if ready {
            break;
        }
        sleep(Duration::from_millis(200)).await;
        cached = client.execute_async(sw_probe, vec![]).await?;
    }

    // A precondition, not the thing under test: on an origin with no service
    // worker there is no offline path to break, and that is a scope fact rather
    // than a bug (§1.1a). Say so instead of asserting a red about the harness.
    if cached.get("shell_cached").and_then(|b| b.as_bool()) != Some(true) {
        client.close().await.ok();
        return Err(format!(
            "precondition failed: no cached shell on this origin, so there is nothing \
             for a black-holed fetch to fall back TO and this gate cannot say anything. \
             Got: {cached:?}. A secure context is required for a service worker — \
             `localhost` qualifies, a LAN address over plain HTTP does not."
        )
        .into());
    }

    // (2) Black-hole the shell. From here every navigation to `/` is accepted
    // and never answered.
    let control = set_blackhole(&["/"])?;
    assert!(
        control.contains("stalling: /"),
        "the black-hole control endpoint did not take the stall set: {control}"
    );

    // (3) Reload. `networkFirst` must give up on the network and serve the
    // cached shell within its deadline.
    let goto = client.goto(&url).await;
    let boot = if goto.is_ok() {
        wait_for_boot(&client, BOOT_BUDGET_MS).await
    } else {
        Err("navigation timed out".into())
    };

    let stalled = server.request_count("STALL");
    client.close().await.ok();

    assert!(
        stalled >= 1,
        "VACUOUS: the origin was never actually asked for a black-holed path, so the \
         reload proved nothing. Control endpoint said: {control}"
    );

    if let Err(e) = boot {
        return Err(format!(
            "G1/SW RED — a reload against a black-holing origin never painted, even \
             though the shell IS cached ({stalled} request(s) stalled at the wire).\n\
             This is brick-matrix cell #2 at E6: `networkFirst` awaits the network with \
             no deadline, so the cached shell one line below is never reached.\n\
             The fix is a ~3 s deadline on the network leg (C2), Workbox's documented \
             default for navigations: on expiry, serve the cached copy.\n\
             Navigation result: {:?}\nUnderlying failure: {e}",
            goto.map(|_| "ok").map_err(|e| e.to_string())
        )
        .into());
    }

    println!("  G1/SW: reload painted from cache with {stalled} black-holed request(s)");
    Ok(())
}

/// **C5 — the app names the build it is running, and the name matches `dist/`.**
///
/// Until this landed the application could not report its own version at all:
/// `tools/build-stamp.sh` had been stamping `<meta name="entity-build">` into
/// the shell for months and `grep -rn "entity-build" src/` returned **0 hits**.
/// A bug report could not identify a build, and "are these two domains running
/// the same code" was answerable only by curling them from outside.
///
/// **The assertion is against `dist/index.html`, not against a constant.** A
/// test that checked the log merely contained *some* build string would pass on
/// a build id read from the wrong place, or stale, or invented — which is the
/// entire failure this value exists to prevent. Reading the expected value out
/// of the artifact the browser was actually served makes the two independent.
///
/// It also asserts the **bundle hash** separately, because the two identities
/// answer different questions and only one of them is comparable across
/// deployments: two docs-only commits produce byte-identical output, so the
/// commit alone cannot tell you whether two domains match. Measured on the two
/// production apexes, they did — different commits, same bundle.
#[tokio::test]
async fn the_app_reports_the_build_it_is_running() -> Result<(), Box<dyn std::error::Error>> {
    // Read the expectation from the served artifact FIRST, so a mangled `dist/`
    // fails here with a clear reason rather than as a mismatch later.
    let shell = std::fs::read_to_string("dist/index.html")
        .map_err(|e| format!("cannot read dist/index.html — run `make wasm` first: {e}"))?;
    let expect_commit = shell
        .split("name=\"entity-build\" content=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string)
        .ok_or("dist/index.html carries no entity-build stamp — tools/build-stamp.sh did not run")?;
    let expect_bundle = shell
        .split("entity-browser-")
        .nth(1)
        .map(|rest| {
            rest.chars()
                .take_while(|c| c.is_ascii_hexdigit())
                .collect::<String>()
        })
        .filter(|h| h.len() >= 8)
        .ok_or("dist/index.html names no hashed main bundle")?;

    let (client, _server) = setup().await?;
    client
        .goto(&format!("http://localhost:{}/?log=trace", http_server_port()))
        .await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;

    let log = capture_log(&client).await?;
    let line = log
        .iter()
        .find(|l| l.contains("WASM init: build"))
        .cloned()
        .ok_or_else(|| {
            format!(
                "the app never logged its build id. Expected a 'WASM init: build' line naming \
                 commit {expect_commit} / bundle {expect_bundle}. {} log lines captured.",
                log.len()
            )
        })?;
    client.close().await.ok();

    assert!(
        line.contains(&expect_commit),
        "the app reported a build id that is not the one dist/index.html carries.\n\
         dist/ says commit: {expect_commit}\n  logged: {line}"
    );
    assert!(
        line.contains(&expect_bundle),
        "the app did not report the main bundle hash, or reported a different one. \
         Reading it from the shell's <head> is what keeps this in step with \
         `assets/sw.js`'s BUNDLE_HASH — if those two disagree, the service worker \
         keys its build-scoped worker cache on a build the app does not think it is.\n\
         dist/ says bundle: {expect_bundle}\n  logged: {line}"
    );

    println!("  C5: app reports commit={expect_commit} bundle={expect_bundle}, matching dist/");
    Ok(())
}

/// **The headers-then-stall variant — a hazard hypothesised from reading, then
/// measured, and the reading was wrong.**
///
/// The concern was this: `fetch` is specified to resolve on **headers**, not on
/// the body. A deadline that clears its timer when that promise resolves would
/// therefore already be disarmed when the body fails to arrive, and whatever
/// reads the body next would hang — in `networkFirst` that is
/// `await cache.put('/', resp.clone())` for a navigation, and then the page
/// itself. The same E6, one step later, invisible to the never-answers gate.
///
/// **Measured, and it is not reachable: `fetchWithDeadline` already covers it.**
/// Discriminated by neutering rather than by argument — with
/// `NETWORK_DEADLINE_MS` raised to 300 000 this test goes **red** (the
/// navigation times out at 45 s having never painted), and at 3 000 it is green
/// in ~5 s. So the existing deadline is what saves this case: against an origin
/// that sends complete headers and then nothing, the fetch promise does not
/// settle within the deadline, the abort fires, `networkFirst` reaches its
/// `.catch`, and the cached shell is served.
///
/// **Keep this test even though it found nothing to fix.** It pins a property
/// the design cannot derive — that one deadline covers both stall shapes — and
/// that property is engine behaviour, not our code, so it can change underneath
/// us without any diff of ours touching it. If it ever goes red while
/// `NETWORK_DEADLINE_MS` is unchanged, the original hypothesis has become true
/// and the fix is **not** a bigger number: the deadline must stay armed through
/// the body read for navigations, which changes `fetchWithDeadline`'s contract.
#[tokio::test]
async fn a_cached_shell_survives_an_origin_that_stalls_the_body(
) -> Result<(), Box<dyn std::error::Error>> {
    let server = start_blackhole_server(&[])?;
    let client = connect_browser().await?;
    let port = blackhole_server_port();
    let url = format!("http://localhost:{port}/?log=trace");

    // (1) Normal load: register the SW, let it claim, cache `/`.
    client.goto(&url).await?;
    wait_for_boot(&client, BOOT_BUDGET_MS).await?;
    let sw_probe = r#"
        const cb = arguments[arguments.length - 1];
        (async () => {
            const hit = await caches.match('/');
            return {
                controlled: !!navigator.serviceWorker.controller,
                shell_cached: !!hit,
            };
        })().then(r => cb(r), e => cb({error: String(e)}));
        "#;
    let deadline = Instant::now() + ASYNC_ROUND_TRIP_BUDGET;
    let mut cached = client.execute_async(sw_probe, vec![]).await?;
    while Instant::now() < deadline {
        let ready = cached.get("shell_cached").and_then(|b| b.as_bool()) == Some(true)
            && cached.get("controlled").and_then(|b| b.as_bool()) == Some(true);
        if ready {
            break;
        }
        sleep(Duration::from_millis(200)).await;
        cached = client.execute_async(sw_probe, vec![]).await?;
    }
    if cached.get("shell_cached").and_then(|b| b.as_bool()) != Some(true) {
        client.close().await.ok();
        return Err(format!("precondition failed: no cached shell. Got: {cached:?}").into());
    }

    // (2) Headers arrive; the body never does.
    let control = set_blackhole_body(&["/"])?;
    assert!(
        control.contains("body-stalling: /"),
        "the control endpoint did not take the body-stall set: {control}"
    );

    // (3) Reload.
    let goto = client.goto(&url).await;
    let boot = if goto.is_ok() {
        wait_for_boot(&client, BOOT_BUDGET_MS).await
    } else {
        Err("navigation timed out".into())
    };

    let stalled = server.request_count("STALL-BODY");
    client.close().await.ok();

    assert!(
        stalled >= 1,
        "VACUOUS: the origin was never asked for a body-stalled path, so the reload \
         proved nothing. Control endpoint said: {control}"
    );

    if let Err(e) = boot {
        return Err(format!(
            "RED — a reload against an origin that sends HEADERS and then no body never \
             painted, although the shell IS cached ({stalled} body-stalled request(s)).\n\
             This was GREEN when measured, saved by `NETWORK_DEADLINE_MS` (confirmed by \
             raising it to 300000, which makes this exact test red). So check that first: \
             if the deadline is unchanged, the engine's behaviour has moved and `fetch` is \
             now resolving on headers, leaving the body read that follows \
             (`await cache.put('/', resp.clone())` for a navigation) unbounded.\n\
             In that case the fix is NOT a bigger timeout — the deadline must stay armed \
             through the body read for navigations, which changes `fetchWithDeadline`'s \
             contract.\n\
             Navigation result: {:?}\nUnderlying failure: {e}",
            goto.map(|_| "ok").map_err(|e| e.to_string())
        )
        .into());
    }

    println!("  body-stall: reload painted with {stalled} headers-then-stall request(s)");
    Ok(())
}
