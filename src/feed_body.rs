//! How a feed entry's `body` becomes something a person reads — EMBED §6's
//! degradation ladder, made a decision instead of a habit.
//!
//! ## ⭐⭐ Why this module exists: we were shipping the ladder's LAST RUNG as the renderer
//!
//! `APP-CONVENTION-FEED` §2.3 types `body` as an `embed-node` and states in as
//! many words that *"a photo post and a text post are one shape with a different
//! embed inside"*. `APP-CONVENTION-EMBED` §6 then defines a **ladder**: render
//! the payload if you can, and fall back to the authored `fallback` sentence if
//! you cannot — §8's anti-graveyard rule, so an embed nobody can render is still
//! a sentence rather than a hole.
//!
//! Until 2026-09-16 the Feed window drew **`body.data.fallback`, always**, at
//! both of its call sites, and `media_type` appeared **zero** times in
//! `dom::feed`. So an image post rendered as its alt text and a markdown post
//! rendered as its plain-text degradation — not as a bug anybody wrote, but
//! because *the bottom of the ladder is a correct-looking string* and nothing
//! ever asked whether there was a rung above it. ⇒ **a degradation path that is
//! always taken is indistinguishable from a renderer, and it is the one shape a
//! green test suite cannot see**, because every assertion about *"the text is on
//! screen"* passes either way.
//!
//! ## The coherence this buys, which is the operator's ask and not a nicety
//!
//! `app/site-page` carries `format: "markdown"/"html"` with `::embed` directives
//! (SITE §3.1/§3.2); `app/feed/entry` carries an `embed-node`. **Both import
//! EMBED.** So a markdown feed entry, rendered by the same sanitized renderer a
//! site page goes through, is *conformant today with no spec change* — and it is
//! what makes *"learn SITE, and you know how a feed entry looks"* true rather
//! than aspirational.
//!
//! [`crate::content_site::markdown_to_html`] is that renderer, reused rather
//! than restated: it escapes raw HTML to inert text, resolves `entity://` links
//! and parses `::embed` directives, and it carries the XSS tests. **A second
//! markdown path in this crate would be C15 with a sanitizer on the end of it**
//! — the worst possible place for two expressions of one rule.
//!
//! ## The order of the checks is the correctness argument
//!
//! 1. **`render_verdict` first, before the media type is even looked at.**
//!    EMBED §3's passive-only rule is about the *node*, not about what it
//!    carries: a non-empty `requires`/`sandbox` declares an **active** embed and
//!    v0.2 has no enforcement to honour it with. Checking the media type first
//!    would render an active `text/markdown` node, which is exactly the refusal
//!    that rule exists to make.
//! 2. **Then the payload arm.** `Inline` bytes are in hand by construction; a
//!    `Pointer` is in hand only if the **async** side resolved its
//!    `system/content` blob and handed it to [`decide_with`], because this
//!    decision is consulted from inside a synchronous render pass. A `Child`
//!    needs a fetch nothing here builds.
//!
//!    ⭐ **That arm is not an edge case — it is the only path to a long post.**
//!    EMBED §3 caps an inline payload at 16 KiB, so anything above it is
//!    *required* to be a pointer. Until 2026-09-16 nothing on the reader side
//!    resolved one, so a long post arrived as its title while its bytes sat
//!    published and verified at the origin. See [`BodyBlob`].
//! 3. **Then the media type**, and an unknown one is *the ladder working*, never
//!    a defect.
//!
//! ## Every fallback says WHY (AP40)
//!
//! [`FallbackReason`] has four arms and they name **three different parties**:
//! *the author declared something we refuse to run* · *we hold no bytes yet* ·
//! **ours** — *we have no renderer for this type* · *theirs* — *the bytes are
//! not text*. Collapsing them would put *"this build cannot show a PNG"* and
//! *"this author shipped broken bytes"* through one sentence, and those route to
//! different people. It is the same split
//! [`crate::embed::PayloadError`] already makes one layer down.
//!
//! **And *we hold no bytes yet* splits again**, because it had the same defect
//! one level in: [`BlobMiss`] separates *nobody looked* (ours — a source with no
//! blob resolver) from *the origin has none* (theirs — a signed closure naming a
//! blob they do not serve) from *we could not look* (neither — transport). One
//! `not-in-hand` would have made our missing capability read as the publisher's
//! broken publish.
//!
//! ## Pure, and native
//!
//! No DOM, no store, no clock — so `make test` gates the whole ladder on both
//! arms instead of through Selenium. The renderer's only job is to place the
//! result, and placing it is the one thing that *must* be `cfg(wasm32)`.

use crate::embed::{EmbedNode, EmbedPayload, RenderVerdict};

/// The IANA media type a markdown body carries, making its entity type tag
/// `app/embed/text/markdown`.
///
/// ⚠ **The corpus spells this two ways and we picked the one the CDDL demands.**
/// EMBED's §1.1 prose cites *"a transcluded `app/embed/markdown` fragment"*
/// (media-type = `markdown`), while its CDDL is `type = "app/embed/" .cat
/// media-type` with every worked example an IANA type (`image/png`,
/// `text/plain`). `text/markdown` is IANA-registered (RFC 7763), so it is the
/// only spelling that satisfies the normative half — but the prose is what an
/// implementer reads first, and a second seat reading it reaches a different
/// tag for the same bytes with **nothing failing loudly**: a type-filtered query
/// on the wrong tag returns a correct, complete, empty answer (SHARE §2).
/// **And the choice was already made and already published**, which is what
/// settles it: [`crate::feed_ingest`] has minted `text/markdown` since it
/// shipped, so every post in `entity-core-papers`' 58-entry corpus carries this
/// tag on the wire. `G-PIN-4`'s rule — *whatever publishes first is the
/// baseline* — so this constant records the baseline rather than picking one,
/// and `feed_ingest` consumes it rather than restating it.
///
/// Pinned by [`tests::the_markdown_media_type_is_the_iana_one`] so a ruling
/// moves one file, and routed rather than decided quietly.
pub const MARKDOWN_MEDIA_TYPE: &str = "text/markdown";

/// The IANA media type a plain-text body carries.
pub const PLAIN_MEDIA_TYPE: &str = "text/plain";

/// What a renderer should place for this body.
///
/// Three outcomes, and [`BodyRender::Markup`] is the only one that may be
/// treated as HTML — the variant *is* the safety claim, so a renderer cannot
/// reach for `set_inner_html` on a value this module did not sanitize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyRender {
    /// Sanitized markup from [`crate::content_site::markdown_to_html`]. Safe to
    /// place as inner HTML **because that function produced it** and for no
    /// other reason.
    Markup(String),
    /// Place as text.
    Text(String),
    /// EMBED §6's last rung: the authored fallback, plus why we are on it.
    Fallback { text: String, reason: FallbackReason },
}

/// Why the ladder bottomed out. **Each arm names whose situation it is.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    /// EMBED §3/§7 — the author declared an **active** embed (`requires` or
    /// `sandbox`) and this build has no enforcement to honour it with. A
    /// refusal we are making, not a capability we lack.
    RefusedActive { declaration: &'static str },
    /// The bytes are not in hand: a `Pointer` names a `system/content` blob and
    /// a `Child` needs a fetch, neither of which a synchronous render pass can
    /// make. **Not a permanent verdict** — it is a statement about what the
    /// reader was handed, which is why it is kept apart from
    /// [`Self::NoRendererFor`], and why it carries [`BlobMiss`]: *nobody
    /// looked*, *the origin has no such blob* and *we tried and could not look*
    /// are three different situations naming three different parties.
    PayloadNotInHand { miss: BlobMiss },
    /// **Ours.** We hold the bytes and have no renderer for this media type.
    /// The ladder working exactly as designed.
    NoRendererFor { media_type: String },
    /// **Theirs.** A text media type whose payload is not valid UTF-8.
    NotUtf8,
}

impl FallbackReason {
    /// A stable key for the catalog / a gate, never a sentence.
    pub fn key(&self) -> &'static str {
        match self {
            FallbackReason::RefusedActive { .. } => "active",
            FallbackReason::PayloadNotInHand { .. } => "not-in-hand",
            FallbackReason::NoRendererFor { .. } => "no-renderer",
            FallbackReason::NotUtf8 => "not-utf8",
        }
    }
}

/// The bytes a `Pointer` body names, as the reader was handed them.
///
/// ## Why this exists at all — and it is measured, not anticipated
///
/// EMBED §3 caps an inline payload at `bstr .size (1..16384)`, so **any post
/// over 16 KiB is REQUIRED to take the pointer arm**. Until 2026-09-16 nothing
/// on the reader side resolved one: the publisher wrote the blob and its chunks
/// into the closure, `--verify` confirmed them, and the reader drew the authored
/// fallback — so a long post arrived on screen as **its title alone**, with the
/// bytes sitting at the origin, published and verified.
///
/// It was invisible because every feed fixture on either seat was short. Pointed
/// at `entity-core-papers`' real corpus it fires immediately: **2 of 58 posts**
/// (18,219 B and 17,109 B) are over the ceiling, and they are the two longest —
/// i.e. exactly the subset a reader most wants. *A stated bound is not a
/// measured one; the trigger condition is its own measurement.*
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyBlob {
    /// The body is inline. There is nothing to resolve and nothing was owed.
    Inline,
    /// A pointer, resolved — these are the blob's reassembled bytes.
    Resolved(Vec<u8>),
    /// A pointer that was not resolved, and whose situation it is.
    Unresolved(BlobMiss),
}

/// Why a pointer body's bytes are not in hand. **Three parties, kept apart for
/// AP40's reason:** what a reader may be told, and who could fix it, differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobMiss {
    /// **Ours.** This source does not resolve blobs at all — the defaulted
    /// [`FeedSource::blob`](crate::feed_read::FeedSource::blob) arm. Nobody
    /// looked, so nothing is known about the origin.
    SourceCannot,
    /// **Theirs.** The origin answered and has no such blob. The publisher
    /// committed to a pointer whose closure they do not serve.
    Absent,
    /// **Neither.** We tried to look and could not — transport, not content.
    Failed(String),
}

impl BlobMiss {
    /// A stable key for a gate, never a sentence.
    pub fn key(&self) -> &'static str {
        match self {
            BlobMiss::SourceCannot => "source-cannot",
            BlobMiss::Absent => "absent",
            BlobMiss::Failed(_) => "failed",
        }
    }
}

/// Decide how to draw one entry body, for a caller holding **no resolved
/// pointer**.
///
/// Equivalent to [`decide_with`] handed [`BlobMiss::SourceCannot`], and that is
/// the honest reading rather than a convenience default: a caller that supplies
/// no blob did not look, which is precisely what that arm says.
pub fn decide(node: &EmbedNode) -> BodyRender {
    decide_with(node, &BodyBlob::Unresolved(BlobMiss::SourceCannot))
}

/// Decide how to draw one entry body. See the module doc for why the checks are
/// in this order.
///
/// `blob` is what the **async** side resolved for this entry's pointer payload,
/// because a `system/content` walk is not something a synchronous render pass
/// can make. An inline body ignores it.
pub fn decide_with(node: &EmbedNode, blob: &BodyBlob) -> BodyRender {
    let fallback = node.data.fallback.clone();

    // 1 — the node, before anything it carries.
    if let RenderVerdict::RefuseActive { declaration } = node.render_verdict() {
        return BodyRender::Fallback {
            text: fallback,
            reason: FallbackReason::RefusedActive { declaration },
        };
    }

    // 2 — do we hold the bytes at all? Inline bytes are in hand by
    // construction; a pointer is in hand only if somebody fetched its blob.
    //
    // **The pointer arm is not optional for a long post.** EMBED §3 caps inline
    // at 16 KiB, so this branch IS the reader's only path to any post above it —
    // see [`BodyBlob`] for the measurement that made that concrete.
    let owned;
    let bytes: &[u8] = match &node.data.payload {
        EmbedPayload::Inline(b) => b,
        EmbedPayload::Pointer(_) => match blob {
            BodyBlob::Resolved(b) => {
                owned = b.clone();
                &owned
            }
            BodyBlob::Unresolved(miss) => {
                return BodyRender::Fallback {
                    text: fallback,
                    reason: FallbackReason::PayloadNotInHand { miss: miss.clone() },
                }
            }
            // The node says pointer and the resolver says there was nothing to
            // resolve. Nobody fetched it, which is what `SourceCannot` means.
            BodyBlob::Inline => {
                return BodyRender::Fallback {
                    text: fallback,
                    reason: FallbackReason::PayloadNotInHand { miss: BlobMiss::SourceCannot },
                }
            }
        },
        // A `Child` names an entity, not a blob, and nothing in this crate mints
        // one. It is a fetch we have not built, not a blob we failed to get.
        _ => {
            return BodyRender::Fallback {
                text: fallback,
                reason: FallbackReason::PayloadNotInHand { miss: BlobMiss::SourceCannot },
            }
        }
    };

    // 3 — the media type. Compared case-insensitively on the bare type, since
    // IANA types are case-insensitive and a `; charset=` parameter is not part
    // of the type.
    let media = node.media_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if media != MARKDOWN_MEDIA_TYPE && media != PLAIN_MEDIA_TYPE {
        return BodyRender::Fallback {
            text: fallback,
            reason: FallbackReason::NoRendererFor { media_type: node.media_type.clone() },
        };
    }

    let Ok(text) = std::str::from_utf8(bytes) else {
        return BodyRender::Fallback { text: fallback, reason: FallbackReason::NotUtf8 };
    };

    if media == MARKDOWN_MEDIA_TYPE {
        BodyRender::Markup(crate::content_site::markdown_to_html(text))
    } else {
        BodyRender::Text(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{EmbedData, EmbedPayload};
    use entity_hash::Hash;

    fn node(media: &str, body: &str) -> EmbedNode {
        EmbedNode::new(media, EmbedData::new(EmbedPayload::Inline(body.as_bytes().to_vec()), "FB"))
    }

    /// See [`MARKDOWN_MEDIA_TYPE`] — the corpus spells this two ways and this
    /// test is the one file a ruling moves.
    #[test]
    fn the_markdown_media_type_is_the_iana_one() {
        assert_eq!(MARKDOWN_MEDIA_TYPE, "text/markdown");
        assert_eq!(
            EmbedNode::new(MARKDOWN_MEDIA_TYPE, EmbedData::new(EmbedPayload::Inline(vec![]), "x"))
                .type_name(),
            "app/embed/text/markdown",
            "the dispatch key is `app/embed/` .cat media-type"
        );
    }

    /// ⭐ The whole point of the module: a markdown body reaches a person as
    /// **markup**, through the site renderer, rather than as its degradation.
    #[test]
    fn a_markdown_body_is_rendered_as_markup_by_the_site_renderer() {
        let out = decide(&node(MARKDOWN_MEDIA_TYPE, "# Hi\n\nHello **world**"));
        let BodyRender::Markup(html) = out else {
            panic!("a markdown body must render as markup, got {out:?}");
        };
        assert!(html.contains("<h1"), "headings survive: {html}");
        assert!(html.contains("<strong>"), "emphasis survives: {html}");
        assert!(!html.contains("FB"), "the fallback is NOT what a person reads");
    }

    /// The sanitizer is reached **because we call the site renderer**, not
    /// because this module escapes anything itself. Asserting it here is what
    /// stops a later author "simplifying" the call into a raw passthrough.
    #[test]
    fn a_markdown_body_carrying_script_is_inert() {
        let BodyRender::Markup(html) = decide(&node(MARKDOWN_MEDIA_TYPE, "<script>alert(1)</script>"))
        else {
            panic!("expected markup");
        };
        assert!(!html.contains("<script"), "raw HTML must be escaped to inert text: {html}");
    }

    #[test]
    fn a_plain_body_is_text_and_is_not_run_through_markdown() {
        let out = decide(&node(PLAIN_MEDIA_TYPE, "# not a heading"));
        assert_eq!(
            out,
            BodyRender::Text("# not a heading".into()),
            "text/plain is text — rendering it as markdown would reinterpret the author's bytes"
        );
    }

    /// **Each fallback names a different situation, and the count is asserted**
    /// so a fifth cannot quietly reuse one of the four.
    #[test]
    fn every_fallback_says_which_situation_it_is() {
        // ours — no renderer for this type
        let png = decide(&node("image/png", "\u{fffd}"));
        assert!(
            matches!(&png, BodyRender::Fallback { reason: FallbackReason::NoRendererFor { .. }, text } if text == "FB"),
            "an unrenderable type falls back WITH the authored sentence: {png:?}"
        );

        // theirs — not UTF-8
        let bad = EmbedNode::new(
            PLAIN_MEDIA_TYPE,
            EmbedData::new(EmbedPayload::Inline(vec![0xff, 0xfe, 0xfd]), "FB"),
        );
        assert!(matches!(
            decide(&bad),
            BodyRender::Fallback { reason: FallbackReason::NotUtf8, .. }
        ));

        // not in hand — a pointer the render pass cannot resolve
        let ptr = EmbedNode::new(
            MARKDOWN_MEDIA_TYPE,
            EmbedData::new(EmbedPayload::Pointer(Hash::compute("t", b"x")), "FB"),
        );
        assert!(
            matches!(
                decide(&ptr),
                BodyRender::Fallback {
                    reason: FallbackReason::PayloadNotInHand { miss: BlobMiss::SourceCannot },
                    ..
                }
            ),
            "a pointer is not a missing RENDERER — it is bytes we have not read"
        );

        let keys = [
            FallbackReason::RefusedActive { declaration: "requires" }.key(),
            FallbackReason::PayloadNotInHand { miss: BlobMiss::SourceCannot }.key(),
            FallbackReason::NoRendererFor { media_type: "x".into() }.key(),
            FallbackReason::NotUtf8.key(),
        ];
        let distinct: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(distinct.len(), 4, "four situations, four words: {keys:?}");
    }

    /// ⭐ **The order check, and it is the one that would go wrong first.** An
    /// ACTIVE node whose media type we *can* render must still be refused —
    /// EMBED §3's passive-only rule is about the node, not its payload. A
    /// `decide` that looked at the media type first would render this.
    #[test]
    fn an_active_node_is_refused_even_when_we_could_render_its_type() {
        let mut n = node(MARKDOWN_MEDIA_TYPE, "# would render fine");
        n.data.requires = Some(entity_ecf::text("compute"));
        assert!(
            matches!(
                decide(&n),
                BodyRender::Fallback { reason: FallbackReason::RefusedActive { declaration: "requires" }, .. }
            ),
            "passive-only is a property of the NODE, checked before the media type"
        );
    }

    /// A `; charset=` parameter is not part of the type, and IANA types are
    /// case-insensitive. Both are the kind of thing a conformant publisher
    /// emits and a naive `==` refuses.
    #[test]
    fn the_media_type_comparison_ignores_case_and_parameters() {
        assert!(matches!(
            decide(&node("Text/Markdown; charset=utf-8", "**x**")),
            BodyRender::Markup(_)
        ));
    }
}
