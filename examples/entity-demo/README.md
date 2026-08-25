# entity-demo — an example content domain

A worked example of the **publish ingest format** (the tool-agnostic site
contract, `docs/architecture/guides/PUBLISH-INGEST-FORMAT.md`). It is also the
app's bundled demo, so the demo dogfoods the exact directory an external author
would hand the pipeline. Modeled on the canonical fixture
`entity-core-papers/render/examples/billslab-slice/`.

## Layout — a domain, with sites beneath it

```text
entity-demo/                     # a DOMAIN = the ingest root (a set of sites)
  demo/                          # one SITE
    site.manifest.json           # the curated nav "cover" (site_id + title + nav)
    pages/**/*.md                # one .md = one page; slug = path minus .md
    assets/**                    # content-addressed; referenced as assets/<name>
  info/                          # a SECOND site in the same domain
    site.manifest.json
    pages/**/*.md
```

`--ingest` scans **recursively** for every `site.manifest.json`, so pointing it
at this domain dir ingests **both** sites at once. Each page may open with a
`+++ … +++` TOML frontmatter block (`title` becomes the page title). Images
written `![alt](assets/…)` normalize to `::embed[alt]{ref=assets/…}` at ingest.

## Ingest vs Publish — two different operations

- **Ingest** = load this directory of markdown *into a tree* (a peer's tree).
  That's all it does — populate a tree. It does **not** build HTML or a
  distribution. (The app also ingests a small seed into the WASM runtime on boot
  so the site browser has something with no server — a separate use of ingest.)
- **Publish** = take a tree and **project it to a static site under a peer id**:
  `sites/{peer}/{site}/*.html` (legacy-web/CDN) **and** the entity-native `.bin`
  content data (what the live app resolves) into an output dir → deploy to R2/CDN.

`publish --ingest=<dir>` chains them: ingest this dir into a fresh peer, then
publish that peer's tree. They are distinct steps.

## Publish it

```bash
# Both sites → static HTML + .bin distribution, then serve:
make site INGEST=examples/entity-demo OUT=dist/demo
make serve                       # → http://localhost:8081/sites/

# Full build + publish + serve on one origin:
make site-serve INGEST=examples/entity-demo
```

Point `INGEST` at any directory shaped like this — that is the
"give me a folder of markdown, I'll publish the sites" tool.
