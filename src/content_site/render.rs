//! Markdown → HTML for content-site pages.
//!
//! Pure (no DOM, no peers) so it's unit-testable on native. The DOM
//! renderer (`dom/content_site.rs`) calls this, mounts the result, then
//! rewrites entity-native `<a>` links into in-app navigation.
//!
//! **Raw HTML is neutralized** (v0 safety): markdown's embedded HTML
//! blocks/spans are rendered as escaped *text* rather than passed
//! through, so a page body can't inject `<script>` or arbitrary markup.
//! A constrained allowlist for intentional embedded HTML (menus, etc.)
//! is a later refinement (design doc §3.4).
//!
//! **A `format:html` page is a different KIND of output, so it has a different
//! TYPE** — see [`PageRender`]. The two are not interchangeable strings, and
//! that is the whole safety design: a caller physically cannot hand a raw
//! document to `set_inner_html` by forgetting a branch.

#![allow(dead_code)] // renderer/model consumers land alongside this in P1

use pulldown_cmark::{html, Event, Options, Parser};

/// What a page body rendered *into*, and therefore how it must be mounted.
///
/// The two variants are deliberately **not** both `String`. `format:html`
/// (`APP-CONVENTION-SEMANTIC-CONTENT-SITE` §3.1) carries a complete, untrusted
/// HTML document; sanitized markdown output carries markup we generated. Both
/// are "HTML" and mounting one the way you mount the other is stored-XSS, so
/// the distinction lives in the type rather than in a comment a call site can
/// skip. (This is the "give the two outcomes different types" move, applied
/// before the bug rather than after it.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRender {
    /// Markup **we** generated from markdown — every embedded HTML span was
    /// escaped to inert text ([`markdown_to_html`]). Safe to `set_inner_html`
    /// into our own document; that is what the link/image rewriters expect.
    Markup(String),
    /// A complete, **untrusted** HTML document (`format:html`), verbatim.
    ///
    /// **MUST be mounted in a restricted sandbox, never in our document.**
    /// The host is `dom::content_site`'s document frame: an `<iframe sandbox="">`
    /// with *every* restriction on — no scripts, opaque origin, no forms, no
    /// navigation. That is a strictly stronger answer than the allowlist
    /// sanitizer gate G2 anticipates (§8 flags allowlist completeness as the
    /// fragile part, and "text-escaping ≠ sanitization"): nothing in the
    /// document executes, so completeness is not a property we have to get
    /// right. F-CONTENT-1's rule — raw HTML never reaches *our* origin — is
    /// preserved, not relaxed.
    Document(String),
}

/// An empty render is the **inert** one. Hand-written rather than derived so
/// the choice is explicit: defaulting to `Document` would make a value nobody
/// filled in eligible for the raw-mount path.
impl Default for PageRender {
    fn default() -> Self {
        PageRender::Markup(String::new())
    }
}

impl PageRender {
    /// The rendered markup, whatever the variant — for callers that only need
    /// the bytes (the static exporter writes both to a file). A DOM caller must
    /// match on the variant instead; mounting is variant-specific.
    pub fn as_str(&self) -> &str {
        match self {
            PageRender::Markup(s) | PageRender::Document(s) => s,
        }
    }

    /// True when this is an untrusted document needing the sandboxed host.
    pub fn is_document(&self) -> bool {
        matches!(self, PageRender::Document(_))
    }
}

/// Render a page body according to its `format`, returning *how it must be
/// mounted* along with the markup.
///
/// **Security boundary (F-CONTENT-1, drift audit).** Content-site bodies are
/// *untrusted cross-peer data* — a hash-valid page can still be malicious, and
/// the hash proves only that the publisher signed exactly these bytes. So raw
/// HTML must never reach our origin. It previously could not reach *anywhere*:
/// `format:html` was downgraded to escaped text, because the only mount we had
/// was `set_inner_html` and there is still no sanitizer in the tree.
///
/// It now reaches a sandbox instead of a sanitizer. [`PageRender::Document`]
/// documents the tier and why it is stronger than the sanitizer G2 assumes.
/// **Do not "simplify" this to return a bare `String`** — the type is the
/// enforcement point.
pub fn render_page(format: &str, body: &str) -> PageRender {
    if format == super::format::HTML_PAGE_FORMAT {
        PageRender::Document(body.to_string())
    } else {
        PageRender::Markup(markdown_to_html(body))
    }
}

/// Render a markdown body to an HTML string, neutralizing raw HTML.
///
/// Embeds are lowered first: the stored body's canonical `::embed[fallback]
/// {ref=…}` directives become markdown images (`![fallback](ref)`) so they
/// render through pulldown_cmark as plain `<img alt src>` — no raw HTML, no
/// event handlers. The `src` stays the site-relative ref (`assets/figures/…`);
/// the DOM layer resolves it to the asset bytes after mount (see
/// `dom/content_site::rewrite_images`).
pub fn markdown_to_html(markdown: &str) -> String {
    let lowered = super::embed::embed_to_markdown_image(markdown);
    let markdown = lowered.as_str();
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;

    let parser = Parser::new_ext(markdown, options).map(|event| match event {
        // Demote raw HTML (block + inline) to text so push_html escapes
        // it instead of emitting it live. Inert but visible.
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
        other => other,
    });

    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_heading_and_emphasis() {
        let html = markdown_to_html("# Title\n\nHello **world** and _italics_.");
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>world</strong>"));
        assert!(html.contains("<em>italics</em>"));
    }

    #[test]
    fn preserves_link_href_for_later_rewrite() {
        // The renderer rewrites these hrefs into nav handlers; the codec
        // just has to keep the href intact.
        let html = markdown_to_html("See [About](./about) and [Labs](entity://P/sites/l/pages/i).");
        assert!(html.contains(r#"href="./about""#));
        assert!(html.contains(r#"href="entity://P/sites/l/pages/i""#));
    }

    #[test]
    fn neutralizes_raw_html_block() {
        let html = markdown_to_html("<script>alert(1)</script>\n\nsafe");
        assert!(!html.contains("<script>"), "raw <script> must not pass through: {html}");
        assert!(html.contains("&lt;script&gt;"), "should be escaped to text: {html}");
    }

    #[test]
    fn neutralizes_inline_html() {
        let html = markdown_to_html("text with <b>inline</b> html");
        assert!(!html.contains("<b>inline</b>"));
        assert!(html.contains("&lt;b&gt;"));
    }

    #[test]
    fn renders_lists_and_code() {
        let html = markdown_to_html("- one\n- two\n\n`code`");
        assert!(html.contains("<ul>"));
        assert!(html.contains("<li>one</li>"));
        assert!(html.contains("<code>code</code>"));
    }

    #[test]
    fn embed_directive_renders_as_a_plain_img() {
        // The stored canonical form lowers to a sanitized <img> (alt + the
        // site-relative src the DOM layer later resolves) — no raw HTML.
        let html = markdown_to_html("::embed[A figure]{ref=assets/figures/x.svg}");
        assert!(html.contains("<img"), "embed should render an <img>: {html}");
        assert!(html.contains(r#"src="assets/figures/x.svg""#), "src is the ref: {html}");
        assert!(html.contains(r#"alt="A figure""#), "alt is the fallback: {html}");
    }

    #[test]
    fn embed_img_has_no_event_handler_attributes() {
        // Lowering keeps images in pulldown_cmark's generated-<img> lane, which
        // escapes the alt text — so a hostile fallback cannot break out of the
        // alt="" attribute to inject a live `onerror=` handler. The literal
        // text survives, but only as inert escaped content (`onerror=&quot;`).
        let html = markdown_to_html("::embed[x\" onerror=\"alert(1)]{ref=assets/a.png}");
        assert!(!html.contains("onerror=\""), "no live onerror attribute may form: {html}");
        assert!(html.contains("&quot;"), "the breakout quote must be escaped: {html}");
    }

    #[test]
    fn a_markdown_page_never_yields_a_document() {
        // F-CONTENT-1, the half that has NOT changed: raw HTML written into a
        // *markdown* body is still escaped to inert text, and the result is
        // `Markup` — the variant the DOM layer is allowed to `set_inner_html`.
        let malicious = "<script>alert(document.cookie)</script><img src=x onerror=alert(1)>";
        let rendered = render_page("markdown", malicious);
        let PageRender::Markup(html) = &rendered else {
            panic!("a markdown page must render as Markup, got {rendered:?}");
        };
        assert!(!html.contains("<script>"), "raw <script> must not pass through: {html}");
        assert!(!html.contains("<img"), "raw <img> tag must not pass through: {html}");
        assert!(html.contains("&lt;script&gt;"), "script must be escaped to text: {html}");
        assert!(html.contains("&lt;img"), "img must be escaped to text: {html}");
    }

    #[test]
    fn an_unknown_format_falls_closed_to_the_escaping_path() {
        // A format we don't know (a future base, a typo, a hostile publisher's
        // invention) must NOT be treated as a document. Fail closed: anything
        // that isn't exactly `html` goes through the escaping markdown path.
        for format in ["", "HTML", "html5", "text/html", "xhtml", "markdown", "weird"] {
            let rendered = render_page(format, "<script>alert(1)</script>");
            assert!(
                !rendered.is_document(),
                "format {format:?} must not be honored as a raw document"
            );
            assert!(
                !rendered.as_str().contains("<script>"),
                "format {format:?} must escape raw HTML: {}",
                rendered.as_str()
            );
        }
    }

    #[test]
    fn an_html_page_is_carried_verbatim_as_a_document() {
        // The escape hatch is honored now — but as a *typed* document, whose
        // only mount is the restricted sandbox. The bytes are untouched: a book
        // is a complete standalone file and rewriting any of it would corrupt
        // it. The safety lives in WHERE this goes, not in what it contains.
        let doc = "<!DOCTYPE html><html><head><style>body{color:red}</style></head>\
                   <body><h1>Paper 0</h1><script>alert(1)</script></body></html>";
        let rendered = render_page("html", doc);
        let PageRender::Document(out) = &rendered else {
            panic!("an html page must render as Document, got {rendered:?}");
        };
        assert_eq!(out, doc, "a document is carried byte-for-byte");
        assert!(rendered.is_document());
    }

    #[test]
    fn the_two_variants_are_not_interchangeable_for_the_same_bytes() {
        // The property that makes the type the enforcement point: identical
        // input bytes produce two values that are NOT equal, so a call site
        // cannot accidentally treat one as the other. If someone "simplifies"
        // this back to a bare String return, this test is what goes red.
        let body = "<b>hi</b>";
        assert_ne!(render_page("html", body), render_page("markdown", body));
    }
}
