//! Theme tokens — the single source of truth for the app's colors + fonts.
//!
//! Theming works through **CSS custom properties** defined once on the
//! document `:root`. There is exactly one shadow root in the app
//! (`#dom-layer`); everything else (status bar, `#site-layer` overlay,
//! loading screen, runtime banners) is light DOM. CSS custom properties
//! inherit *through* the single shadow boundary, so a `:root` block drives
//! the entire app. A theme is one `token → value` map; switching themes =
//! rewriting one `<style id="theme-vars">` element. No DOM rebuild.
//!
//! Style code references tokens as `var(--token, #literal)` — the literal
//! fallback means a missing token is invisible, never a blank color. The
//! base chrome palette (surfaces / text / borders / accents) is captured
//! **byte-identical** to the pre-theming look. The semantic *status* family
//! (ok / err / info / warn) is gently harmonized: each was duplicated with
//! slightly different shades across windows (`#0f0` vs `#7c7` vs `#9c9` for
//! "ok"); they now share `--status-*`.
//!
//! Survey + rationale: the theming-survey reference.

/// A named theme: an ordered list of `(custom-property, value)` pairs.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// Stable id, persisted in `SettingsState.theme` (e.g. `"dark"`).
    pub name: &'static str,
    /// Human label for the Settings radio (e.g. `"Dark"`).
    pub label: &'static str,
    /// CSS `color-scheme` keyword for this theme (`"dark"` / `"light"`).
    /// Emitted into the `:root` block so the browser renders NATIVE form
    /// controls (the `<select>` popup + `<option>` list, scrollbars, the
    /// caret) in the matching scheme. Without it, WebKitGTK (Tauri) themes
    /// the dropdown popup with the system/default scheme — light option
    /// chrome under our dark UI → unreadable pale-on-pale text. Chromium/
    /// Firefox mask this, so it only bit the desktop WebView.
    pub scheme: &'static str,
    /// The `:root` variable values for this theme.
    pub vars: &'static [(&'static str, &'static str)],
}

/// Semantic status colors — reference these instead of raw hex so every
/// window's success/error/info/warn glyphs share one family that retunes
/// per theme. (Was duplicated as `#0f0`/`#f66`/`#9cf`/`#fc9` across
/// shell / event_log / content_stream / path_tap / wire_recorder / chain_trace.)
pub const STATUS_OK: &str = "var(--status-ok, #6c6)";
pub const STATUS_ERR: &str = "var(--status-err, #f66)";
pub const STATUS_INFO: &str = "var(--status-info, #9cf)";
pub const STATUS_WARN: &str = "var(--status-warn, #fc9)";

/// The default dark theme — values captured byte-identical from the
/// pre-theming hardcoded palette (status family harmonized per module docs).
pub const DARK: Theme = Theme {
    name: "dark",
    label: "Dark",
    scheme: "dark",
    vars: &[
        // -- surfaces --
        ("--bg", "#1a1a2e"),             // app base / :host / status bar
        ("--bg-body", "#111"),           // html/body backstop
        ("--surface", "#2a2a4e"),        // raised: buttons, rows, palette btns
        ("--surface-header", "#1e1e3e"), // window header bar
        ("--surface-hover", "#3a3a5e"),  // hover states
        ("--surface-sunken", "#0a0a1a"), // output panes (PRE_OUTPUT)
        ("--surface-max", "#14141c"),    // maximized window surface
        ("--input-bg", "#0e0e1e"),       // inputs, code, entity-content
        ("--overlay-bg", "#101018"),     // #site-layer overlay
        ("--selected-bg", "#2a4a6e"),    // tree-row aria-selected
        // -- text --
        ("--text", "#e0e0e0"),
        ("--text-muted", "#c0c0c0"), // button text, selection-source
        ("--text-dim", "#888"),      // hints, labels, footers, placeholders
        ("--text-faint", "#666"),    // faint placeholders, footers
        ("--title-muted", "#a0a0c0"), // window-header h3, tree-toggle, dt
        // -- borders --
        ("--border", "#333"),
        ("--border-strong", "#444"),
        ("--border-bold", "#555"),
        // -- accents --
        ("--accent", "#90d0ff"),       // links, selected, mode label
        ("--accent-green", "#c0e0c0"), // primary-button text
        ("--accent-2", "#c0c0e0"),     // secondary-button text
        ("--btn-primary-bg", "#2a4a2e"),
        ("--btn-primary-border", "#4a4"),
        ("--btn-secondary-border", "#66f"),
        // -- categorical peer badges --
        ("--peer-primary", "#6b8"),
        ("--peer-local", "#8ab"),
        ("--peer-remote", "#b8a"),
        // -- semantic status (harmonized) --
        ("--status-ok", "#6c6"),
        ("--status-err", "#f66"),
        ("--status-info", "#9cf"),
        ("--status-warn", "#fc9"),
        // -- app/game launcher card accents (hue is per-card from the app id;
        //    saturation/lightness/tint-alpha come from the theme so the icons
        //    stay readable in both modes). Dark: bright icon on a faint tint. --
        ("--app-card-s", "70%"),
        ("--app-card-l", "68%"),
        ("--app-card-tint-s", "60%"),
        ("--app-card-tint-l", "55%"),
        ("--app-card-tint-a", "0.16"),
        // -- fonts --
        ("--font-ui", "system-ui, -apple-system, sans-serif"),
        ("--font-mono", "monospace"),
        ("--fs-base", "14px"),
    ],
};

/// Light theme — dark text on light surfaces. Same token keys as [`DARK`]
/// (so the `:root` block fully overrides). Accents/status are re-tuned for
/// contrast on a light background (a `#90d0ff` link is unreadable on white;
/// red still reads as error).
pub const LIGHT: Theme = Theme {
    name: "light",
    label: "Light",
    scheme: "light",
    vars: &[
        // -- surfaces --
        ("--bg", "#f4f4f8"),
        ("--bg-body", "#e8e8ee"),
        ("--surface", "#e6e6f0"),
        ("--surface-header", "#dcdce8"),
        ("--surface-hover", "#d4d4e4"),
        ("--surface-sunken", "#ececf2"),
        ("--surface-max", "#ffffff"),
        ("--input-bg", "#ffffff"),
        ("--overlay-bg", "#f6f6fa"),
        ("--selected-bg", "#cfe0f5"),
        // -- text --
        ("--text", "#1a1a22"),
        ("--text-muted", "#3a3a46"),
        ("--text-dim", "#6a6a76"),
        ("--text-faint", "#9494a2"),
        ("--title-muted", "#4a4a64"),
        // -- borders --
        ("--border", "#d2d2dc"),
        ("--border-strong", "#bcbcca"),
        ("--border-bold", "#a4a4b4"),
        // -- accents --
        ("--accent", "#1366c0"),
        ("--accent-green", "#1f7a3a"),
        ("--accent-2", "#3a3a7a"),
        ("--btn-primary-bg", "#d8efdb"),
        ("--btn-primary-border", "#6ab06e"),
        ("--btn-secondary-border", "#8a8ad0"),
        // -- categorical peer badges --
        ("--peer-primary", "#2e7d4f"),
        ("--peer-local", "#2f6ea3"),
        ("--peer-remote", "#8a3a7a"),
        // -- semantic status (re-tuned for light bg) --
        ("--status-ok", "#1f9d4d"),
        ("--status-err", "#d23030"),
        ("--status-info", "#1f6fd0"),
        ("--status-warn", "#b5701a"),
        // -- app/game launcher card accents — darker, less-saturated icon so it
        //    reads against a light badge tint (the bright dark-mode icon washes
        //    out on a light background). --
        ("--app-card-s", "55%"),
        ("--app-card-l", "42%"),
        ("--app-card-tint-s", "50%"),
        ("--app-card-tint-l", "58%"),
        ("--app-card-tint-a", "0.15"),
        // -- fonts (shared) --
        ("--font-ui", "system-ui, -apple-system, sans-serif"),
        ("--font-mono", "monospace"),
        ("--fs-base", "14px"),
    ],
};

/// All registered themes. Settings renders a radio per entry; adding a
/// theme is one entry here. `DARK` stays first (the default).
pub const THEMES: &[Theme] = &[DARK, LIGHT];

/// localStorage key mirroring the chosen theme name. The tree
/// (`SettingsState.theme`) is the durable record, but it isn't readable
/// until peers boot; this mirror lets [`boot_choice`] pick the theme
/// synchronously at first paint so a non-default theme doesn't flash dark.
/// Mirrors the `boot_fast_paint` localStorage-mirror pattern.
pub const THEME_LS_KEY: &str = "entity_theme";

/// The theme to install at boot: the localStorage mirror, else [`DARK`].
#[cfg(target_arch = "wasm32")]
pub fn boot_choice() -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|ls| ls.get_item(THEME_LS_KEY).ok().flatten())
        .filter(|name| THEMES.iter().any(|t| t.name == *name))
        .unwrap_or_else(|| DARK.name.to_string())
}

/// Native stub — no localStorage; always the default.
#[cfg(not(target_arch = "wasm32"))]
pub fn boot_choice() -> String {
    DARK.name.to_string()
}

/// Persist the chosen theme to the localStorage boot mirror AND recolor the
/// live page (rewrite `#theme-vars`). The durable tree write is the caller's
/// job (`SettingsState`); this is the appearance side.
#[cfg(target_arch = "wasm32")]
pub fn apply_and_persist(theme_name: &str) {
    if let Some(ls) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = ls.set_item(THEME_LS_KEY, theme_name);
    }
    install_root(theme_name);
}

/// Native stub — no DOM / localStorage.
#[cfg(not(target_arch = "wasm32"))]
pub fn apply_and_persist(_theme_name: &str) {}

/// Look up a theme by `name`, falling back to [`DARK`] for an unknown id
/// (e.g. a persisted `"light"` from before that theme existed).
pub fn lookup(name: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.name == name).unwrap_or(&DARK)
}

/// Build the `:root { … }` CSS block for a theme.
pub fn root_block(theme: &Theme) -> String {
    let mut s = String::from(":root{");
    // Native-control rendering scheme — see `Theme::scheme`. Must lead the
    // block so it's set before any control paints.
    s.push_str("color-scheme:");
    s.push_str(theme.scheme);
    s.push(';');
    for (k, v) in theme.vars {
        s.push_str(k);
        s.push(':');
        s.push_str(v);
        s.push(';');
    }
    s.push('}');
    s
}

/// Inject (or rewrite) the `<style id>` element in `<head>` with `css`.
/// `None` means "no block to define": an existing element is **emptied** (so
/// the CSS `var()` literal fallbacks resume), a missing one is left absent.
/// Idempotent — reuses the element when present. The shared core of
/// [`install_root`] and [`install_site_root`].
#[cfg(target_arch = "wasm32")]
fn install_style_block(id: &str, css: Option<&str>) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if let Some(existing) = document.get_element_by_id(id) {
        existing.set_text_content(Some(css.unwrap_or("")));
        return;
    }
    let Some(css) = css else {
        return; // nothing to define yet — the fallbacks render.
    };
    let Ok(style_el) = document.create_element("style") else {
        return;
    };
    let _ = style_el.set_attribute("id", id);
    style_el.set_text_content(Some(css));
    // Append into <head> (fall back to <html>) so app stylesheets / inline
    // styles that reference these vars resolve from the first paint.
    let host = document
        .query_selector("head")
        .ok()
        .flatten()
        .or_else(|| document.document_element());
    if let Some(host) = host {
        let _ = host.append_child(&style_el);
    }
}

/// Inject (or rewrite) the `<style id="theme-vars">` chrome `:root` block.
/// Call at boot (before first paint) and on theme change.
#[cfg(target_arch = "wasm32")]
pub fn install_root(theme_name: &str) {
    install_style_block("theme-vars", Some(&root_block(lookup(theme_name))));
}

// ---------------------------------------------------------------------------
// Site overlay theme layer (`--site-*`)
// ---------------------------------------------------------------------------
//
// The Content Site overlay (`#site-layer`) and its window-directory rail carry
// their OWN palette — they never receive `dom::style::DOM_STYLES`, and by
// design the site surface reads independently of the chrome theme. So the
// overlay's colors are a SECOND token family, `--site-*`, defined the same way
// (a `:root` block in `<head>`, inherited through the one shadow boundary).
//
// `content_site.rs` references each color as `var(--site-X, #literal)` where the
// literal is the overlay's original hex. The **"Site appearance"** setting picks
// what (if anything) defines `--site-*`:
//
//   - `"site"`   → nothing is injected; the CSS fallbacks apply → the overlay's
//                  own palette, byte-identical to the pre-theming look (default).
//   - `"system"` → `--site-X: var(--app-token)` live aliases → the overlay
//                  tracks the chrome theme (re-resolves on a chrome flip with no
//                  re-install, since `var()` re-evaluates).
//   - `"<name>"` → a strict override to a specific registered theme: each
//                  `--site-X` is frozen to that theme's value for its app token
//                  (stays put even when the chrome theme changes).

/// The overlay's `--site-*` tokens: `(site_token, default_hex, app_token)`.
/// `default_hex` is the overlay's original color (the `var()` fallback, and the
/// strict-override fallback for a theme missing the app token). `app_token` is
/// the chrome token this site token aliases to in `"system"` / strict modes.
pub const SITE_TOKENS: &[(&str, &str, &str)] = &[
    // -- surfaces --
    ("--site-bg", "#101018", "--overlay-bg"),
    ("--site-nav-bg", "#15151f", "--surface-header"),
    ("--site-sidebar-bg", "#13131c", "--surface-header"),
    ("--site-rail-bg", "#0d0d14", "--bg"),
    ("--site-control-bg", "#22223a", "--surface"),
    ("--site-panel-bg", "#1b1b28", "--surface"),
    ("--site-toggle-bg", "#1a1a26", "--surface"),
    ("--site-toggle-bg-rail", "#14141f", "--surface"),
    ("--site-exit-bg", "#2a2a4e", "--surface"),
    ("--site-error-bg", "#1c1418", "--input-bg"),
    ("--site-selected-bg", "#1b1b2c", "--selected-bg"), // directory rail current row
    // -- text --
    ("--site-text", "#e2e2ea", "--text"),
    ("--site-text-strong", "#c3c9d6", "--text-muted"),
    ("--site-text-muted", "#9aa3b2", "--text-dim"),
    ("--site-text-muted-2", "#7a8294", "--text-dim"),
    ("--site-text-faint", "#454a59", "--text-faint"),
    ("--site-text-faint-2", "#565d6e", "--text-faint"), // rail sublines / off-state icons
    ("--site-control-text", "#cfe3ff", "--accent"),
    ("--site-accent", "#9fd0ff", "--accent"),
    ("--site-link", "#a6c0de", "--accent"),
    ("--site-bc-current", "#cdd3df", "--text"),
    ("--site-exit-text", "#c0c0e0", "--accent-2"),
    ("--site-error-text", "#ff9b9b", "--status-err"),
    // -- borders --
    ("--site-border", "#20202e", "--border"),
    ("--site-border-2", "#2a2a3e", "--border"),
    ("--site-control-border", "#3a3a52", "--border-strong"),
    ("--site-panel-border", "#2f2f46", "--border"),
    ("--site-exit-border", "#555", "--border-bold"),
    ("--site-error-border", "#553333", "--status-err"),
];

/// localStorage key mirroring the chosen site-appearance mode, so [`site_appearance_boot_choice`]
/// can install it synchronously at first paint (no flash when booting into a
/// site overlay). Mirrors [`THEME_LS_KEY`].
pub const SITE_APPEARANCE_LS_KEY: &str = "entity_site_appearance";

/// The "Site appearance" dropdown catalog: `(value, label)` in display order.
/// Two fixed modes (the site's own theme; follow the system theme) followed by
/// a strict override per registered theme. Adding a theme adds an "Always X"
/// override automatically.
pub fn site_appearance_catalog() -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("site", "Site's theme".to_string()),
        ("system", "Match system theme".to_string()),
    ];
    for t in THEMES {
        v.push((t.name, format!("Always {}", t.label)));
    }
    v
}

/// Is `mode` a valid "Site appearance" value? `"site"` / `"system"` / a
/// registered theme name. Used to reject stale or corrupt persisted values so
/// the boot path falls back to the `"site"` default instead of silently
/// freezing the overlay to a strict override (mirrors [`boot_choice`]'s filter).
pub fn is_valid_site_appearance(mode: &str) -> bool {
    mode == "site" || mode == "system" || THEMES.iter().any(|t| t.name == mode)
}

/// Look up an app token's value within a theme (e.g. `--text` in [`LIGHT`]).
fn theme_value<'a>(theme: &'a Theme, app_token: &str) -> Option<&'a str> {
    theme.vars.iter().find(|(k, _)| *k == app_token).map(|(_, v)| *v)
}

/// Build the `:root { --site-*: … }` block for a site-appearance `mode`, or
/// `None` for `"site"` (inject nothing — the CSS fallbacks render the overlay's
/// own palette). `"system"` emits live `var(--app-token)` aliases; any other
/// value is treated as a strict override to that registered theme (unknown →
/// [`DARK`] via [`lookup`]), freezing each token to that theme's value.
pub fn site_root_block(mode: &str) -> Option<String> {
    match mode {
        "site" => None,
        "system" => {
            let mut s = String::from(":root{");
            for (site, _default, app) in SITE_TOKENS {
                s.push_str(site);
                s.push_str(":var(");
                s.push_str(app);
                s.push_str(");");
            }
            s.push('}');
            Some(s)
        }
        name => {
            let theme = lookup(name);
            let mut s = String::from(":root{");
            for (site, default, app) in SITE_TOKENS {
                let val = theme_value(theme, app).unwrap_or(default);
                s.push_str(site);
                s.push(':');
                s.push_str(val);
                s.push(';');
            }
            s.push('}');
            Some(s)
        }
    }
}

/// The site-appearance mode to install at boot: the localStorage mirror, else
/// `"site"` (the overlay's own theme).
#[cfg(target_arch = "wasm32")]
pub fn site_appearance_boot_choice() -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|ls| ls.get_item(SITE_APPEARANCE_LS_KEY).ok().flatten())
        .filter(|mode| is_valid_site_appearance(mode))
        .unwrap_or_else(|| "site".to_string())
}

/// Native stub — no localStorage; always the overlay's own theme.
#[cfg(not(target_arch = "wasm32"))]
pub fn site_appearance_boot_choice() -> String {
    "site".to_string()
}

/// Inject / rewrite the `<style id="site-theme-vars">` element with the
/// `--site-*` block for `mode`. For `"site"` the block is `None`: any existing
/// element is emptied (CSS fallbacks resume), never injected. Call at boot and
/// on the "Site appearance" setting change. `"system"` needs no re-install on a
/// chrome flip — its `var()` aliases re-resolve.
#[cfg(target_arch = "wasm32")]
pub fn install_site_root(mode: &str) {
    // `"site"` → `None` → the element is emptied (or never created), so the
    // overlay's `var(--site-X, #literal)` fallbacks render its own palette.
    install_style_block("site-theme-vars", site_root_block(mode).as_deref());
}

/// Native stub — no DOM.
#[cfg(not(target_arch = "wasm32"))]
pub fn install_site_root(_mode: &str) {}

/// Persist the chosen site-appearance mode to the localStorage boot mirror AND
/// recolor the live overlay (rewrite `#site-theme-vars`). The durable tree
/// write is the caller's job ([`crate::views::settings`]); this is the
/// appearance side. Mirrors [`apply_and_persist`].
#[cfg(target_arch = "wasm32")]
pub fn apply_site_appearance(mode: &str) {
    if let Some(ls) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = ls.set_item(SITE_APPEARANCE_LS_KEY, mode);
    }
    install_site_root(mode);
}

/// Native stub — no DOM / localStorage.
#[cfg(not(target_arch = "wasm32"))]
pub fn apply_site_appearance(_mode: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_is_first_and_default() {
        assert_eq!(THEMES[0].name, "dark");
        assert_eq!(lookup("dark").name, "dark");
        assert_eq!(lookup("nonexistent").name, "dark", "unknown → dark");
    }

    #[test]
    fn root_block_is_well_formed() {
        let block = root_block(&DARK);
        assert!(block.starts_with(":root{"));
        assert!(block.ends_with('}'));
        // color-scheme must be present so native controls (the <select>
        // popup) render in the theme's scheme on WebKitGTK/Tauri.
        assert!(block.contains("color-scheme:dark;"));
        assert!(root_block(&LIGHT).contains("color-scheme:light;"));
        assert!(block.contains("--bg:#1a1a2e;"));
        assert!(block.contains("--status-ok:#6c6;"));
        assert!(block.contains("--font-ui:system-ui, -apple-system, sans-serif;"));
    }

    #[test]
    fn every_token_has_a_value() {
        for (k, v) in DARK.vars {
            assert!(k.starts_with("--"), "token {k} must start with --");
            assert!(!v.is_empty(), "token {k} has empty value");
        }
    }

    #[test]
    fn site_tokens_are_well_formed_and_alias_real_app_tokens() {
        for (site, default, app) in SITE_TOKENS {
            assert!(site.starts_with("--site-"), "{site} must be a --site-* token");
            assert!(default.starts_with('#'), "{site} default must be a hex literal");
            // Every app token a site token aliases must exist in both themes
            // (so "system" / strict modes always resolve, never fall through).
            assert!(theme_value(&DARK, app).is_some(), "DARK missing {app} (aliased by {site})");
            assert!(theme_value(&LIGHT, app).is_some(), "LIGHT missing {app} (aliased by {site})");
        }
    }

    #[test]
    fn site_mode_injects_nothing() {
        // "site" = the overlay's own theme; CSS fallbacks render it.
        assert_eq!(site_root_block("site"), None);
    }

    #[test]
    fn system_mode_aliases_to_live_app_tokens() {
        let block = site_root_block("system").expect("system emits a block");
        assert!(block.starts_with(":root{") && block.ends_with('}'));
        // Live alias: the overlay bg follows the app's overlay bg, re-resolving
        // on a chrome flip with no re-install.
        assert!(block.contains("--site-bg:var(--overlay-bg);"), "block: {block}");
        assert!(block.contains("--site-text:var(--text);"));
        assert!(block.contains("--site-error-text:var(--status-err);"));
    }

    #[test]
    fn strict_override_freezes_the_named_themes_values() {
        // Strict "light" pins the overlay to LIGHT's palette regardless of the
        // current chrome theme — frozen literals, not var() aliases.
        let block = site_root_block("light").expect("a named theme emits a block");
        assert!(!block.contains("var("), "strict override must be frozen literals: {block}");
        // --site-text aliases --text, whose LIGHT value is #1a1a22.
        assert!(block.contains("--site-text:#1a1a22;"), "block: {block}");
        // --site-bg aliases --overlay-bg, whose LIGHT value is #f6f6fa.
        assert!(block.contains("--site-bg:#f6f6fa;"));

        // Strict "dark" pins to DARK's app palette.
        let dark = site_root_block("dark").expect("dark block");
        assert!(dark.contains("--site-text:#e0e0e0;"), "dark: {dark}");

        // Unknown name → DARK (lookup fallback), still a valid frozen block.
        let unknown = site_root_block("nonexistent").expect("unknown → dark block");
        assert_eq!(unknown, dark);
    }

    #[test]
    fn overlay_var_fallbacks_match_site_token_defaults() {
        // The whole "site" (default) mode rests on this invariant: it injects NO
        // `:root` block, so the CSS `var(--site-X, #literal)` FALLBACKS in the
        // overlay renderers ARE the look. If a fallback literal drifts from its
        // SITE_TOKENS default, "site" mode (fallback) and strict "Always Dark"
        // (SITE_TOKENS default, frozen) would render a token differently — a
        // silent divergence only visible when the user toggles appearance. Scan
        // both overlay source files and assert every fallback matches its token.
        const SOURCES: &[&str] = &[
            include_str!("dom/content_site.rs"),
            include_str!("dom/site_directory.rs"),
        ];
        let default_for =
            |tok: &str| SITE_TOKENS.iter().find(|(t, _, _)| *t == tok).map(|(_, d, _)| *d);
        let mut checked = 0;
        for src in SOURCES {
            let mut rest = *src;
            while let Some(pos) = rest.find("var(--site") {
                rest = &rest[pos + 4..]; // past "var("
                let end = rest.find(|c: char| c == ',' || c == ')').unwrap_or(rest.len());
                let token = rest[..end].trim();
                // Only the fallback-bearing usages (have a `,`); generated blocks
                // aren't in source, so every source usage should carry a fallback.
                if rest.as_bytes().get(end) == Some(&b',') {
                    let after = &rest[end + 1..];
                    let close = after.find(')').unwrap_or(after.len());
                    let fallback = after[..close].trim();
                    let expected = default_for(token).unwrap_or_else(|| {
                        panic!("var({token}) references a token not in SITE_TOKENS")
                    });
                    assert_eq!(
                        fallback, expected,
                        "overlay fallback for {token} is {fallback:?} but its SITE_TOKENS \
                         default is {expected:?} — 'site' mode would diverge from strict/dark"
                    );
                    checked += 1;
                }
                rest = &rest[end..];
            }
        }
        assert!(checked >= 20, "expected to scan many overlay fallbacks, found {checked}");
    }

    #[test]
    fn is_valid_site_appearance_accepts_modes_and_theme_names() {
        assert!(is_valid_site_appearance("site"));
        assert!(is_valid_site_appearance("system"));
        assert!(is_valid_site_appearance("dark"));
        assert!(is_valid_site_appearance("light"));
        assert!(!is_valid_site_appearance("bogus"));
        assert!(!is_valid_site_appearance(""));
    }

    #[test]
    fn appearance_catalog_lists_two_modes_then_a_strict_override_per_theme() {
        let cat = site_appearance_catalog();
        assert_eq!(cat[0].0, "site");
        assert_eq!(cat[1].0, "system");
        assert_eq!(cat.len(), 2 + THEMES.len());
        // Each theme yields an "Always <Label>" strict override keyed by name.
        for t in THEMES {
            let entry = cat.iter().find(|(v, _)| *v == t.name).expect("theme override listed");
            assert_eq!(entry.1, format!("Always {}", t.label));
        }
    }
}
