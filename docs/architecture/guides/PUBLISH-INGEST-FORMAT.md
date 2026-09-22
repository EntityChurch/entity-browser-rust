# Publish ingest format — the tool-agnostic site contract

> **The publishing model lives in one place:** [`REFERENCE-PUBLISHING-PIPELINE.md` §0.1](../specs/REFERENCE-PUBLISHING-PIPELINE.md#01-the-model-in-one-screen--read-this-before-changing-anything-that-publishes) — *publish translates an input into the tree, then projects the tree.* This document covers the on-disk ingest format; it does not restate the model.

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
| `theme` | no | The site's own theme — a **registered app theme name** (`"dark"`, `"light"`; every future registered theme works automatically). Applied when the reader's Site-appearance mode is *Site's theme* (the default), frozen into static exports, and always overridden by the reader's explicit appearance choice. An unknown name is ignored with a loud warning and the site renders with the default palette — the app owns every CSS byte; site-supplied CSS is not accepted. |
| `nav` | no | Site menu tree. Each node: `title`, `path`, optional `children[]`. |

**`nav[].path`** is an emitted page path (`pages/research/index.md`); it is
projected to an in-site **root-absolute** link (`/research/index`) so the menu
resolves identically from any page. A node with `path: ""` is a **group header**
(no page of its own) — it lands on the longest common directory of its children
(`/papers`), not the site root.

The **landing page** is not set in the manifest: the ingester picks `index` if a
`pages/index.md` exists, else the first page (sorted).

---

## 3. Pages — `pages/**/*.{md,html}`

Every `*.md` **or `*.html`** file under `pages/` is one page. The two may sit
side by side in one site; `.md` is the universal base, `.html` the web tier for
**pre-rendered documents** (§3a).

- **Slug** = the path under `pages/`, with the extension stripped,
  slash-separated. `pages/guide/intro.md` → slug `guide/intro`. Nesting is
  preserved.
- **One source per slug.** `about.md` beside `about.html` is **refused**, naming
  both files — there is no correct pick between them, and silently choosing one
  would publish the file the author did not mean while leaving no trace of the
  other. Rename one.
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

## 3a. Document pages — `pages/**/*.html`

A `.html` page is a **complete, pre-rendered HTML document** — a Pandoc paper or
book, an exported report — carried into the tree **byte-for-byte**. This is the
base format the content-site convention §3.1 permits at the web tier.

- **Nothing is transformed.** No frontmatter stripping, no `::embed`
  normalization, no sanitizing pass. Every one of those would corrupt a
  standalone document, and the safety boundary is not here (below).
- **Title** comes from the document's first `<title>` element, entity-decoded.
  No `<title>` → the key is left unset and the slug-humanizing fallback names
  the page, exactly as for a markdown file with no frontmatter title.
- **It renders in `<iframe sandbox="allow-same-origin">`, loaded from the
  document's own `blob:` URL.** That is where the safety lives, which is why
  ingest does not rewrite anything. `allow-same-origin` is the *only* token
  granted, and it grants an origin to a document with **no way to use one** —
  scripts do not run, so nothing in the frame can read a cookie, touch storage,
  or reach our DOM. (The frame needs a base URL of its own or the document's
  internal anchors resolve against *our* page and navigate away from the paper;
  `srcdoc` was tried and broke every anchor, and a `data:` URL is capped at
  2 MiB in Chrome, which is under one corpus book. See
  `specs/REFERENCE-CONTENT-SITE-APP.md` §4.3 for the measured delivery matrix.)
  Consequences to design for:
  - **Scripts do not run.** A document depending on a CDN (MathJax, a
    highlighter) shows its un-processed source instead. Emit self-contained
    output — for math, pandoc `--mathml`.
  - **External resources do not load.** Use `--embed-resources --standalone` so
    figures ride as `data:` URLs; a relative path out of the artifact
    (`../../output/figures/x.png`) is broken here for the same reason it is
    broken on a static host.
  - **An external link REPLACES the document** unless you mark it. A sandbox
    blocks *top-level* navigation, but navigating the frame itself is never
    sandboxed — so `<a href="https://example.com/">` with no `target` loads that
    site in place of the paper. Emit `target="_blank" rel="noopener"` on external
    links; we currently block `_blank` (no `allow-popups`), so such a link is
    inert, which is the safe half of the trade. **A citation with no `target` is
    the one that loses the reader's place.** Note that a pandoc `--lua-filter`
    cannot do this when the links are generated by `--citeproc`, which runs
    *after* filters — it has to be a post-process.
  - **The document's own CSS applies** and our theme does not reach it. It
    supplies all its own typography and page furniture.
- **Static export** emits a document page verbatim, skipping the export
  template — so it carries no site nav in the static projection.

```text
pages/papers/paper-00.html    <!DOCTYPE html><html><head><title>Paper 0 …
site.manifest.json            { "nav": [ { "title": "Paper 0",
                                           "path": "pages/papers/paper-00.html" } ] }
```

A `nav[].path` naming a `.html` page projects to the same slug the page is
stored under (`/papers/paper-00`) — the extension is stripped exactly as `.md`
is.

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

The low-level `site` target already accepts a generic `INGEST=<dir>`:

```bash
# OUT and INGEST must live UNDER the repo tree — publish runs in-container with
# only the parent meta dir bind-mounted, so an absolute /tmp path writes to the
# container's throwaway /tmp and the result never reaches the host.
make site INGEST=path/to/your/site-root OUT=dist/my-site
```

Or call the binary directly on the host (native build):

```bash
cargo run --bin entity-browser -- publish dist/my-site --ingest=path/to/site-root
```

Useful flags (full list in TOOLS.md §4): `--live=<origin>` (live banner),
`--prefix=<path>` (multi-tenant hosting scope), `--html-only` (skip `.bin`),
`--deployment-config` + `--config-site=<id>` + `--surface=<…>` (boot a generic
SPA into the published home), `--bare-root --site=<id>` (single site at the
domain root, no entity branding).

Without `--ingest`, `publish` emits a **bundled demo site set** (a built-in
demo/SSG generator) — handy for testing the pipeline with zero inputs.

---

## 6. Publish-pipeline edge-case audit

Status of the fragility classes in the publish targets, prompted by the
`PAPERS_REPO`-path break:

| Edge case | Status |
|---|---|
| **`PAPERS_REPO` hardcoded to a path without `render/`** | **FIXED** — `Makefile` now auto-detects the papers checkout (first candidate whose `render/` exists: sibling layout, then meta-nested layout); env/`caps.local.mk` override still wins. Was broken by a prior release-prep "leak scrub" that repointed it to the bare sibling. |
| **`publish-papers` crashes raw when tools/render absent** | **FIXED** — `publish-papers-preflight` runs *before* the `wasm` build and fails fast with an actionable message (missing `go`/`python3`/`PAPERS_REPO`/`render`), pointing at `make site` / `make site-serve`. |
| **`APPS_REPO` hardcoded sibling (`../entity-apps`)** | **OK / graceful** — same path-assumption class, but degrades to the bundled app seed when absent (`if [ -d … ]`), and exists as a real sibling here. Latent: if `APPS_REPO` exists but `python3`/`build.py` is missing, the `build.py` step in `site-serve`/`publish-papers` fails hard (unguarded). Low risk; `python3` is near-universal. |
| **Absolute `OUT=/tmp/x` vanishes** | **DOCUMENTED** — in-container publish only persists paths under the mounted repo tree; an absolute `/tmp` OUT writes the container's throwaway `/tmp`. Caveat added at the `OUT` description; default OUT is repo-relative. |
| **Host-tool assumptions in `publish-papers`** | **BY DESIGN** — `publish-papers` runs on the host (its `cargo run` is a bare call, not `$(call RUN,…)`), chaining host `go`→`cargo`→`python3` via `/tmp` and ending in a host server. It is explicitly outside the bare-box podman gate (Makefile header). The release publish targets (`publish`, `site-bare`) ARE containerized and pure-cargo. |
| **Malformed / missing manifest** | **CLEAR ERRORS** — missing `site_id` → `"… : missing site_id"`; no manifest anywhere under the ingest root → `"no site.manifest.json at … or anywhere below it"`. |

---

## 7. Release generalization (cutover — DONE)

The papers repo is **not shipped**. The pipeline was split into a generic core and
a papers-specific convenience wrapper; the wrapper has now been **removed** and the
generic core is the only path. What the cutover did:

**KEPT — generic, ships, tool-agnostic:**
- `entity-browser publish` + `--ingest=<dir>` and all the projection flags.
- `src/content_site/ingest.rs` and this format. Producer-agnostic by construction.
- `make site INGEST=<dir>` / `make site-bare` — the generic entry points.
- This document + TOOLS.md §4 + the worked example `examples/demo-site/`.

**REMOVED — papers-specific:**
- The `publish-papers` + `publish-papers-preflight` Makefile targets and every
  `PAPERS_*` / `PRERENDERED` / `SKIP_STAGE0` / `NO_SERVE` variable, plus the
  `go build … render` + `./render/render …` steps (the content team's engine is
  theirs). To publish your own content, use `make site INGEST=<dir>` (or
  `make site-serve INGEST=<dir>`) per §5 — no external repo.

**Apps — standardized, kept in scope:** the embedded-apps ingest is now driven by
a single `APPS_DIST=<dir>` parameter pointing at a **pre-built** entity-apps `dist/`
(the pipeline consumes it; building it is the app repo's concern — no `build.py`
step, no `APPS_REPO` checkout assumption). Empty = the bundled demo app seed.

**Leak scrub:** the `[internal]` candidate path formerly in `PAPERS_REPO` is gone
with the `publish-papers` removal — the scrub and the generalization were the same
cutover action, done together.
