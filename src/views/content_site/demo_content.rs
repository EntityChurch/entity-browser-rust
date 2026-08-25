//! The bundled demo site’s **content bodies** — the page/asset payloads
//! `ensure_demo_site` seeds, extracted from the seeding logic beside it.
//!
//! These are **content, not UI**, and the split is what makes that checkable.
//! A demo document carries its own stylesheet and a demo figure its own
//! palette *by definition*: the document renders in an opaque-origin frame and
//! the figure is asset bytes, so `--site-*` tokens cannot reach either one.
//! Their hex colours are therefore data, indistinguishable from a colour in a
//! user’s uploaded image — which is why `tools/ui-lint.sh` exempts this file
//! by name rather than carrying its palette in the raw-hex baseline. Nothing
//! that renders through *our* DOM belongs here; put that in `mod.rs`, where
//! the lint still counts it.
//!
//! i18n-ignore-file — these are content **bodies**, not application chrome.
//! The browser does not translate the pages it renders (a published paper
//! arrives in the language its author wrote it in), and the demo site is a
//! published site like any other. The app's own strings around it — nav,
//! breadcrumbs, the empty-document notice — go through `crate::i18n::t` and
//! are counted normally.

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
