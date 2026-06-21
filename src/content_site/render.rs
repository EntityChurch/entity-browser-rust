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

#![allow(dead_code)] // renderer/model consumers land alongside this in P1

use pulldown_cmark::{html, Event, Options, Parser};

/// Render a page body to sanitized HTML, honoring the page `format`.
///
/// **Security boundary (F-CONTENT-1, drift audit).** The page
/// `format` field carries an `html` "web escape hatch" (`content_site/format.rs`
/// §3.1), but **there is no HTML sanitizer in the tree** and content-site bodies
/// are *untrusted cross-peer data* (a hash-valid page can still be malicious).
/// Honoring `format: html` as **raw passthrough** into `set_inner_html` would be
/// instant stored-XSS. Until an allowlist sanitizer lands, raw HTML is
/// **forbidden**: every format renders through [`markdown_to_html`], which
/// escapes embedded HTML to inert text. An `html`-format page is rendered as
/// escaped text (and logged once) rather than silently honored — so wiring raw
/// HTML here becomes a *deliberate* act, not a default. Decision recorded in the
/// audit's §2c security cluster ("cheap hardening now").
pub fn render_page_body(format: &str, body: &str) -> String {
    if format == "html" {
        // The escape hatch is declared in the page model but NOT honored as raw
        // HTML — no sanitizer exists. Render as escaped text via the markdown
        // path. Do NOT change this to pass `body` raw without an allowlist
        // sanitizer (F-CONTENT-1).
        tracing::warn!(
            "content-site page declares format:html — rendered as escaped text \
             (no HTML sanitizer; raw passthrough forbidden, F-CONTENT-1)"
        );
    }
    markdown_to_html(body)
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
    fn html_format_is_not_honored_as_raw_passthrough() {
        // F-CONTENT-1: a page declaring format:html must NOT inject raw HTML.
        // It is rendered as escaped text, identically to the markdown path.
        let malicious = "<script>alert(document.cookie)</script><img src=x onerror=alert(1)>";
        let via_html = render_page_body("html", malicious);
        let via_md = render_page_body("markdown", malicious);
        assert_eq!(via_html, via_md, "html format must route through the same escaping path");
        // The security property is that the markup is ESCAPED — no live tags.
        // (The literal text "onerror=" survives inside escaped text, but inert:
        // `&lt;img ... onerror=...&gt;` is displayed text, not an attribute.)
        assert!(!via_html.contains("<script>"), "raw <script> must not pass through: {via_html}");
        assert!(!via_html.contains("<img"), "raw <img> tag must not pass through: {via_html}");
        assert!(via_html.contains("&lt;script&gt;"), "script must be escaped to text: {via_html}");
        assert!(via_html.contains("&lt;img"), "img must be escaped to text: {via_html}");
    }
}
