//! Locale registry — the single source of truth for the app's languages and
//! layout direction. **Theming's structural twin** (see `theme_tokens.rs`):
//! where a theme is `name → color map` + a `scheme` (ltr/dark render mode),
//! a locale is `id → (label, dir)` + eventually a message catalog. This
//! module is the i18n counterpart of the theme *registry* — it owns the roster
//! of locales, boot detection, persistence, and the act of driving `lang`/`dir`
//! onto the DOM. The message *catalog* (`t()` / `t_plural()`, the compiled-in
//! `en` strings, plural selection) is a separate layer that lands in P1; this
//! is P0 — the wiring that makes the app *announce* its locale and light up
//! `dir` correctly, useful on its own even before a single string is
//! translated. Design: `docs/architecture/reviews/DESIGN-I18N-L10N.md` (§3.2,
//! §5 P0); format ADR: `docs/adr/0001-i18n-catalog-format.md`.
//!
//! ## The `dir` primitive (the load-bearing part)
//!
//! `dir` is to layout what a theme's `scheme` is to color: an irreducible,
//! per-locale primitive with exactly two values (`ltr` / `rtl`). Applying a
//! locale sets `lang`/`dir` on **two** elements (implementation-review
//! finding, DESIGN §3.2):
//!
//! - the **shadow host** (`#dom-layer`) — the app renders into an open shadow
//!   root, and `dir` on `<html>` does *not* cross that boundary. The host's
//!   `dir` inherits inward (the same way `dom/style.rs` pushes theme vars
//!   across `:host`). Setting only `<html>` is a silent RTL no-op.
//! - `<html>` — for the **light DOM** that lives outside the shadow root
//!   (status bar, `#site-layer` overlay, loading screen, runtime banners).
//!
//! Both, every time. The localStorage mirror (`LANG_LS_KEY`) lets [`boot_choice`]
//! pick the locale synchronously at first paint — no flash, no wait on the tree
//! round-trip (the same paint-hint pattern the theme boot mirror uses, and the
//! same reason: the Worker-arm settings read returns the default until the
//! cache seeds, so we detect from `navigator`/localStorage directly).

/// A registered locale: a stable id, a human label for the Settings picker,
/// and its layout direction. The twin of [`crate::theme_tokens::Theme`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locale {
    /// BCP-47 language id, persisted in `SettingsState.language` (e.g. `"en"`).
    pub id: &'static str,
    /// Human label for the Settings picker (e.g. `"English"`).
    pub label: &'static str,
    /// Layout direction: `"ltr"` or `"rtl"`. The non-string primitive — twin of
    /// a theme's `scheme`. Emitted as the `dir` attribute on the shadow host +
    /// `<html>`.
    pub dir: &'static str,
    /// A development pseudo-locale (e.g. `en-XA`), never auto-selected by
    /// [`detect`] — it exists to surface RTL / hardcoded-string bugs, not to be
    /// a real user language. Explicitly pickable in Settings.
    pub pseudo: bool,
}

/// The default locale id — always complete, compiled in, frame one.
pub const DEFAULT: &str = "en";

/// The locale roster. **P0 ships the real `en` plus one pseudo-locale**
/// (`en-XA`), deliberately: `en-XA` forces `dir=rtl` with *no* string change,
/// so the shadow-host `dir` plumbing and every physical-CSS layout bug become
/// visible and testable end-to-end *now* — before any catalog or translation
/// exists ("don't get blindsided by RTL", DESIGN §5.1). In P1 the same id grows
/// its string-mangling transform (bracket + accent + pad) once `t()` exists;
/// today it is dir-only. Real target locales (`es`/`ar`/`he`) join here as
/// their catalogs land (P4) — one row each, exactly as adding a built-in theme
/// is one entry in `THEMES`.
pub const LOCALES: &[Locale] = &[
    Locale { id: "en", label: "English", dir: "ltr", pseudo: false },
    // First real target locales (P4). Labels are endonyms (each language's own
    // name — picker convention). Their catalogs are the `locales/*.json`
    // overlays baked at build time; `he`/`ar` carry `dir: "rtl"` — the primitive
    // that drives the whole logical-CSS layout flip. A locale is pickable from
    // this roster even under a lean `en`-only build (strings fall back to `en`,
    // but `dir` still flips) — the roster and the embedded catalog are
    // independent (see `locales/README.md`).
    Locale { id: "es", label: "Español", dir: "ltr", pseudo: false },
    Locale { id: "he", label: "עברית", dir: "rtl", pseudo: false },
    Locale { id: "ar", label: "العربية", dir: "rtl", pseudo: false },
    // Pseudo-locale: RTL, no translation. The RTL-blindside detector.
    Locale { id: "en-XA", label: "Pseudo (RTL)", dir: "rtl", pseudo: true },
];

/// The full roster (for the Settings picker). Twin of `all_themes()`.
pub fn available_locales() -> &'static [Locale] {
    LOCALES
}

/// Look up a locale by id. `None` for an unknown id.
pub fn locale(id: &str) -> Option<&'static Locale> {
    LOCALES.iter().find(|l| l.id == id)
}

/// Resolve a locale by id, falling back to [`DEFAULT`] (`en`) for an unknown or
/// stale id (e.g. a persisted locale from before it existed / after removal).
/// Never fails — the twin of theming's `lookup(name) -> DARK` fallback.
pub fn resolve(id: &str) -> &'static Locale {
    locale(id).unwrap_or_else(|| locale(DEFAULT).expect("en locale always present"))
}

// ---------------------------------------------------------------------------
// Boot detection, persistence, and applying `lang`/`dir` (wasm)
// ---------------------------------------------------------------------------

/// localStorage key mirroring the chosen locale id. The tree
/// (`SettingsState.language`) is the durable record, but it isn't readable
/// until peers boot; this mirror lets [`boot_choice`] pick the locale
/// synchronously at first paint. Twin of [`crate::theme_tokens::THEME_LS_KEY`].
pub const LANG_LS_KEY: &str = "entity_language";

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

/// Detect the user's preferred locale from `navigator.language`, intersected
/// with the **selectable** (non-pseudo) roster, matching on the primary subtag
/// (`"en-US"` → `en`). Falls back to [`DEFAULT`]. Pseudo-locales are never
/// auto-detected — they are dev tools, chosen explicitly in Settings.
#[cfg(target_arch = "wasm32")]
pub fn detect() -> String {
    let nav_lang = web_sys::window()
        .map(|w| w.navigator())
        .and_then(|n| n.language())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let primary = nav_lang.split('-').next().unwrap_or("");
    LOCALES
        .iter()
        .filter(|l| !l.pseudo)
        .find(|l| l.id.eq_ignore_ascii_case(&nav_lang) || l.id.eq_ignore_ascii_case(primary))
        .map(|l| l.id.to_string())
        .unwrap_or_else(|| DEFAULT.to_string())
}

/// Native stub — no `navigator`; always the default.
#[cfg(not(target_arch = "wasm32"))]
pub fn detect() -> String {
    DEFAULT.to_string()
}

/// The locale to install at boot: the localStorage mirror if it names a known
/// locale, else the `navigator.language` [`detect`]ion, else [`DEFAULT`].
/// Twin of [`crate::theme_tokens::boot_choice`].
#[cfg(target_arch = "wasm32")]
pub fn boot_choice() -> String {
    ls_get(LANG_LS_KEY)
        .filter(|id| locale(id).is_some())
        .unwrap_or_else(detect)
}

/// Native stub — no localStorage; always the default.
#[cfg(not(target_arch = "wasm32"))]
pub fn boot_choice() -> String {
    DEFAULT.to_string()
}

/// Apply a locale, the twin of [`crate::theme_tokens::apply_and_persist`]:
///
/// 1. set the process-active locale (what [`t`] resolves against),
/// 2. bump [`locale_generation`] so the frame loop force-rebuilds every open
///    window ([`mark_all_dirty`]) — a language switch changes baked DOM *text*
///    (unlike a CSS-only theme flip), so per-window dirty flags aren't enough
///    (DESIGN §3.2, finding 1),
/// 3. (wasm) persist the boot mirror + drive `lang`/`dir` onto the shadow host
///    (`#dom-layer`, inherits into the shadow tree) **and** `<html>` (light DOM).
///
/// The durable tree write is the caller's job (`SettingsState`); this is the
/// runtime side. Steps 1–2 run on native too so `t()` + the generation counter
/// are unit-testable.
pub fn apply(id: &str) {
    let loc = resolve(id);
    set_active(loc.id);
    mark_all_dirty();
    #[cfg(target_arch = "wasm32")]
    {
        ls_set(LANG_LS_KEY, Some(loc.id));
        install_lang_dir(loc);
    }
}

/// Set `lang`/`dir` on the shadow host + `<html>`. Idempotent; safe to call at
/// boot (the `#dom-layer` host exists in the static HTML before the shadow root
/// attaches — the attribute persists and inheritance applies once shadow
/// content renders).
#[cfg(target_arch = "wasm32")]
pub fn install_lang_dir(loc: &Locale) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    // `<html>` — light DOM (status bar, site overlay, loading screen, banners)
    // and the document's declared language.
    if let Some(html) = document.document_element() {
        let _ = html.set_attribute("lang", loc.id);
        let _ = html.set_attribute("dir", loc.dir);
    }
    // Shadow host — `dir` on `<html>` does NOT cross the shadow boundary, so it
    // must be set here for the app's rendered UI to flip (DESIGN §3.2, finding
    // 2). The same boundary the theme layer already crosses for `:host` vars.
    if let Some(host) = document.get_element_by_id("dom-layer") {
        let _ = host.set_attribute("lang", loc.id);
        let _ = host.set_attribute("dir", loc.dir);
    }
}

// ===========================================================================
// P1 — the message catalog + `t()` (the string seam)
// ===========================================================================
//
// The twin of the theme *token* layer: raw English → a message **key** resolved
// through one indirection (`t(key, args)`), just as raw hex became `var(--tok)`.
// P1 stands up the seam and its reactivity primitive; the 358-string extraction
// is P4. Only a small seed catalog + the Settings demonstrators flow through
// `t()` today ("migrate no strings yet" — the mechanism-first / theming play).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// CLDR plural categories. `en`/`es` use only `One`/`Other`; `he` uses
/// `One`/`Two`/`Other`; `ar` uses all six. Category sets are pinned to the
/// authoritative CLDR cardinal rules (see [`plural_category`] + its vector
/// tests, and ADR-0001 §"Plural resolution"). The selector stays **honest**
/// about which categories it supports — that honesty was the single input that
/// could have flipped the bespoke-vs-Fluent decision (design §8); it did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluralCategory {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

/// One catalog entry: a simple template, or a set of CLDR plural forms selected
/// by the `n` argument. Templates carry `{named}` slots interpolated (and
/// bidi-isolated) by [`t`] / [`t_plural`].
pub enum Message {
    Simple(&'static str),
    Plural(&'static [(PluralCategory, &'static str)]),
}

/// The compiled-in `en` base catalog — always complete, frame one (the twin of
/// the built-in `DARK` theme). Keys are dotted + namespaced by surface. P1 seeds
/// a small **base vocabulary** (shared across apps — translate once, extend
/// per-app, design §7) plus the Settings demonstrators actually wired through
/// `t()`; the long-tail extraction is P4.
pub const EN: &[(&str, Message)] = &[
    // -- base vocabulary (ecosystem-shareable common UI terms) --
    // Buttons/verbs — reused across every window; translate once, extend per-app.
    ("btn.save", Message::Simple("Save")),
    ("btn.cancel", Message::Simple("Cancel")),
    ("btn.delete", Message::Simple("Delete")),
    ("btn.copy", Message::Simple("Copy")),
    ("btn.connect", Message::Simple("Connect")),
    ("btn.edit", Message::Simple("Edit")),
    ("btn.refresh", Message::Simple("Refresh")),
    ("btn.back", Message::Simple("← Back")),
    ("btn.revert", Message::Simple("Revert")),
    ("btn.create", Message::Simple("Create")),
    ("btn.move", Message::Simple("Move")),
    ("btn.authorize", Message::Simple("Authorize")),
    ("btn.add_peer", Message::Simple("Add peer")),
    ("btn.find", Message::Simple("Find")),
    ("btn.count", Message::Simple("Count")),
    ("btn.trace", Message::Simple("Trace")),
    // Field/section labels — common nouns shared across windows.
    ("label.status", Message::Simple("Status")),
    ("label.title", Message::Simple("Title")),
    ("label.peer", Message::Simple("Peer")),
    ("label.target", Message::Simple("Target")),
    ("label.address", Message::Simple("Address")),
    ("label.name", Message::Simple("Name")),
    ("label.label", Message::Simple("Label")),
    // Transient status glyphs.
    ("status.copied", Message::Simple("Copied ✓")),
    // -- execute console surface --
    ("execute.handler", Message::Simple("Handler")),
    ("execute.operation", Message::Simple("Operation")),
    ("execute.handler_uri", Message::Simple("Handler URI")),
    ("execute.resource", Message::Simple("Resource")),
    ("execute.execute", Message::Simple("Execute")),
    ("execute.guided", Message::Simple("Guided")),
    ("execute.raw", Message::Simple("Raw")),
    // -- peer connections surface --
    ("peers.known_devices", Message::Simple("Known devices")),
    ("peers.connect_device", Message::Simple("Connect to a device")),
    ("peers.pair_qr", Message::Simple("Pair a device (QR)")),
    // -- window-chrome tooltips (title attrs) --
    ("tooltip.close_window", Message::Simple("Close window")),
    ("tooltip.open_windows", Message::Simple("Open windows")),
    ("tooltip.fill_window", Message::Simple("Fill the window")),
    ("tooltip.restore_size", Message::Simple("Back to normal size")),
    ("tooltip.menu", Message::Simple("Menu")),
    ("tooltip.back", Message::Simple("Back")),
    ("tooltip.site_home", Message::Simple("Go to site home")),
    ("tooltip.unsaved", Message::Simple("Unsaved changes")),
    // -- input placeholders --
    ("kb.title_placeholder", Message::Simple("Article title")),
    ("theme.name_placeholder", Message::Simple("name (a-z, 0-9, dashes)")),
    ("theme.label_placeholder", Message::Simple("display label")),
    // -- file transfer surface --
    ("filetransfer.device", Message::Simple("Device")),
    ("filetransfer.shared_files", Message::Simple("Shared files")),
    ("filetransfer.send_file", Message::Simple("Send a file")),
    ("filetransfer.from_peer", Message::Simple("From peer")),
    ("filetransfer.results", Message::Simple("Results")),
    ("filetransfer.pull_selected", Message::Simple("\u{2b07} Pull selected file")),
    ("filetransfer.upload_file", Message::Simple("Upload a file")),
    // -- site editor surface --
    ("siteeditor.create_site", Message::Simple("Create site")),
    ("siteeditor.delete_site", Message::Simple("Delete site")),
    ("siteeditor.save_page", Message::Simple("Save page")),
    ("siteeditor.delete_page", Message::Simple("Delete page")),
    // -- system peers surface --
    ("syspeers.system_peer", Message::Simple("System peer")),
    ("syspeers.system_backend", Message::Simple("System backend")),
    // -- system overview surface --
    ("sysoverview.clear_logs", Message::Simple("Clear logs")),
    ("sysoverview.logs", Message::Simple("Logs")),
    ("sysoverview.device_auth", Message::Simple("Device authorizations")),
    (
        "sysoverview.grant_backend_hint",
        Message::Simple("Authorized on the backend; the specific scope isn't recorded locally."),
    ),
    // -- knowledge base surface --
    ("kb.new_article", Message::Simple("+ New article")),
    ("kb.back_to_list", Message::Simple("← Back to list")),
    // -- settings surface (the P1 demonstrators — wired through t()) --
    ("settings.appearance", Message::Simple("Appearance")),
    ("settings.theme", Message::Simple("Theme")),
    ("settings.language", Message::Simple("Language")),
    ("settings.windows", Message::Simple("Windows")),
    ("settings.site_surface", Message::Simple("Site & Surface")),
    ("settings.rendering", Message::Simple("Rendering")),
    ("settings.network", Message::Simple("Network")),
    // -- theme editor surface --
    ("theme.themes", Message::Simple("Themes")),
    ("theme.new_from", Message::Simple("New theme from")),
    (
        "settings.language.hint",
        Message::Simple("Sets the interface language and layout direction."),
    ),
    // -- menu category headers + blurbs (WindowCategory label/description) --
    ("category.apps", Message::Simple("Apps & Content")),
    ("category.system", Message::Simple("System")),
    ("category.developer", Message::Simple("Developer")),
    (
        "category.apps.desc",
        Message::Simple("Browse sites, play games, run apps"),
    ),
    (
        "category.system.desc",
        Message::Simple("Peers, keys, connections, settings, storage"),
    ),
    (
        "category.developer.desc",
        Message::Simple("Entity tree, query & execute consoles, shell, logs"),
    ),
    // -- window titles (title bar + menu label, via i18n::window_title) --
    ("window.entity_tree", Message::Simple("Entity Tree")),
    ("window.games", Message::Simple("Games")),
    ("window.apps", Message::Simple("Apps")),
    ("window.knowledge_base", Message::Simple("Knowledge Base")),
    ("window.key_manager", Message::Simple("Key Manager")),
    ("window.peer_connections", Message::Simple("Peer Connections")),
    ("window.file_transfer", Message::Simple("File Transfer")),
    ("window.execute_console", Message::Simple("Execute Console")),
    ("window.query_console", Message::Simple("Query Console")),
    ("window.settings", Message::Simple("Settings")),
    ("window.event_log", Message::Simple("Event Log")),
    ("window.peers", Message::Simple("Peers")),
    ("window.shell", Message::Simple("Shell")),
    ("window.chain_trace", Message::Simple("Chain Trace")),
    ("window.path_tap", Message::Simple("Path Tap")),
    ("window.wire_recorder", Message::Simple("Wire Recorder")),
    ("window.content_stream", Message::Simple("Content Stream")),
    ("window.site_browser", Message::Simple("Site Browser")),
    ("window.storage", Message::Simple("Storage")),
    ("window.site_creator", Message::Simple("Site Creator")),
    ("window.system_overview", Message::Simple("System Overview")),
    ("window.access_log", Message::Simple("Access Log")),
    ("window.theme_editor", Message::Simple("Theme Editor")),
    // -- QR scanner surface (dom/scanner.rs) --
    ("scanner.scanned_codes", Message::Simple("Scanned Codes")),
    ("scanner.scanned_codes_count", Message::Simple("Scanned Codes ({n} unique)")),
    ("scanner.photo_capture", Message::Simple("Photo Capture")),
    ("scanner.photo_hint", Message::Simple("Take a photo of a QR code")),
    ("scanner.take_photo", Message::Simple("Take Photo")),
    ("scanner.processing", Message::Simple("Processing...")),
    (
        "scanner.no_detector",
        Message::Simple("No BarcodeDetector — enter code manually"),
    ),
    ("scanner.detector_init_failed", Message::Simple("Detector init failed")),
    ("scanner.detect_call_failed", Message::Simple("Detect call failed")),
    ("scanner.qr_found", Message::Simple("QR code found!")),
    ("scanner.no_qr", Message::Simple("No QR found — try again")),
    ("scanner.decode_error", Message::Simple("Decode error")),
    ("scanner.live_scanner", Message::Simple("Live Scanner")),
    ("scanner.live_hint", Message::Simple("Real-time camera scanning")),
    ("scanner.start_live", Message::Simple("Start Live Scan")),
    ("scanner.stop", Message::Simple("Stop")),
    ("scanner.stopped", Message::Simple("Stopped")),
    ("scanner.camera_unavailable", Message::Simple("Camera not available")),
    ("scanner.camera_denied", Message::Simple("Camera denied")),
    ("scanner.starting_camera", Message::Simple("Starting camera...")),
    ("scanner.stream_error", Message::Simple("Stream error")),
    ("scanner.scanning", Message::Simple("Scanning...")),
    ("scanner.detector_failed", Message::Simple("Detector failed")),
    ("scanner.scan_detect_error", Message::Simple("Scan {n} — detect error")),
    ("scanner.found_continuing", Message::Simple("Found! Continuing scan...")),
    ("scanner.scan_n", Message::Simple("Scan {n}...")),
    ("scanner.scan_error", Message::Simple("Scan {n} — error")),
    // -- a plural example: proves the selector round-trips (not yet consumed) --
    (
        "peer.count",
        Message::Plural(&[
            (PluralCategory::One, "{n} peer"),
            (PluralCategory::Other, "{n} peers"),
        ]),
    ),
];

/// `en` catalog as a lookup map, built once. (Linear scan would do at this size;
/// the map keeps `t()` O(1) as the catalog grows toward P4.)
fn en_map() -> &'static HashMap<&'static str, &'static Message> {
    static MAP: OnceLock<HashMap<&'static str, &'static Message>> = OnceLock::new();
    MAP.get_or_init(|| EN.iter().map(|(k, m)| (*k, m)).collect())
}

/// A build-time-embedded overlay locale: a locale id + its `(key, message)`
/// entries, generated by `build.rs` from the JSON files under
/// `I18N_LOCALES_ROOT` (default `locales/`). `en` is **not** here — it is the
/// compiled-in [`EN`] base; these are the *additional* locales that layer over
/// it (a missing key falls back to `en`). The pseudo-locale (`en-XA`) is not
/// embedded either — it transforms `en`'s resolved string.
pub struct EmbeddedLocale {
    pub id: &'static str,
    pub entries: &'static [(&'static str, Message)],
}

// Generated by build.rs at compile time. Defines:
//   pub static EMBEDDED_LOCALES: &[EmbeddedLocale] = &[ ... ];
include!(concat!(env!("OUT_DIR"), "/embedded_locales.rs"));

/// The embedded overlay catalogs as `id → (key → &Message)`, built once. Empty
/// on a lean `en`-only build.
fn embedded_map() -> &'static HashMap<&'static str, HashMap<&'static str, &'static Message>> {
    static MAP: OnceLock<HashMap<&'static str, HashMap<&'static str, &'static Message>>> =
        OnceLock::new();
    MAP.get_or_init(|| {
        EMBEDDED_LOCALES
            .iter()
            .map(|loc| (loc.id, loc.entries.iter().map(|(k, m)| (*k, m)).collect()))
            .collect()
    })
}

/// Look up `key` for `locale_id`: the active locale's own overlay catalog wins;
/// a key it doesn't define (a **partial** locale — the common case, since
/// catalogs start as the base vocabulary only) falls back to the compiled-in
/// `en` base. `en` itself and the pseudo-locale have no overlay → straight to
/// `en`. Never returns `None` for a key present in `en` — the D13 "always
/// render, never a raw key" guarantee rides on the `en` completeness.
fn catalog_entry(locale_id: &str, key: &str) -> Option<&'static Message> {
    if let Some(overlay) = embedded_map().get(locale_id) {
        if let Some(msg) = overlay.get(key) {
            return Some(*msg);
        }
    }
    en_map().get(key).copied()
}

/// Slugify a canonical window name into its catalog-key suffix:
/// `"File Transfer"` → `"file_transfer"` (non-alphanumerics → `_`).
fn window_slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// The localized display title for a window, by its **canonical (English)
/// name**. The one place both the title bar (`WindowView::title`) and the
/// menu/picker label (`window::window_display_name`) resolve through, so they
/// stay in sync (the invariant `window.rs` flags). The canonical name remains
/// the identity key — matching, grouping, and persistence use it verbatim; only
/// the *rendered* title is translated. A name with no `window.<slug>` key
/// renders verbatim (never a raw `window.` key leaks to the user — D13).
pub fn window_title(name: &str) -> String {
    let key = format!("window.{}", window_slug(name));
    match catalog_entry(active_id(), &key) {
        Some(_) => t(&key, &[]),
        None => name.to_string(),
    }
}

thread_local! {
    /// The process-active locale id. WASM is single-threaded (render + apply
    /// share the main thread); on native each test thread carries its own,
    /// which is exactly what per-test isolation wants.
    static ACTIVE_LOCALE: std::cell::RefCell<&'static str> = const { std::cell::RefCell::new(DEFAULT) };
    /// Keys already logged missing — so a gap logs once, not every frame.
    static LOGGED_MISSING: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
}

/// Monotonic locale generation. Bumped by [`mark_all_dirty`] on every locale
/// apply; the DOM frame loop compares it to its last-seen value and force-
/// rebuilds every open window when it advances (the "dirty all" the per-window
/// `WindowWatch` can't express). Process-global (an atomic), unlike the
/// thread-local active locale, because the one renderer is the sole reader.
static LOCALE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Set the process-active locale (resolved to a known id; unknown → `en`).
pub fn set_active(id: &str) {
    let loc = resolve(id);
    ACTIVE_LOCALE.with(|a| *a.borrow_mut() = loc.id);
}

/// The process-active locale id.
pub fn active_id() -> &'static str {
    ACTIVE_LOCALE.with(|a| *a.borrow())
}

/// The current locale generation — read by the DOM frame loop.
pub fn locale_generation() -> u64 {
    LOCALE_GENERATION.load(Ordering::Relaxed)
}

/// Advance the locale generation, signalling the frame loop to rebuild **every**
/// open window on its next pass. Called by [`apply`]; the reactivity primitive
/// the whole feature rides on (DESIGN §3.2, finding 1).
pub fn mark_all_dirty() {
    LOCALE_GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Resolve a message key to a localized, interpolated string. Args fill
/// `{named}` slots and are **bidi-isolated by construction** (wrapped in
/// FSI/PDI) so an LTR datum (a peer id, a URI) dropped into an RTL sentence
/// can't reorder the surrounding text — no call site can forget it (DESIGN
/// §3.2, finding 4). A missing key falls back to `en`, and if absent there too
/// logs once and returns the key (never a silent blank — D13). Twin of a
/// `var(--token)` lookup.
pub fn t(key: &str, args: &[(&str, &str)]) -> String {
    let active = active_id();
    let pseudo = resolve(active).pseudo;
    match catalog_entry(active, key) {
        Some(Message::Simple(template)) => render_template(template, args, pseudo),
        Some(Message::Plural(_)) => {
            // A plural key rendered without a count — caller should use
            // `t_plural`. Fall back to the `Other` form so we still show text.
            plural_render(key, PluralCategory::Other, args, pseudo).unwrap_or_else(|| {
                log_missing(key);
                key.to_string()
            })
        }
        None => {
            log_missing(key);
            key.to_string()
        }
    }
}

/// Resolve a plural message key, selecting the CLDR form for `n` in the active
/// locale. Args (typically `("n", n)`) are bidi-isolated like [`t`].
///
/// The plural half of the P1 string seam — unit-tested, with no non-test caller
/// until the P4 extraction routes count strings (`"{n} peers"`) through it.
#[allow(dead_code)] // P1 seam; first consumed at P4 (preserve the public surface).
pub fn t_plural(key: &str, n: i64, args: &[(&str, &str)]) -> String {
    let active = active_id();
    let pseudo = resolve(active).pseudo;
    let cat = plural_category(active, n);
    plural_render(key, cat, args, pseudo).unwrap_or_else(|| {
        log_missing(key);
        key.to_string()
    })
}

/// Select the CLDR **cardinal** plural category for `n` in `locale_id`.
///
/// Rules pinned to the authoritative CLDR source (`unicode.org/cldr` v47 +
/// `unicode-org/cldr-json`, corroborated — ADR-0001 §"Plural resolution"), for
/// the **integer-count scope** the app actually renders (every count is a
/// non-negative integer, so the CLDR `v`/`f`/`e` operands are 0 and `i = n`):
///
/// - `en` / `es` (+ the `en`-derived pseudo): `One` ⇔ n=1, else `Other`. (`es`
///   also has a CLDR `many`, but **only for compact notation** — "1 M", the `e`
///   exponent operand — which we never emit.)
/// - `he`: `One` ⇔ n=1, `Two` ⇔ n=2, else `Other`. **No `many`** (removed from
///   CLDR in v42). The CLDR `one` branch `i=0 and v!=0` is fraction-only and
///   can't arise from an integer count.
/// - `ar`: the full six. `Zero`/`One`/`Two` are exact; `Few` ⇔ n%100∈3..10,
///   `Many` ⇔ n%100∈11..99; everything else (incl. n%100∈{0,1,2} for n≥100) is
///   `Other`.
///
/// An unknown locale falls through to the `en` rule — documented, not silent —
/// until its own catalog + rules land.
#[allow(dead_code)] // reached via t_plural (the plural seam); live once counts route through it.
fn plural_category(locale_id: &str, n: i64) -> PluralCategory {
    use PluralCategory::*;
    let base = locale_id.split('-').next().unwrap_or(locale_id);
    match base {
        "en" | "es" => {
            if n == 1 {
                One
            } else {
                Other
            }
        }
        "he" => match n {
            1 => One,
            2 => Two,
            _ => Other,
        },
        "ar" => {
            // CLDR operand n is the absolute value; counts are non-negative, but
            // rem_euclid keeps the modulo correct even if a negative ever slips in.
            let m = n.rem_euclid(100);
            match n {
                0 => Zero,
                1 => One,
                2 => Two,
                _ if (3..=10).contains(&m) => Few,
                _ if (11..=99).contains(&m) => Many,
                _ => Other,
            }
        }
        _ => {
            if n == 1 {
                One
            } else {
                Other
            }
        }
    }
}

/// Render a plural key's `cat` form (falling back to `Other`, then the first
/// listed form), interpolated. `None` if the key isn't a plural entry.
fn plural_render(
    key: &str,
    cat: PluralCategory,
    args: &[(&str, &str)],
    pseudo: bool,
) -> Option<String> {
    let active = active_id();
    let Some(Message::Plural(forms)) = catalog_entry(active, key) else {
        return None;
    };
    let pick = |c: PluralCategory| forms.iter().find(|(fc, _)| *fc == c).map(|(_, t)| *t);
    let template = pick(cat)
        .or_else(|| pick(PluralCategory::Other))
        .or_else(|| forms.first().map(|(_, t)| *t))?;
    Some(render_template(template, args, pseudo))
}

/// Interpolate `{named}` slots and (for a pseudo-locale) transform the literal
/// text. Args are wrapped in Unicode isolates (FSI `\u{2068}` … PDI `\u{2069}`)
/// so bidi reordering can't leak across them. In pseudo mode the *literal*
/// segments are accented, bracketed (`⟦…⟧`), and padded +~33% — placeholders
/// and their (real, un-accented) arg values pass through untouched, so the
/// pseudo output still reads structurally while flagging un-extracted strings
/// and truncation.
fn render_template(template: &str, args: &[(&str, &str)], pseudo: bool) -> String {
    const FSI: char = '\u{2068}';
    const PDI: char = '\u{2069}';
    let mut out = String::new();
    if pseudo {
        out.push('⟦');
    }
    let mut literal_len = 0usize;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            for nc in chars.by_ref() {
                if nc == '}' {
                    break;
                }
                name.push(nc);
            }
            let val = args
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| *v)
                .unwrap_or("");
            out.push(FSI);
            out.push_str(val);
            out.push(PDI);
        } else {
            out.push(if pseudo { accent(c) } else { c });
            literal_len += 1;
        }
    }
    if pseudo {
        for _ in 0..(literal_len / 3) {
            out.push('·');
        }
        out.push('⟧');
    }
    out
}

/// Accent a Latin letter for the pseudo-locale (leaves everything else alone).
fn accent(c: char) -> char {
    match c {
        'a' => 'á',
        'e' => 'é',
        'i' => 'í',
        'o' => 'ó',
        'u' => 'ú',
        'n' => 'ñ',
        'c' => 'ç',
        's' => 'š',
        'A' => 'Á',
        'E' => 'É',
        'I' => 'Í',
        'O' => 'Ó',
        'U' => 'Ú',
        'N' => 'Ñ',
        other => other,
    }
}

/// Log a missing catalog key once (per thread). A gap is a bug to fix, not a
/// crash — never render a raw key silently to the user (D13).
fn log_missing(key: &str) {
    LOGGED_MISSING.with(|set| {
        if set.borrow_mut().insert(key.to_string()) {
            tracing::warn!(key, "i18n: missing catalog key (rendered fallback)");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn en_is_present_and_ltr() {
        let en = locale("en").expect("en present");
        assert_eq!(en.dir, "ltr");
        assert!(!en.pseudo);
    }

    #[test]
    fn resolve_falls_back_to_en() {
        assert_eq!(resolve("zz-nonexistent").id, "en");
        assert_eq!(resolve("zz-nonexistent").dir, "ltr");
    }

    #[test]
    fn pseudolocale_is_rtl_and_flagged() {
        let xa = locale("en-XA").expect("pseudo present");
        assert_eq!(xa.dir, "rtl");
        assert!(xa.pseudo, "pseudo-locale must be flagged so detect() skips it");
    }

    #[test]
    fn default_is_a_real_locale() {
        let d = locale(DEFAULT).expect("DEFAULT resolves");
        assert!(!d.pseudo, "the default must be a real, selectable locale");
    }

    // -- P1: catalog + t() -------------------------------------------------

    #[test]
    fn t_resolves_a_seeded_key() {
        set_active("en");
        assert_eq!(t("btn.save", &[]), "Save");
        assert_eq!(t("settings.language", &[]), "Language");
    }

    #[test]
    fn t_missing_key_returns_the_key_not_blank() {
        set_active("en");
        // D13: never a silent blank; the key itself surfaces the gap.
        assert_eq!(t("no.such.key", &[]), "no.such.key");
    }

    #[test]
    fn t_interpolates_and_bidi_isolates_args() {
        set_active("en");
        // en has no interpolated seed key, so exercise the renderer directly.
        let s = render_template("hi {name}!", &[("name", "Ann")], false);
        assert_eq!(s, "hi \u{2068}Ann\u{2069}!", "args must be FSI/PDI-wrapped");
    }

    #[test]
    fn pseudolocale_transforms_literals_but_not_args() {
        set_active("en-XA");
        let s = t("btn.save", &[]);
        assert!(s.starts_with('⟦') && s.ends_with('⟧'), "bracketed: {s:?}");
        assert!(s.contains('š') || s.contains('á'), "accented: {s:?}");
        // Args stay real (un-accented) even in pseudo mode.
        let interp = render_template("x {v}", &[("v", "raw")], true);
        assert!(interp.contains("raw"), "arg value must survive: {interp:?}");
    }

    #[test]
    fn t_plural_selects_by_count() {
        set_active("en");
        assert_eq!(t_plural("peer.count", 1, &[("n", "1")]), "\u{2068}1\u{2069} peer");
        assert_eq!(t_plural("peer.count", 3, &[("n", "3")]), "\u{2068}3\u{2069} peers");
        // zero uses Other in en/es.
        assert_eq!(t_plural("peer.count", 0, &[("n", "0")]), "\u{2068}0\u{2069} peers");
    }

    // -- P4: CLDR cardinal plural vectors (ADR-0001 §"Plural resolution") -----
    //
    // Boundaries pinned to the authoritative CLDR source (unicode.org/cldr v47
    // chart + unicode-org/cldr-json plurals.json, corroborated) — asserted, not
    // trusted to memory. Representative n per category, including the modulo
    // boundaries that are the real correctness test. Integer scope (v=0).

    use PluralCategory::{Few, Many, Other, Two, Zero};

    #[test]
    fn cldr_plural_es_one_other() {
        // es: one ⇔ n=1, else other. (CLDR `many` is compact-notation only.)
        assert_eq!(plural_category("es", 1), PluralCategory::One);
        for n in [0, 2, 3, 11, 100, 1000] {
            assert_eq!(plural_category("es", n), Other, "es n={n}");
        }
    }

    #[test]
    fn cldr_plural_he_one_two_other_no_many() {
        // he: one ⇔ n=1, two ⇔ n=2, else other. Crucially NO `many` — 20/30/100
        // (old memory-rule multiples of ten) are `other`, not `many`.
        assert_eq!(plural_category("he", 1), PluralCategory::One);
        assert_eq!(plural_category("he", 2), Two);
        for n in [0, 3, 10, 17, 20, 30, 100, 1000] {
            assert_eq!(plural_category("he", n), Other, "he n={n}");
        }
    }

    #[test]
    fn cldr_plural_ar_all_six_with_modulo_boundaries() {
        // ar: the six-category worst case — the modulo logic is the real test.
        assert_eq!(plural_category("ar", 0), Zero);
        assert_eq!(plural_category("ar", 1), PluralCategory::One);
        assert_eq!(plural_category("ar", 2), Two);
        // few: n%100 = 3..10 (CLDR examples: 3~10, 103~110, 1003)
        for n in [3, 10, 103, 110, 1003] {
            assert_eq!(plural_category("ar", n), Few, "ar few n={n}");
        }
        // many: n%100 = 11..99 (CLDR examples: 11~26, 111, 1011)
        for n in [11, 26, 99, 111, 1011] {
            assert_eq!(plural_category("ar", n), Many, "ar many n={n}");
        }
        // other: n%100 ∈ {0,1,2} for n≥100 (CLDR examples: 100~102, 200~202, 1000)
        for n in [100, 101, 102, 200, 202, 1000] {
            assert_eq!(plural_category("ar", n), Other, "ar other n={n}");
        }
    }

    #[test]
    fn unknown_locale_falls_back_to_en_rule() {
        // Documented fallback (not silent): an un-modeled locale uses en's rule.
        assert_eq!(plural_category("zz", 1), PluralCategory::One);
        assert_eq!(plural_category("zz", 5), Other);
    }

    // -- P4: the embedded overlay catalogs (loader) ------------------------

    #[test]
    fn embedded_locales_have_no_orphan_keys_and_matching_shape() {
        // Subset-VALIDITY, not completeness: an overlay may be partial (missing
        // keys fall back to `en` by design — the "partial → English" policy).
        // But every key it DOES define must exist in `en` (else it's a
        // typo/orphan that silently never renders), and its Simple/Plural shape
        // must match `en` so the selector behaves. The i18n analog of theming's
        // key-parity test, deliberately weakened: a missing string is a safe en
        // fallback; a missing color is a broken render.
        for loc in EMBEDDED_LOCALES {
            for (key, msg) in loc.entries {
                let en_msg = en_map().get(key).copied().unwrap_or_else(|| {
                    panic!(
                        "overlay locale '{}' defines key '{key}' not in the en \
                         base catalog (orphan/typo)",
                        loc.id
                    )
                });
                assert_eq!(
                    matches!(en_msg, Message::Plural(_)),
                    matches!(msg, Message::Plural(_)),
                    "overlay '{}' key '{key}': Simple/Plural shape differs from en",
                    loc.id
                );
            }
        }
    }

    #[test]
    fn overlay_locale_is_consulted() {
        // Each overlay Simple entry must resolve to ITS OWN template through the
        // full `t()` path — proves `catalog_entry` consults the overlay, not en.
        for loc in EMBEDDED_LOCALES {
            for (key, msg) in loc.entries {
                if let Message::Simple(tpl) = msg {
                    set_active(loc.id);
                    assert_eq!(
                        t(key, &[]),
                        render_template(tpl, &[], false),
                        "overlay '{}' key '{key}' must resolve to its own template",
                        loc.id
                    );
                }
            }
        }
        set_active("en");
    }

    #[test]
    fn overlay_ar_plural_selects_distinct_forms() {
        // The ar worst case, integrated: Arabic peer.count carries all six CLDR
        // forms, and the selector must pick DISTINCT templates across categories
        // *through the overlay* (few ≠ many ≠ other). Guards the plural × overlay
        // × selector wiring. Skipped on a lean en-only build (no ar overlay →
        // en's one/other fallback can't distinguish the categories).
        if !EMBEDDED_LOCALES.iter().any(|l| l.id == "ar") {
            return;
        }
        set_active("ar");
        let few = t_plural("peer.count", 3, &[("n", "3")]); // n%100=3 → few
        let many = t_plural("peer.count", 11, &[("n", "11")]); // n%100=11 → many
        let other = t_plural("peer.count", 100, &[("n", "100")]); // → other
        set_active("en");
        assert_ne!(few, many, "ar few vs many must select different forms");
        assert_ne!(many, other, "ar many vs other must select different forms");
        assert!(few.contains('3'), "the count arg must render: {few:?}");
    }

    #[test]
    fn window_title_maps_and_falls_back() {
        set_active("en");
        assert_eq!(window_title("File Transfer"), "File Transfer");
        assert_eq!(window_title("System Overview"), "System Overview");
        // An unkeyed name renders verbatim — never a raw `window.<slug>` key.
        let unknown = window_title("Totally Unknown Window");
        assert_eq!(unknown, "Totally Unknown Window");
        assert!(!unknown.starts_with("window."), "raw key leaked: {unknown}");
    }

    #[test]
    fn every_registered_window_has_a_title_key() {
        // Parity guard: adding a window without a `window.<slug>` en key would
        // silently fall back to the verbatim name (untranslatable). Catch it.
        for wt in crate::window_registry::standard_window_types() {
            let key = format!("window.{}", window_slug(wt.name));
            assert!(
                en_map().contains_key(key.as_str()),
                "window '{}' has no '{key}' key in the EN catalog",
                wt.name
            );
        }
    }

    #[test]
    fn overlay_missing_key_falls_back_to_en() {
        // The user's "partial → English" policy: a key an overlay lacks renders
        // the `en` template verbatim (not the raw key, not a blank).
        for loc in EMBEDDED_LOCALES {
            let overlay: std::collections::HashSet<&str> =
                loc.entries.iter().map(|(k, _)| *k).collect();
            let fallback_key = EN
                .iter()
                .filter(|(k, m)| matches!(m, Message::Simple(_)) && !overlay.contains(k))
                .map(|(k, _)| *k)
                .next();
            if let Some(key) = fallback_key {
                set_active(loc.id);
                let via_locale = t(key, &[]);
                set_active("en");
                let via_en = t(key, &[]);
                assert_eq!(
                    via_locale, via_en,
                    "overlay '{}' lacks '{key}' → must render the en fallback verbatim",
                    loc.id
                );
            }
        }
        set_active("en");
    }

    #[test]
    fn apply_bumps_locale_generation() {
        let before = locale_generation();
        apply("en-XA");
        assert!(locale_generation() > before, "apply must advance the generation");
        assert_eq!(active_id(), "en-XA");
        apply("en"); // restore for other tests on this thread
    }
}
