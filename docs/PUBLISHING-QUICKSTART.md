# Publish a site — the quickstart

**Audience:** you have found this project, you want to put your own content on
your own domain, and you want the commands. **Scope:** identity → content →
build → verify → upload → check → republish, end to end, with the CDN
configuration that actually matters.

> **The one-paragraph version.** You mint a **publisher identity** (a 32-byte
> seed you keep, like an SSH key). You point the publisher at a directory of
> markdown. It emits a single static tree: the app at the apex, your content
> beside it, all content-addressed. You upload that tree to any static host —
> S3, R2, a plain nginx — with **two serving properties set correctly** (CORS,
> and cache headers keyed on mutability). That is the whole deploy. There is no
> server to run, no database, and nothing dynamic.

Everything here was run against `dev` on 2026-08-24. Where something is
unverified, it says so.

---

## 0. What you need — and what the installer does *not* give you

| You want to | You need |
|---|---|
| **Run** the app (desktop) | a release installer — `.deb` / `.rpm` / `.AppImage` / `.msi` / NSIS `.exe` |
| **Publish** a site | **a source checkout** + `make` + `podman`. Nothing else. |

**The desktop installers do not contain the publishing tool.** They bundle
`entity-browser-tauri` — the app. The publisher is the *native* build of the
`entity-browser` binary, and the supported way to drive it is `make`, which runs
it inside a pinned toolchain container. So:

```bash
git clone https://github.com/EntityChurch/entity-browser-rust
git clone https://github.com/EntityChurch/entity-core-rust    # required sibling
cd entity-browser-rust
```

The two checkouts **must be siblings** — this crate reaches `../entity-core-rust`
by path dependency. See the README's *Repository layout* for the exact tree.

Host requirements are `make` and `podman`, and that is the whole list. Every
publish target runs in the image; you do not need Rust, cargo, trunk, or node on
the host. (A host toolchain works too if you prefer — see the README — but
nothing in this guide assumes one.)

> **One container rule you will hit:** the build bind-mounts the *parent*
> directory and nothing else, so **every output path must stay inside the repo
> tree**. `OUT=/tmp/x` writes into the container's throwaway filesystem and never
> reaches your disk. This is enforced — `make` refuses the path rather than
> printing success over an empty directory — but it is the first thing people
> try. `INGEST` and `APPS_DIST` are the exception: they are *inputs*, and they
> are staged in for you, so they may point anywhere.

---

## 1. The model, in one picture

```
   ┌──────────────────────────┐        ┌───────────────────────────────┐
   │  ONE generic WASM app     │   +    │  /entity-deployment.json       │
   │  identical on every domain│        │  small, per-domain, fetched at │
   │  (index.html + .wasm)     │        │  boot over plain HTTP          │
   └──────────────────────────┘        └───────────────────────────────┘
                 │                                    │
                 └──────────────────┬─────────────────┘
                                    ▼
                    what THIS domain looks like on a cold boot
                (which site, locked or not, apps-only, where content lives)
```

Two facts follow, and both save you work:

- **The app is never rebuilt per domain.** The same bundle serves every
  deployment; a small JSON file at the origin root shapes it. If that file is
  missing or malformed the app boots to its defaults and never fails — it will
  simply show none of your content.
- **Your content is addressed by the hash of its bytes.** `content/{aa}/{bb}/{hex}`
  is immutable forever by construction. That is what makes a republish nearly
  incremental and a CDN cache trivially correct — *once you set the headers in
  §6.1*.

---

## 2. Mint a publisher identity

This is the "generate a peer" step, and it is the one thing you must not lose.

**Your published content lives under a peer-id derived from a keypair.** That
peer-id is the address of your site: it is in every URL path, in your deployment
config, and in any name binding that points at you. **If the key changes, every
one of those breaks.** Treat it exactly like a private key.

Two ways to fix the identity:

| | Where the key lives | Use for |
|---|---|---|
| **durable** *(default)* | `.entity-publish/publish/keypair` in the repo | one machine publishing one deployment |
| **`IDENTITY_SEED=<64 hex>`** | nowhere — derived from the seed | **CI, backups, more than one deployment, anything you must reproduce** |

**Use the seed.** The durable file works and is the zero-config default, but it
is a file in a working directory: a fresh clone, a new CI runner, or a `git
clean` mints a *different* identity and silently republishes your site at a new
address. A seed is 64 hex characters you can put in a password manager.

```bash
openssl rand -hex 32 > publisher.seed     # keep this. back it up. never commit it.
```

`.entity-publish*` is gitignored for this reason — but a seed in a file next to
your checkout is your responsibility. Both are secrets.

```bash
# Every later command in this guide takes:
IDENTITY_SEED=$(cat publisher.seed)
```

The derived peer-id is **deterministic for a given seed**, and the build prints
it. A malformed seed fails the build rather than falling back to a demo
identity.

> **A registry (§9) has its own, separate identity.** One key issuing names *and*
> publishing the content those names point at is one key doing two jobs — a
> name-issuer vouching for itself. Keep two seeds if you do both.

---

## 3. Author your content

The publisher consumes a plain directory. Nothing about it is specific to any
generator — hand-write it, script it, or emit it from whatever you already use.

```text
my-content/
  my-site/
    site.manifest.json
    pages/
      index.md
      guide/intro.md
    assets/
      diagram.svg
```

`site.manifest.json` — only `site_id` is required:

```json
{
  "site_id": "my-site",
  "title": "My Site",
  "nav": [
    { "title": "Home",  "path": "pages/index.md" },
    { "title": "Guide", "path": "pages/guide/intro.md" }
  ]
}
```

A page is any `pages/**/*.md` (or `.html`); its slug is the path with the
extension stripped, so `pages/guide/intro.md` is `guide/intro`. Optional TOML
frontmatter in a leading `+++ … +++` block sets the title.

```markdown
+++
title = "Home"
+++

# Hello

Some words, and a picture: ![a diagram](assets/diagram.svg)
```

`--ingest` **scans recursively** for every directory containing a
`site.manifest.json`, so one `INGEST=` can carry one site or fifty. Two rules
worth knowing before you have fifty:

- **`site_id` must be unique across everything you publish under one identity.**
  Prefix it (`mydomain-research`) rather than hoping.
- **The nav is authored, not derived.** Delete a page and leave its nav entry and
  you ship a dangling link — the publisher warns by name, so read the output.

Full contract, including assets and pre-rendered HTML documents:
[`PUBLISH-INGEST-FORMAT.md`](architecture/guides/PUBLISH-INGEST-FORMAT.md).

---

## 4. Build the uploadable tree

```bash
make site-dist \
  INGEST=../my-content \
  IDENTITY_SEED=$(cat publisher.seed) \
  CONFIG_SITE=my-site
```

That is the whole build. It emits `dist-site/`, and it is the tree you upload.

> ### ⚠️ Use `site-dist`, not `site`
>
> `make site` emits the **content half only**. Its root `index.html` is a
> redirect to `/sites/`, and there is no app bundle. Upload *that* to a domain
> root and you replace the live app with a redirect page, orphaning every app
> bundle in the tree. `site-dist` is the composition: build the release app,
> publish content into the same directory, then verify.
>
> The ordering is load-bearing — the app build wipes its output directory, so it
> must run *first*, and the publish is written to clean only its own roots.
> That is exactly why this target exists instead of a note telling you to run two
> commands in the right order.

What lands in `dist-site/`:

```text
dist-site/
├── index.html                     the app
├── *.wasm  *.js  sw.js            the app bundle (hash-named)
├── entity-deployment.json         ← per-domain config, ALWAYS at the root
├── sites/{peer}/{site}/…          plain .html — what a crawler and a no-JS reader see
├── content/{aa}/{bb}/{hex}        your bytes, content-addressed
├── transport-profile              where this publisher is served from
└── {peer}/                        the entity-native tree the live app reads
    ├── sites/{site}/…
    ├── apps/{set}/…               only if you published apps (APPS_DIST)
    └── system/peer/published-root the signed root — the trust anchor
```

Both representations of your content ship by default: the `.html` projection for
dumb consumers (search engines, text browsers, JS-off readers) and the
`.bin` + `content/` form for the live app. Add `HTML_ONLY=1` if you want a pure
static site and no live app path at all.

### 4.1 The flags that matter

The full knob table is
[`GUIDE-DEPLOYMENT-AND-CONFIGURATION.md` §6](architecture/guides/GUIDE-DEPLOYMENT-AND-CONFIGURATION.md#6-the-make-site-command-surface),
which is authoritative. These are the ones you will actually set:

| Flag | Default | What it does |
|---|---|---|
| `INGEST=<dir>` | bundled demo | Your content. May point **anywhere** — it is staged in for you. |
| `IDENTITY_SEED=<64 hex>` | durable keypair | §2. Set it. |
| `CONFIG_SITE=<site_id>` | demo site | Which site the domain lands on. **Must be one you published**, or the build fails. |
| `SITE_DIST_OUT=<dir>` | `dist-site` | Where the uploadable tree goes. Must stay in the repo tree. |
| `APPS_DIST=<dir>` | demo seed | Embedded HTML/JS apps, from an `entity-apps` build. May point anywhere. |
| `SURFACE=<chrome\|window\|site>` | `window` | What a visitor sees on a cold boot. See §4.2. |
| `LIVE=<origin>` | *empty = same-origin* | **Leave it empty.** Empty means relative, so the same tree works at localhost and on your domain with no rebuild. |
| `PREFIX=<path>` | *empty = root* | Nest content under a sub-path so one domain can host several publishers. |
| `REGISTRY_PIN=<PEER_ID@ORIGIN>` | none | Pre-trust a name registry (§9). |
| `PLAN=1` | off | Dry run: report what a publish would add/keep/**remove**, write nothing. See §8. |

> **Never bake a loopback into a shipped bundle.** `LIVE=http://localhost…`
> produces a build that serves a content-less shell off a machine nobody else can
> reach. The publisher warns; the empty default is correct for essentially
> everyone.

### 4.2 Pick a surface

`SURFACE` is what a first-time visitor sees. There are three, and they are
config, not different builds:

| Goal | Flags | Emitted `site_mode` |
|---|---|---|
| **Show my site** *(the default, and what most people want)* | `SURFACE=window WINDOW_TYPE="Site Browser"` — your site, maximized, with the full workspace still there underneath | `enabled:false, show_toggle:false` |
| **Just the apps** — no site at all | `SURFACE=chrome` | `enabled:false, show_toggle:false` |
| **Escapable full-viewport site** | `SURFACE=site` | `enabled:true, show_toggle:true, locked:false` |
| **Locked kiosk** — no escape | `SURFACE=site LOCKED=1` | `enabled:true, show_toggle:false, locked:true` (+ `peer_creation_enabled:false`) |

**You do not hand-edit `site_mode`** — the surface determines it, and the
publisher emits it explicitly. The overlay is *off* for `window` and `chrome`
deliberately: their content lives in a window or the workspace, so the status-bar
"View Site" toggle would be redundant, and on a fresh peer it resolves to a site
that is not there.

`?chrome=1` on the URL escapes any posture, including a locked kiosk. Keep that
in your back pocket; it is how you get out if you lock yourself in during
testing.

### 4.3 Try it locally first

```bash
make site-serve INGEST=../my-content CONFIG_SITE=my-site
# → http://localhost:8081
```

One command: rebuild the app, publish into an isolated directory, and serve it
on one origin with the correct headers. This is the round-trip that tells you
your content is shaped right before any of it leaves your machine.

---

## 5. Verify before you upload

```bash
make site OUT=dist-site VERIFY=1 IDENTITY_SEED=$(cat publisher.seed)
```

`site-dist` already runs this as its last step, so you get it for free — but run
it by hand on anything you kept around, and **run it against the tree you are
about to upload**, not the one you built last week.

It reads an already-published directory (it needs no sources) and checks four
independent things:

| Check | Catches |
|---|---|
| every pointer names a blob that exists | a truncated or partial upload |
| every blob's bytes hash to the address claiming them | a corrupted or tampered body |
| bytes are the **canonical** pre-image, not merely hash-equal | appended garbage that a naive hash check passes |
| the signed root verifies **and its closure is complete** | a missing interior node — the shape where every pointer resolves and a visitor still gets nothing |

Exit codes: **0** clean · **2** defect found · **1** could not run. So
`make site OUT=… VERIFY=1 && upload` is a real gate.

Orphan blobs are reported, not failed — nothing links to them and a tree
mid-cutover legitimately has some.

> **Verify runs the same check the browser runs on every fetch.** A clean result
> means a visitor's client will not reject anything either. This is the cheapest
> minute in the whole process; spend it.

---

## 6. Upload

The tree is static files. `aws s3 sync`, `rclone`, `wrangler`, `rsync` — all
fine. What is **not** optional is how the host serves them.

### 6.1 The serving contract

Two properties. Get either wrong and the failure is confusing rather than loud.

**(a) CORS — if the app and the content are on different origins.**

```
Access-Control-Allow-Origin: *
Access-Control-Allow-Methods: GET, HEAD
Access-Control-Allow-Headers: Range
```

`*` is correct here and is not a shortcut: the tree is fully public and fully
content-addressed, there are no cookies and no credentials, and there is nothing
CORS protects that the hash does not. Do **not** add
`Access-Control-Allow-Credentials` — authority in this system lives in signatures,
never in HTTP.

If you serve the app and the content from one origin (the normal single-domain
deploy), you can skip this. Add it the moment a second origin appears.

> **`curl` will not tell you this is wrong.** A native client happily reads a
> response a browser discards. Check with a browser, or check the header
> explicitly.

**(b) Cache headers keyed on mutability — the one that bites.**

The rule is **opt *in* to immutable**, and that direction is the whole point:

```
…/content/{aa}/{bb}/{hash}     public, max-age=31536000, immutable
*-<8+ hex>.wasm  *-<8+ hex>.js public, max-age=31536000, immutable
EVERYTHING ELSE                no-store
```

Content blobs are addressed by their own hash and the app bundle is hash-named,
so for both of those a new build is a new URL and a one-year cache is free and
correct.

**Match the blob store by its SHARD STRUCTURE, not by the word `content`** — the
two directory levels are the hash's own first four hex characters, so the pattern
verifies itself:

```
content/[0-9a-f]{2}/[0-9a-f]{2}/[0-9a-f]{32,}
```

That distinction is the one that bites, in both directions:

- **Too loose is unrecoverable.** Hugo, Zola and Lektor all name their source
  tree `content/`, so an ingested site publishes
  `/{peer}/sites/<site>/content/about.html`. A rule that matches *any* path
  containing `content/` pins that mutable HTML for a year at a stable URL.
- **Too tight is silent.** A rule anchored at `/content/` (path start) misses
  every **prefixed** deployment — `/docs/content/…`, `/protocol/content/…` — so
  the entire blob store loses immutable caching and nothing tells you.

**Everything else in the tree is mutable**, including four files it is
very easy to forget:

| File | If you cache it for a year |
|---|---|
| `entity-deployment.json` | change your config, your home site, or your registry pin — no returning visitor ever sees it |
| `index.html` / `sw.js` | ship a new build — nobody loads it |
| `…/system/peer/published-root` | a stale signed root silently defeats the rollback protection it exists to provide |
| `…/sites.list`, `transport-profile` | your site list and where you are served from freeze |

The default in most CDN configurations is a blanket rule, and a blanket
*immutable* rule is a deployment you cannot correct. **This is invisible
locally** — a fresh container has no cache — and it is not something you notice
the day you set it. You notice the first time you need to fix something.

The reference implementation of this policy is `tools/cors-serve.py`
(~80 lines, readable). It is what our own local serving uses, and translating its
`is_immutable()` rule into a CDN config is the intended path — the worked version
is §6.2 below.

> **This rule has one definition and several expressions, and they are gated
> against each other.** `tools/cache-policy-vectors.txt` lists the cases; the
> Rust servers, the Python server and this document are all checked against it
> (`make lint`). That machinery exists because these expressions were once
> hand-copied and drifted four ways — the loosest of them matching any path
> containing `content/`. If you extend the rule, add the case to that file first.

### 6.2 Cloudflare (R2) — the shape we run

This is our own deployment, written down. It is not the only way; it is a known
working one.

**1. Bucket + binding.** Push the tree into an R2 bucket and serve it at the
domain — either the R2 custom-domain binding or a Worker in front of it. Upload
with `wrangler`, `rclone`, or any S3-compatible client (R2 speaks S3).

**2. Cache Rules — two, in this order.** Cloudflare's default is *not* what you
want, so set them explicitly:

| # | Match | Set |
|---|---|---|
| 1 | `URI Path matches ".*content/[0-9a-f]{2}/[0-9a-f]{2}/[0-9a-f]{32,}$"` **or** `URI Path matches ".*-[0-9a-f]{8,}(_bg)?\.(wasm\|js)$"` | Edge TTL: **1 year** · Browser TTL: **1 year** |
| 2 | *(everything else)* | **Bypass cache** / `no-store` |

Rule 2 is the one people skip. Add it.

**Rule 1 used to read `URI Path starts with "/content/"` here.** That is safe but
it silently drops every prefixed deployment out of immutable caching; the
shard-matching form above covers those and still excludes an ingested site's own
`content/` directory. Do not "simplify" it back to a prefix or a substring —
§6.1 explains what each of those two costs.

**3. Compression — one rule you will want.** Cloudflare Brotlis text
(HTML/JS/WASM) automatically, for free, nothing to do. It does **not** compress
`application/octet-stream` by default — which is exactly what the entity-native
`.bin` pointers and `content/` blobs are served as, so your live-app page bodies
ship **uncompressed** while the static HTML arm is already compressed. Fix it
with a **Compression Rule** forcing compression on the `/content/*` path (or on
`application/octet-stream`). Measured on a live site; worth doing.

**4. Images are not a compression problem.** Already-compressed formats do not
shrink further, and the publisher treats assets as opaque bytes — it does zero
image optimization. The levers are **source-side** (resize to display size,
recompress, WebP, *before* ingest) or Cloudflare Polish. Prefer source-side: it
also shrinks what you store and upload.

**5. Cutover, for anything live and large.** Do **not** republish in place. The
publish cleans its own roots before rewriting them, so republishing straight over
a live tree opens a window where the site is broken until the upload finishes.
Instead:

1. Publish to a **fresh directory** (`bucket/v2/`), never over the live one.
2. Test it on a **subdomain**, not a subpath. This matters: the same-origin
   default expands to `window.location.origin`, which **drops any path**, so a
   `yourdomain.com/v2/` preview resolves `/content/…` at the root and breaks.
   `staging.yourdomain.com` → `bucket/v2/` resolves correctly.
3. Cut over with a transform rule / rewrite pointing the domain at `v2/`. The
   rewrite is transparent to the browser — it still sees `yourdomain.com/…`, so
   the same-origin config keeps working with no rebuild, and
   `entity-deployment.json` flips with it because it sits at the served root.
4. **Roll back** by repointing the rewrite. The old directory is untouched.

### 6.2a Retained builds — giving a bad deploy somewhere to fall back to

`make site-dist` now also writes two things, and they are the only reason a broken
release is recoverable rather than terminal:

```
/builds.json                     the list of releases, newest first
/builds/<build_id>/index.html    each release's shell, kept
```

Without them, `/` is the only shell and **every deploy destroys the previous one**
while all its hashed assets survive — so there is nothing to roll back *to*, only
orphaned parts. Retaining a shell costs ~50 KB; the bundles are already shared by
hash and already retained.

**Both are MUTABLE** — `no-store`, per §6.1. `builds.json` changes every release,
and a retained shell is overwritten whenever the same build id is republished.

**Upload order matters, and it is the same rule as §6.2(5)'s cutover:**

```
1. assets  (hashed, immutable — safe in any order, nothing points at them yet)
2. /builds/<id>/index.html      the new release's retained shell
3. /                            the shell everyone gets
4. /builds.json                 last, because it ADVERTISES what steps 2–3 placed
5. (much later, separately)     prune
```

A `builds.json` uploaded before the shell it names advertises a rollback target
that 404s — which fails at exactly the moment someone needs it. **Never delete an
asset a retained build names**; `entity-browser builds --prune` enforces that for
the shells it manages, but a hand-run `--delete` sync does not know about it.

`make builds-manifest DIST=<dir> NOTES="…" RELEASED_AT="…"` runs it on a tree you
already have; `KEEP=N` sets how many shells to retain (default 3).

### 6.3 S3 / anything else

```bash
aws s3 sync --delete dist-site/ s3://your-bucket/
```

> **This command alone is not a deployment.** S3 sets no `Cache-Control` of its
> own, and a response with none is *not* uncached — browsers apply heuristic
> freshness and will serve it without revalidating. Set the headers from §6.1 in
> the same session you first sync (see the end of this section), or you have
> shipped the mutable half of the tree with no freshness policy at all.

`--delete` is what keeps the bucket from accumulating orphans across republishes
— removed pages, pruned blobs, and an old `{peer}/` subtree if you ever changed
identity.

Add **`--size-only`** *scoped to the blob store* if you want to skip re-uploading it:
unchanged files are physically rewritten on each publish (fresh mtimes), so a
default size+mtime comparison re-uploads byte-identical blobs. Since a
`content/…` path *is* the content hash, a path match already proves the bytes
match, which makes `--size-only` safe there. The mutable files are few and small
— let them sync normally.

**Scope it, do not add it to the whole sync.** `--size-only` on the mutable half
skips any file whose length did not change, and the files most likely to change
without changing length are exactly the ones that must not go stale: a
`entity-deployment.json` whose registry pin was edited in place, an `index.html`
whose bundle hash moved. Two passes:

```bash
aws s3 sync --delete --size-only dist-site/ s3://your-bucket/ --exclude "*" --include "*/content/*"
aws s3 sync --delete           dist-site/ s3://your-bucket/ --exclude "*/content/*"
```

Then set the headers from §6.1 on the bucket/CDN. On S3 that is per-object
metadata (`--cache-control` on the sync, scoped by prefix); on nginx it is two
`location` blocks.

---

## 7. Check it live

Four things, in this order — each one fails differently:

```bash
# 1. The app loads at the apex (not a redirect page)
curl -sI https://yourdomain.com/ | head -3
curl -s  https://yourdomain.com/ | grep -o '<title>[^<]*'

# 2. The config is served at the ROOT, whatever your PREFIX is
curl -s https://yourdomain.com/entity-deployment.json

# 3. The mutable files are NOT cached hard
curl -sI https://yourdomain.com/entity-deployment.json | grep -i cache-control
#   → want: no-store  (NOT max-age=31536000)

# 4. A content blob IS
curl -sI https://yourdomain.com/content/<aa>/<bb>/<hex> | grep -i cache-control
```

Then open it in a browser — **in a fresh profile**. This is not optional advice:

> **Your own previous session beats the deployment config.** A returning
> visitor's durable local settings win over `entity-deployment.json`, by design.
> So if you have opened your own site before, you are testing your old posture,
> not the one you just shipped. Use a private window or a fresh profile, or open
> the recovery console at `?systemrecovery=1` to clear state.

---

## 8. Republish

The good news is structural: the layout is content-addressed and path-stable, so
a republish is a sync, not a migration.

```bash
# 1. Dry run FIRST. It writes nothing and tells you what would DISAPPEAR.
make site-dist PLAN=1 INGEST=../my-content IDENTITY_SEED=$(cat publisher.seed)

# 2. Then for real, with the SAME seed.
make site-dist INGEST=../my-content IDENTITY_SEED=$(cat publisher.seed) CONFIG_SITE=my-site

# 3. Sync.
aws s3 sync --delete dist-site/ s3://your-bucket/
```

**`PLAN=1` is the gate, and it is not decoration.** A publish replaces
everything under your peer prefix — so ingesting a *subset* of your sites deletes
every one you omitted, and omitting `APPS_DIST` deletes your apps. `--plan` exits
`0` when nothing would be removed and `2` when something would, which makes
`make site PLAN=1 … && make site …` a one-line safety net. Use it.

Three rules for a republish:

1. **Same seed, always.** The peer-id is the site address; changing it orphans
   every deep link and every registry binding pointing at you.
2. **Ingest the complete set.** Or scope it with `PREFIX`.
3. **Republish into the same output directory** for anything consumers have
   already seen. The publisher reads the previous signed root out of that
   directory and advances a sequence number — which is what lets a returning
   visitor detect a host serving them an older copy. A publish into a fresh
   empty directory restarts that sequence at zero. (`make site` and `make
   registry` do this correctly; the local `make federation` rig deliberately
   wipes first, so do not use it to update a real deployment.)

Deeper: [`GUIDE-REPUBLISH-AND-INCREMENTAL.md`](architecture/guides/GUIDE-REPUBLISH-AND-INCREMENTAL.md)
has the per-change table (edit / add / delete a page, and the dangling-nav
gotcha).

---

## 9. Optional — names

Everything above works with no names at all: your domain serves your app and
your content, and a visitor is trusting your domain the way they trust any web
page.

A **name registry** is the layer above that. It publishes signed
`name → peer-id` bindings so a visitor can pin **one** key and reach any
publisher it names — with the web host trusted for nothing at either hop.

```bash
# A registry is the same publisher at a different identity — keep the seeds apart.
openssl rand -hex 32 > registry.seed

make registry \
  REGISTRY_OUT=dist-registry \
  TTL_DAYS=30 \
  SEED=--identity-seed=$(cat registry.seed) \
  BIND='--bind=yourdomain.com=<your-publisher-peer-id>@https://yourdomain.com'
```

Then upload `dist-registry/` like any other tree (§6), and seed the pin into your
deployment so a visitor types nothing:

```bash
make site-dist … REGISTRY_PIN=<registry-peer-id>@https://registry.yourdomain.com
```

Three things to know before you go further:

- **`@ORIGIN` is mandatory** on a binding. A name resolving to a peer-id nobody
  can reach is not a resolution.
- **Use the `REGISTRY_PIN` flag, never a hand-edit of the emitted JSON.** It is
  two strings in a file, so editing looks equivalent — it is not. The flag
  validates the pin *on your machine, where you can read the refusal*. A
  hand-edited bad pin fails later, in a visitor's browser, where it looks
  identical to a registry that is merely down.
- **`--verify` a registry with the `registry` verb**, not `publish --verify` —
  they resolve different identities, and pointing the wrong one at a clean tree
  reports it as unverifiable.

The full trust chain — what a consumer checks at each hop, what is proven, and
what is explicitly still open — is
[`GUIDE-PUBLISHING-AND-NAMES.md`](architecture/guides/GUIDE-PUBLISHING-AND-NAMES.md).
Read it before you rely on names for anything that matters.

---

## 10. When it goes wrong

Keyed on **the symptom you actually see**, because none of these announce their
cause.

| Symptom | Cause | Fix |
|---|---|---|
| Domain shows a **redirect page**, not the app | uploaded `make site` output instead of `make site-dist` | §4 |
| App loads, **"no sites"** / empty rail | `entity-deployment.json` missing at the root, or 404 | it must be at `/entity-deployment.json` **regardless of `PREFIX`** |
| Site looks right for you, **wrong for everyone else** | your durable local settings are winning | test in a fresh profile (§7) |
| **New build/config never reaches anyone** | mutable files cached immutable | §6.1(b) — this is the most common one |
| Content **404s in the browser, 200s in `curl`** | missing CORS on a cross-origin content host | §6.1(a) |
| **Everything vanished** after a republish | published a subset — the omitted sites were deleted | `PLAN=1` before every publish (§8) |
| Site reappeared at a **new address**; old links dead | identity churned (fresh clone / CI / lost keypair) | always pass `IDENTITY_SEED` (§2) |
| Nav links **404** | page deleted, nav entry left behind | remove the `nav` entry too; read the publish warnings |
| `make` refuses your `OUT` path | output outside the repo tree | §0 — the container mounts only the repo |
| **Verify passes locally, visitors get nothing** | partial upload | re-sync with `--delete`, then re-verify §5 |
| Subpath staging preview is broken | same-origin config drops the path | test on a **subdomain**, not a subpath (§6.2 step 5) |

---

## 11. Where to go deeper

| For | Read |
|---|---|
| Every flag, the config schema, precedence, deployment recipes | [`GUIDE-DEPLOYMENT-AND-CONFIGURATION.md`](architecture/guides/GUIDE-DEPLOYMENT-AND-CONFIGURATION.md) |
| The content directory contract | [`PUBLISH-INGEST-FORMAT.md`](architecture/guides/PUBLISH-INGEST-FORMAT.md) |
| Republish mechanics, per-change effects, CDN sync | [`GUIDE-REPUBLISH-AND-INCREMENTAL.md`](architecture/guides/GUIDE-REPUBLISH-AND-INCREMENTAL.md) |
| Signed roots, registries, what a consumer verifies, what is open | [`GUIDE-PUBLISHING-AND-NAMES.md`](architecture/guides/GUIDE-PUBLISHING-AND-NAMES.md) |
| Every make target and CLI flag | [`TOOLS.md`](architecture/guides/TOOLS.md) |
| Bundling content into the desktop app instead of a CDN | [`GUIDE-DEPLOYMENT-AND-CONFIGURATION.md` §8.1](architecture/guides/GUIDE-DEPLOYMENT-AND-CONFIGURATION.md#81-bundling-content-into-the-tauri-desktop-app) |

---

## 12. What this is not, yet

Stated plainly, because a quickstart that oversells is worse than one that is
narrow:

- **This is a research preview.** It is suitable for evaluation, exploration, and
  publishing real content you can re-publish. It is not a hardened production
  platform, and the project says so in its own release notes.
- **Publishing needs the source checkout.** There is no standalone publisher
  binary in the release artifacts today (§0).
- **We do not ship our own deployment scripts.** The mechanics here — the publish
  contract, the ingest format, the layout, the exit codes — are general-purpose
  and complete; the *process* around them (which sources are canonical, which
  bucket, when to cut over) is deliberately yours. If you find you need a
  mechanic that does not exist to express your process correctly, that is a gap
  in this repo and worth reporting.
- **The Site Browser trusts the origin for the path→hash mapping.** Content is
  hash-verified against the pointer the origin served, which is a real gate
  against corruption and truncation — but the origin supplied the pointer, so it
  is not a defence against a *lying* host. The signed-root path that closes that
  is built and reachable through the Shell's `name` verb; it is not yet wired
  into the browsing surface. `GUIDE-PUBLISHING-AND-NAMES.md` §7 is the honest
  statement of exactly where that stops.
