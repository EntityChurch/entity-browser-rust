# Phase 0a — OPFS + Dedicated Worker Probe

Verifies the two browser capabilities the WASM worker migration plan
depends on:

1. Spawning a dedicated Web Worker via `new Worker(...)`.
2. Opening an OPFS `FileSystemSyncAccessHandle` *inside* that worker.

Per the WASM worker-migration analysis (§8 Phase 0a) — this probe is the
half-day investigation owned by the egui team that gates Phase 2
(OPFS ContentStore backend).

## What it tests

| # | Test | What it checks |
|---|------|----------------|
| 1 | `new Worker('worker.js')` | Dedicated worker spawn works at all. |
| 2 | Main↔worker postMessage | Workers + messaging are functional. |
| 3 | `navigator.storage.getDirectory()` in worker | OPFS root reachable from worker context. |
| 4 | `getFileHandle({ create: true })` | File handle creation works. |
| 5 | `createSyncAccessHandle()` | **The critical capability** — sync file I/O API exists. |
| 6 | Write + read 4KB via sync handle | Sync I/O actually works end-to-end. |
| 7 | Close + cleanup | Lifecycle works; doesn't leak handle. |

If step 5 or 6 fails, the platform is **not viable** for the v1 worker
migration's OPFS-backed `ContentStore`. Per the plan's R10b decision,
that platform gets a documented minimum-version commitment, not an IDB
fallback.

## How to run

### Plain browser (Chrome / Firefox / Safari / Edge)

OPFS requires a [secure context](https://developer.mozilla.org/en-US/docs/Web/Security/Secure_Contexts):
HTTPS, or `http://localhost`. Don't run via `file://` — modern browsers
disable storage APIs there.

```bash
cd tools/phase-0a-opfs-probe
python3 -m http.server 8080
# Then open http://localhost:8080/ in the target browser.
```

Read the results table, copy the "Raw data" JSON block into the
results spreadsheet / report.

### Tauri WebView (the actual deployment target)

The probe is platform-agnostic HTML/JS; serve it the same way and
point the Tauri WebView at it. Two options:

1. **Quickest:** start the HTTP server above, then in a Tauri dev
   build set the WebView URL to `http://localhost:8080/`. This works
   for capturing results on Linux WebKitGTK, macOS WKWebView, Windows
   WebView2.
2. **More representative:** copy the two files (`index.html`,
   `worker.js`) into a minimal Tauri scaffold's `dist/` and run that
   Tauri binary. This matches the actual WebView config and CSP that
   the egui app will ship with.

Both produce the same probe results; (2) catches CSP / sandbox
issues that (1) wouldn't. For Phase 0a it's enough to start with (1)
and only escalate to (2) if a result looks suspect.

### Target platforms to cover

Per the plan §8 Phase 0a table:

| Target | WebView | Notes |
|---|---|---|
| Linux (Tauri default) | WebKitGTK | System-WebKit-version dependent; older distros may lag. **Highest-risk target.** |
| macOS (Tauri) | WKWebView | Expected OK on macOS 14+ (Sonoma, 2023). |
| Windows (Tauri) | WebView2 (Chromium) | Expected OK since 2022. |
| iOS (Tauri) | WKWebView | Expected OK on iOS 17+. |
| Plain Chrome / Firefox / Safari | own | Reference baseline. |

For each, run the probe, record pass/fail per row, and the env block.

## Reporting

When all platforms are covered, write up the results in
`docs/architecture/reviews/PHASE-0A-OPFS-PROBE-RESULTS-2026-05-MM.md`
with one section per platform. The verdict line should be one of:

- `OPFS available everywhere → Phase 2 unblocked, no minimum-version footnote needed.`
- `OPFS available on N of M platforms → Phase 2 ships; platforms X, Y get a documented minimum-version commitment.`
- `OPFS unavailable on the primary target → escalate, reconsider plan with entity-core-rust team before Phase 2 commits.`
