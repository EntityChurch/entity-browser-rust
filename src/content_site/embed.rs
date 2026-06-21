//! The embed standard — the L5 "embedded asset" representation, plus the
//! translators that lower each input grammar into it.
//!
//! Content arrives with images in two grammars: the papers render tool's
//! structured directive `::embed[fallback]{ref=...}` (the canonical wire
//! form, used for compute figures) and plain markdown `![alt](src)` (the
//! lightweight, universal form). **Both lower to one [`Embed`].** The model:
//! translate every grammar UP into the canonical `::embed` directive at
//! **ingest** ([`markdown_to_embed`]) so the stored page body speaks one
//! standard — per-source-base translators live at that one seam, and a future
//! base ships its own lowering rule. The **renderer** lowers `::embed` back to
//! a sanitized markdown image ([`embed_to_markdown_image`]) so images stay in
//! pulldown_cmark's generated-`<img>` lane (no raw-HTML injection), and the
//! DOM layer resolves the `ref` against the site's asset subgraph.
//!
//! The `ref` is a **site-relative** asset path (`assets/figures/x.png`),
//! resolved at render/closure time against `/{peer}/sites/{site}/assets/…`
//! (the bytes are content-addressed, so the same image dedups across sites).
//! [`parse_embeds`] / [`embed_refs`] read the refs straight off the stored
//! body for the closure walk (publish / cache).

#![allow(dead_code)] // consumers (ingest normalize, render lower, closure walk) land alongside

use std::ops::Range;

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// One embedded asset — the L5 understanding of an image/figure/media node.
/// Today only images are produced; the type generalizes (a `media_type`
/// attribute is the next field when non-image embeds land).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Embed {
    /// Authored text fallback (alt text / caption). May be empty.
    pub fallback: String,
    /// The asset reference — a site-relative path (`assets/figures/x.png`)
    /// resolved against the site's asset subgraph at render/closure time.
    pub reference: String,
}

/// The markdown options the site renderer uses — kept in sync with
/// [`super::render::markdown_to_html`] so offset spans match what is rendered.
fn md_options() -> Options {
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS
}

/// Translate every markdown image `![alt](src)` in `body` into the canonical
/// `::embed[alt]{ref=src}` directive, leaving the rest of the body untouched.
/// This is the **default-base ingest translator**: it lowers markdown's image
/// grammar up into the embed standard. Idempotent on a body that already uses
/// `::embed` (it has no markdown images to translate).
pub fn markdown_to_embed(body: &str) -> String {
    let mut reps: Vec<(Range<usize>, Embed)> = Vec::new();
    let mut iter = Parser::new_ext(body, md_options()).into_offset_iter();
    while let Some((event, range)) = iter.next() {
        let Event::Start(Tag::Image { dest_url, .. }) = &event else {
            continue;
        };
        let reference = dest_url.to_string();
        // The Start(Image) range spans the whole `![alt](src)` construct; the
        // alt text is the concatenation of the inner text events.
        let mut fallback = String::new();
        for (ev, _r) in iter.by_ref() {
            match ev {
                Event::End(TagEnd::Image) => break,
                Event::Text(t) | Event::Code(t) => fallback.push_str(&t),
                _ => {}
            }
        }
        reps.push((range.clone(), Embed { fallback, reference }));
    }
    apply_replacements(body, reps, format_embed)
}

/// Lower every `::embed[fallback]{ref=…}` directive back into a markdown image
/// `![fallback](ref)` for the HTML renderer. Keeping it markdown means
/// pulldown_cmark emits a plain `<img alt src>` (no event handlers, no raw
/// HTML) — the DOM layer then rewrites the `src` to the resolved asset.
pub fn embed_to_markdown_image(body: &str) -> String {
    let reps = scan_embeds(body);
    apply_replacements(body, reps, |e| {
        format!("![{}]({})", md_escape_alt(&e.fallback), encode_ref(&e.reference))
    })
}

/// Every embed in `body`, in document order. The closure walk (publish /
/// cache) reads this off the stored body to find the assets a page pulls in.
pub fn parse_embeds(body: &str) -> Vec<Embed> {
    scan_embeds(body).into_iter().map(|(_, e)| e).collect()
}

/// The asset references of every embed in `body` (de-duplicated, order
/// preserved). The set of asset paths a page's closure must carry.
pub fn embed_refs(body: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    parse_embeds(body)
        .into_iter()
        .map(|e| e.reference)
        .filter(|r| !r.is_empty() && seen.insert(r.clone()))
        .collect()
}

/// Render one [`Embed`] to the canonical directive string.
pub fn format_embed(e: &Embed) -> String {
    format!("::embed[{}]{{ref={}}}", escape_fallback(&e.fallback), e.reference)
}

/// Minimal, dependency-free standard base64 (RFC 4648) encoder. The DOM layer
/// uses it to inline asset bytes into an `<img>` `data:` URL; co-located here
/// (the asset-support module) so it's native-testable rather than buried in the
/// WASM-only renderer.
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}

// ── internals ────────────────────────────────────────────────────────────

const EMBED_OPEN: &str = "::embed[";

/// Scan `body` for `::embed[fallback]{attrs}` directives, returning each one's
/// byte range and the parsed [`Embed`]. Tolerant: the fallback ends at the
/// first `]{` (so a `]` inside the fallback is fine unless immediately followed
/// by `{`), and the attrs end at the next `}`. `ref=` is pulled from the attrs.
fn scan_embeds(body: &str) -> Vec<(Range<usize>, Embed)> {
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(rel) = body[search..].find(EMBED_OPEN) {
        let start = search + rel;
        let after_open = start + EMBED_OPEN.len();
        let Some(fb_rel) = body[after_open..].find("]{") else { break };
        let fb_end = after_open + fb_rel;
        let fallback = body[after_open..fb_end].to_string();
        let attrs_start = fb_end + "]{".len();
        let Some(close_rel) = body[attrs_start..].find('}') else { break };
        let attrs_end = attrs_start + close_rel;
        let reference = parse_ref(&body[attrs_start..attrs_end]).unwrap_or_default();
        let end = attrs_end + 1; // include the closing `}`
        out.push((start..end, Embed { fallback, reference }));
        search = end;
    }
    out
}

/// Pull the `ref=` value out of a directive's attribute blob (space-separated
/// `key=value` tokens). Surrounding quotes are trimmed.
fn parse_ref(attrs: &str) -> Option<String> {
    attrs.split_whitespace().find_map(|tok| {
        tok.strip_prefix("ref=").map(|v| v.trim_matches(|c| c == '"' || c == '\'').to_string())
    })
}

/// Splice `reps` into `body`, replacing each range with `render(embed)`.
/// Overlapping/out-of-order ranges are dropped defensively.
fn apply_replacements(
    body: &str,
    mut reps: Vec<(Range<usize>, Embed)>,
    render: impl Fn(&Embed) -> String,
) -> String {
    if reps.is_empty() {
        return body.to_string();
    }
    reps.sort_by_key(|(r, _)| r.start);
    let mut out = String::with_capacity(body.len());
    let mut pos = 0;
    for (r, e) in &reps {
        if r.start < pos || r.end > body.len() {
            continue; // overlap / stale range — skip
        }
        out.push_str(&body[pos..r.start]);
        out.push_str(&render(e));
        pos = r.end;
    }
    out.push_str(&body[pos..]);
    out
}

/// Escape a fallback for the `::embed[…]` slot: a literal `]{` would close the
/// fallback early, so break that adjacency. (Fallbacks from the render tool are
/// plain prose; this is belt-and-suspenders.)
fn escape_fallback(s: &str) -> String {
    s.replace("]{", "] {")
}

/// Escape alt text for a markdown `![…]` slot: brackets would unbalance it.
fn md_escape_alt(s: &str) -> String {
    s.replace('[', "\\[").replace(']', "\\]").replace('\n', " ")
}

/// Encode an asset ref for a markdown `(…)` slot: spaces and parens would break
/// the link. Asset paths are normally clean; this guards the edge.
fn encode_ref(s: &str) -> String {
    s.replace(' ', "%20").replace('(', "%28").replace(')', "%29")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_image_lowers_to_embed() {
        let body = "Intro.\n\n![A landscape figure](assets/figures/landscape.png)\n\nMore.";
        let out = markdown_to_embed(body);
        assert!(
            out.contains("::embed[A landscape figure]{ref=assets/figures/landscape.png}"),
            "got: {out}"
        );
        // Surrounding prose is preserved verbatim.
        assert!(out.starts_with("Intro."));
        assert!(out.trim_end().ends_with("More."));
        // No markdown image syntax remains.
        assert!(!out.contains("!["), "markdown image should be gone: {out}");
    }

    #[test]
    fn multiple_images_all_lower_and_text_survives() {
        let body = "![one](a.png)\n\nmiddle\n\n![two](b.png)";
        let out = markdown_to_embed(body);
        assert!(out.contains("::embed[one]{ref=a.png}"), "got: {out}");
        assert!(out.contains("::embed[two]{ref=b.png}"), "got: {out}");
        assert!(out.contains("middle"));
    }

    #[test]
    fn body_with_no_images_is_unchanged() {
        let body = "# Title\n\nJust [a link](./about) and **bold**, no images.";
        assert_eq!(markdown_to_embed(body), body);
    }

    #[test]
    fn parses_the_render_tools_own_directive() {
        // The exact form `render/build.go` emits for a compute figure.
        let body = "::embed[Figure: Entity Landscape — text fallback for the entity-landscape figure.]{ref=assets/figures/entity-landscape.png}";
        let embeds = parse_embeds(body);
        assert_eq!(embeds.len(), 1);
        assert_eq!(embeds[0].reference, "assets/figures/entity-landscape.png");
        assert!(embeds[0].fallback.starts_with("Figure: Entity Landscape"));
    }

    #[test]
    fn embed_lowers_to_markdown_image_for_render() {
        let body = "Before.\n\n::embed[Caption]{ref=assets/figures/x.png}\n\nAfter.";
        let out = embed_to_markdown_image(body);
        assert!(out.contains("![Caption](assets/figures/x.png)"), "got: {out}");
        assert!(out.contains("Before."));
        assert!(out.contains("After."));
        assert!(!out.contains("::embed"), "directive should be gone: {out}");
    }

    #[test]
    fn markdown_then_embed_round_trips_to_an_equivalent_image() {
        // ingest lowers ![]() → ::embed; render lowers ::embed → ![]().
        let body = "![alt text](assets/figures/y.png)";
        let stored = markdown_to_embed(body);
        let rendered_md = embed_to_markdown_image(&stored);
        assert_eq!(rendered_md, "![alt text](assets/figures/y.png)");
    }

    #[test]
    fn embed_refs_dedupes_and_preserves_order() {
        let body = "::embed[a]{ref=p/1.png}\n::embed[b]{ref=p/2.png}\n::embed[c]{ref=p/1.png}";
        assert_eq!(embed_refs(body), vec!["p/1.png".to_string(), "p/2.png".to_string()]);
    }

    #[test]
    fn directive_with_extra_attrs_still_finds_ref() {
        let body = "::embed[cap]{ref=assets/z.png type=image/png}";
        let embeds = parse_embeds(body);
        assert_eq!(embeds.len(), 1);
        assert_eq!(embeds[0].reference, "assets/z.png");
    }

    #[test]
    fn malformed_directive_is_left_alone() {
        // No closing brace → not a directive; scan finds nothing, body intact.
        let body = "::embed[unterminated]{ref=oops";
        assert!(parse_embeds(body).is_empty());
        assert_eq!(embed_to_markdown_image(body), body);
    }

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors (the padding edges are what matter).
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // A non-ASCII byte (PNG magic) round-trips into the alphabet, no panic.
        assert_eq!(base64_encode(&[0x89, 0x50, 0x4e, 0x47]), "iVBORw==");
    }

    #[test]
    fn alt_text_with_brackets_is_escaped_when_lowering() {
        // A fallback containing `]` must not break the generated markdown image.
        let body = "::embed[see fig [2]]{ref=a.png}";
        let out = embed_to_markdown_image(body);
        // The generated alt escapes the brackets; the ref is intact.
        assert!(out.contains("(a.png)"), "got: {out}");
        assert!(out.contains("\\]"), "brackets in alt should be escaped: {out}");
    }
}
