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
        ("--accent-text", "#1a1a2e"),  // text ON an accent-filled surface (chips)
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
        ("--accent-text", "#ffffff"),
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

/// Sepia — warm paper-and-ink light theme (an example of a retuned
/// built-in; same token keys as [`DARK`], values shifted warm).
pub const SEPIA: Theme = Theme {
    name: "sepia",
    label: "Sepia",
    scheme: "light",
    vars: &[
        // -- surfaces (warm paper) --
        ("--bg", "#f0e7d6"),
        ("--bg-body", "#e6dac2"),
        ("--surface", "#e6d9c0"),
        ("--surface-header", "#dccdb0"),
        ("--surface-hover", "#d4c2a2"),
        ("--surface-sunken", "#ece0ca"),
        ("--surface-max", "#faf4e6"),
        ("--input-bg", "#faf4e6"),
        ("--overlay-bg", "#f5edda"),
        ("--selected-bg", "#e2cea4"),
        // -- text (ink) --
        ("--text", "#3a2f20"),
        ("--text-muted", "#544636"),
        ("--text-dim", "#7a6a54"),
        ("--text-faint", "#a3907a"),
        ("--title-muted", "#6a5638"),
        // -- borders --
        ("--border", "#d6c5a8"),
        ("--border-strong", "#c2af8e"),
        ("--border-bold", "#a8926c"),
        // -- accents (aged brown link; contrast-tuned on paper) --
        ("--accent", "#8a5a20"),
        ("--accent-text", "#faf4e6"),
        ("--accent-green", "#4a6a28"),
        ("--accent-2", "#5a4a7a"),
        ("--btn-primary-bg", "#e2e2c4"),
        ("--btn-primary-border", "#8a9a4a"),
        ("--btn-secondary-border", "#9a86c0"),
        // -- categorical peer badges (darkened for the light ground) --
        ("--peer-primary", "#4a7a3a"),
        ("--peer-local", "#3a688f"),
        ("--peer-remote", "#8a4070"),
        // -- semantic status --
        ("--status-ok", "#3a7a30"),
        ("--status-err", "#b03430"),
        ("--status-info", "#2a68a8"),
        ("--status-warn", "#9a6414"),
        // -- app/game launcher card accents (light-ground tuning) --
        ("--app-card-s", "50%"),
        ("--app-card-l", "40%"),
        ("--app-card-tint-s", "45%"),
        ("--app-card-tint-l", "56%"),
        ("--app-card-tint-a", "0.15"),
        // -- fonts (shared) --
        ("--font-ui", "system-ui, -apple-system, sans-serif"),
        ("--font-mono", "monospace"),
        ("--fs-base", "14px"),
    ],
};

/// Neon — green-phosphor-on-black terminal theme (the "hacker" example;
/// mono UI font on purpose — it's a token like any other, retune it in a
/// duplicate if you want the palette without the typeface).
pub const NEON: Theme = Theme {
    name: "neon",
    label: "Neon",
    scheme: "dark",
    vars: &[
        // -- surfaces (near-black, green-cast) --
        ("--bg", "#0a120a"),
        ("--bg-body", "#050905"),
        ("--surface", "#122412"),
        ("--surface-header", "#0e1c0e"),
        ("--surface-hover", "#1b331b"),
        ("--surface-sunken", "#040a04"),
        ("--surface-max", "#0c160c"),
        ("--input-bg", "#061006"),
        ("--overlay-bg", "#071107"),
        ("--selected-bg", "#1a3d1a"),
        // -- text (phosphor) --
        ("--text", "#8dfc8d"),
        ("--text-muted", "#6fd66f"),
        ("--text-dim", "#4a9a4a"),
        ("--text-faint", "#2f6b2f"),
        ("--title-muted", "#66cc66"),
        // -- borders --
        ("--border", "#1e421e"),
        ("--border-strong", "#2a582a"),
        ("--border-bold", "#367036"),
        // -- accents (classic terminal green) --
        ("--accent", "#39ff14"),
        ("--accent-text", "#041004"),
        ("--accent-green", "#aaffaa"),
        ("--accent-2", "#7dff7d"),
        ("--btn-primary-bg", "#103310"),
        ("--btn-primary-border", "#2fbf2f"),
        ("--btn-secondary-border", "#2a8a2a"),
        // -- categorical peer badges (kept distinguishable within the cast) --
        ("--peer-primary", "#4ae04a"),
        ("--peer-local", "#3ac0a0"),
        ("--peer-remote", "#a0d040"),
        // -- semantic status (err/warn stay off-green so they still read) --
        ("--status-ok", "#39ff14"),
        ("--status-err", "#ff5f56"),
        ("--status-info", "#56d8ff"),
        ("--status-warn", "#ffc856"),
        // -- app/game launcher card accents --
        ("--app-card-s", "80%"),
        ("--app-card-l", "62%"),
        ("--app-card-tint-s", "70%"),
        ("--app-card-tint-l", "45%"),
        ("--app-card-tint-a", "0.18"),
        // -- fonts (mono UI is the aesthetic) --
        ("--font-ui", "monospace"),
        ("--font-mono", "monospace"),
        ("--fs-base", "14px"),
    ],
};

/// All **built-in** themes — compiled into the app, never stored in the
/// tree (updating the app updates them; they can't be deleted, only
/// duplicated in the Theme Editor). Settings renders an option per
/// registered theme; adding a built-in is one entry here. `DARK` stays
/// first (the default). User-defined themes live in the runtime registry
/// beside this slice — resolution goes through [`all_themes`] /
/// [`lookup`] / [`registered`], never by iterating `THEMES` directly
/// (that would skip user themes).
pub const THEMES: &[Theme] = &[DARK, LIGHT, SEPIA, NEON];

// ---------------------------------------------------------------------------
// User-defined themes — the runtime registry
// ---------------------------------------------------------------------------
//
// `Theme` threads `&'static str` through the whole resolution layer
// (`site_token_value`, `doc_css::PaletteMode::Frozen`, the exporter). Rather
// than rewrite that verified layer to owned strings, a user theme is LEAKED
// on registration (`Box::leak`) into a true `&'static Theme` — a deliberate,
// bounded leak: one per explicit Save (never per preview keystroke; the
// editor previews from owned strings without touching the registry), ~1–2 KB
// each, freed on reload. Replacing or deleting a theme drops it from the
// registry; the old allocation stays (same bound). DESIGN-USER-THEMES §1.
//
// The registry is a rebuildable PROJECTION of the tree entities under
// `app/entity-browser/themes/` (`crate::user_themes` owns persistence + the
// boot subscription) — never a second source of truth.

/// An owned theme definition, as edited/persisted. Registration leaks it
/// into a [`Theme`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserThemeSpec {
    pub name: String,
    pub label: String,
    /// CSS `color-scheme` keyword (`"dark"` / `"light"`) — see [`Theme::scheme`].
    pub scheme: String,
    pub vars: Vec<(String, String)>,
}

use std::cell::RefCell;
thread_local! {
    /// User themes, kept sorted by name (deterministic dropdown order).
    /// Main-thread only on wasm (all theme code is); per-thread isolation
    /// for native tests.
    static USER_THEMES: RefCell<Vec<&'static Theme>> = const { RefCell::new(Vec::new()) };
}

/// Validate a prospective user-theme name: it is a persisted id AND a tree
/// path segment AND a site-appearance dropdown value, so it must be short
/// `[a-z0-9-]` and must not shadow a built-in theme nor the site-appearance
/// mode values `"site"` / `"system"` (a user theme named "system" would
/// corrupt that value space).
pub fn validate_theme_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("theme name is empty".into());
    }
    if name.len() > 40 {
        return Err("theme name too long (max 40)".into());
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err("theme name must be lowercase letters, digits, and dashes".into());
    }
    if name == "site" || name == "system" {
        return Err(format!("\"{name}\" is a reserved appearance mode"));
    }
    if THEMES.iter().any(|t| t.name == name) {
        return Err(format!("\"{name}\" is a built-in theme"));
    }
    Ok(())
}

/// Register (or replace, keyed by name) a user theme. Validates the name and
/// scheme, then leaks the spec into a `&'static Theme` (module docs above).
pub fn register_user_theme(spec: UserThemeSpec) -> Result<(), String> {
    validate_theme_name(&spec.name)?;
    if spec.scheme != "dark" && spec.scheme != "light" {
        return Err(format!("scheme must be \"dark\" or \"light\", got \"{}\"", spec.scheme));
    }
    if spec.vars.is_empty() {
        return Err("theme has no token values".into());
    }
    for (k, v) in &spec.vars {
        if !k.starts_with("--") || v.is_empty() {
            return Err(format!("bad token entry {k:?}"));
        }
    }
    let vars: Vec<(&'static str, &'static str)> = spec
        .vars
        .into_iter()
        .map(|(k, v)| (&*k.leak(), &*v.leak()))
        .collect();
    let theme: &'static Theme = Box::leak(Box::new(Theme {
        name: spec.name.leak(),
        label: spec.label.leak(),
        scheme: spec.scheme.leak(),
        vars: vars.leak(),
    }));
    USER_THEMES.with(|u| {
        let mut u = u.borrow_mut();
        u.retain(|t| t.name != theme.name);
        let pos = u.partition_point(|t| t.name < theme.name);
        u.insert(pos, theme);
    });
    Ok(())
}

/// Remove a user theme from the registry (the tree entity is the caller's
/// job — `crate::user_themes`). Returns whether it was present. Built-ins
/// are not removable.
pub fn unregister_user_theme(name: &str) -> bool {
    USER_THEMES.with(|u| {
        let mut u = u.borrow_mut();
        let before = u.len();
        u.retain(|t| t.name != name);
        u.len() != before
    })
}

/// Names of the currently registered user themes (sorted).
pub fn user_theme_names() -> Vec<&'static str> {
    USER_THEMES.with(|u| u.borrow().iter().map(|t| t.name).collect())
}

/// Every registered theme: built-ins first (DARK the default), then user
/// themes sorted by name. The ONE iteration surface for dropdowns/catalogs.
pub fn all_themes() -> Vec<&'static Theme> {
    let mut v: Vec<&'static Theme> = THEMES.iter().collect();
    USER_THEMES.with(|u| v.extend(u.borrow().iter().copied()));
    v
}

/// localStorage key mirroring the chosen theme name. The tree
/// (`SettingsState.theme`) is the durable record, but it isn't readable
/// until peers boot; this mirror lets [`boot_choice`] pick the theme
/// synchronously at first paint so a non-default theme doesn't flash dark.
/// Mirrors the `boot_fast_paint` localStorage-mirror pattern.
pub const THEME_LS_KEY: &str = "entity_theme";

/// localStorage key carrying the current theme's **computed `:root` CSS**
/// when (and only when) the current theme is user-defined. A user theme
/// lives in the tree and isn't in the runtime registry until the boot
/// subscription syncs — this mirror is the paint hint that lets it render
/// on frame one anyway (no dark flash). It is never the record: the
/// registry sync re-installs (and rewrites this mirror) after every sync,
/// so a stale mirror self-heals. DESIGN-USER-THEMES §2.
pub const THEME_CSS_LS_KEY: &str = "entity_theme_css";

#[cfg(target_arch = "wasm32")]
fn ls_get(key: &str) -> Option<String> {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|ls| ls.get_item(key).ok().flatten())
}

#[cfg(target_arch = "wasm32")]
fn ls_set(key: &str, value: Option<&str>) {
    if let Some(ls) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        match value {
            Some(v) => {
                let _ = ls.set_item(key, v);
            }
            None => {
                let _ = ls.remove_item(key);
            }
        }
    }
}

/// The theme to install at boot: the localStorage mirror, else [`DARK`].
/// A name is accepted when it's registered (built-in, or a user theme in a
/// same-session re-entry) **or** when the CSS paint-hint mirror is present
/// (a user theme before the boot subscription syncs); anything else —
/// corrupt value, deleted user theme with no mirror — falls back to dark.
#[cfg(target_arch = "wasm32")]
pub fn boot_choice() -> String {
    ls_get(THEME_LS_KEY)
        .filter(|name| registered(name).is_some() || ls_get(THEME_CSS_LS_KEY).is_some())
        .unwrap_or_else(|| DARK.name.to_string())
}

/// Native stub — no localStorage; always the default.
#[cfg(not(target_arch = "wasm32"))]
pub fn boot_choice() -> String {
    DARK.name.to_string()
}

/// Persist the chosen theme to the localStorage boot mirror AND recolor the
/// live page (rewrite `#theme-vars`). The durable tree write is the caller's
/// job (`SettingsState`); this is the appearance side. A user theme also
/// mirrors its computed CSS ([`THEME_CSS_LS_KEY`]) so the next boot paints
/// it before the registry loads; a built-in clears that mirror.
#[cfg(target_arch = "wasm32")]
pub fn apply_and_persist(theme_name: &str) {
    ls_set(THEME_LS_KEY, Some(theme_name));
    let user_css = registered(theme_name)
        .filter(|_| !THEMES.iter().any(|t| t.name == theme_name))
        .map(|t| root_block(t));
    ls_set(THEME_CSS_LS_KEY, user_css.as_deref());
    install_root(theme_name);
}

/// Native stub — no DOM / localStorage.
#[cfg(not(target_arch = "wasm32"))]
pub fn apply_and_persist(_theme_name: &str) {}

/// Look up a theme by `name` — built-in or user — falling back to [`DARK`]
/// for an unknown id (e.g. a persisted `"light"` from before that theme
/// existed, or a deleted user theme).
pub fn lookup(name: &str) -> &'static Theme {
    registered(name).unwrap_or(&DARK)
}

/// Look up a **registered** theme (built-in or user) by exact name — `None`
/// for an unknown id. Unlike [`lookup`] this does NOT fall back to [`DARK`]:
/// callers that take a name from outside the app (a site manifest's `theme`
/// param) must treat an unknown name as "no theme", loudly, not silently
/// restyle to dark.
pub fn registered(name: &str) -> Option<&'static Theme> {
    THEMES
        .iter()
        .find(|t| t.name == name)
        .or_else(|| USER_THEMES.with(|u| u.borrow().iter().copied().find(|t| t.name == name)))
}

/// Build the `:root { … }` CSS block for a theme.
pub fn root_block(theme: &Theme) -> String {
    root_block_write(theme.scheme, theme.vars.iter().map(|(k, v)| (*k, *v)))
}

/// [`root_block`] over owned pairs — the theme editor's live-preview form
/// (a draft being edited exists only as owned strings; previewing must not
/// leak a registration). Identical output for identical inputs (tested).
pub fn root_block_from_pairs(scheme: &str, vars: &[(String, String)]) -> String {
    root_block_write(scheme, vars.iter().map(|(k, v)| (k.as_str(), v.as_str())))
}

fn root_block_write<'a>(scheme: &str, vars: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let mut s = String::from(":root{");
    // Native-control rendering scheme — see `Theme::scheme`. Must lead the
    // block so it's set before any control paints.
    s.push_str("color-scheme:");
    s.push_str(scheme);
    s.push(';');
    // Native checkbox/radio/progress glyphs follow the theme accent instead of
    // the UA default (blue on Chromium/Firefox, GTK-theme-dependent on
    // WebKitGTK) — without this the check glyph is the one control fragment
    // that ignores a theme flip. Inherited, so :root covers the shadow root too.
    s.push_str("accent-color:var(--accent);");
    for (k, v) in vars {
        s.push_str(k);
        s.push(':');
        s.push_str(v);
        s.push(';');
    }
    s.push('}');
    s
}

/// Live-preview install for the theme editor: rewrite `#theme-vars` from a
/// DRAFT (owned pairs) without registering or persisting anything. The next
/// [`install_root`] / [`reinstall_current`] (save, revert, registry sync)
/// replaces it with the real block.
#[cfg(target_arch = "wasm32")]
pub fn install_preview(scheme: &str, vars: &[(String, String)]) {
    install_style_block("theme-vars", Some(&root_block_from_pairs(scheme, vars)));
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
/// Call at boot (before first paint) and on theme change. An unregistered
/// name with a CSS paint-hint mirror (a user theme before the boot
/// subscription syncs — [`THEME_CSS_LS_KEY`]) installs the mirrored CSS
/// verbatim; otherwise unknown falls back to dark via [`lookup`].
#[cfg(target_arch = "wasm32")]
pub fn install_root(theme_name: &str) {
    if registered(theme_name).is_none() {
        if let Some(css) = ls_get(THEME_CSS_LS_KEY) {
            install_style_block("theme-vars", Some(&css));
            return;
        }
    }
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
    ("--site-code-bg", "#0a0a1a", "--surface-sunken"),  // markdown code / pre background
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

/// localStorage key carrying the computed `--site-*` `:root` CSS when the
/// site-appearance mode is a strict override to a **user** theme — the site
/// counterpart of [`THEME_CSS_LS_KEY`] (same paint-hint/self-heal contract).
pub const SITE_CSS_LS_KEY: &str = "entity_site_theme_css";

/// The "Site appearance" dropdown catalog: `(value, label)` in display order.
/// Two fixed modes (the site's own theme; follow the system theme) followed by
/// a strict override per registered theme. Adding a theme adds an "Always X"
/// override automatically.
pub fn site_appearance_catalog() -> Vec<(&'static str, String)> {
    let mut v = vec![
        ("site", "Site's theme".to_string()),
        ("system", "Match system theme".to_string()),
    ];
    for t in all_themes() {
        v.push((t.name, format!("Always {}", t.label)));
    }
    v
}

/// Is `mode` a valid "Site appearance" value? `"site"` / `"system"` / a
/// registered theme name. Used to reject stale or corrupt persisted values so
/// the boot path falls back to the `"site"` default instead of silently
/// freezing the overlay to a strict override (mirrors [`boot_choice`]'s filter).
pub fn is_valid_site_appearance(mode: &str) -> bool {
    mode == "site" || mode == "system" || registered(mode).is_some()
}

/// Look up an app token's value within a theme (e.g. `--text` in [`LIGHT`]).
fn theme_value<'a>(theme: &'a Theme, app_token: &str) -> Option<&'a str> {
    theme.vars.iter().find(|(k, _)| *k == app_token).map(|(_, v)| *v)
}

/// The effective value of a `--site-*` token under a theme: the theme's
/// value for the aliased app token, else the site default (a theme missing
/// the app token). The ONE resolution rule shared by the strict-override
/// `:root` block, the manifest-theme container block, and the static
/// exporter's frozen palette — so "Always Light", a `"theme": "light"`
/// manifest, and a light published page can never disagree on a color.
pub fn site_token_value(theme: &'static Theme, site_token: &str) -> &'static str {
    SITE_TOKENS
        .iter()
        .find(|(t, _, _)| *t == site_token)
        .map(|(_, default, app)| theme_value(theme, app).unwrap_or(default))
        .unwrap_or_else(|| panic!("unknown site token {site_token}"))
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
            for (site, _default, _app) in SITE_TOKENS {
                s.push_str(site);
                s.push(':');
                s.push_str(site_token_value(theme, site));
                s.push(';');
            }
            s.push('}');
            Some(s)
        }
    }
}

/// The manifest site-theme **container block**: inline-style custom-property
/// declarations (`--site-X:val;…`) freezing the whole `--site-*` family to a
/// registered theme. `None` for an unknown name — with a **once-per-session
/// warn** (D13: a publisher debugging "why doesn't my theme apply" gets a
/// loud line, not silence; once, not per frame). Applied by the site
/// renderer to the site's own wrapper element, ONLY when the effective
/// "Site appearance" mode is `"site"` — container properties override
/// inherited `:root` values, so an unconditional install would defeat a
/// strict user override (DESIGN-MANIFEST-SITE-THEME §3).
pub fn site_container_block(name: &str) -> Option<String> {
    let Some(theme) = registered(name) else {
        warn_unknown_site_theme(name);
        return None;
    };
    let mut s = String::new();
    for (site, _default, _app) in SITE_TOKENS {
        s.push_str(site);
        s.push(':');
        s.push_str(site_token_value(theme, site));
        s.push(';');
    }
    Some(s)
}

/// Warn once per session per unknown manifest theme name. The site render
/// output is rebuilt every frame while a site surface is live — an unguarded
/// warn would flood the console/log at 60Hz.
fn warn_unknown_site_theme(name: &str) {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    thread_local! {
        static WARNED: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
    }
    let first = WARNED.with(|w| w.borrow_mut().insert(name.to_string()));
    if first {
        tracing::warn!(
            theme = %name,
            "site manifest declares unknown theme — not a registered theme name; \
             rendering with the default site palette"
        );
    }
}

/// The site-appearance mode to install at boot: the localStorage mirror, else
/// `"site"` (the overlay's own theme). A strict override naming a user theme
/// is accepted before the registry syncs when its CSS paint-hint mirror is
/// present (mirrors [`boot_choice`]'s rule).
#[cfg(target_arch = "wasm32")]
pub fn site_appearance_boot_choice() -> String {
    ls_get(SITE_APPEARANCE_LS_KEY)
        .filter(|mode| is_valid_site_appearance(mode) || ls_get(SITE_CSS_LS_KEY).is_some())
        .unwrap_or_else(|| "site".to_string())
}

/// Native stub — no localStorage; always the overlay's own theme.
#[cfg(not(target_arch = "wasm32"))]
pub fn site_appearance_boot_choice() -> String {
    "site".to_string()
}

/// The **current** effective site-appearance mode. The localStorage mirror
/// is authoritative at any instant, not just boot: `apply_site_appearance`
/// rewrites it on every Settings change, so the boot read doubles as the
/// live read. Consumed by the site render-output builder to gate the
/// manifest theme (a per-frame call; one synchronous `getItem`).
pub fn site_appearance_current() -> String {
    site_appearance_boot_choice()
}

/// Inject / rewrite the `<style id="site-theme-vars">` element with the
/// `--site-*` block for `mode`. For `"site"` the block is `None`: any existing
/// element is emptied (CSS fallbacks resume), never injected. Call at boot and
/// on the "Site appearance" setting change. `"system"` needs no re-install on a
/// chrome flip — its `var()` aliases re-resolve.
#[cfg(target_arch = "wasm32")]
pub fn install_site_root(mode: &str) {
    // A strict override to a not-yet-registered user theme installs the CSS
    // paint-hint mirror verbatim (see `install_root`); the registry sync
    // re-installs the real block once the theme loads.
    if mode != "site" && mode != "system" && registered(mode).is_none() {
        if let Some(css) = ls_get(SITE_CSS_LS_KEY) {
            install_style_block("site-theme-vars", Some(&css));
            return;
        }
    }
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
    ls_set(SITE_APPEARANCE_LS_KEY, Some(mode));
    // A strict override to a USER theme mirrors its computed block as the
    // boot paint hint; every other mode clears it (built-ins resolve from
    // the static registry at boot; site/system need no palette).
    let user_css = (mode != "site" && mode != "system")
        .then(|| registered(mode))
        .flatten()
        .filter(|_| !THEMES.iter().any(|t| t.name == mode))
        .and_then(|_| site_root_block(mode));
    ls_set(SITE_CSS_LS_KEY, user_css.as_deref());
    install_site_root(mode);
}

/// Native stub — no DOM / localStorage.
#[cfg(not(target_arch = "wasm32"))]
pub fn apply_site_appearance(_mode: &str) {}

/// Re-derive and re-install BOTH appearance surfaces from the persisted
/// choices against the now-current registry, rewriting the CSS paint-hint
/// mirrors. Called by the user-theme registry sync (`crate::user_themes`)
/// after every membership/value change: an edited theme recolors live
/// surfaces, a stale boot mirror self-heals, and a theme deleted elsewhere
/// falls back to dark instead of a ghost palette. Idempotent and cheap
/// (runs only on actual theme-prefix changes, not per frame).
#[cfg(target_arch = "wasm32")]
pub fn reinstall_current() {
    let name = ls_get(THEME_LS_KEY).unwrap_or_else(|| DARK.name.to_string());
    apply_and_persist(&name);
    let mode = ls_get(SITE_APPEARANCE_LS_KEY).unwrap_or_else(|| "site".to_string());
    apply_site_appearance(&mode);
}

/// Native stub — no DOM / localStorage.
#[cfg(not(target_arch = "wasm32"))]
pub fn reinstall_current() {}

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
        // Native control glyphs (checkbox/radio check) must ride the theme
        // accent — the UA default ignores a theme flip.
        assert!(block.contains("accent-color:var(--accent);"));
        assert!(root_block(&LIGHT).contains("accent-color:var(--accent);"));
        assert!(block.contains("--bg:#1a1a2e;"));
        assert!(block.contains("--status-ok:#6c6;"));
        assert!(block.contains("--font-ui:system-ui, -apple-system, sans-serif;"));
    }

    #[test]
    fn every_token_has_a_value() {
        for theme in THEMES {
            for (k, v) in theme.vars {
                assert!(k.starts_with("--"), "{}: token {k} must start with --", theme.name);
                assert!(!v.is_empty(), "{}: token {k} has empty value", theme.name);
            }
        }
    }

    #[test]
    fn every_builtin_carries_exactly_darks_token_keys() {
        // §7's rule, mechanized: a `:root` block fully overrides only the
        // keys it lists — a builtin missing a key would silently fall back
        // to the (dark) var() literal on that surface. Same keys, same
        // ORDER (the editor's group tables ride the source order).
        let dark_keys: Vec<&str> = DARK.vars.iter().map(|(k, _)| *k).collect();
        for theme in THEMES {
            let keys: Vec<&str> = theme.vars.iter().map(|(k, _)| *k).collect();
            assert_eq!(keys, dark_keys, "{} must carry DARK's exact token key set", theme.name);
        }
    }

    #[test]
    fn site_tokens_are_well_formed_and_alias_real_app_tokens() {
        for (site, default, app) in SITE_TOKENS {
            assert!(site.starts_with("--site-"), "{site} must be a --site-* token");
            assert!(default.starts_with('#'), "{site} default must be a hex literal");
            // Every app token a site token aliases must exist in every
            // builtin (so "system" / strict modes always resolve, never
            // fall through).
            for theme in THEMES {
                assert!(
                    theme_value(theme, app).is_some(),
                    "{} missing {app} (aliased by {site})",
                    theme.name
                );
            }
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
    fn site_container_block_matches_the_strict_override_values() {
        // The manifest theme (container block) and the user's strict "Always
        // Light" (`:root` block) resolve through the SAME rule
        // (site_token_value) — assert value-identity so they can never
        // disagree on a color. The container form is bare declarations (an
        // inline style), the root form wraps them in `:root{…}`.
        let container = site_container_block("light").expect("light is registered");
        assert!(!container.contains("var("), "frozen literals only: {container}");
        let root = site_root_block("light").expect("strict light block");
        assert_eq!(format!(":root{{{container}}}"), root);
        // Spot values (LIGHT --overlay-bg / --text).
        assert!(container.contains("--site-bg:#f6f6fa;"), "container: {container}");
        assert!(container.contains("--site-text:#1a1a22;"));
    }

    #[test]
    fn site_container_block_rejects_unknown_names() {
        // Unlike the strict-override path (user input, falls back to DARK),
        // a manifest name is OUTSIDE input: unknown must be None (the caller
        // renders today's look), never a silent restyle to dark.
        assert_eq!(site_container_block("lab"), None);
        assert_eq!(site_container_block(""), None);
        assert_eq!(registered("dark").map(|t| t.name), Some("dark"));
        assert_eq!(registered("nonexistent").map(|t| t.name), None);
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
            include_str!("dom/site_editor.rs"),
        ];
        // (The `.cs-doc` document rules moved to `content_site/doc_css.rs`,
        // which BUILDS its fallbacks from SITE_TOKENS — correct by
        // construction, nothing to scan.)
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

    // -- user-theme registry (thread_local → each #[test] thread is isolated) --

    fn spec(name: &str) -> UserThemeSpec {
        UserThemeSpec {
            name: name.into(),
            label: format!("My {name}"),
            scheme: "dark".into(),
            // Derived from DARK with one visible difference, the way the
            // editor's duplicate-and-edit flow builds one.
            vars: DARK
                .vars
                .iter()
                .map(|(k, v)| {
                    let v = if *k == "--bg" { "#101010" } else { *v };
                    (k.to_string(), v.to_string())
                })
                .collect(),
        }
    }

    #[test]
    fn user_theme_registers_resolves_and_unregisters() {
        register_user_theme(spec("mytheme")).expect("registers");
        assert_eq!(registered("mytheme").map(|t| t.name), Some("mytheme"));
        assert_eq!(lookup("mytheme").label, "My mytheme");
        assert!(is_valid_site_appearance("mytheme"));
        // The whole resolution layer sees it: strict site override freezes
        // the USER theme's values (--site-bg aliases --overlay-bg = DARK's,
        // --bg itself isn't aliased — spot the edited surface via root_block).
        assert!(root_block(lookup("mytheme")).contains("--bg:#101010;"));
        let block = site_root_block("mytheme").expect("user theme emits a block");
        assert!(block.contains("--site-bg:#101018;"), "block: {block}");
        // Dropdown catalogs pick it up.
        assert!(all_themes().iter().any(|t| t.name == "mytheme"));
        let cat = site_appearance_catalog();
        assert!(cat.iter().any(|(v, l)| *v == "mytheme" && l == "Always My mytheme"));

        assert!(unregister_user_theme("mytheme"));
        assert!(registered("mytheme").is_none());
        assert_eq!(lookup("mytheme").name, "dark", "deleted user theme → dark");
        assert!(!is_valid_site_appearance("mytheme"));
        assert!(!unregister_user_theme("mytheme"), "second remove is a no-op");
    }

    #[test]
    fn user_theme_replace_updates_values_without_duplicating() {
        register_user_theme(spec("mine")).unwrap();
        let mut edited = spec("mine");
        for (k, v) in &mut edited.vars {
            if k == "--bg" {
                *v = "#202020".into();
            }
        }
        register_user_theme(edited).unwrap();
        assert_eq!(all_themes().iter().filter(|t| t.name == "mine").count(), 1);
        assert!(root_block(lookup("mine")).contains("--bg:#202020;"));
    }

    #[test]
    fn user_theme_missing_app_token_falls_back_to_site_default() {
        // A user theme lacking an aliased app token must resolve the site
        // token to its SITE_TOKENS default — the site_token_value rule.
        let mut s = spec("sparse");
        s.vars.retain(|(k, _)| k != "--overlay-bg");
        register_user_theme(s).unwrap();
        assert_eq!(site_token_value(lookup("sparse"), "--site-bg"), "#101018");
        unregister_user_theme("sparse");
    }

    #[test]
    fn theme_name_validation_rejects_reserved_and_malformed() {
        for bad in ["", "dark", "light", "site", "system", "Has Caps", "sp ace", "usr/../x"] {
            assert!(validate_theme_name(bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(validate_theme_name(&"x".repeat(41)).is_err(), "over-long rejected");
        for good in ["mytheme", "solarized-2", "x"] {
            assert!(validate_theme_name(good).is_ok(), "{good:?} must be accepted");
        }
        // register enforces the same gate + scheme/vars sanity.
        assert!(register_user_theme(spec("dark")).is_err());
        let mut bad_scheme = spec("ok-name");
        bad_scheme.scheme = "mauve".into();
        assert!(register_user_theme(bad_scheme).is_err());
        let mut no_vars = spec("ok-name");
        no_vars.vars.clear();
        assert!(register_user_theme(no_vars).is_err());
    }

    #[test]
    fn all_themes_lists_builtins_first_then_users_sorted() {
        register_user_theme(spec("zeta")).unwrap();
        register_user_theme(spec("alpha")).unwrap();
        let names: Vec<&str> = all_themes().iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["dark", "light", "sepia", "neon", "alpha", "zeta"]);
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
