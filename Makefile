# Entity Browser — build targets
#
# Active deployments:
#   make wasm        — browser build (DOM)
#   make tauri-run   — desktop build (DOM in WebView + native backend peer)
#
# === make + podman build convention ===========================================
# A bare machine needs ONLY `make` and `podman` (no rust/cargo/trunk on host).
# The real deliverable is the browser build: `make wasm` (debug) /
# `make wasm-release`. Both run Trunk INSIDE the toolchain container defined by
# the Dockerfile. The PARENT meta dir is bind-mounted at /src/entity-systems so
# the sibling `../entity-core-rust` workspace path-deps resolve, with workdir set
# to this repo. A persistent cargo registry cache volume makes rebuilds fast.
#
#   make image        — build the toolchain image (rust 1.94.1 + wasm32 + trunk)
#   make build        — alias for `make wasm` (the conventional bare-box entry)
#   make wasm         — debug browser build -> dist/   (in container)
#   make wasm-release — size-optimized release build -> dist/  (in container)
#   make test / lint  — native unit + peer-integration tests / clippy (in container)
#   make publish*     — pure-cargo publish targets (in container)
#
# Host-only targets (NOT part of the bare-box gate, and depend on host services
# or an attached display): the `python3 -m http.server` serve steps (serve /
# build-serve / publish-serve / publish-papers), `e2e-worker` (external Selenium
# on :4444), the `go`/`python3` render+apps build inside publish-papers, and
# `tauri-run` (needs a desktop session). `make native` is a deprecation stub.
IMAGE       := entity-browser-rust-build
PARENT      := $(shell dirname $(CURDIR))
CARGO_CACHE := $(HOME)/.cache/cargo-entity-browser
# Trunk downloads its own version-matched wasm-bindgen-cli into its tool cache
# (`/root/.cache/trunk` in the image). Without persisting it, every `podman run
# --rm` starts cold and re-downloads + re-installs wasm-bindgen — the dominant
# "why is it doing this AGAIN" cost. Persist it like the cargo registry so the
# download happens once. (wasm-opt is baked into the image, not downloaded.)
TRUNK_CACHE := $(HOME)/.cache/cargo-entity-browser-trunk

# Output isolation — so two builds can run AT THE SAME TIME against the same
# source without clobbering each other. `DIST` = trunk's WASM output dir;
# `TARGET_DIR` = cargo's build dir. Both default to the canonical locations,
# so every existing target behaves exactly as before. The `publish-*` family
# overrides them (target-specific vars below) to `dist-publish`/`target-publish`,
# which is what lets `make tauri-run` and `make publish-serve` run concurrently.
# The cargo registry + trunk tool caches stay SHARED (cargo/trunk lock them
# safely, and they're read-mostly once warm) — only the OUTPUT splits, so the
# isolated build is still fast (deps aren't recompiled/redownloaded, only the
# crate's own artifacts live in a separate target dir).
DIST       ?= dist
TARGET_DIR ?= target

# ============================================================================
# Podman resource caps — entity-systems standard (docs/release-readiness/
# RESOURCE-CAPS.md). Per-container ceilings so a build/run can't take the host
# down. Tune the COMMITTED defaults for THIS project; override per-machine
# WITHOUT editing this file via env vars or an untracked caps.local.mk.
#
#   Precedence (highest first):  env var  >  caps.local.mk  >  defaults below
#   CAP_SWAP == CAP_MEM  =>  zero swap: container is OOM-killed cleanly at the
#   cap instead of thrashing the host into a freeze.
# ============================================================================
-include caps.local.mk          # untracked per-machine overrides (gitignored)

# COMMITTED default sized from this repo's measured peak + headroom. The
# heaviest target is `make test` (595 native unit + 17 peer-integration + the
# other integration suites, compiling + linking the full sibling workspace):
# measured cold worst-case peak ~3.5 GiB at full --cpus=12 (see
# RELEASE-READINESS.md). wasm-release peaks lower (~1.9 GiB). 6g leaves room for
# more-core machines + ongoing test growth while staying a hard protective
# ceiling. A smaller machine lowers this via caps.local.mk (§4a).
CAP_MEM           ?= 6g         # hard memory ceiling per container
CAP_SWAP          ?= $(CAP_MEM) # keep == CAP_MEM (no swap); raise only deliberately
CAP_PIDS          ?= 4096       # max procs/threads (RUN only) — stops fork bombs
CAP_CPUS          ?= 6          # CPU cores at runtime (RUN only; fractional ok)
CAP_CGROUP_PARENT ?=            # optional host slice to nest under, e.g. dev-heavy.slice

_cap_cgp := $(if $(strip $(CAP_CGROUP_PARENT)),--cgroup-parent=$(CAP_CGROUP_PARENT),)

# podman BUILD accepts --memory/--memory-swap/--cgroup-parent (NOT --cpus/--pids-limit)
PODMAN_BUILD_CAPS := --memory=$(CAP_MEM) --memory-swap=$(CAP_SWAP) $(_cap_cgp)
# podman RUN accepts the full set
PODMAN_RUN_CAPS   := --memory=$(CAP_MEM) --memory-swap=$(CAP_SWAP) \
                     --pids-limit=$(CAP_PIDS) --cpus=$(CAP_CPUS) $(_cap_cgp)

.PHONY: image build help fmt check clean

.DEFAULT_GOAL := help

# ADR-0019 Tier-1 verbs: help build test lint fmt check clean. `build` (alias of
# `wasm`), `test`, `lint` already exist below; help/fmt/check/clean are added
# here. Every recipe runs inside the toolchain image (host needs only make+podman).
help:
	@echo "entity-browser-rust — make + podman (host needs only make + podman)"
	@echo
	@echo "  build    WASM debug build → dist/ (alias of wasm; conventional entry)"
	@echo "  test     native unit + peer-integration suite, in-container"
	@echo "  lint     cargo clippy, in-container (read-only)"
	@echo "  fmt      cargo fmt, in-container (writes)"
	@echo "  check    lint + test (the green gate)"
	@echo "  clean    remove dist/ and the toolchain image"
	@echo
	@echo "  wasm / wasm-release / serve / build-serve / tauri-run / e2e-worker"
	@echo "  — see the Makefile header for the full target catalogue."

# Build the toolchain image (rust 1.94.1 + wasm32 + trunk + binaryen + webkit2gtk).
image:
	podman build $(PODMAN_BUILD_CAPS) -t $(IMAGE) .

# Run a command inside the toolchain image with the parent meta dir mounted at
# /src/entity-systems (sibling path-deps resolve) and workdir = this repo. The
# cargo registry/cache is a persistent volume so deps aren't re-downloaded every
# build. Resource caps (PODMAN_RUN_CAPS) bound every container.
define RUN
	mkdir -p $(CARGO_CACHE) $(TRUNK_CACHE)
	podman run --rm $(PODMAN_RUN_CAPS) \
		-v $(PARENT):/src/entity-systems:z \
		-v $(CARGO_CACHE):/usr/local/cargo/registry:z \
		-v $(TRUNK_CACHE):/root/.cache:z \
		-e CARGO_TARGET_DIR=$(TARGET_DIR) \
		-w /src/entity-systems/$(notdir $(CURDIR)) \
		$(IMAGE) \
		sh -c '$(1)'
endef

# There is no native UI build. The native binary is a deprecation stub
# that prints a redirect to the active targets. (The legacy eframe
# renderer was removed.)
native:
	@echo "make native is deprecated — there is no native UI build."
	@echo ""
	@echo "  make wasm         — browser build (DOM)"
	@echo "  make tauri-run    — desktop build (DOM in WebView + native backend peer)"
	@echo ""
	@exit 1

# Run tests (native unit + peer-integration), in-container. The
# Selenium-dependent e2e suite is gated behind the `e2e` cargo feature
# (off by default), so this target is fully bare-box: no Selenium needed.
# Run `make e2e-worker` for the browser E2E path.
test: image
	$(call RUN,cargo test)

# Lint, in-container.
lint: image
	$(call RUN,cargo clippy)

# Tier-1 fmt = autoformat (writes), in-container.
fmt: image
	$(call RUN,cargo fmt)

# Tier-1 check = the green gate (lint + test).
check: lint test

# Tier-1 clean = remove the host-visible build output (dist/) and the toolchain
# image. The cargo registry/target cache lives in a persistent named volume and
# is left intact (delete $(CARGO_CACHE) by hand for a full cold reset).
clean:
	rm -rf dist/ dist-publish/
	-podman rmi $(IMAGE)

# WASM debug build → dist/. Single bundle for both Direct (default)
# and Worker (`?worker=1`) modes — capability detection at boot picks
# Worker automatically when available and falls back to Direct on
# failure (Stage 1B).
wasm: image
	$(call RUN,trunk build --dist $(DIST) && ./tools/check-dist.sh $(DIST))

# Alias — `make build` is the conventional bare-box entry point across the repo group.
build: wasm

# WASM release build → dist/
wasm-release: image
	$(call RUN,trunk build --release --dist $(DIST) && ./tools/check-dist.sh $(DIST))

# Deploy-staleness guard: verify dist/ is internally consistent (index.html
# references only bundles that exist + are non-empty). Catches the class
# `make e2e-worker` can't — see the SW-cache-and-durability review.
check-dist:
	@./tools/check-dist.sh

# Phase 0c — frame-time measurement build. Same as `wasm` but with the
# `measurement` cargo feature, which enables per-frame L0-call counters
# logged to the browser console (src/frame_counters.rs). Use with
# `make serve` and a representative working session to gather counts
# before Phase 1 freezes the cache API.
wasm-measurement: image
	$(call RUN,trunk build --features measurement --dist $(DIST))

# E2E browser test (exercises Worker mode via `?worker=1`). Requires
# the Selenium-firefox container running on :4444 — see
# tools/e2e/README.md. The test prints the full captured browser
# console under --nocapture, so this is the primary diagnostic path
# without manual browser refresh.
e2e-worker: image
	# The e2e dist is built WITH `--features demo-apps`: the launcher→player
	# e2e (Phase 2h.2) needs a deterministic baked app (war/calculator) to
	# render + launch without a live origin. demo-apps is OFF in every
	# release/serve/tauri build — no fake apps are shipped (see Cargo.toml
	# [features]) — so this is the ONE build that bakes them.
	$(call RUN,trunk build --features demo-apps --dist $(DIST) && ./tools/check-dist.sh $(DIST))
	# --features e2e: the e2e_worker suite is `#![cfg(feature = "e2e")]`, so it
	# compiles to nothing (and `make test` stays bare-box green) UNLESS the
	# feature is on. This target turns it on; it needs Selenium on :4444.
	# --test-threads=1: the e2e tests share one Selenium session + http port,
	# so they must run serially (the main boot test + the multi-tab guard test).
	cargo test --features e2e --test e2e_worker -- --nocapture --test-threads=1

# Tauri desktop (size-optimized release WASM + debug backend, logs to stdout).
# Use this for development — stable WASM (no overflow panics). Right-click
# in the WebView → Inspect Element for the WebKit Inspector (Console, DOM,
# Performance, Memory). NOTE (C8): the release profile is now size-optimized
# (debug=false + wasm-opt -Oz), so the Inspector no longer source-maps Rust.
# To debug Rust here, set [profile.release] debug=true in Cargo.toml and
# index.html data-wasm-opt="0" temporarily.
#
# NOTE: Same unified bundle as `make wasm`; runtime boot detects Tauri
# and forces Direct mode (src/main.rs). WebKitGTK ≤ 2.52 lacks
# `WorkerNavigator.storage`, so worker-mode OPFS init HARD-FAILS there
# (it does not silently in-memory — build_async returns Err, which would
# round-trip and fall back to Direct+banner). We preemptively force Direct
# in Tauri to skip that wasted failed-worker spawn. Browser deployments get
# worker-mode automatically via capability detection. Tracked in
# WORKER-MODE-LIVING-DOC §3.6.
tauri: wasm-release
	# Re-embed the freshly-built frontend. `tauri::generate_context!()`
	# (src-tauri/src/lib.rs) reads ../dist at macro-expansion time, but
	# cargo's incremental compiler can't see that dependency — so when only
	# dist/ changed it leaves lib.rs "up to date" and embeds the STALE
	# frontend (you launch an old UI). We bypass Tauri's CLI asset pipeline
	# here (raw `cargo build`), so nothing else tracks it. Touching the
	# embedding source forces a re-expand + re-embed every build. Cheap:
	# this target always reruns wasm-release anyway, so it's never a no-op.
	$(call RUN,touch src-tauri/src/lib.rs && cd src-tauri && cargo build)
	@echo ""
	@echo "Built: ./src-tauri/target/debug/entity-browser-tauri"
	@echo "Launch on a desktop session (needs webkit2gtk + a display): make tauri-run"
	@echo "Inspector: right-click → Inspect Element in the WebView"

# Tauri — build and run in one step
tauri-run: tauri
	./src-tauri/target/debug/entity-browser-tauri

# Serve whatever is currently in dist/ (no rebuild). Fast, but does NOT
# guarantee the bundle is current — use `make build-serve` when you need
# certainty you're serving the latest optimized build.
serve:
	@echo "  → http://localhost:$(PORT)   (override with: make serve PORT=8082)"
	python3 -m http.server $(PORT) --directory dist

# Build the SHIPPING (release-optimized) WASM, then serve it — always the
# latest. Use this when you want certainty you're running the real
# optimized release artifact (`make serve` alone does NOT rebuild). Prints
# the bundle sizes so you can see it's the optimized build. Slower than
# `make serve` because the release profile is opt-level=z + fat LTO.
build-serve: wasm-release
	@echo ""
	@echo "=== SHIPPING (release-optimized) build — serving latest ==="
	@ls -lh dist/*.wasm 2>/dev/null | awk '{print "  " $$9 "  " $$5}'
	@echo "  → http://localhost:$(PORT)   (override with: make build-serve PORT=8082)"
	@echo ""
	python3 -m http.server $(PORT) --directory dist

# Publish — render the site set to static no-JS HTML (the legacy-web /
# CDN / permalink projection). Headless native, no browser: builds a peer,
# reads its sites off the tree ([A] reader), projects them to
# `dist/static-demo/sites/{peer}/{site}/…` ([B1] emitter). Override the
# output dir: `make publish OUT=path/to/dir`. OUT must stay UNDER the repo tree
# (the default is repo-relative): publish runs in-container with only the parent
# meta dir bind-mounted, so an absolute OUT like `/tmp/x` writes to the
# container's throwaway /tmp and the result never reaches the host. Today the source is a fresh
# peer seeded with the demo site set (a demo/SSG generator) — publishing a
# durable dedicated hosting peer's real tree is the deferred peer-source
# seam (src/content_site/publish.rs). Serve the result with `make serve`
# (open /static-demo/sites/<peer>/) or the printed python3 one-liner.
# Emits BOTH forms into one dir: legacy-web `.html` (sites/{peer}/…, no-JS)
# AND entity-native `.bin` content data (content/… + {peer}/sites/…, what a
# live peer ingests). Sites-scoped — never the whole peer tree.
#   INGEST=<dir>    source sites from a content-team render/ emit (disk→tree)
#                   instead of the bundled demo seed — one site dir, or a
#                   parent of site dirs (the render/output/sites/ layout).
#   PREFIX=<path>   the per-peer HOSTING SCOPE: nest everything (.html, .bin,
#                   deployment-config origin) under {OUT}/{PREFIX}/… so a domain
#                   can host many isolated peers side by side. Empty (default) =
#                   the domain root, byte-identical to the un-prefixed layout.
#                   Validated (no leading/trailing /, no .., not sites/content).
#   LIVE=<origin>   add the "open in live entity browser" banner ([F2]).
#   HTML_ONLY=1     skip the .bin content data (dumb-CDN-only).
#   DEPLOY_CONFIG=1 also emit /entity-deployment.json (cut 2b) so a GENERIC SPA
#                   bundle served from this origin boots into the published home
#                   site — no per-domain WASM rebuild. CONFIG_PROFILE=<full|
#                   tutorial|strict-site> (default tutorial), CONFIG_SITE=<id>
#                   (default demo). Origin = LIVE if set, else same-origin.
#                   full = chrome-first, no site overlay (apps-only); tutorial =
#                   site overlay, escapable; strict-site = locked kiosk. Details:
#                   docs/architecture/guides/GUIDE-DEPLOYMENT-AND-CONFIGURATION.md
#   IDENTITY_SEED=<64-hex>  publish under a SPECIFIC system identity (any
#                   `entity_system_seed`-form hex seed) so each site/deployment
#                   gets its own stable peer-id. Empty (default) = the fixed demo
#                   publisher identity (the first-push default). Bad seed fails.
# === Embedded apps (games + tools) ride EVERY full publish ===
# The JS-apps platform reads its catalog + bundles off the published tree exactly
# like sites. APPS_REPO = the entity-apps checkout; its build.py (re)bundles
# dist/<id>.html + index.json so newly-added apps flow through on each publish.
# The full-publish targets (publish-serve, publish-papers) REBUILD it fresh then
# ingest the WHOLE set via --ingest-apps. This low-level `publish` target ingests
# the dist if it already exists ($(wildcard) → empty = the bundled 2-app demo
# seed). Override APPS_REPO=<path>, or set APPS_DIST= (empty) to force the seed.
APPS_REPO ?= ../entity-apps
APPS_DIST  ?= $(wildcard $(APPS_REPO)/dist)
OUT ?= dist/static-demo
publish: image
	$(call RUN,cargo run --quiet --bin entity-browser -- publish $(OUT) $(if $(INGEST),--ingest=$(INGEST),) $(if $(APPS_DIST),--ingest-apps=$(APPS_DIST),) $(if $(PREFIX),--prefix=$(PREFIX),) $(if $(LIVE),--live=$(LIVE),) $(if $(HTML_ONLY),--html-only,) $(if $(DEPLOY_CONFIG),--deployment-config,) $(if $(CONFIG_PROFILE),--config-profile=$(CONFIG_PROFILE),) $(if $(CONFIG_SITE),--config-site=$(CONFIG_SITE),) $(if $(IDENTITY_SEED),--identity-seed=$(IDENTITY_SEED),))

# Bare-root SSG: render ONE site at the domain root (no sites/{peer}/{site}/
# prefix, no entity branding) — the "just a site generator" output. Pick the
# site with `SITE=<id>` (default: the demo site). Output dir = OUT (default
# `dist/static-bare`). Serve with `make serve`-style static server and open /.
OUT_BARE ?= dist/static-bare
publish-bare: image
	$(call RUN,cargo run --quiet --bin entity-browser -- publish $(OUT_BARE) --bare-root $(if $(SITE),--site=$(SITE),) $(if $(LIVE),--live=$(LIVE),))

# === THE standard "build everything fresh, publish, and serve" command ===
# One command, ONE origin, to test the whole round-trip end-to-end. It ALWAYS
# rebuilds the SPA first (no stale-bundle guessing — the #1 cause of "the root
# doesn't work / do I have an old build?"), publishes ALL sites (full
# projection, NOT bare) in BOTH forms (legacy-web `.html` + entity-native
# `.bin`) INTO `dist/` itself with the live banner pointed at the SPA, then
# serves `dist/` on a SINGLE port:
#
#   ▶ live entity browser (the SPA)  → http://localhost:PORT/              (default 8081)
#   ▶ static published sites         → http://localhost:PORT/sites/        (same origin)
#
# Same origin: the SPA owns `/`, the static projection owns `/sites/…` (its
# root-absolute links resolve), the `.bin` content data owns `/content/` +
# `/<peer>/`. Browse `/sites/` (lists the sites) → a static page carries the
# "open in live" banner → clicking it lands in the SPA at `/?site=self/…` (the
# static→live round-trip, same origin). The SPA overlay's "Live link" + "Static
# link" share controls round-trip the other way. Publishing into `dist/` writes
# only `sites/`, `content/`, `<peer>/` — it never touches the SPA's index.html
# or bundles.
#   PORT=<n>   the one serve + banner port (default 8081)
#   LIVE=<origin>  ADVANCED. Default EMPTY = same-origin: the published config
#                  and the static→live banner are RELATIVE, so the SAME `dist/`
#                  works at localhost here AND dropped on any CDN/R2 ROOT with no
#                  rebuild (portability is the default — we do NOT bake a domain).
#                  Set LIVE=https://host ONLY to pin a deliberate cross-origin
#                  banner. NEVER set LIVE=http://localhost for a bundle you ship
#                  (it serves a content-less shell off your machine — guarded).
PORT ?= 8081
LIVE ?=
# The serving targets (publish-serve / publish-papers) publish into an ISOLATED
# copy of the SPA bundle here — NOT the shared, git-ignored `dist/`. `make wasm`
# and `make e2e-worker` rebuild/republish `dist/` as a side effect, which would
# otherwise wipe a running deployment out from under you. SERVE_DIR lives under
# /tmp (fully outside the repo), so background work on `dist/` can never touch
# what you're serving. Override SERVE_DIR=<path under /tmp> to relocate.
SERVE_DIR ?= /tmp/entity-serve
# Snapshot the freshly-built `dist/` (the SPA bundle: index.html + wasm + js)
# into SERVE_DIR. SAFETY: the rm is constrained to a path UNDER /tmp via a
# shell case-guard — it can never touch the repo, '/', '$$HOME', or an empty
# value (same guard convention as PAPERS_RENDER_OUT below).
define snapshot_serve_dir
	@dir='$(SERVE_DIR)'; case "$$dir" in \
	  /tmp/?*) ;; \
	  *) echo "refusing SERVE_DIR='$$dir' (only /tmp/* is auto-cleaned; set SERVE_DIR to a path under /tmp)"; exit 1 ;; \
	esac
	@echo "==> snapshot fresh SPA ($(DIST)/) → isolated serve dir $(SERVE_DIR) (background builds can't clobber it)"
	@rm -rf "$(SERVE_DIR)"
	@mkdir -p "$(SERVE_DIR)"
	@cp -a $(DIST)/. "$(SERVE_DIR)/"
endef
# Output-isolated from the canonical dist/ + target/ so this can run
# CONCURRENTLY with `make tauri-run` (which keeps dist/ + target/). Override
# DIST=/TARGET_DIR= to relocate. See the DIST/TARGET_DIR header note.
publish-serve: DIST       := dist-publish
publish-serve: TARGET_DIR := target-publish
publish-serve: wasm
	$(snapshot_serve_dir)
	@APPS_FLAG=""; \
	if [ -d "$(APPS_REPO)" ]; then \
	  echo "==> rebuild embedded apps (games + tools) — $(APPS_REPO)"; \
	  python3 "$(APPS_REPO)/build.py"; \
	  APPS_FLAG="--ingest-apps=$(APPS_REPO)/dist"; \
	else \
	  echo "==> APPS_REPO '$(APPS_REPO)' absent — publishing the bundled demo app seed"; \
	fi; \
	CARGO_TARGET_DIR=$(TARGET_DIR) cargo run --quiet --bin entity-browser -- publish $(SERVE_DIR) --live=$(LIVE) $$APPS_FLAG
	@echo ""
	@echo "=== fresh build + published sites — serving on :$(PORT) (one origin, isolated $(SERVE_DIR)) ==="
	@echo "  ▶ live entity browser (SPA):  http://localhost:$(PORT)/"
	@echo "  ▶ static published sites:     http://localhost:$(PORT)/sites/   (banner → live)"
	@echo "  (hard-refresh once if an older build is cached)"
	@echo ""
	python3 -m http.server $(PORT) --directory $(SERVE_DIR)

# === Cross-team demo: papers render/ → our tree → live overlay + serve ===
# The whole loop in one command, in the STANDARD deployment shape (same origin
# as `publish-serve`): the live peer (WASM SPA) owns `/`, the static published
# site lives under `/sites/`, the entity-native tree data under `/content/` +
# `/<peer>/`. It builds the content team's `render/` engine, renders a site to a
# deterministic markdown emit, INGESTS that emit disk→tree
# (src/content_site/ingest.rs), then builds the SPA + publishes both forms INTO
# dist/ AND emits a per-domain `entity-deployment.json` pointing the generic SPA
# at the ingested site, then serves it on ONE origin.
#
# PORTABILITY (the key contract): the emitted config + banner are SAME-ORIGIN
# (relative), so the SAME `dist/` runs unchanged at localhost here AND dropped on
# any CDN/R2 served at the domain ROOT — DevOps just `aws s3 sync dist/ → bucket`,
# no rebuild, no domain to specify. CAVEAT: portable at the domain ROOT only; a
# SUBDIRECTORY deploy (`host/sub/`) is known, deliberate debt — see the
# publish-portability and origin-model analysis.
#   LIVE=<origin>  (advanced; default empty=same-origin) see publish-serve above.
#
#   ▶ live entity browser (SPA)   → http://localhost:PORT/            (raw domain)
#   ▶ static published site       → http://localhost:PORT/sites/      (same origin)
#
# The SPA at `/` now BOOTS INTO the ingested site as a **cache-backed foreign-
# site overlay**: on boot it fetches `/entity-deployment.json`, registers the
# published peer's same-origin origin, and the overlay resolves the site lazily
# over HTTP-poll from the served `.bin` content data (`src/content_site/
# http_poll.rs` → in-memory cache). The published peer-id ≠ the SPA's fresh boot
# peer, so this exercises the genuine remote/foreign-peer path, not a local seed.
# (This is the deployment-config / HTTP-poll mechanism — distinct from the
# ingest-INTO-the-live-root-peer move, which would make the site the SPA's OWN
# local content and is still a separate future step.)
#
#   CONFIG_PROFILE=<full|tutorial|strict-site>  the SPA's boot posture (default
#                        tutorial = boot INTO the site overlay but keep the
#                        chrome toggle, so the author can always pop out to
#                        inspect the peer/tree — never a one-way door). `full`
#                        boots to chrome with the overlay one toggle away;
#                        `strict-site` is the locked KIOSK posture (no toggle) —
#                        opt in explicitly, and use `?chrome=1` to escape it.
#   PAPERS_REPO=<path>   the entity-core-papers checkout (default: the sibling
#                        meta tree). PAPERS_SITE=<domain> picks the DOMAIN to
#                        publish — its WHOLE site set is rendered + ingested +
#                        browsable (the render tool moved to a domain/site model,
#                        per RENDER-TOOL-HANDOFF). Default billslab (11
#                        sites); other domains: entity-church-foundation,
#                        entity-core-protocol, entity-church-registry. Use
#                        PAPERS_SITE=all to publish every domain at once.
#   PAPERS_HOME=<id>     which site the SPA boots into (default {domain}-main,
#                        the domain's landing site; site ids are domain-prefixed).
#   PAPERS_RENDER_OUT    where their engine emits (default /tmp/papers-render).
#   PORT=<n>             the one serve port (default 8081).
# PAPERS_REPO points at a local entity-core-papers checkout that contains the
# content team's render/ engine; that repo is NOT part of this release (separate
# publishing concern). It is AUTO-DETECTED from generic relative candidates — the
# first one whose render/ engine actually exists wins — so publish-papers works
# whether this egui checkout sits beside papers (release/sibling layout) or two
# levels down in the meta tree (dev layout). Override explicitly via env or the
# gitignored caps.local.mk:  make publish-papers PAPERS_REPO=/path/to/checkout
_papers_candidates := ../entity-core-papers ../../[internal]/[internal]/entity-core-papers
PAPERS_REPO       ?= $(firstword $(foreach d,$(_papers_candidates),$(if $(wildcard $(d)/render),$(d))) ../entity-core-papers)
PAPERS_SITE       ?= billslab
# Which site the SPA boots into. Per-scope default is `<scope>-main`; but the
# whole-constellation `all` build has no single "all-main" site, so it defaults
# to the billslab landing (override PAPERS_HOME=<id> for another domain's home).
PAPERS_HOME       ?= $(if $(filter all,$(PAPERS_SITE)),billslab-main,$(PAPERS_SITE)-main)
PAPERS_RENDER_OUT ?= /tmp/papers-render
# --- Canonical-publish / ingest controls (entity-core-papers
#     docs/CONTENT-SOURCE-RENDER-RUNBOOK.md Step 6) ---
# PRERENDERED=<dir>: deploy a pre-rendered, scrubbed output dir produced by
#   the CONTENT TEAM (the validated "we render, they ingest" model). When set,
#   the render engine build + render step are SKIPPED and <dir> is ingested
#   directly — no Go toolchain, render engine, or pull-source repos needed here.
#   Default empty = render locally (demo/dev path).
PRERENDERED ?=
# SKIP_STAGE0=1: when rendering LOCALLY, bypass the release-builder Stage-0 scrub
#   (non-canonical — reads the working tree, no leak/date scrub). Default OFF
#   so a local render is canonical. (This was hardcoded ON; now opt-in — the
#   wrong default for a public publish. See CANONICAL-PUBLISH-STAGE0-HANDOFF.md.)
STAGE0_FLAG := $(if $(SKIP_STAGE0),--skip-stage0,)
# NO_SERVE=1: build the deploy bundle into SERVE_DIR and STOP — don't block on the
#   local http server. The bundle is then ready for `content-publish` → R2.
NO_SERVE ?=
# Dir to ingest. PRERENDERED wins outright. Otherwise the render tool nests a
# site/domain scope under $(PAPERS_RENDER_OUT)/<scope>, but `all` emits per-DOMAIN
# dirs directly under the output root — so for `all` ingest the root itself (the
# ingester recurses for every site.manifest.json at any depth, so the extra domain
# level is fine).
PAPERS_INGEST_DIR := $(if $(PRERENDERED),$(PRERENDERED),$(if $(filter all,$(PAPERS_SITE)),$(PAPERS_RENDER_OUT),$(PAPERS_RENDER_OUT)/$(PAPERS_SITE)))
CONFIG_PROFILE    ?= tutorial
# PREFLIGHT — publish-papers is the CROSS-TEAM papers->web demo flow and is NOT
# part of the release gate (see TOOLS.md §6). It needs host `go` + `python3` and
# the content team's render engine (PAPERS_REPO/render/), which is NOT committed
# to entity-core-papers master — it's out of release scope. This runs BEFORE the
# `wasm` prerequisite so a missing tool fails fast (no wasted SPA build), with one
# clear, actionable message naming exactly what's missing instead of a raw
# `go: chdir … no such file` / missing-tool crash.
publish-papers-preflight:
	@miss=""; \
	command -v python3 >/dev/null 2>&1 || miss="$$miss\n  - 'python3' — embedded-apps build + static serve"; \
	if [ -n '$(PRERENDERED)' ]; then \
	  [ -d "$(PRERENDERED)" ] || miss="$$miss\n  - PRERENDERED dir '$(PRERENDERED)' — the content team's pre-rendered, scrubbed output (ingest-only mode)"; \
	else \
	  command -v go >/dev/null 2>&1 || miss="$$miss\n  - 'go' toolchain — builds the papers render engine (or pass PRERENDERED=<dir> to ingest a pre-rendered output instead)"; \
	  [ -d "$(PAPERS_REPO)" ]        || miss="$$miss\n  - papers checkout at PAPERS_REPO='$(PAPERS_REPO)' — override PAPERS_REPO=<path>"; \
	  [ -d "$(PAPERS_REPO)/render" ] || miss="$$miss\n  - render engine dir '$(PAPERS_REPO)/render' — the content team's Go tool (NOT in entity-core-papers master; out of release scope)"; \
	fi; \
	if [ -n "$$miss" ]; then \
	  printf '\n>>> make publish-papers cannot run — missing:%b\n\n' "$$miss"; \
	  echo 'publish-papers is the cross-team papers->web flow.'; \
	  echo 'Canonical (the validated runbook): the content team renders the scrubbed'; \
	  echo 'export, then you ingest + deploy their output here — no Go/render engine needed:'; \
	  echo '    make publish-papers PRERENDERED=/path/to/cs-render NO_SERVE=1'; \
	  echo 'Local demo (renders here): needs host go + python3 + the render engine.'; \
	  echo 'See entity-core-papers/docs/CONTENT-SOURCE-RENDER-RUNBOOK.md + TOOLS.md §6.'; \
	  exit 1; \
	fi

# `publish-papers-preflight` is listed FIRST so (in serial make) it aborts before
# the wasm build when host tools / the render engine are absent.
publish-papers: DIST       := dist-publish
publish-papers: TARGET_DIR := target-publish
publish-papers: publish-papers-preflight wasm
	@# RENDER PHASE — skipped entirely in PRERENDERED ingest-only mode (the
	@# content team already rendered the scrubbed output). One shell so the
	@# build+clean+render either all run or all skip. SAFETY: the rm is constrained
	@# to a path UNDER /tmp via a case-guard — never '/', '$$HOME', empty, or
	@# anything outside /tmp; override PAPERS_RENDER_OUT elsewhere → you clean it.
	@if [ -n '$(PRERENDERED)' ]; then \
	  echo "==> ingest-only: deploying the content team's pre-rendered output '$(PRERENDERED)' (no render engine / Go / pull-sources)"; \
	  test -d '$(PRERENDERED)' || { echo "PRERENDERED dir '$(PRERENDERED)' not found"; exit 1; }; \
	else \
	  echo "==> build the papers render/ engine — $(PAPERS_REPO)/render"; \
	  go build -C $(PAPERS_REPO)/render -o render ./... || exit 1; \
	  echo "==> render scope '$(PAPERS_SITE)' → $(PAPERS_RENDER_OUT)/  (Stage-0 $(if $(SKIP_STAGE0),SKIPPED — non-canonical,ON — canonical))"; \
	  dir='$(PAPERS_RENDER_OUT)'; case "$$dir" in \
	    /tmp/?*) echo "    cleaning $$dir"; rm -rf -- "$$dir" ;; \
	    *) echo "refusing to auto-clean PAPERS_RENDER_OUT='$$dir' (only /tmp/* is auto-cleaned; remove it manually if intended)"; exit 1 ;; \
	  esac; \
	  ( cd $(PAPERS_REPO) && ./render/render --site $(PAPERS_SITE) --repo . $(STAGE0_FLAG) --output $(PAPERS_RENDER_OUT) ) || exit 1; \
	fi
	$(snapshot_serve_dir)
	@echo "==> ingest from $(PAPERS_INGEST_DIR)/ (disk→tree, recursive) + ALL embedded apps + publish both forms + deployment-config INTO $(SERVE_DIR)/ (SPA home='$(PAPERS_HOME)')$(if $(PREFIX), under prefix '$(PREFIX)',)"
	@APPS_FLAG=""; \
	if [ -d "$(APPS_REPO)" ]; then \
	  echo "==> rebuild embedded apps (games + tools) — $(APPS_REPO)"; \
	  python3 "$(APPS_REPO)/build.py"; \
	  APPS_FLAG="--ingest-apps=$(APPS_REPO)/dist"; \
	else \
	  echo "==> APPS_REPO '$(APPS_REPO)' absent — publishing the bundled demo app seed"; \
	fi; \
	CARGO_TARGET_DIR=$(TARGET_DIR) cargo run --quiet --bin entity-browser -- publish $(SERVE_DIR) --ingest=$(PAPERS_INGEST_DIR) $$APPS_FLAG --deployment-config --config-site=$(PAPERS_HOME) --config-profile=$(CONFIG_PROFILE) --live=$(LIVE) $(if $(PREFIX),--prefix=$(PREFIX),) $(if $(IDENTITY_SEED),--identity-seed=$(IDENTITY_SEED),)
	@echo ""
	@if [ -n '$(NO_SERVE)' ]; then \
	  echo "=== NO_SERVE=1 → deploy bundle BUILT (not serving) ==="; \
	  echo "  bundle: $(SERVE_DIR)"; \
	  echo "  deploy: content-publish <bucket> $(SERVE_DIR) '' --key-var R2_CONTENT_<DOMAIN>_PROD"; \
	else \
	  echo "=== serving on :$(PORT) (one origin, isolated $(SERVE_DIR)) ==="; \
	  echo "  ▶ live entity browser (SPA):       http://localhost:$(PORT)/   (boots into '$(PAPERS_HOME)' overlay, posture=$(CONFIG_PROFILE); the Content Site window lists every site in the domain)"; \
	  echo "  ▶ static published '$(PAPERS_SITE)':  http://localhost:$(PORT)$(if $(PREFIX),/$(PREFIX),)/sites/"; \
	  echo "  (hard-refresh once if an older build is cached)"; \
	  echo ""; \
	  python3 -m http.server $(PORT) --directory $(SERVE_DIR); \
	fi

.PHONY: native test lint wasm wasm-release wasm-measurement e2e-worker tauri tauri-run serve build-serve check-dist publish publish-bare publish-serve publish-papers publish-papers-preflight
