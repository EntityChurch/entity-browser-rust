# entity-demo — an example publisher: two sites and a feed

A worked example of the **publish ingest format** (the tool-agnostic site
contract, `docs/architecture/guides/PUBLISH-INGEST-FORMAT.md`). It is also the
app's bundled demo, so the demo dogfoods the exact directory an external author
would hand the pipeline. Modeled on the canonical fixture
`entity-core-papers/render/examples/billslab-slice/`.

## Layout — a publisher, with two kinds of thing beneath it

```text
entity-demo/                     # the authoring root
  demo/                          # one SITE
    site.manifest.json           # the curated nav "cover" (site_id + title + nav)
    pages/**/*.md                # one .md = one page; slug = path minus .md
    assets/**                    # content-addressed; referenced as assets/<name>
  info/                          # a SECOND site, same publisher
    site.manifest.json
    pages/**/*.md
  feed/                          # this publisher's FEED — a different convention
    *.md                         # one .md = one post; `created_at` REQUIRED
```

`--ingest` scans **recursively** for every `site.manifest.json`, so pointing it
at this root ingests **both** sites at once — and ignores `feed/`, which has no
manifest. Each page may open with a `+++ … +++` TOML frontmatter block (`title`
becomes the page title). Images written `![alt](assets/…)` normalize to
`::embed[alt]{ref=assets/…}` at ingest.

## The feed is a separate axis, not a third site

`feed/` is `APP-CONVENTION-FEED`, not `APP-CONVENTION-SEMANTIC-CONTENT-SITE`:
different entity types, a different reader, its own `--ingest-feed=<dir>` flag.
What it is **not** is a different publisher, a different identity, a different
signed root or a different transport — one publish run reads every axis out of
the one tree and signs one root over all of it.

That is the thing worth seeing in this directory: **two application-tier
conventions under one peer, over one wire.**

```text
feed/<anything>.md               # discovered recursively, sorted; the filename
                                 # is not read — ordering comes from created_at
```

```toml
+++
title = "A post"                 # optional. Absent ⇒ the body's first non-empty
                                 #   line becomes the reader-visible fallback
created_at = 2026-09-04T16:20:00Z   # REQUIRED, and refused if absent
+++
Markdown body.
```

**`created_at` has no default, deliberately.** The obvious one is the file's
mtime, and `git clone` rewrites mtimes — so the same posts would produce
different entities, different hashes and a different archive on every machine
that checked them out. A post with no date has no correct answer, so it is
refused where the person who can fix it is standing. Any other string key in the
frontmatter is carried into the body's open attribute bag rather than dropped.

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

# Sites AND the feed, one publish, one signed root:
make site INGEST=examples/entity-demo FEED=examples/entity-demo/feed OUT=dist/demo

# Full build + publish + serve on one origin — the app boots against it, and its
# Feed window can be pointed at this publisher's peer id:
make site-serve INGEST=examples/entity-demo FEED=examples/entity-demo/feed
```

Point `INGEST` at any directory shaped like this — that is the
"give me a folder of markdown, I'll publish the sites" tool. `FEED` is the same
deal one convention over.

**Both variables are staged**, so they may point anywhere on the host: the
containerized publish bind-mounts only this repo, and a path outside it would be
invisible inside. You do not copy anything by hand.

## Reading it back in a browser

Publishing a feed and *seeing* it are two steps, because a reader follows a
publisher by peer id rather than by origin. After `make site-serve … FEED=…`:

1. the closing output names the peer id the publish signed under;
2. open the app at `http://localhost:8081`, open the **Feed** window;
3. paste that peer id and follow it.

The origin comes from the deployment document's `origins` map, which the publish
wrote — which is why a publisher you have no route to renders *"no route"* rather
than *"this publisher has posted nothing"*. Those are different facts and the
surface keeps them apart.
