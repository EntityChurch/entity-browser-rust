//! **An inline style outranks every stylesheet, so one attribute can make a
//! shipped CSS fix inert on one surface and nowhere say so.**
//!
//! `entity-core-papers` reported published figures as unreadable
//! (`docs/FIGURE-PRESENTATION-HANDOFF.md`): a diagram shares the prose measure,
//! and eight of their eleven published views rendered 11 px notes under 8 px.
//! The fix is one rule — `doc_css::figure_css` — clamped by each host, and it
//! lands on the static exporter immediately because there nothing but the
//! stylesheet touches an image.
//!
//! **On the live surface it did not.** `dom::content_site::rewrite_images`
//! resolves each `src` to a `data:` URL and, until 2026-09-19, also set
//! `style="max-width:100%;height:auto;"` on the element. That was redundant the
//! day it was written — the body is mounted as `.cs-doc`, so `doc_css`'s own
//! `img` rule already said it — and it became load-bearing in the wrong
//! direction the moment a figure earned a wider rule: CSS resolves an inline
//! declaration above any author stylesheet, so every figure stayed pinned to
//! the prose measure **on the surface the operator was actually looking at**,
//! with the identical rule visibly working on the exported page.
//!
//! ⚠ **This is the shape that costs a session, not the shape that reds a
//! test.** Both surfaces would have been green: the exporter's gate asserts the
//! emitted CSS and passes, and there is no native gate that can mount a DOM. A
//! half-landed fix reports success and the defect survives under a closed
//! ticket.
//!
//! **Why a census and not a behavioural gate.** `rewrite_images` takes a
//! `web_sys::Element` and is `cfg(wasm32)` in practice, so `make test` cannot
//! call it and no native assertion can observe what it sets. The honest
//! instrument left is the source text, and the honest thing to say about a text
//! check is what it cannot see: it reads one function in one file, it cannot
//! evaluate a style string built at runtime, and it would not notice a sizing
//! declaration added from a different function. What it does cover is the exact
//! regression that already happened — a well-meaning "keep images from blowing
//! out the column" line reappearing next to the resolve.
//!
//! **Falsified both ways below**, on synthetic input, because *a census you
//! have not falsified reports what you hoped*.

/// The sizing declarations that must never appear inline on a content image.
/// `height` is here beside `width` because the pair is how the old line was
/// spelled and how the next one will be — a figure's aspect is the
/// stylesheet's business, and an inline `height:auto` is the tell that someone
/// re-stated the sizing contract rather than reading it.
const SIZING: [&str; 4] = ["max-width", "min-width", "width:", "height:"];

/// The function whose job is to bind a `ref` to bytes — and only that.
const FN_HEAD: &str = "fn rewrite_images(";

fn dom_source() -> String {
    std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/dom/content_site.rs"))
        .expect("src/dom/content_site.rs is readable from the test's manifest dir")
}

/// The body of `rewrite_images`, from its signature to the next top-level
/// `fn` — deliberately a *region*, not the whole file, so a sizing style set
/// legitimately somewhere else (the nav, the chrome) is not this test's
/// business and cannot make it fire.
fn rewrite_images_body(source: &str) -> Result<&str, String> {
    let start = source.find(FN_HEAD).ok_or_else(|| {
        format!("`{FN_HEAD}` is gone — if it was renamed, this census now measures nothing")
    })?;
    let rest = &source[start + FN_HEAD.len()..];
    let end = rest.find("\nfn ").unwrap_or(rest.len());
    Ok(&rest[..end])
}

/// Every line that sets an attribute named `style`, with its sizing content.
fn inline_sizing_styles(body: &str) -> Vec<String> {
    body.lines()
        .filter(|l| {
            let code = l.split("//").next().unwrap_or("");
            code.contains("set_attribute") && code.contains("\"style\"")
        })
        .filter(|l| SIZING.iter().any(|d| l.contains(d)))
        .map(|l| l.trim().to_string())
        .collect()
}

#[test]
fn the_image_rewriter_binds_bytes_and_never_re_states_the_sizing_contract() {
    let source = dom_source();
    let body = rewrite_images_body(&source).expect("the rewriter is still there");
    let found = inline_sizing_styles(body);
    assert!(
        found.is_empty(),
        "`rewrite_images` sets image sizing inline, which outranks `doc_css` and pins every \
         figure to the prose measure on the live surface while the exported page renders it \
         correctly — the exact half-landed state of 2026-09-19.\n\
         Offending line(s):\n  {}\n\n\
         Sizing belongs to `content_site::doc_css` (S-T1) and to the host's own clamp \
         (`figure_css`). If an image genuinely needs a per-element size, it needs a class and \
         a rule in the sheet, not an attribute.",
        found.join("\n  ")
    );
}

#[test]
fn the_census_fires_on_the_line_it_was_written_about() {
    // The real regression, verbatim from the pre-fix source.
    let neutered = format!(
        "{FN_HEAD}body: &Element) {{\n\
         img.set_attribute(\"src\", &data_url()).ok();\n\
         img.set_attribute(\"style\", \"max-width:100%;height:auto;\").ok();\n\
         }}\nfn next() {{}}"
    );
    let body = rewrite_images_body(&neutered).expect("region found");
    assert_eq!(
        inline_sizing_styles(body).len(),
        1,
        "the census cannot see the defect it exists for: {neutered}"
    );
}

#[test]
fn the_census_does_not_fire_on_a_style_that_is_not_about_size() {
    // A non-sizing inline style is not this test's business — a census that
    // fires on anything nearby teaches people to skip it.
    let benign = format!(
        "{FN_HEAD}body: &Element) {{\n\
         img.set_attribute(\"style\", \"image-rendering:crisp-edges;\").ok();\n\
         }}\nfn next() {{}}"
    );
    let body = rewrite_images_body(&benign).expect("region found");
    assert!(inline_sizing_styles(body).is_empty(), "false positive on a non-sizing style");
}

#[test]
fn a_renamed_rewriter_fails_loudly_rather_than_reporting_clean() {
    // The census's own vacuity: with the function gone, "no offending lines" is
    // indistinguishable from "nothing to look at". It must say which.
    let err = rewrite_images_body("fn something_else() {}").unwrap_err();
    assert!(err.contains("measures nothing"), "{err}");
}
