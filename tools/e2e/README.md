# E2E browser test workflow

`tests/e2e_worker.rs` drives the real wasm-worker build in a headless
Firefox (Podman + Selenium-standalone), booting the app in **Worker
mode** (`?worker=1`) and walking a multi-phase flow: window spawn, KB
create → reload → persistence, backend Memory/OPFS peer create +
classification + delete/OPFS-tombstone cleanup, reload-survival, etc.
It asserts no panics and makes per-phase assertions, printing the full
captured browser console under `--nocapture`.

No host-side binaries installed — Podman pulls a pinned image.

## One-time setup

```bash
# Pull the pinned Selenium-standalone-firefox image (~600MB).
podman pull docker.io/selenium/standalone-firefox:149.0.2-geckodriver-0.36.0-20260404
```

The image tag (Firefox version + date stamp) is pinned in
`tests/e2e_worker.rs`. To upgrade, change it in both this README and
the test file.

## Running

```bash
# Terminal 1 (once): start the Selenium container. --network=host lets
# it reach the host's test http.server.
podman run -d --rm --name e2e-firefox --network=host \
    docker.io/selenium/standalone-firefox:149.0.2-geckodriver-0.36.0-20260404

# Terminal 2: build wasm + run the test.
make e2e-worker            # = `make wasm` then `cargo test --test e2e_worker -- --nocapture`

**On a machine WITH a display, first build the Tauri binary** — Phase 14 spawns
`./src-tauri/target/debug/entity-browser-tauri` for the ConnectPeer test and
**hard-errors** if it is missing (headless it self-skips instead, so this only
bites on a desktop). `make e2e-worker` does not build it:

```bash
make tauri                 # builds the binary Phase 14 needs (slow, one-time)
```

Without it the run dies at Phase 14 and **every later phase is silently never
exercised** — which is exactly how two stale assertions after it went unnoticed
for several commits. If you are changing anything the later phases cover, build
it.

# When done:
podman stop e2e-firefox
```

`make e2e-worker` builds the **default** wasm (`KB_DOCS_ROOT` unset →
**0 embedded docs**; do NOT set `KB_DOCS_ROOT` for the e2e — a large
embedded corpus floods the worker's OPFS on load and destabilises the
KB-persistence phases). Runtime is ~45s with the empty default.

### Port: NOT 8081

The test starts its **own** `python3 -m http.server` on **port 8092**
(override with `E2E_HTTP_PORT`). This is deliberately *not* 8081 —
`make serve` uses 8081 for phone/manual testing, and the e2e is built
so the two can run **in parallel** without colliding. (Earlier docs
said 8081 / "don't run make serve in parallel" — no longer true.)

## Troubleshooting

- **`New session request timed out` / `connectionFailure` / "failed to
  connect to WebDriver at :4444"** — infra, not a code failure. The
  standalone image allows one session at a time; a killed/abandoned
  prior run leaves it stuck. Fix:
  ```bash
  podman restart e2e-firefox
  # wait until ready:
  until curl -s -m2 localhost:4444/status | grep -q '"ready": *true'; do sleep 2; done
  ```
- Capture full output to a file, **not** `| tail` — the panic line and
  per-phase `println!`s must survive for diagnosis.
- A failure deep in a later phase still means earlier phases passed —
  read the progress prints to see how far it got.

- **Known intermittent (pre-`2026-07-21`, unfixed):** the Phase-2 Site Editor
  `delete site` assertion occasionally reports `still_lists: true` even though
  the assertion now polls for 10 s. The delete itself measures ~160 ms when it
  works, and the failure reproduces on older commits, so it is not fallout from
  the i18n pass. Re-run to confirm before investigating anything else; the
  suspicion is a race in which site is selected when `Delete site` fires.

- **Second known intermittent (recorded `2026-07-22`, unfixed):** Phase 26.8's
  *"after delete the theme must leave every registry-driven dropdown"*. This is
  a **different failure from the Site Editor one above** and should not be
  filed under it — the observed status line read `Deleted "⁨e2e-neon⁩"`, so the
  right theme was deleted and the dropdown still listed it. That rules out the
  wrong-selection suspicion that covers the Site Editor case.

  Suspected mechanism, from reading the path (**not yet proven** — record what
  you see before assuming this):

  1. `user_themes::delete_theme` clears the main-thread registry
     *synchronously* (`theme_tokens::unregister_user_theme`) and then issues
     `peers.dispatch_remove` for the tree entity.
  2. The Worker-arm cache mirror is updated **from subscription events, not
     from write responses** (`wasm-worker-proxy` cache invariant 4), so
     immediately after `dispatch_remove` the mirror still holds the theme.
  3. `UserThemes::sync` re-derives the registry *from* `peers.tree_listing`
     over that mirror. If anything dirties the themes watch in the window
     between (1) and the removal event landing, sync **re-registers the theme
     it just deleted** — and the dropdown lists it again.

  `sync` reconciles both directions, so it should converge on the next event;
  what is not yet explained is why it sometimes fails to inside the 5 s poll.
  The cheap next diagnostic is the `registered=/removed=` counts already
  emitted by `sync`'s `tracing::info!("user themes synced from tree")` — a
  `registered=1` *after* the delete confirms resurrection. The
  `audit-worker-reads` cargo feature (`peers_worker::warn_if_unsubscribed`) is
  the break-glass lamp for the related "read a prefix nobody subscribed" case.

  Both intermittents are delete-reflection races and are **orthogonal to i18n** —
  neither asserts on translated text.

- **Matching localized text:** `t()`/`t_plural()` wrap every interpolated arg in
  Unicode bidi isolates (FSI `\u2066` … PDI `\u2069`). `sec.textContent` therefore
  contains invisible marks *between the number and its noun*, and a regex like
  `/\d+ peers/` will not match. Strip them first:
  `text.replace(/[\u2066-\u2069]/g, '')`. This silently broke the Peers-footer
  parse; see the comment at `read_sdk_count_script`.

## Visual / mobile verification (screenshots)

The same Selenium Firefox can be driven directly over the W3C
WebDriver HTTP API (curl + jq, no Rust compile) to **screenshot the
UI at any viewport** — used to verify responsive/mobile layout.
Recipe:

1. `make wasm` then serve `dist/` on a spare port (e.g. `python3 -m
   http.server 8099 --directory dist &`). `--network=host` means the
   container sees `localhost:8099`.
2. `POST /session` (firefox) → `POST /session/{id}/window/rect`
   `{"width":390,"height":844}` (Firefox-headless clamps innerWidth to
   ~500px min; still ≤768px so the mobile media query applies) →
   `POST /session/{id}/url {"url":"http://localhost:8099/?worker=1"}`.
3. Poll `POST /session/{id}/execute/sync` until
   `document.getElementById('dom-layer').shadowRoot` has
   `button.spawn-btn` (boot done). The app DOM lives in that shadow
   root.
4. `execute/sync` to click the `+ <WindowName>` spawn button, then to
   read `getBoundingClientRect` / `getComputedStyle` of the elements
   under test.
5. `GET /session/{id}/screenshot` → base64 → decode to a PNG and
   inspect it. `DELETE /session/{id}` when done.

This is the path for "does it actually look right on a phone" — not
just code-soundness. (Used to verify the Peers window
responsive fix at 390 vs 1280.)

## Architecture

```
┌─────────────────────┐         ┌───────────────────────────┐
│  cargo test         │ ←HTTP→  │  Selenium standalone      │
│  - fantoccini       │  :4444  │  Firefox + geckodriver    │
│  - tokio runtime    │  WebDr. │  (podman, --network=host) │
│  - spawns python3   │         │                           │
│    http.server :8092│         │                           │
└─────────────────────┘         └───────────────────────────┘
       │                                    │
       │ spawns                             │ navigates to
       ▼                                    ▼
┌─────────────────────┐         ┌───────────────────────────┐
│  python3 -m         │ ←HTTP→  │  http://localhost:8092/   │
│  http.server :8092  │         │  ?worker=1&log=trace      │
│  dir=dist           │         │  (trunk-built wasm)       │
└─────────────────────┘         └───────────────────────────┘
```

Console capture is client-side: `index.html` wraps `console.*` into
`window.__entity_browser_log`; the test reads it via `execute_script`.

## Image versions

Selenium tags `standalone-firefox` by **Firefox version + date**, not
Selenium version. Pinned default:
`docker.io/selenium/standalone-firefox:149.0.2-geckodriver-0.36.0-20260404`
(Firefox 149.0.2, geckodriver 0.36.0). The date stamp is
immutable. List current tags:

```bash
curl -s "https://registry.hub.docker.com/v2/repositories/selenium/standalone-firefox/tags?page_size=20&ordering=last_updated" \
    | python3 -m json.tool | grep '"name":'
```

Avoid `latest`/`nightly`/`beta` for reproducibility. For maximum
reproducibility pin by digest:
`podman image inspect <tag> --format '{{.Digest}}'` then use
`standalone-firefox@sha256:...`.

**Source:** https://github.com/SeleniumHQ/docker-selenium (Apache-2.0,
published to Docker Hub).

## Why this setup

- **Podman + Selenium image** — zero host install, pinned signed
  image, reproducible.
- **fantoccini, not Playwright** — Rust-only toolchain, no Node/npm;
  tests live alongside cargo tests.
- **`--network=host`** — simplest path for the container to reach the
  host http.server (weaker isolation; fine for one trusted dev image).
- **Client-side console capture** — dependency-light, works across
  WebDriver versions (W3C BiDi log API not broadly supported by
  fantoccini yet).
