# Publish ingest format — the tool-agnostic site contract

**Read this when** you want to publish a content site **without** the papers
`render/` engine — by hand, from a script, or from any other generator — or when
you are planning the release-time removal of the papers-specific wiring.

The publish pipeline (`entity-browser publish --ingest=<dir>`) consumes a plain
**on-disk directory format**. Nothing about that format is papers-specific: the
papers `render/` tool is merely *one producer* of it. Any process that lays down
the directory shape below feeds the pipeline identically. This was verified by
hand-authoring a site (no renderer) and publishing it — see §4.

Consumer: `src/content_site/ingest.rs` (`ingest_path`). The format notes here are
the authoring-facing view of that module's contract — it is the source of truth.

---

## 1. Directory format

You hand `--ingest=<root>` a directory. The ingester **recursively scans** for
every directory that contains a `site.manifest.json` and treats each as one site
(a site dir is a leaf — its `pages/`/`assets/` are content, never sub-sites). So a
single `--ingest=<root>` works whether `<root>` is one site dir or a parent of
many (at any depth — `domains/<domain>/<site>/`, a flat `sites/<id>/`, etc.).

```text
<root>/                         # the --ingest target (scanned recursively)
  <anything>/<site-dir>/        # any nesting; a "site dir" = one with a manifest
    site.manifest.json          # REQUIRED — the site descriptor (see §2)
    pages/                       # REQUIRED to have content — one .md = one page (see §3)
      index.md
      guide/intro.md
    assets/                      # OPTIONAL — images/files, content-addressed (see §4-assets)
      figures/dot.svg
    run-manifest.json           # OPTIONAL — IGNORED (producer provenance; not read)
```

Anything the scan doesn't recognize (stray dirs, `constellation.manifest.json`,
`run-manifest.json`) is skipped, not an error.

---

## 2. `site.manifest.json`

Plain JSON. Only `site_id` is strictly required.

```json
{
  "site_id": "hello",
  "title": "Hello World",
  "tagline": "optional cover subtitle",
  "theme": "optional theme name",
  "nav": [
    { "title": "Home",  "path": "pages/index.md" },
    { "title": "Guide", "path": "pages/guide/intro.md", "children": [
      { "title": "Intro", "path": "pages/guide/intro.md" }
    ]},
    { "title": "Papers", "path": "", "children": [ /* group header — see below */ ] }
  ]
}
```

| Field | Required | Meaning |
|---|---|---|
| `site_id` | **yes** | Stable site identity (also the URL segment). **Must be globally unique** across everything published into one peer — use a domain prefix (`billslab-research`) when publishing many sites together. Empty/missing → hard error. |
| `title` | no | Display title. Defaults to `site_id`. |
| `tagline` | no | Cover subtitle. Stored in the manifest params bag. |
| `theme` | no | Theme name. Stored in the manifest params bag. |
| `nav` | no | Site menu tree. Each node: `title`, `path`, optional `children[]`. |

**`nav[].path`** is an emitted page path (`pages/research/index.md`); it is
projected to an in-site **root-absolute** link (`/research/index`) so the menu
resolves identically from any page. A node with `path: ""` is a **group header**
(no page of its own) — it lands on the longest common directory of its children
(`/papers`), not the site root.

The **landing page** is not set in the manifest: the ingester picks `index` if a
`pages/index.md` exists, else the first page (sorted).

---

## 3. Pages — `pages/**/*.md`

Every `*.md` file under `pages/` is one page.

- **Slug** = the path under `pages/`, with `.md` stripped, slash-separated.
  `pages/guide/intro.md` → slug `guide/intro`. Nesting is preserved.
- **Frontmatter** (optional): a leading `+++ … +++` block of **TOML**. `title` is
  lifted to the page title; every other key is carried as page frontmatter (so
  producer metadata like `content_class`, `source`, `recipe`, `status` survives).
  A file with no frontmatter is all body.
- **Body**: markdown. Image syntax is normalized to one standard at ingest:
  `![alt](src)` becomes `::embed[alt]{ref=src}`; an existing `::embed[…]{ref=…}`
  is left as-is. Reference assets as `assets/<name>` to bind staged bytes (§4).

```markdown
+++
title = "Home"
content_class = "authored"
+++

# Hello

A paragraph. ![a dot](assets/figures/dot.svg)
```

---

## 4. Assets — `assets/**` (optional)

Every file under `assets/` is ingested as a **content-addressed** asset (identical
bytes dedupe across sites). The asset **name** is its path under `assets/`
(`figures/dot.svg`) — i.e. the suffix of an embed `ref` after the `assets/`
prefix, so a body's `::embed{ref=assets/figures/dot.svg}` binds the staged bytes.

- Media type is inferred from the extension: `png jpg jpeg gif svg webp avif bmp
  ico` → the matching `image/*`; anything else → `application/octet-stream`.
- Files ending in `.placeholder` are skipped (producer's "pinned-but-absent" flag).
- **Known gap**: an embed `ref` that points *outside* the site dir (e.g.
  `![](../../output/x.png)`) has no file under `assets/` to stage — it stays an
  unresolved embed. Producers must stage referenced bytes into the site's
  `assets/`.

### Verified minimal example (no renderer)

This exact tree was published with `entity-browser publish --ingest=/tmp/handsite`
→ "ingested 1 site(s) … published 1 site(s), 3 page(s)", HTML + `.bin` emitted,
the SVG staged and the body rendered:

```text
/tmp/handsite/mysite/
  site.manifest.json     {"site_id":"hello","title":"Hello World","nav":[…]}
  pages/index.md         +++\ntitle = "Home"\n+++\n\n# Hello\n…![dot](assets/figures/dot.svg)
  pages/guide/intro.md   +++\ntitle = "Intro"\n+++\n\n## Guide intro\n
  assets/figures/dot.svg <svg …/>
```

---

## 5. Running it (no papers repo needed)

The low-level `publish` target already accepts a generic `INGEST=<dir>`:

```bash
# OUT and INGEST must live UNDER the repo tree — publish runs in-container with
# only the parent meta dir bind-mounted, so an absolute /tmp path writes to the
# container's throwaway /tmp and the result never reaches the host.
make publish INGEST=path/to/your/site-root OUT=dist/my-site
```

Or call the binary directly on the host (native build):

```bash
cargo run --bin entity-browser -- publish dist/my-site --ingest=path/to/site-root
```

Useful flags (full list in TOOLS.md §4): `--live=<origin>` (live banner),
`--prefix=<path>` (multi-tenant hosting scope), `--html-only` (skip `.bin`),
`--deployment-config` + `--config-site=<id>` + `--config-profile=<…>` (boot a
generic SPA into the published home), `--bare-root --site=<id>` (single site at
the domain root, no entity branding).

Without `--ingest`, `publish` emits a **bundled demo site set** (a built-in
demo/SSG generator) — handy for testing the pipeline with zero inputs.

---

## 6. Publish-pipeline edge-case audit

Status of the fragility classes in the publish targets, prompted by the
`PAPERS_REPO`-path break:

| Edge case | Status |
|---|---|
| **`PAPERS_REPO` hardcoded to a path without `render/`** | **FIXED** — `Makefile` now auto-detects the papers checkout (first candidate whose `render/` exists: sibling layout, then meta-nested layout); env/`caps.local.mk` override still wins. Was broken by a prior release-prep "leak scrub" that repointed it to the bare sibling. |
| **`publish-papers` crashes raw when tools/render absent** | **FIXED** — `publish-papers-preflight` runs *before* the `wasm` build and fails fast with an actionable message (missing `go`/`python3`/`PAPERS_REPO`/`render`), pointing at `make publish` / `make publish-serve`. |
| **`APPS_REPO` hardcoded sibling (`../entity-apps`)** | **OK / graceful** — same path-assumption class, but degrades to the bundled app seed when absent (`if [ -d … ]`), and exists as a real sibling here. Latent: if `APPS_REPO` exists but `python3`/`build.py` is missing, the `build.py` step in `publish-serve`/`publish-papers` fails hard (unguarded). Low risk; `python3` is near-universal. |
| **Absolute `OUT=/tmp/x` vanishes** | **DOCUMENTED** — in-container publish only persists paths under the mounted repo tree; an absolute `/tmp` OUT writes the container's throwaway `/tmp`. Caveat added at the `OUT` description; default OUT is repo-relative. |
| **Host-tool assumptions in `publish-papers`** | **BY DESIGN** — `publish-papers` runs on the host (its `cargo run` is a bare call, not `$(call RUN,…)`), chaining host `go`→`cargo`→`python3` via `/tmp` and ending in a host server. It is explicitly outside the bare-box podman gate (Makefile header). The release publish targets (`publish`, `publish-bare`) ARE containerized and pure-cargo. |
| **Malformed / missing manifest** | **CLEAR ERRORS** — missing `site_id` → `"… : missing site_id"`; no manifest anywhere under the ingest root → `"no site.manifest.json at … or anywhere below it"`. |

---

## 7. Release generalization plan (cutover)

The papers repo is **not shipped** in this release. The pipeline is already split
into a generic core and a papers-specific convenience wrapper; at cutover, remove
the latter.

**KEEP — generic, ships, tool-agnostic:**
- `entity-browser publish` + `--ingest=<dir>` and all the projection flags.
- `src/content_site/ingest.rs` and this format. Producer-agnostic by construction.
- `make publish INGEST=<dir>` / `make publish-bare` — the generic entry points.
- This document + TOOLS.md §4.

**RIP OUT / GENERALIZE — papers-specific, do at cutover:**
- The `publish-papers` Makefile target, the `publish-papers-preflight` target, and
  every `PAPERS_*` variable (`PAPERS_REPO` incl. its meta-tree candidate path,
  `PAPERS_SITE`, `PAPERS_HOME`, `PAPERS_RENDER_OUT`, `PAPERS_INGEST_DIR`).
- The `go build … render` + `./render/render …` steps (the content team's engine
  is theirs, not ours).
- Replace with a short "publish your own content" pointer to §5 here. A generic
  `make publish INGEST=<dir>` + `make serve` covers the demo flow with no external
  repo. The render→ingest→publish→serve convenience can become an example script
  shipped in `tools/`, parameterized on `INGEST`, not on a papers checkout.
- Re-check the `APPS_REPO`/`--ingest-apps` games/apps default: if `entity-apps` is
  also out of release scope, make the embedded-apps ingest opt-in rather than the
  silent default, so a clean checkout publishes content-only.

**Note for the leak scrub:** the `[internal]` candidate path in
`PAPERS_REPO` exists only to make the dogfood flow work from the release-prep
checkout. It disappears entirely with the `publish-papers` removal above, so the
scrub and the generalization are the same cutover action — do them together.
