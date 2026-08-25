# Build Flow Review — make + podman build pipeline

**Status:** Living review. Captures the current build pipeline design and the
open cleanups we intend to do later. Update as items land.
**Trigger:** The make+podman flows were re-derived from scratch on a fresh box
and the slow/colliding behavior surfaced: every build re-downloaded
`wasm-bindgen`, and two concurrent builds (`make tauri-run` + `make
publish-serve`) stepped on each other's `dist/` and serialized on `target/`
locks.

> Canonical command surface lives in [`../guides/TOOLS.md`](../guides/TOOLS.md)
> (every target, variable, knob). This doc is the **rationale + open-work
> tracker** for the build *pipeline* itself — not a target reference.

## Design as it stands

The build interface is `make` over a single podman toolchain image
(`Dockerfile`: Rust 1.94.1 + wasm32 + Trunk 0.21.14 + binaryen-119 +
webkit2gtk). A bare host needs only `make` + `podman`. Source is bind-mounted;
the meta parent dir is mounted at `/src/entity-systems` so sibling workspace
deps (`entity-core-rust`) resolve.

### Caching (shared, persistent — populated once)
- **cargo registry** → `$HOME/.cache/cargo-entity-browser` ↦ `/usr/local/cargo/registry`.
- **trunk tool cache** → `$HOME/.cache/cargo-entity-browser-trunk` ↦ `/root/.cache`.
  This is what stops the per-build `wasm-bindgen` re-download: Trunk fetches its
  version-matched `wasm-bindgen-cli` into `/root/.cache/trunk`, which used to be
  thrown away every `podman run --rm`. `wasm-opt` is baked into the image (not
  downloaded). The Dockerfile deliberately does NOT bake `wasm-bindgen` (avoids
  a per-bump image rebuild); the persistent cache gives the same "download once"
  without that maintenance.

### Output isolation (so builds run in parallel)
`DIST` (trunk `--dist`) and `TARGET_DIR` (`CARGO_TARGET_DIR`) are parameterized,
defaulting to the canonical `dist`/`target`. The `publish-*` family overrides
them to `dist-publish`/`target-publish` (target-specific make vars that
propagate into their `wasm` prerequisite and the host-side `cargo run`). Net:

| Flow | DIST | TARGET_DIR | Notes |
|---|---|---|---|
| `tauri-run` (basic Tauri native run) | `dist` | `target` | `src-tauri` embeds `../dist`, so it must use `dist`. |
| `publish-serve` (normal local startup) | `dist-publish` | `target-publish` | snapshots to `/tmp/entity-serve`, serves :8081. |
| `publish-papers` | `dist-publish` | `target-publish` | = publish-serve + a papers render→ingest first (needs host `go` + render engine). |
| `wasm` / `wasm-release` / `build-serve` / `serve` / `e2e-worker` | `dist` | `target` | the canonical pair. |

`tauri-run` ↔ `publish-serve`/`publish-papers` now share **zero output** → safe
concurrently. They still share the (read-mostly, safely-locked) caches.

### SELinux volume labels — `:z`, NOT `:Z` (required for concurrency)
On SELinux-enforcing hosts (Fedora Kinoite/Silverblue here), the bind mounts use
**`:z`** (lowercase = *shared* label), not `:Z` (uppercase = *private, per-container*
category). With `:Z`, two concurrent build containers each relabel the shared
source tree to their **own** private category pair (e.g. `c57,c486`), which locks
the other container out → `Permission denied` (`failed to create directory …/target`)
mid-build — even with separate target/dist dirs, because the **source** mount is
shared. `:z` gives every container the same shared label, so concurrent access
works; it's also faster on repeat builds (a stable label means podman skips the
recursive relabel that `:Z`'s ever-changing category forced every run). This bit
us once: output isolation alone wasn't enough until the source/cache mounts moved
to `:z`.

## Open cleanups (the "later" list)

1. **Host vs. container cargo on the same target dir.** `publish-serve` /
   `publish-papers` / `publish` / `publish-bare` run `cargo run` on the **host**
   while the `wasm` step runs in the **container** — different rustc, same
   target tree → occasional full recompiles (host-triple build artifacts
   invalidate each other). Output isolation moved the publish family to
   `target-publish`, which contains the blast radius but does not eliminate the
   host/container split within that flow. Decide: run the publish CLI
   *in-container* too (consistent toolchain), or keep host but accept the churn.
2. **`serve` / `build-serve` / `publish` / `publish-bare` are not output-isolated.**
   They still use `dist`/`target` (host), so they can't run alongside
   `tauri-run`. Fine for the current workflow; isolate the same way if a
   concurrent serve-next-to-tauri need arises.
3. **Publish flow optimization / cleanup** (operator-flagged). The whole
   publish pipeline (snapshot → apps rebuild → render → ingest → publish →
   serve) is to be reviewed and streamlined once papers/apps are wired back in.
   Today with no `APPS_REPO`/papers present, `publish-serve` publishes the
   empty/demo seed — which is the intended normal startup.
4. **First `publish-serve` after isolation is a cold compile** into
   `target-publish` (deps from the shared cache, so it's the crate's own code
   only). One-time; warm thereafter. A `cp -a target target-publish` seed would
   skip even that if it ever matters.

## Verification
- Defaults preserved: `make -n {wasm,tauri,test}` show `CARGO_TARGET_DIR=target`,
  `--dist dist`, and the `tauri` binary still at `src-tauri/target/debug/entity-browser-tauri`.
- Concurrency: run `make tauri-run` and `make publish-serve` simultaneously →
  no "Blocking waiting for file lock on artifact directory" between them, no
  `dist/` clobber. (Not yet exercised end-to-end on this box — pending.)
