//! The ONE content-document stylesheet (S-T1).
//!
//! `render_page_body` HTML used to be styled by three hand-maintained
//! sheets — the live overlay's `.cs-doc` rules, the static exporter's
//! `PAGE_CSS`, and the Site Creator preview (which had none) — and they had
//! already drifted (blockquote `#9aa2b1` vs `#9aa3b2`, borders `#333` vs
//! `#20202e`). This module is now the single definition; the three hosts
//! *derive* their sheet from it:
//!
//! - **Live** (overlay, window site view, editor preview): every color is
//!   `var(--site-X, #default)` — themable through the `--site-*` family,
//!   fallback == the `SITE_TOKENS` default by construction.
//! - **Frozen** (static export): every color is the palette literal —
//!   published pages carry the site's own palette to other people's
//!   browsers with no runtime token layer.
//!
//! The scope selector is a parameter so the exporter keeps its established
//! `main.page` class vocabulary while the app uses `.cs-doc`.

use crate::theme_tokens::{site_token_value, Theme, SITE_TOKENS};

/// How a color slot renders in the emitted CSS.
#[derive(Clone, Copy)]
pub enum PaletteMode {
    /// `var(--site-X, #default)` — the runtime-themable form.
    Live,
    /// The palette literal — for emitted/exported CSS with no token layer.
    /// `None` freezes the site's own palette (the `SITE_TOKENS` defaults);
    /// `Some(theme)` freezes a manifest-declared registered theme's values
    /// (S-T2, DESIGN-MANIFEST-SITE-THEME §4) — the site's *effective* own
    /// palette, so a published page matches "Site's theme" mode in-app.
    #[allow(dead_code)] // spec'd frozen-palette mode (S-T2); not yet emitted on the wasm publish path
    Frozen(Option<&'static Theme>),
}

/// The default (site's-own) literal for a `--site-*` token.
fn site_default(token: &str) -> &'static str {
    SITE_TOKENS
        .iter()
        .find(|(t, _, _)| *t == token)
        .map(|(_, d, _)| *d)
        .unwrap_or_else(|| panic!("doc_css references unknown site token {token}"))
}

/// The frozen literal for a `--site-*` token under the site's effective own
/// palette — for emitted CSS *around* the document (the exporter's page
/// chrome: header, nav, footer), so its palette derives from the same
/// `SITE_TOKENS` source instead of a parallel hand-maintained set. `None` =
/// the site defaults; `Some(theme)` = that theme's resolved values (one
/// resolution rule, `theme_tokens::site_token_value`). Panics on an unknown
/// token (build-time discipline).
pub fn frozen(theme: Option<&'static Theme>, token: &str) -> &'static str {
    match theme {
        Some(t) => site_token_value(t, token),
        None => site_default(token),
    }
}

/// Resolve a color slot for the mode.
fn color(mode: PaletteMode, token: &str) -> String {
    match mode {
        PaletteMode::Live => format!("var({token}, {})", site_default(token)),
        PaletteMode::Frozen(theme) => frozen(theme, token).to_string(),
    }
}

/// The monospace font slot — a chrome token, not a site token, so it is
/// resolved separately (frozen form has no token layer to alias).
fn mono(mode: PaletteMode) -> &'static str {
    match mode {
        PaletteMode::Live => "var(--font-mono, monospace)",
        PaletteMode::Frozen(_) => "monospace",
    }
}

/// The FIGURE rule — one expression, two hosts, each supplying its own clamp.
///
/// A standalone image (`<p><img></p>` — what pulldown_cmark emits for an
/// embed directive on its own line) is a **figure**, and a figure is not
/// prose. Prose wants a ~70-character measure; a diagram wants the room its
/// author drew it for, and sharing one measure is what made published figures
/// unreadable: `entity-core-papers` measured eleven published views against a
/// 720 px column and **eight rendered their 11 px notes under 8 px**
/// (`docs/FIGURE-PRESENTATION-HANDOFF.md` §2). Raising the font size does not
/// help — type sizes the boxes, so the ratio is unchanged.
///
/// So the rule is *do not shrink a figure below the size it was authored at*:
/// `width:auto` takes the image's intrinsic width and `max-width` only ever
/// clamps down. A figure narrower than the column is untouched; a wider one
/// grows to its natural size, up to what the host can give it.
///
/// `max` is a CSS length **expression the host supplies**, because available
/// width is a host concern (this module's contract, one doc comment down) and
/// the two hosts measure it differently: the exporter's figure is bounded by
/// the viewport, the live overlay's by a scrolling pane that is usually much
/// narrower than the viewport — a window is not the screen. **A host that
/// cannot bound it must not call this**, and keeps the prose measure.
///
/// The breakout is `margin-left:50%` + `translateX(-50%)` rather than
/// `:has()` or a negative margin: it centres on the parent's centre line with
/// no support floor and no arithmetic against a width we do not know. It is
/// safe **only while `max` stays within the host's own scroll container** —
/// left overflow is not reachable by scrolling, so a clamp that can exceed
/// the container clips the left half of the figure irrecoverably.
/// ⚠ **Two shapes, because the two hosts give a figure its zoom differently.**
/// The exporter wraps the image in an anchor to its own asset (a static host
/// has no JS, and a link is what a reader can use); the live surface leaves the
/// bare `<img>` and opens an in-page overlay, because there the `src` is a
/// `data:` URL and browsers refuse top-level `data:` navigation — a link there
/// would be a control that silently does nothing.
///
/// So the selector must match `p>img` **and** `p>a>img`. Getting this wrong is
/// silent and total: wrapping the image moves it one level down, `:only-child`
/// stops matching its parent `<p>`, and every published figure quietly returns
/// to the prose measure with the rule still in the sheet.
pub const FIGURE_LINK_CLASS: &str = "figure-open";

pub fn figure_css(scope: &str, max: &str) -> String {
    let link = FIGURE_LINK_CLASS;
    format!(
        "{scope} p>a.{link}:only-child{{display:block;}}\
         {scope} p>img:only-child,\
         {scope} p>a.{link}:only-child>img:only-child{{\
         display:block;width:auto;height:auto;max-width:{max};\
         margin-left:50%;transform:translateX(-50%);cursor:zoom-in;}}"
    )
}

/// Build the content-document rules for a scope selector (`.cs-doc` live,
/// `main.page` in the exporter). The scope element itself gets the body
/// typography; hosts add their own *layout* rule for the same selector
/// (width/margins are host concerns, typography is not).
pub fn doc_css(scope: &str, mode: PaletteMode) -> String {
    let link = color(mode, "--site-link");
    let text = color(mode, "--site-text");
    let strong = color(mode, "--site-text-strong");
    let muted = color(mode, "--site-text-muted");
    let border = color(mode, "--site-border");
    let border2 = color(mode, "--site-border-2");
    let code_bg = color(mode, "--site-code-bg");
    let panel_bg = color(mode, "--site-panel-bg");
    let stripe_bg = color(mode, "--site-nav-bg");
    let mono = mono(mode);
    format!(
        "{scope} a{{color:{link};}}\
         {scope} a:hover{{text-decoration:underline;}}\
         {scope}{{line-height:1.6;font-size:16px;color:{text};word-wrap:break-word;}}\
         {scope}>*:first-child{{margin-top:0;}}\
         {scope}>*:last-child{{margin-bottom:0;}}\
         {scope} h1,{scope} h2,{scope} h3,{scope} h4,{scope} h5,{scope} h6{{\
         margin:24px 0 16px;font-weight:600;line-height:1.25;color:{strong};}}\
         {scope} h1{{font-size:1.9em;padding-bottom:.3em;border-bottom:1px solid {border};}}\
         {scope} h2{{font-size:1.5em;padding-bottom:.3em;border-bottom:1px solid {border};}}\
         {scope} h3{{font-size:1.25em;}}\
         {scope} h4{{font-size:1em;}}\
         {scope} p,{scope} ul,{scope} ol,{scope} blockquote,{scope} table,{scope} pre{{margin:0 0 16px;}}\
         {scope} ul,{scope} ol{{padding-left:2em;}}\
         {scope} li+li{{margin-top:.25em;}}\
         {scope} code{{font-family:{mono};\
         font-size:85%;padding:.2em .4em;border-radius:6px;background:{code_bg};}}\
         {scope} pre{{padding:14px 16px;border-radius:6px;overflow:auto;line-height:1.45;\
         background:{code_bg};border:1px solid {border2};}}\
         {scope} pre code{{background:none;padding:0;font-size:100%;border-radius:0;}}\
         {scope} blockquote{{padding:0 1em;color:{muted};\
         border-left:.25em solid {border2};}}\
         {scope} table{{border-collapse:collapse;display:block;width:max-content;max-width:100%;overflow:auto;}}\
         {scope} th,{scope} td{{padding:6px 13px;border:1px solid {border2};}}\
         {scope} th{{font-weight:600;background:{panel_bg};text-align:left;}}\
         {scope} tr:nth-child(2n) td{{background:{stripe_bg};}}\
         {scope} img{{max-width:100%;height:auto;}}\
         {scope} hr{{height:.25em;border:0;margin:24px 0;background:{border};}}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_form_is_var_wrapped_with_site_defaults_as_fallbacks() {
        let css = doc_css(".cs-doc", PaletteMode::Live);
        // The whole "site" (default) appearance mode rests on fallback ==
        // SITE_TOKENS default; here that holds by construction — assert the
        // shape anyway so a refactor can't quietly de-tokenize the live form.
        assert!(css.contains(".cs-doc a{color:var(--site-link, #a6c0de);}"), "{css}");
        assert!(css.contains("color:var(--site-text, #e2e2ea)"));
        assert!(css.contains("background:var(--site-code-bg, #0a0a1a)"));
        assert!(css.contains("font-family:var(--font-mono, monospace)"));
        assert!(!css.contains("§"), "no unresolved slots");
    }

    #[test]
    fn frozen_form_has_no_token_layer() {
        let css = doc_css("main.page", PaletteMode::Frozen(None));
        assert!(!css.contains("var("), "exported CSS must be self-contained: {css}");
        assert!(css.contains("main.page a{color:#a6c0de;}"), "{css}");
        assert!(css.contains("color:#e2e2ea"));
        assert!(css.contains("border:1px solid #2a2a3e"));
        assert!(css.contains("font-family:monospace"));
    }

    #[test]
    fn frozen_theme_form_freezes_that_themes_values() {
        // A manifest-declared theme freezes the SAME values the strict
        // "Always X" override installs live (`site_token_value` is the one
        // resolution rule) — export parity with "Site's theme" mode.
        use crate::theme_tokens::{site_token_value, LIGHT};
        let css = doc_css("main.page", PaletteMode::Frozen(Some(&LIGHT)));
        assert!(!css.contains("var("), "themed export must be self-contained: {css}");
        let text = site_token_value(&LIGHT, "--site-text");
        assert!(css.contains(&format!("color:{text}")), "{css}");
        assert_ne!(text, "#e2e2ea", "LIGHT text must differ from the dark default");
    }

    #[test]
    fn both_forms_carry_identical_rule_structure() {
        // Same selectors, same declarations, only the color values differ —
        // strip values and the two forms must be equal, so the exporter can
        // never drift structurally from the live overlay again.
        let strip = |css: &str| -> String {
            let mut out = String::new();
            for decl in css.split(';') {
                out.push_str(decl.split(':').next().unwrap_or(""));
                out.push(';');
            }
            out
        };
        let live = doc_css(".x", PaletteMode::Live);
        let frozen = doc_css(".x", PaletteMode::Frozen(None));
        assert_eq!(strip(&live), strip(&frozen));
        let themed = doc_css(".x", PaletteMode::Frozen(Some(&crate::theme_tokens::LIGHT)));
        assert_eq!(strip(&live), strip(&themed));
    }

    /// The app shell must not take the reader's own zoom away.
    ///
    /// This lives beside the figure rule because it is the other half of one
    /// contract — *a figure the reader can get a better look at.* `index.html`
    /// carried `user-scalable=no` from the initial public release as
    /// "feels-like-an-app" boilerplate, and on 2026-09-19 that was measured as
    /// half of why a figure opened on a phone was unusable: the overlay showed
    /// a corner of the drawing and pinch could not get the reader out of it.
    /// The overlay half is fixed in `dom::content_site`; this is the half that
    /// a later tidy-up would silently undo, because the attribute looks like
    /// chrome polish and its cost is two surfaces away.
    ///
    /// A surface that genuinely must swallow a gesture says so for itself with
    /// `touch-action` (the window resize grips, the on-screen pad, the game
    /// fixtures all do) — which is scoped, where this is not.
    #[test]
    fn the_app_shell_never_takes_the_readers_own_zoom_away() {
        let shell = include_str!("../../index.html");
        let viewport = shell
            .lines()
            .find(|l| l.contains("name=\"viewport\""))
            .expect("index.html must declare a viewport meta");
        assert!(
            !viewport.contains("user-scalable=no"),
            "the shell forbids pinch-zoom, which is a WCAG 1.4.4 failure and \
             removes the only way out of a figure too large for the surface: \
             {viewport}"
        );
        assert!(
            !viewport.contains("maximum-scale"),
            "`maximum-scale` caps the reader's zoom, which is `user-scalable=no` \
             by another spelling: {viewport}"
        );
    }

    #[test]
    fn a_figure_may_exceed_the_prose_measure_and_is_never_scaled_below_its_own_size() {
        // The two halves of the rule, and the second is the one that fixes the
        // reported defect: `width:auto` means the clamp can only ever shrink a
        // figure, so an 853-unit drawing in a 676-unit column renders at 853
        // and its 11px notes are 11px. A rule that set a width would scale
        // small figures UP and blur them, which is a different bug.
        let css = figure_css(".cs-doc", "min(1400px, 94cqw)");
        assert!(css.contains("width:auto"), "a figure must keep its intrinsic width: {css}");
        assert!(css.contains("max-width:min(1400px, 94cqw)"), "{css}");
        // Standalone only. An image inside a sentence is not a figure, and
        // breaking it out of the line would be worse than leaving it small.
        assert!(css.contains("p>img:only-child"), "{css}");
    }

    #[test]
    fn the_rule_matches_a_figure_whether_or_not_a_host_wrapped_it_in_a_link() {
        // The exporter wraps a figure in an anchor (a static page has no JS, so
        // a link is its only zoom); the live surface does not (its `src` is a
        // `data:` URL and top-level `data:` navigation is refused). One rule,
        // both shapes — and the wrapped arm is the easy one to lose, because
        // losing it is silent: correct markup, rule still in the sheet, figure
        // quietly back at the prose measure.
        let css = figure_css(".cs-doc", "X");
        assert!(css.contains(".cs-doc p>img:only-child"), "bare shape: {css}");
        assert!(
            css.contains(".cs-doc p>a.figure-open:only-child>img:only-child"),
            "wrapped shape: {css}"
        );
        // Both arms share one declaration block, so they cannot drift apart.
        assert_eq!(css.matches("max-width:X").count(), 1, "{css}");
        // A figure announces that it opens, on both surfaces.
        assert!(css.contains("cursor:zoom-in"), "{css}");
    }

    #[test]
    fn the_figure_rule_is_the_same_expression_whoever_clamps_it() {
        // Two hosts, two clamps, ONE rule — the live overlay bounds a figure by
        // its scrolling pane and the exporter by the viewport, and those are
        // different numbers for the same reason a window is not a screen. What
        // must NOT differ is anything else, or the surfaces drift the way the
        // three hand-maintained sheets this module replaced already did once.
        let live = figure_css(".cs-doc", "A");
        let frozen = figure_css("main.page", "B");
        assert_eq!(
            live.replace(".cs-doc", "{}").replace("max-width:A", "max-width:{}"),
            frozen.replace("main.page", "{}").replace("max-width:B", "max-width:{}"),
        );
    }

    #[test]
    fn the_figure_rule_is_not_in_the_base_sheet_so_a_host_that_cannot_bound_it_opts_out() {
        // `figure_css` is deliberately NOT folded into `doc_css`: the Site
        // Creator preview renders `.cs-doc` outside any pane whose width it
        // knows, and a breakout there would overflow left, where scrolling
        // cannot reach it. Silence is the correct behaviour for a host with no
        // answer, and it is only available while the rule is opt-in.
        for mode in [PaletteMode::Live, PaletteMode::Frozen(None)] {
            let css = doc_css(".cs-doc", mode);
            assert!(!css.contains("only-child"), "the figure rule must stay opt-in: {css}");
            // The base clamp still applies to every image, figure or not.
            assert!(css.contains(".cs-doc img{max-width:100%;height:auto;}"), "{css}");
        }
    }

    #[test]
    fn every_color_slot_resolves_against_site_tokens() {
        // `site_default` panics on an unknown token — building both forms
        // exercises every slot, so a token rename in SITE_TOKENS that misses
        // this table fails the suite instead of shipping a broken sheet.
        let _ = doc_css(".cs-doc", PaletteMode::Live);
        let _ = doc_css("main.page", PaletteMode::Frozen(None));
    }
}
