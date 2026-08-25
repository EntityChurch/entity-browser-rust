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

    #[test]
    fn every_color_slot_resolves_against_site_tokens() {
        // `site_default` panics on an unknown token — building both forms
        // exercises every slot, so a token rename in SITE_TOKENS that misses
        // this table fails the suite instead of shipping a broken sheet.
        let _ = doc_css(".cs-doc", PaletteMode::Live);
        let _ = doc_css("main.page", PaletteMode::Frozen(None));
    }
}
