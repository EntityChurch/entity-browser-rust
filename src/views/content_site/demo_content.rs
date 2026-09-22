//! The bundled demo site — **the whole of it**: the seeder (`ensure_demo_site`)
//! and every byte it writes, manifest and nav labels and page titles and
//! bodies.
//!
//! These are **content, not UI**, and the split is what makes that checkable.
//! A demo document carries its own stylesheet and a demo figure its own
//! palette *by definition*: the document renders in an opaque-origin frame and
//! the figure is asset bytes, so `--site-*` tokens cannot reach either one.
//! Their hex colours are therefore data, indistinguishable from a colour in a
//! user’s uploaded image — which is why `tools/ui-lint.sh` exempts this file
//! by name rather than carrying its palette in the raw-hex baseline.
//!
//! i18n-ignore-file — this is a published site's content, not application
//! chrome. The browser does not translate the pages it renders (a published
//! paper arrives in the language its author wrote it in), and the demo site is
//! a published site like any other. The app's own strings *around* it — the
//! breadcrumb trail, the empty-document notice, the directory rail — go
//! through `crate::i18n::t` and are counted normally.
//!
//! **The axis is AUTHORSHIP, not the render path** — corrected 2026-09-03, and
//! the correction is why the seeder moved in here. This header used to say
//! *"nothing that renders through our DOM belongs here"*, which is the wrong
//! test: a site's nav labels and page titles are rendered by our chrome and
//! are still the publisher's words. Under the old rule the demo's manifest
//! title, five nav labels and nine page titles stayed in `mod.rs` and were
//! counted as untranslated app prose — 24 strings that no correct action could
//! ever clear, because translating a site's own nav into the reader's locale
//! is not a thing this app does. Ask *who wrote this string*, not *who paints
//! it*.
//!
//! **And it needed a file, not a marker (AP44).** One of those 24 — the
//! `Pre-Rendered Document` page title — already carried a `// i18n-ignore`
//! with this exact reasoning written beside it, while the fourteen identical
//! cases on the surrounding lines did not. A rule that needs the word *every*
//! cannot live in a per-line marker somebody has to remember; it lives in a
//! file boundary the scanner enforces for free.

/// A small authored SVG figure the demo site's index page embeds — proof of
/// the embed→asset→`<img>` path with a visible artifact.
pub(super) const DEMO_FIGURE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="480" height="240" viewBox="0 0 480 240">
  <rect width="480" height="240" rx="12" fill="#15151f"/>
  <rect x="1" y="1" width="478" height="238" rx="12" fill="none" stroke="#2a2a3e"/>
  <circle cx="120" cy="150" r="44" fill="#3a6ea5"/>
  <rect x="200" y="100" width="84" height="96" rx="6" fill="#9fd0ff"/>
  <polygon points="330,196 392,100 440,196" fill="#6ad0a0"/>
  <text x="24" y="46" fill="#cfe3ff" font-family="system-ui,sans-serif" font-size="22" font-weight="700">Entity Demo Figure</text>
  <text x="24" y="72" fill="#9aa3b2" font-family="system-ui,sans-serif" font-size="13">A content-addressed SVG asset, embedded via ::embed</text>
</svg>"##;

/// The demo site's **document** page — a `format:html` page (convention §3.1's
/// web tier), so the document surface has a visible artifact end to end exactly
/// as [`DEMO_FIGURE_SVG`] does for assets. Shaped like the real thing it exists
/// for: a pre-rendered Pandoc paper — own `<head>`, own `<title>`, own inline
/// stylesheet, its own serif typography, an internal anchor link.
///
/// **Three things in here are load-bearing for the gate, not decoration.**
///
/// 1. `id="entity-demo-doc-marker"` — a document mounted correctly is inside an
///    opaque-origin frame, so this id is **unreachable** from our DOM. If
///    someone ever routes a document through `set_inner_html`, the marker
///    appears in our own tree and the e2e sees it. The assertion is a negative
///    that only the bug can satisfy.
/// 2. The `<script>` — it would rewrite the paragraph text if it ran. Under
///    `sandbox=""` it cannot, and the e2e checks the *behaviour* rather than
///    the spelling of the sandbox attribute. A gate that only compared the
///    attribute string would still pass if the tier were widened elsewhere.
/// 3. The `#entity-demo-doc-toc` → `#entity-demo-doc-deep` jump **across a
///    deliberately tall spacer**. This is a paper's table of contents, and it
///    was broken for a whole release: under `srcdoc` the link resolved against
///    the *app's* base URL and navigated the frame away from the document
///    entirely. The previous version of this page carried a jump-to-top link
///    and a sentence asserting "internal anchors work" — the document was too
///    short for the jump to be observable, so the prose was the only evidence,
///    and it was false. Height is what makes it a gate.
///
/// 4. `#entity-demo-doc-chapters` — **eight** further links, because one jump
///    is not evidence about navigation. The `blob:` URL was originally revoked
///    on the frame's `load` event, on a measurement that clicked a single
///    anchor; a revoked URL serves the first few same-document navigations and
///    then stops, so every book died after ~5 chapter jumps while this gate
///    stayed green. The chapter run is what makes the repetition observable.
///
/// Keep all four if you edit this. They are the only reason it isn't prose.
pub(super) const DEMO_DOCUMENT_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>A Pre-Rendered Document</title>
<style>
  body { font-family: "Liberation Serif", Georgia, serif; line-height: 1.55;
         color: #1a1a1a; background: #fafafa; margin: 0; padding: 2rem 1rem; }
  main { max-width: 42rem; margin: 0 auto; background: #fff; padding: 2rem 2.5rem;
         border: 1px solid #e0e0e0; border-radius: 4px; }
  h1 { font-size: 2rem; margin-top: 0; }
  code { font-family: "Liberation Mono", monospace; background: #f5f5f5; padding: 0 3px; }
  /* Tall on purpose: an anchor jump is only observable in a document that
     does not fit on screen. The gate measures where the target sits after
     the click, and in a short document it never moved. */
  #entity-demo-doc-spacer { height: 2400px; }
  /* Trailing height, so the target can reach the TOP of the frame. Without
     it the document runs out of scroll and the target stops part-way down —
     still a working jump, but one the gate can only assert loosely. */
  #entity-demo-doc-tail { height: 1600px; }
  /* One jump is not evidence about navigation. See the chapter list below. */
  .demo-chapter { height: 900px; }
</style>
</head>
<body>
<main>
<h1 id="entity-demo-doc-marker">A Pre-Rendered Document</h1>
<p>This page is stored as a <code>format:html</code> entity and carried
<strong>verbatim</strong> from the tree — the app renders none of it. It is
mounted in a fully-restricted sandbox, which is why its own typography survives
while nothing in it can execute or reach the page around it.</p>
<p id="script-probe">Scripts do not run here.</p>
<p>Jump to <a id="entity-demo-doc-toc" href="#entity-demo-doc-deep">the deep
section</a> — this is what a paper's table of contents is, and what a one-file
book's chapter links are.</p>
<div id="entity-demo-doc-spacer">
<p>Everything between here and the deep section exists so the jump is
<em>observable</em>: a document short enough to fit on screen cannot show
whether its own anchors work, which is how a broken table of contents shipped
under prose claiming it worked.</p>
</div>
<h2 id="entity-demo-doc-deep">The deep section</h2>
<p>If you arrived here by clicking the link above, the document kept its own
base URL. Under <code>srcdoc</code> that link resolved against the surrounding
app instead, and this document was replaced by it.</p>
<div id="entity-demo-doc-tail">
<p>The tail below exists so the jump above can complete. Without it the
document runs out of scroll and the target lands part-way down the frame,
which is a working jump the gate can only check loosely.</p>
</div>
<nav id="entity-demo-doc-chapters">
<p>A reader does not click one link. This list is the repetition:</p>
<a class="demo-ch" href="#entity-demo-ch1">1</a>
<a class="demo-ch" href="#entity-demo-ch2">2</a>
<a class="demo-ch" href="#entity-demo-ch3">3</a>
<a class="demo-ch" href="#entity-demo-ch4">4</a>
<a class="demo-ch" href="#entity-demo-ch5">5</a>
<a class="demo-ch" href="#entity-demo-ch6">6</a>
<a class="demo-ch" href="#entity-demo-ch7">7</a>
<a class="demo-ch" href="#entity-demo-ch8">8</a>
</nav>
<h2 id="entity-demo-ch1">Chapter 1</h2><div class="demo-chapter"></div>
<h2 id="entity-demo-ch2">Chapter 2</h2><div class="demo-chapter"></div>
<h2 id="entity-demo-ch3">Chapter 3</h2><div class="demo-chapter"></div>
<h2 id="entity-demo-ch4">Chapter 4</h2><div class="demo-chapter"></div>
<h2 id="entity-demo-ch5">Chapter 5</h2><div class="demo-chapter"></div>
<h2 id="entity-demo-ch6">Chapter 6</h2><div class="demo-chapter"></div>
<h2 id="entity-demo-ch7">Chapter 7</h2><div class="demo-chapter"></div>
<h2 id="entity-demo-ch8">Chapter 8</h2><div class="demo-chapter"></div>
<script>
  document.getElementById('script-probe').textContent =
    'SCRIPT RAN — the document sandbox is not restricting scripts';
</script>
</main>
</body>
</html>
"##;

/// The demo site's "Markdown Showcase" page body — one page exercising the full
/// rendered feature set (headings, emphasis, strikethrough, inline + fenced
/// code, blockquote, ordered/unordered/nested/task lists, an aligned table, a
/// rule, an embedded image, links) so markdown fidelity can be evaluated at a
/// glance. Content-only (no raw HTML — the renderer neutralizes it); the
/// `::embed` reuses the seeded demo figure asset. Column-0 content so nothing
/// is accidentally indented into a code block.
pub(super) const SHOWCASE_MD: &str = r#"# Markdown Showcase

One page that exercises the full markdown feature set, so rendering fidelity is easy to evaluate at a glance — in the live overlay and in the static export.

## Text styles

**Bold**, *italic*, ***bold italic***, ~~strikethrough~~, and `inline code`. A [link back Home](./index) and a [link to the web](https://example.com).

## Blockquote

> Format ⊥ transport — the same page renders from the local tree, a peer, or a CDN.
> A second line of the same quote, to show it wraps as one block.

## Lists

- First item
- Second item
  - Nested item a
  - Nested item b
- Third item

1. Step one
2. Step two
3. Step three

- [x] Styled tables
- [x] Fenced code blocks
- [ ] Footnotes (not yet)

## Table

| Feature     | Status |               Notes |
| :---------- | :----: | ------------------: |
| Tables      |  Done  | header + zebra rows |
| Code blocks |  Done  | monospace, bordered |
| Blockquotes |  Done  |     left accent bar |
| Images      |  Done  |      scale to width |

## Code block

```rust
fn markdown_to_html(md: &str) -> String {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    render(md, opts)
}
```

## Image

::embed[Entity Demo Figure — a content-addressed SVG asset]{ref=assets/figures/demo.svg}

---

Back to [Home](./index).
"#;

use crate::content_site::format::{SiteAsset, SITE_MANIFEST_TYPE};
use crate::content_site::{paths, NavItem, SiteManifest, SitePage};
use crate::peers::Peers;
use super::{DEMO_NOTES_SITE_ID, DEMO_SITE_ID};

/// Seed the bundled demo site into `peer_id`'s tree if it isn't there
/// yet. Idempotent (gated on the manifest's presence). Synchronous L0
/// writes so the first render resolves without waiting on a dispatch
/// round-trip — the Direct-arm path the Content Site window opens on.
/// (Worker-arm-bound sites are a later concern, like the rest of
/// cross-peer/transport work.)
pub fn ensure_demo_site(peers: &Peers, peer_id: &str) {
    // Re-seed when the manifest is absent OR carries the OLD type tag.
    // Worker mode (default) is durable: pre-migration demo entities
    // persist in OPFS under the old `content/site/*` tags at this same
    // path. Gating on path-presence alone (the original guard) would
    // serve a stale-typed ghost forever; gate on the CURRENT type so the
    // rename actually takes (D16 — the durable-Worker orphan trap).
    let manifest_current = |site: &str| {
        peers
            .get_entity(peer_id, &paths::manifest_path(peer_id, site))
            .map(|e| e.entity_type == SITE_MANIFEST_TYPE)
            .unwrap_or(false)
    };
    // Re-seed unless BOTH the primary demo AND its cross-site companion are
    // present at the current type — adding the companion to an existing install
    // (durable Worker) must trigger a re-seed, not serve a half-seeded demo.
    if manifest_current(DEMO_SITE_ID) && manifest_current(DEMO_NOTES_SITE_ID) {
        return;
    }

    // A genuinely *deep* demo site (2- and 3-level page paths under a
    // "Guide" section) so the overlay exercises nested navigation +
    // active-trail, not just flat pages — the deep-site cycle's live
    // surface. Top-level nav still carries Home/About/Theory
    // (Phase 19/20 assert these) plus the Guide section.
    let manifest = SiteManifest::new(
        DEMO_SITE_ID,
        "Entity Demo Site",
        "index",
        vec![
            // Nav targets are root-absolute (`/slug`): nav is a site-global
            // menu rendered on every page, so it must resolve identically from
            // any current page (the link-resolution convention, location.rs).
            NavItem::new("Home", "/index"),
            // Guide is a section (its pages nest under guide/*); the format
            // can now carry a children sub-menu (GAP3), but until a sidebar
            // renderer consumes children we keep the demo's declared nav ==
            // what's rendered (a flat top bar) — no unrendered data (AP10).
            NavItem::new("Guide", "/guide/intro"),
            NavItem::new("About", "/about"),
            NavItem::new("Theory", "/theory"),
            NavItem::new("Showcase", "/showcase"),
        ],
    );

    let pages = [
        (
            "index",
            SitePage::markdown(
                "Welcome",
                "# Welcome to the Entity Demo Site\n\nThis page is a **content-addressed entity** rendered as HTML — you're browsing it inside a full entity peer, but it looks like any other site.\n\n::embed[Entity Demo Figure — a content-addressed SVG asset, embedded via the ::embed directive]{ref=assets/figures/demo.svg}\n\n- It's just markdown stored in the tree.\n- Links navigate within the entity system.\n- The overlay toggle reveals the peer underneath.\n\nStart with the [Guide](./guide/intro), read [About](./about) or the [Theory](./theory), open a [Pre-Rendered Document](./document), hop to the companion [Field Notes](site:demo-notes/index) site, or visit [the web](https://example.com).\n",
            ),
        ),
        (
            "guide/intro",
            SitePage::markdown(
                "Guide — Intro",
                "# Guide: Intro\n\nThis page lives at `guide/intro` — a **nested** content entity. The *Guide* nav item stays highlighted across the whole section (active-trail).\n\nNext: [Install](install), or jump straight to the [Internals](advanced/internals).\n\nBack to [Home](../index).\n",
            ),
        ),
        (
            "guide/install",
            SitePage::markdown(
                "Guide — Install",
                "# Guide: Install\n\nStill in the Guide section (`guide/install`). Notice *Guide* is still the active nav item.\n\nBack to the [Intro](intro), or deeper to [Internals](advanced/internals).\n",
            ),
        ),
        (
            "guide/advanced/internals",
            SitePage::markdown(
                "Guide — Internals",
                "# Guide: Internals\n\nThree levels deep (`guide/advanced/internals`) and still resolving from the tree by path. The *Guide* section nav stays lit the whole way down.\n\nBack to the [Intro](../intro).\n",
            ),
        ),
        (
            "about",
            SitePage::markdown(
                "About",
                "# About\n\nThe Entity Demo Site is a tiny showcase of **Site Mode**: content-addressed static sites with reactivity, served from the entity system.\n\n```\nsite/demo/\n  manifest\n  pages/{index,about,theory}\n  pages/guide/{intro,install}\n  pages/guide/advanced/internals\n```\n\nBack to [Home](./index).\n",
            ),
        ),
        (
            "theory",
            SitePage::markdown(
                "Theory",
                "# Theory\n\nA *site* is a content subgraph rooted at a signed manifest. Pages are markdown entities; links are entity-native and resolve across sites and peers.\n\n> Format ⊥ transport: the same page renders from the local tree, a peer, or a CDN.\n\nBack to [Home](./index).\n",
            ),
        ),
        // A full markdown feature showcase — the one page that exercises every
        // rendered construct (tables w/ alignment, fenced code, blockquotes,
        // nested + task lists, strikethrough, hr, embedded image) so rendering
        // fidelity is evaluable at a glance in both the live overlay and the
        // static export. Authored as a raw string (real newlines/indent) below.
        ("showcase", SitePage::markdown("Markdown Showcase", SHOWCASE_MD)),
        // The document tier's visible artifact (§3.1 web tier) — a page whose
        // body is a whole pre-rendered HTML file rather than markdown. It
        // renders in a restricted sandbox, so it is also the one demo page
        // whose content our own DOM never touches. See `DEMO_DOCUMENT_HTML`.
        // A demo site's page title is content, like every other title in this
        // list; the app does not translate the pages it renders.
        ("document", SitePage::html("Pre-Rendered Document", DEMO_DOCUMENT_HTML)),
    ];

    // Arm-aware seed write via the blessed `Peers::seed_write` router
    // method: Direct (native, Tauri WebView, tests) → synchronous L0 put
    // so the demo is readable in the same render pass (the sync `#[test]`s
    // depend on it); Worker (browser) → async `dispatch_write`, the
    // resolver's `Pending`/repaint seam absorbs the delay. This replaced
    // an open-coded `direct_peer_context` reach-through whose original
    // unconditional `store().put()` panicked on Worker spawn and froze the
    // rAF loop — routing it removes the hatch *and* the panic class.
    peers.seed_write(peer_id, paths::manifest_path(peer_id, DEMO_SITE_ID), manifest.to_entity());
    // The figure the index page embeds — a small, human-authored SVG so the
    // asset path has a *visible* artifact end-to-end (independent of the
    // papers compute-figure pipeline, whose PNGs may be unpinned placeholders).
    // SVG is text, content-addressed like any asset, and safe in an <img>.
    peers.seed_write(
        peer_id,
        paths::asset_path(peer_id, DEMO_SITE_ID, "figures/demo.svg"),
        // Inline unconditionally: the figure is a few hundred bytes of
        // hand-authored SVG, well under EMBED §3's 16 KiB ceiling, and the
        // demo seed has no content store to point into. `inline` rather than
        // `asset_store::stage` says that is a property of this artifact
        // rather than a size we did not check.
        SiteAsset::inline("image/svg+xml", DEMO_FIGURE_SVG.as_bytes().to_vec()).to_entity(),
    );
    for (slug, page) in pages {
        peers.seed_write(peer_id, paths::page_path(peer_id, DEMO_SITE_ID, slug), page.to_entity());
    }

    // --- Companion site (cross-site nav) -------------------------------------
    // A SECOND owned site on this same peer, reached from the primary demo via
    // the `site:demo-notes/index` link above. Opening it exercises `site:`
    // cross-site navigation (location.rs → CrossSite → go_to) through the exact
    // shared `rewrite_links` path the Site Browser window and the overlay both
    // use — so the bundled demo proves the feature billslab ships on. The return
    // trip is a `site:demo/index` link back.
    let mut notes_manifest = SiteManifest::new(
        DEMO_NOTES_SITE_ID,
        "Entity Demo — Field Notes",
        "index",
        vec![
            NavItem::new("Notes", "/index"),
            NavItem::new("First Entry", "/entries/first"),
        ],
    );
    // The companion declares a manifest theme (S-T2) — the bundled demo
    // showcases per-site theming live: following the cross-site link flips
    // the palette to light (in "Site's theme" mode), returning flips back.
    notes_manifest.params.insert("theme".into(), "light".into());
    let notes_pages = [
        (
            "index",
            SitePage::markdown(
                "Field Notes",
                "# Field Notes\n\nYou followed a **cross-site link** to get here — a `site:demo-notes/index` target that stayed inside the entity system, hopping from one owned site to another on the same peer.\n\nNotice the light palette: this site's manifest declares `\"theme\": \"light\"`, so in **Site's theme** mode it renders with its own registered theme while the Demo next door stays dark. (Your Settings → Site appearance override always wins.)\n\nRead the [First Entry](entries/first), or head [back to the Demo](site:demo/index).\n",
            ),
        ),
        (
            "entries/first",
            SitePage::markdown(
                "Field Notes — First Entry",
                "# First Entry\n\nA nested page (`entries/first`) in the companion site. Cross-site links land on a site's pages the same way in-site links resolve within one.\n\nBack to the [Notes index](index), or [back to the Demo](site:demo/index).\n",
            ),
        ),
    ];
    peers.seed_write(
        peer_id,
        paths::manifest_path(peer_id, DEMO_NOTES_SITE_ID),
        notes_manifest.to_entity(),
    );
    for (slug, page) in notes_pages {
        peers.seed_write(peer_id, paths::page_path(peer_id, DEMO_NOTES_SITE_ID, slug), page.to_entity());
    }
}
